//! Orders own immutable commercial snapshots, stock effects and transactional notice events.
use crate::{
    cart::{CartError, Carts, Guest},
    checkout_details::Details,
    checkout_methods::{self, Selection},
    notifications::{self, Event, OrderSnapshot},
    order_statuses::OrderStatuses,
    pricing::Money,
    taxes::charges,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use talocart_platform::{
    content::Language,
    identity::{Identity, IdentityError, Permission, Session},
};
use thiserror::Error;
mod cancellation;
mod confirmation;
mod notices;
mod read;
mod stock;
use notices::enqueue;
use read::load;
use stock::{movement, restore, stock_change};
pub mod editor;
mod fulfillment;
mod health;
mod list;
mod timing;
pub use list::{ListFilters, ListOrder, ListPage, ListSort, check_list_customers};
mod manual;
pub use fulfillment::{Fulfillment, FulfillmentEvent, FulfillmentInput};
mod online;
pub use manual::ManualPayment;
pub use online::{Attempt, OnlinePayments};
#[derive(Debug, Error)]
pub enum OrderError {
    #[error("Order cannot be edited in its current state")]
    EditBlocked,
    #[error("Invalid order editor fields")]
    EditorFields(Vec<String>),
    #[error(
        "Online payments pending: restore the payment module and its configuration before serving"
    )]
    ProviderRequired,
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid order list filters")]
    ListFilters,
    #[error("Order customer summaries incomplete: run talocart migrate")]
    ListProjection,
    #[error("Invalid fulfillment fields")]
    FulfillmentFields(Vec<&'static str>),
    #[error("Fulfillment transition unavailable")]
    FulfillmentTransition,
    #[error("Payment does not permit dispatch")]
    FulfillmentPayment,
    #[error("Fulfillment request already used with different values")]
    FulfillmentConflict,
    #[error("Shipped orders require a separate return workflow")]
    Shipped,
    #[error(transparent)]
    Cart(#[from] CartError),
    #[error(transparent)]
    Identity(IdentityError),
    #[error("Order not found")]
    NotFound,
    #[error("Review the updated quotation before confirming")]
    Review,
    #[error("Order cannot be restored to stock")]
    Restore,
    #[error("Order notification could not be queued")]
    Notification,
    #[error("A payment reference of 1 to 160 printable characters is required")]
    PaymentReference,
    #[error("A different payment has already been recorded")]
    AlreadyPaid,
    #[error("A cancelled order cannot receive a payment")]
    PaymentCancelled,
    #[error("A paid order requires a separate refund flow")]
    PaidCancellation,
    #[error("This payment method does not support manual recording")]
    PaymentMethod,
    #[error("Invalid order snapshot")]
    Invalid,
}
impl From<sqlx::Error> for OrderError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
/// Classifies failures for transaction retry without inspecting user-visible text.
impl OrderError {
    pub fn technical(&self) -> Option<talocart_platform::technical::TechnicalError> {
        match self {
            Self::Technical(error) => Some(error.clone()),
            Self::Identity(error) => error.technical(),
            Self::Cart(error) => error.technical(),
            _ => None,
        }
    }
}
pub type Result<T> = std::result::Result<T, OrderError>;
fn customer_error(e: talocart_platform::identity::CustomerError) -> OrderError {
    match e {
        talocart_platform::identity::CustomerError::Technical(e) => e.into(),
        talocart_platform::identity::CustomerError::Access(e) => e.into(),
        talocart_platform::identity::CustomerError::Identity(e) => e.into(),
        _ => OrderError::NotFound,
    }
}
impl From<talocart_platform::identity::CustomerError> for OrderError {
    fn from(error: talocart_platform::identity::CustomerError) -> Self {
        customer_error(error)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ChargeSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub code: String,
    pub name_it: String,
    pub name_en: String,
    pub instructions_it: String,
    pub instructions_en: String,
    pub entered_cents: i64,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub gross_cents: i64,
    pub class_id: Option<i64>,
    pub tax_name: String,
    pub basis_points: i32,
    pub zone_name: Option<String>,
    pub price_mode: String,
    pub destination: String,
    pub beneficiary: String,
    pub iban: String,
    pub bic: String,
    pub delivery_days: Option<i32>,
}
impl ChargeSnapshot {
    pub fn name(&self, locale: &str) -> &str {
        if locale == "it" {
            &self.name_it
        } else {
            &self.name_en
        }
    }
    pub fn instructions(&self, locale: &str) -> String {
        let mut text = if locale == "it" {
            self.instructions_it.clone()
        } else {
            self.instructions_en.clone()
        };
        if !self.iban.is_empty() {
            text.push_str(&format!(
                "\n{}: {}\nIBAN: {}",
                if locale == "it" {
                    "Beneficiario"
                } else {
                    "Beneficiary"
                },
                self.beneficiary,
                self.iban
            ));
            if !self.bic.is_empty() {
                text.push_str(&format!("\nBIC: {}", self.bic));
            }
        }
        text
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Frozen {
    #[serde(default)]
    pub commercial_revision: i64,
    #[serde(default)]
    pub group_discount: Option<crate::group_discounts::Snapshot>,
    #[serde(default)]
    pub coupon: Option<crate::coupons::Snapshot>,
    pub schema_version: u8,
    pub customer: Details,
    pub customer_locale: String,
    pub shop_locale: String,
    pub store_name: String,
    pub shipping: ChargeSnapshot,
    pub payment: ChargeSnapshot,
    pub status_id: i64,
    pub status_it: String,
    pub status_en: String,
    pub status_color: String,
}
#[derive(FromRow)]
struct Stored {
    id: i64,
    reference: String,
    snapshot: String,
    currency: String,
    merchandise_cents: i64,
    shipping_cents: i64,
    fee_cents: i64,
    net_cents: i64,
    tax_cents: i64,
    total_cents: i64,
    state: String,
    payment_state: String,
    revision: i64,
    created_at: String,
    cancelled_at: String,
}
#[derive(FromRow)]
pub struct Line {
    pub id: i64,
    pub product_id: i64,
    pub variant_id: Option<i64>,
    pub quantity: i32,
    pub name: String,
    pub label: String,
    pub code: String,
    pub unit_cents: i64,
    pub class_id: i64,
    pub tax_name: String,
    pub basis_points: i32,
    pub zone_name: Option<String>,
    pub price_mode: String,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub gross_cents: i64,
}
pub struct Order {
    pub current_status: crate::order_statuses::OrderStatus,
    pub status_revision: i64,
    pub fulfillment: Fulfillment,
    pub id: i64,
    pub reference: String,
    pub frozen: Frozen,
    pub currency: String,
    pub merchandise_cents: i64,
    pub shipping_cents: i64,
    pub fee_cents: i64,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
    pub state: String,
    pub payment_state: String,
    pub revision: i64,
    pub created_at: String,
    pub cancelled_at: String,
    pub lines: Vec<Line>,
    pub manual_payment: Option<ManualPayment>,
    pub online_review: bool,
    online_review_disputed: bool,
    pub online_attempt_state: Option<String>,
}
mod status;
pub use status::{StatusEvent, check_current_status};
impl Order {
    pub fn fulfillment_key(&self) -> &'static str {
        if self.state == "cancelled" && !self.fulfillment.sent() {
            "fulfillment-unshipped"
        } else {
            self.fulfillment.key()
        }
    }
    pub fn payment_due(&self) -> bool {
        self.state == "open" && self.payment_state == "unpaid" && self.total_cents > 0
    }
    pub fn is_paid(&self) -> bool {
        self.payment_state == "paid"
    }
    pub fn is_refunded(&self) -> bool {
        self.payment_state == "refunded"
    }
    pub fn is_online(&self) -> bool {
        crate::payments::is_online_code(&self.frozen.payment.code)
    }
    /// A verified capture survives transport failures, but financial/provider
    /// discrepancies still require review before showing a confirmation.
    pub fn payment_review_blocks_confirmation(&self) -> bool {
        self.online_review && (!self.is_paid() || self.online_review_disputed)
    }
    pub fn payment_reconciliation_warning(&self) -> bool {
        self.online_review && !self.payment_review_blocks_confirmation()
    }
    pub fn payment_verifying(&self) -> bool {
        self.is_online()
            && self.state == "open"
            && !self.is_paid()
            && !self.is_refunded()
            && !self.online_review
            && matches!(
                self.online_attempt_state.as_deref(),
                Some("creating" | "processing" | "refund_pending")
            )
    }
    pub fn money(&self, cents: &i64) -> String {
        format!("{},{:02}", cents / 100, cents % 100)
    }
    pub fn status(&self, locale: &str) -> &str {
        self.current_status.name(locale)
    }
    fn notice(&self, locale: &str) -> Result<OrderSnapshot> {
        let money = |v| Money::from_cents(v).map_err(|_| OrderError::Invalid);
        Ok(OrderSnapshot {
            fulfillment: self.fulfillment.clone(),
            coupon: self
                .frozen
                .coupon
                .as_ref()
                .map(|c| {
                    Ok::<_, OrderError>((
                        c.code.clone(),
                        money(c.original_cents)?,
                        money(c.discount_cents)?,
                    ))
                })
                .transpose()?,
            reference: self.reference.clone(),
            store_name: self.frozen.store_name.clone(),
            customer_name: format!(
                "{} {}",
                self.frozen.customer.first_name, self.frozen.customer.last_name
            ),
            currency: self.currency.clone(),
            lines: self
                .lines
                .iter()
                .map(|l| {
                    Ok(notifications::Line {
                        description: if l.label.is_empty() {
                            l.name.clone()
                        } else {
                            format!("{} / {}", l.name, l.label)
                        },
                        quantity: u32::try_from(l.quantity).map_err(|_| OrderError::Invalid)?,
                        total: money(l.gross_cents)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            merchandise: money(self.merchandise_cents)?,
            shipping: money(self.shipping_cents)?,
            payment_fee: money(self.fee_cents)?,
            tax: money(self.tax_cents)?,
            total: money(self.total_cents)?,
            paid: money(if self.is_paid() || self.is_refunded() {
                self.total_cents
            } else {
                0
            })?,
            payment_method: self.frozen.payment.name(locale).into(),
            instructions: self.frozen.payment.instructions(locale),
        })
    }
}
#[derive(FromRow)]
pub struct Summary {
    pub status_it: String,
    pub status_en: String,
    pub status_color: String,
    // Customer history does not select the administrative identity projection.
    #[sqlx(default)]
    pub customer_name: String,
    #[sqlx(default)]
    pub customer_email: String,
    pub fulfillment_state: String,
    pub id: i64,
    pub reference: String,
    pub currency: String,
    pub total_cents: i64,
    pub state: String,
    pub created_at: String,
    pub paid: bool,
    pub refunded: bool,
}
impl Summary {
    pub fn status(&self, locale: &str) -> &str {
        if locale == "it" {
            &self.status_it
        } else {
            &self.status_en
        }
    }
    pub fn fulfillment_key(&self) -> &'static str {
        if self.state == "cancelled" {
            "fulfillment-unshipped"
        } else {
            match self.fulfillment_state.as_str() {
                "preparing" => "fulfillment-preparing",
                "shipped" => "fulfillment-shipped",
                "delivered" => "fulfillment-delivered",
                _ => "fulfillment-pending",
            }
        }
    }

    pub fn amount(&self) -> String {
        format!("{},{:02}", self.total_cents / 100, self.total_cents % 100)
    }
}
#[derive(Clone)]
pub struct Orders {
    pool: PgPool,
    identity: Identity,
    online_providers: Option<crate::payments::OnlineProviders>,
    online_limits: Option<talocart_platform::payment_gateway::AmountLimits>,
}
pub struct Confirmation<'a> {
    pub guest: &'a Guest,
    pub csrf: &'a str,
    pub revision: i64,
    pub data: Details,
    pub selection: Selection,
    pub review: &'a str,
    pub language: Language,
    pub online_available: bool,
}
impl Orders {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self {
            pool,
            identity,
            online_limits: None,
            online_providers: None,
        }
    }
    pub fn with_online_providers(
        mut self,
        providers: Option<crate::payments::OnlineProviders>,
    ) -> Self {
        self.online_providers = providers;
        self
    }
    pub fn with_online_limits(
        mut self,
        limits: Option<talocart_platform::payment_gateway::AmountLimits>,
    ) -> Self {
        self.online_limits = limits;
        self
    }
}
impl From<talocart_platform::mail::MailError> for OrderError {
    fn from(error: talocart_platform::mail::MailError) -> Self {
        match error {
            talocart_platform::mail::MailError::Technical(cause) => Self::Technical(cause),
            talocart_platform::mail::MailError::Identity(cause) => cause.into(),
            _ => Self::Notification,
        }
    }
}
impl From<crate::order_statuses::StatusError> for OrderError {
    fn from(error: crate::order_statuses::StatusError) -> Self {
        use crate::order_statuses::StatusError;
        match error {
            StatusError::Technical(cause) => Self::Technical(cause),
            StatusError::Access(cause) => Self::Access(cause),
            StatusError::Identity(cause) => cause.into(),
            _ => Self::Invalid,
        }
    }
}

impl From<IdentityError> for OrderError {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}

#[cfg(test)]
mod failure_classification_tests {
    use super::*;
    #[test]
    fn owner_error_conversions_preserve_technical_and_access_causes() {
        use crate::order_statuses::StatusError;
        use talocart_platform::{
            mail::MailError,
            technical::{AccessError, TechnicalError},
        };
        for error in [
            OrderError::from(MailError::Technical(TechnicalError::Busy)),
            OrderError::from(MailError::Identity(TechnicalError::Busy.into())),
            OrderError::from(StatusError::Technical(TechnicalError::Busy)),
            OrderError::from(StatusError::Identity(TechnicalError::Busy.into())),
        ] {
            assert_eq!(error.technical(), Some(TechnicalError::Busy));
        }
        for error in [
            OrderError::from(MailError::Identity(AccessError::Forbidden.into())),
            OrderError::from(StatusError::Access(AccessError::Forbidden)),
        ] {
            assert!(matches!(error, OrderError::Access(AccessError::Forbidden)));
        }
        assert!(matches!(
            OrderError::from(MailError::Invalid),
            OrderError::Notification
        ));
        assert!(matches!(
            OrderError::from(StatusError::NotFound),
            OrderError::Invalid
        ));
    }
    #[test]
    fn commercial_unavailability_and_access_are_not_database_failures() {
        assert!(
            OrderError::Cart(CartError::Unavailable)
                .technical()
                .is_none()
        );
        assert!(
            OrderError::Access(talocart_platform::technical::AccessError::Forbidden)
                .technical()
                .is_none()
        );
    }
}
