//! Configuration for payment implementations shipped with the application. No payment execution or order state transitions.
mod iban;
use crate::{
    pricing::{Money, PriceError, PriceMode, Rate},
    taxes::charges::{self, ChargeError, ChargeQuote, ClassChoice},
};
use iban::valid as valid_iban;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use talocart_platform::identity::{Identity, IdentityError, Permission, Session};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid payment fields")]
    Invalid(Vec<&'static str>),
    #[error("Configuration changed")]
    Stale,
    #[error("Payment method not found")]
    NotFound,
    #[error("Payment method unavailable for this total")]
    Unavailable,
    #[error(transparent)]
    Price(#[from] PriceError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for PaymentError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
impl From<ChargeError> for PaymentError {
    fn from(e: ChargeError) -> Self {
        match e {
            ChargeError::Unavailable => Self::Unavailable,
            ChargeError::Price(e) => Self::Price(e),
            ChargeError::Technical(e) => Self::Technical(e),
            ChargeError::Access(e) => Self::Access(e),
            ChargeError::Identity(e) => Self::Identity(e),
        }
    }
}
/// Accepting provider identities and their independently declared amount bounds.
pub type OnlineProviders =
    std::collections::BTreeMap<String, Option<talocart_platform::payment_gateway::AmountLimits>>;
pub type Result<T> = std::result::Result<T, PaymentError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum MethodCode {
    BankTransfer,
    CashOnDelivery,
    #[sqlx(rename = "online_checkout")]
    OnlineCheckout,
}
impl MethodCode {
    pub fn key(self) -> &'static str {
        match self {
            Self::BankTransfer => "bank_transfer",
            Self::CashOnDelivery => "cash_on_delivery",
            Self::OnlineCheckout => "online_checkout",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum FeeKind {
    None,
    Fixed,
    Percent,
}
impl FeeKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fixed => "fixed",
            Self::Percent => "percent",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "fixed" => Some(Self::Fixed),
            "percent" => Some(Self::Percent),
            _ => None,
        }
    }
}
/// Untrusted form values; only the validated Method stores numeric values and enums.
#[derive(Clone, Debug, Default)]
pub struct CommonInput {
    pub name_it: String,
    pub name_en: String,
    pub instructions_it: String,
    pub instructions_en: String,
    pub active: bool,
    pub sort_order: String,
    pub fee_kind: String,
    pub fee_value: String,
    pub minimum: String,
    pub currency: String,
    pub price_mode: String,
    pub initial_order_status_id: String,
    pub tax_class_id: String,
}
#[derive(Clone, Debug, Default)]
pub struct BankDetailsInput {
    pub beneficiary: String,
    pub iban: String,
    pub bic: String,
}
#[derive(Clone, Debug, Default)]
pub struct BankTransferInput {
    pub common: CommonInput,
    pub bank: BankDetailsInput,
}
#[derive(Clone, Debug, Default)]
pub struct CashOnDeliveryInput {
    pub common: CommonInput,
}
#[derive(Clone, Debug, FromRow)]
pub struct Method {
    pub code: MethodCode,
    pub provider_id: Option<String>,
    pub id: i64,
    pub name_it: String,
    pub name_en: String,
    pub instructions_it: String,
    pub instructions_en: String,
    pub active: bool,
    pub sort_order: i32,
    pub beneficiary: String,
    pub iban: String,
    pub bic: String,
    pub fee_kind: FeeKind,
    pub fee_value: i64,
    pub minimum_cents: i64,
    pub initial_order_status_id: i64,
    pub tax_class_id: Option<i64>,
}
impl Method {
    pub fn bank_input(&self) -> BankDetailsInput {
        BankDetailsInput {
            beneficiary: self.beneficiary.clone(),
            iban: self.iban.clone(),
            bic: self.bic.clone(),
        }
    }
    pub fn common_input(&self, currency: String, mode: PriceMode) -> CommonInput {
        CommonInput {
            name_it: self.name_it.clone(),
            name_en: self.name_en.clone(),
            instructions_it: self.instructions_it.clone(),
            instructions_en: self.instructions_en.clone(),
            active: self.active,
            sort_order: self.sort_order.to_string(),
            fee_kind: self.fee_kind.key().into(),
            fee_value: format!("{}.{:02}", self.fee_value / 100, self.fee_value % 100),
            minimum: format!(
                "{}.{:02}",
                self.minimum_cents / 100,
                self.minimum_cents % 100
            ),
            currency,
            price_mode: mode.key().into(),
            initial_order_status_id: self.initial_order_status_id.to_string(),
            tax_class_id: self
                .tax_class_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
        }
    }
    /// The caller supplies a server-calculated order total (goods, shipping and taxes,
    /// after discounts, before the payment fee). The fee never helps reach its own threshold.
    /// Private tariff arithmetic; require_method adds destination VAT using store price mode.
    /// Percentage rounds half-up once, in cents; fixed fees use the store currency.
    fn tariff(&self, base: Money) -> Result<Tariff> {
        if !self.active
            || base.cents() < self.minimum_cents
            || (self.code == MethodCode::BankTransfer
                && (self.beneficiary.trim().is_empty()
                    || !valid_iban(&self.iban)
                    || (!self.bic.is_empty() && !valid_bic(&self.bic))))
        {
            return Err(PaymentError::Unavailable);
        }
        let fee = match self.fee_kind {
            FeeKind::None => 0,
            FeeKind::Fixed => self.fee_value,
            FeeKind::Percent => i64::try_from(
                (i128::from(base.cents()) * i128::from(self.fee_value) + 5000) / 10000,
            )
            .map_err(|_| PriceError::Overflow)?,
        };
        let total = base.cents().checked_add(fee).ok_or(PriceError::Overflow)?;
        Ok(Tariff {
            fee: Money::from_cents(fee)?,
            total: Money::from_cents(total)?,
        })
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Tariff {
    pub fee: Money,
    pub total: Money,
}
/// A complete fiscal payment quote; fee is the gross amount added to base.
#[derive(Debug, PartialEq, Eq)]
pub struct Quote {
    pub fee: Money,
    pub total: Money,
    pub breakdown: ChargeQuote,
}
pub struct Snapshot {
    pub revision: i64,
    pub currency: String,
    pub methods: Vec<Method>,
    pub statuses: Vec<crate::order_statuses::OrderStatus>,
    pub tax_classes: Vec<ClassChoice>,
    pub price_mode: PriceMode,
}
#[derive(Clone)]
pub struct Payments {
    online_installed: bool,
    providers: Option<Vec<String>>,
    pool: PgPool,
    identity: Identity,
}
impl Payments {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self {
            pool,
            identity,
            online_installed: false,
            providers: None,
        }
    }
    pub fn with_online(mut self, installed: bool) -> Self {
        self.online_installed = installed;
        self
    }
    pub fn with_providers(mut self, providers: Option<Vec<String>>) -> Self {
        self.providers = providers;
        self
    }
    pub async fn snapshot(&self, s: &Session) -> Result<Snapshot> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let revision = sqlx::query_scalar("SELECT revision FROM payment_configuration WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let currency = sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let methods =
            sqlx::query_as("SELECT * FROM payment_methods WHERE code<>'online_checkout' OR ($1 AND ($2::text[] IS NULL OR provider_id=ANY($2))) ORDER BY sort_order,id LIMIT 34").bind(self.online_installed).bind(&self.providers)
                .fetch_all(&mut *tx)
                .await?;
        let statuses = sqlx::query_as(
            "SELECT * \
                FROM order_statuses s \
                WHERE NOT EXISTS(SELECT 1 \
                    FROM order_status_roles r \
                    WHERE r.status_id=s.id \
                    AND r.role<>'preparing') \
                ORDER BY sort_order,id \
                LIMIT 1000",
        )
        .fetch_all(&mut *tx)
        .await?;
        let tax_classes = charges::choices(&mut tx).await?;
        let mode: String = sqlx::query_scalar("SELECT price_mode FROM pricing_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Snapshot {
            revision,
            currency,
            methods,
            statuses,
            tax_classes,
            price_mode: PriceMode::parse(&mode)?,
        })
    }
    pub async fn save_bank_transfer(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &BankTransferInput,
    ) -> Result<i64> {
        self.save(
            s,
            csrf,
            id,
            revision,
            &input.common,
            Some(&input.bank),
            MethodCode::BankTransfer,
        )
        .await
    }
    pub async fn save_cash_on_delivery(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &CashOnDeliveryInput,
    ) -> Result<i64> {
        self.save(
            s,
            csrf,
            id,
            revision,
            &input.common,
            None,
            MethodCode::CashOnDelivery,
        )
        .await
    }
    pub async fn save_online(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &CommonInput,
        available: bool,
    ) -> Result<i64> {
        if !self.online_installed {
            return Err(PaymentError::NotFound);
        }
        if input.active && !available {
            return Err(PaymentError::Invalid(vec!["active"]));
        }
        self.save(
            s,
            csrf,
            id,
            revision,
            input,
            None,
            MethodCode::OnlineCheckout,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)] // Fixed implementation selector protects method identity.
    async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &CommonInput,
        bank: Option<&BankDetailsInput>,
        code: MethodCode,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        // Same guard as currency changes; acquire before reading currency or locking rows.
        talocart_platform::database::commercial_lock::exclusive(&mut tx).await?;
        let currency: String = sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let mode: String = sqlx::query_scalar("SELECT price_mode FROM pricing_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        if currency != input.currency || mode != input.price_mode {
            return Err(PaymentError::Stale);
        }
        if sqlx::query(
            "UPDATE payment_configuration SET revision=revision+1 WHERE id=1 AND revision=$1",
        )
        .bind(revision)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 1
        {
            return Err(PaymentError::Stale);
        }
        let mut m = validate_for(input, bank)?;
        m.code = code;
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT id \
                FROM order_statuses s \
                WHERE id=$1 \
                AND NOT EXISTS(SELECT 1 \
                    FROM order_status_roles r \
                    WHERE r.status_id=s.id \
                    AND r.role<>'preparing') FOR SHARE",
        )
        .bind(m.initial_order_status_id)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.is_none() {
            return Err(PaymentError::Invalid(vec!["initial_order_status_id"]));
        }
        charges::validate_class(&mut tx, m.tax_class_id)
            .await
            .map_err(|e| match e {
                ChargeError::Unavailable => PaymentError::Invalid(vec!["tax_class_id"]),
                other => other.into(),
            })?;
        let q = sqlx::query_scalar("UPDATE payment_methods \
            SET name_it=$1,name_en=$2,instructions_it=$3,instructions_en=$4,active=$5,sort_order=$6,\
            beneficiary=$7,iban=$8,bic=$9,fee_kind=$10,fee_value=$11,minimum_cents=$12,initial_order_status_id=$15,\
            tax_class_id=$16 \
            WHERE id=$13 \
            AND code=$14 \
            RETURNING id")
            .bind(&m.name_it).bind(&m.name_en).bind(&m.instructions_it).bind(&m.instructions_en)
            .bind(m.active).bind(m.sort_order).bind(&m.beneficiary).bind(&m.iban).bind(&m.bic)
            .bind(m.fee_kind).bind(m.fee_value).bind(m.minimum_cents).bind(id).bind(m.code).bind(m.initial_order_status_id).bind(m.tax_class_id);
        let saved = q
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(PaymentError::NotFound)?;
        tx.commit().await?;
        Ok(saved)
    }
    /// Future checkout must re-read the chosen method in the order transaction.
    /// FOR SHARE prevents edits/disabling while its snapshot and quote are committed.
    /// Assign `method.initial_order_status_id` to the new order in this transaction;
    /// use OrderStatuses::require in that transaction if snapshotting its labels/color.
    /// Destination must be the same server-validated order destination as shipping.
    pub async fn require_method(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        base: Money,
        destination: &str,
    ) -> Result<(Method, Quote)> {
        charges::lock(tx).await?;
        let method: Method = sqlx::query_as("SELECT * FROM payment_methods WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(PaymentError::NotFound)?;
        let tariff = method.tariff(base)?;
        if method.fee_value > 0 && method.tax_class_id.is_none() {
            return Err(PaymentError::Unavailable);
        }
        let breakdown = charges::quote(tx, method.tax_class_id, tariff.fee, destination).await?;
        let fee = breakdown.totals.gross();
        let added_tax = if breakdown.price_mode == PriceMode::Exclusive {
            breakdown.totals.tax().cents()
        } else {
            0
        };
        let total = Money::from_cents(
            tariff
                .total
                .cents()
                .checked_add(added_tax)
                .ok_or(PriceError::Overflow)?,
        )?;
        if method.code == MethodCode::OnlineCheckout {
            let currency: String =
                sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
                    .fetch_one(&mut **tx)
                    .await?;
            if !online_amount(&currency, total.cents()) {
                return Err(PaymentError::Unavailable);
            }
        }
        Ok((
            method,
            Quote {
                fee,
                total,
                breakdown,
            },
        ))
    }
}
/// Identify the provider-neutral online method in stored payment snapshots.
pub fn is_online_code(code: &str) -> bool {
    code == "online_checkout"
}

#[cfg(test)]
fn validate(i: &BankTransferInput) -> Result<Method> {
    validate_for(&i.common, Some(&i.bank))
}
fn validate_for(i: &CommonInput, bank: Option<&BankDetailsInput>) -> Result<Method> {
    let code = if bank.is_some() {
        MethodCode::BankTransfer
    } else {
        MethodCode::CashOnDelivery
    };
    let empty_bank = BankDetailsInput::default();
    let bank = bank.unwrap_or(&empty_bank);
    let mut errors = Vec::new();
    for (field, value, max, required) in [
        ("name_it", i.name_it.as_str(), 120, true),
        ("name_en", &i.name_en, 120, true),
        ("instructions_it", &i.instructions_it, 4000, false),
        ("instructions_en", &i.instructions_en, 4000, false),
        (
            "beneficiary",
            &bank.beneficiary,
            200,
            i.active && code == MethodCode::BankTransfer,
        ),
    ] {
        if (required && value.trim().is_empty())
            || value.chars().count() > max
            || value.contains('\0')
        {
            errors.push(field);
        }
    }
    let order = i
        .sort_order
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=9999).contains(n));
    if order.is_none() {
        errors.push("sort_order");
    }
    let fee_kind = FeeKind::parse(&i.fee_kind);
    let fee = match fee_kind {
        Some(FeeKind::None) => {
            if i.fee_value.trim().is_empty() {
                Some(0)
            } else {
                Money::parse(&i.fee_value)
                    .ok()
                    .filter(|m| m.cents() == 0)
                    .map(Money::cents)
            }
        }
        Some(FeeKind::Fixed) => Money::parse(&i.fee_value).ok().map(Money::cents),
        Some(FeeKind::Percent) => Rate::parse(&i.fee_value)
            .ok()
            .map(|r| i64::from(r.basis_points())),
        None => {
            errors.push("fee_kind");
            None
        }
    };
    if fee.is_none() {
        errors.push("fee_value");
    }
    let minimum = if i.minimum.trim().is_empty() {
        Some(0)
    } else {
        Money::parse(&i.minimum).ok().map(Money::cents)
    };
    if minimum.is_none() {
        errors.push("minimum");
    }
    let iban: String = bank
        .iban
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let bic = bank.bic.trim().to_ascii_uppercase();
    if (!iban.is_empty() && !valid_iban(&iban))
        || (i.active && code == MethodCode::BankTransfer && iban.is_empty())
    {
        errors.push("iban");
    }
    if !bic.is_empty() && !valid_bic(&bic) {
        errors.push("bic");
    }
    if PriceMode::parse(&i.price_mode).is_err() {
        errors.push("price_mode");
    }
    if !talocart_platform::geography::currency_valid(&i.currency) {
        errors.push("currency");
    }
    let initial_status = i
        .initial_order_status_id
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0);
    if initial_status.is_none() {
        errors.push("initial_order_status_id");
    }
    let tax_class_id = charges::parse_class(&i.tax_class_id, fee.is_some_and(|v| v > 0));
    if tax_class_id.is_err() {
        errors.push("tax_class_id");
    }
    if !errors.is_empty() {
        return Err(PaymentError::Invalid(errors));
    }
    Ok(Method {
        code,
        provider_id: None,
        id: 0,
        name_it: i.name_it.trim().into(),
        name_en: i.name_en.trim().into(),
        instructions_it: i.instructions_it.clone(),
        instructions_en: i.instructions_en.clone(),
        active: i.active,
        sort_order: order.unwrap_or_default(),
        beneficiary: bank.beneficiary.trim().into(),
        iban,
        bic,
        fee_kind: fee_kind.unwrap_or(FeeKind::None),
        fee_value: fee.unwrap_or_default(),
        minimum_cents: minimum.unwrap_or_default(),
        initial_order_status_id: initial_status.unwrap_or_default(),
        tax_class_id: tax_class_id.unwrap_or_default(),
    })
}
fn valid_bic(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() == 8 || b.len() == 11)
        && b[..6].iter().all(u8::is_ascii_uppercase)
        && b[6..].iter().all(u8::is_ascii_alphanumeric)
}
/// Conservative integration limits for the two-decimal currencies supported by the shop.
pub fn online_amount(currency: &str, amount: i64) -> bool {
    matches!(
        currency,
        "EUR"
            | "USD"
            | "AUD"
            | "CAD"
            | "CHF"
            | "NZD"
            | "GBP"
            | "CZK"
            | "DKK"
            | "NOK"
            | "SEK"
            | "PLN"
            | "RON"
    ) && (1..=99_999_999).contains(&amount)
}

impl From<IdentityError> for PaymentError {
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
mod tests {
    use super::*;
    fn input() -> BankTransferInput {
        BankTransferInput {
            common: CommonInput {
                name_it: "Bonifico".into(),
                name_en: "Transfer".into(),
                sort_order: "0".into(),
                fee_kind: "none".into(),
                tax_class_id: "1".into(),
                currency: "EUR".into(),
                price_mode: "inclusive".into(),
                initial_order_status_id: "1".into(),
                ..Default::default()
            },
            bank: BankDetailsInput::default(),
        }
    }

    #[test]
    fn online_amount_limits_are_integer_currency_specific_and_nonzero() {
        for (currency, min) in [
            ("EUR", 50),
            ("USD", 50),
            ("GBP", 30),
            ("CHF", 50),
            ("CAD", 50),
            ("AUD", 50),
            ("NZD", 50),
            ("SEK", 300),
            ("NOK", 300),
            ("DKK", 250),
            ("PLN", 200),
            ("CZK", 1500),
            ("RON", 200),
        ] {
            assert!(online_amount(currency, min - 1));
            assert!(online_amount(currency, min));
            assert!(online_amount(currency, 99_999_999));
            assert!(!online_amount(currency, 100_000_000));
            assert!(!online_amount(currency, 0));
        }
        assert!(!online_amount("JPY", 500));
        assert!(!online_amount("EUR", i64::MAX));
    }
    #[test]
    fn initial_order_status_must_be_a_positive_database_identifier() {
        for bad in ["", "0", "-1", "paid", "9223372036854775808"] {
            let mut i = input();
            i.common.initial_order_status_id = bad.into();
            assert!(
                matches!(validate(&i), Err(PaymentError::Invalid(fields)) if fields.contains(&"initial_order_status_id"))
            );
        }
    }

    #[test]
    fn iban_rejects_unknown_country_and_wrong_national_structure() {
        for invalid in [
            "ZZ8112345678901",
            "IT5812345678901",
            "IT88XA542811101000000123456",
        ] {
            assert!(!valid_iban(invalid), "accepted {invalid}");
        }
    }
    #[test]
    fn persisted_invalid_bank_account_cannot_be_quoted() {
        let mut i = input();
        i.common.active = true;
        i.bank.beneficiary = "Example".into();
        i.bank.iban = "IT60X0542811101000000123456".into();
        let mut m = validate(&i).unwrap();
        m.iban = "ZZ8112345678901".into();
        assert!(matches!(
            m.tariff(Money::from_cents(1000).unwrap()),
            Err(PaymentError::Unavailable)
        ));
    }
    #[test]
    fn bank_activation_and_validation() {
        let mut i = input();
        assert!(validate(&i).is_ok());
        i.common.active = true;
        assert!(validate(&i).is_err());
        i.bank.beneficiary = "Example".into();
        i.bank.iban = "IT60 X054 2811 1010 0000 0123 456".into();
        assert!(validate(&i).is_ok());
        i.bank.iban = "IT61X0542811101000000123456".into();
        assert!(validate(&i).is_err());
        for bad in ["-1", "1.001", "NaN", "999999999999999999999"] {
            i = input();
            i.common.minimum = bad.into();
            assert!(validate(&i).is_err());
        }
        i = input();
        i.common.fee_kind = "percent".into();
        i.common.fee_value = "100.01".into();
        assert!(validate(&i).is_err());
    }
    #[test]
    fn cash_on_delivery_activates_without_bank_details_and_validates_terms() {
        let mut i = input();
        i.common.active = true;
        let method = validate_for(&i.common, None).expect("COD does not require a bank account");
        assert!(method.active);
        assert!(method.iban.is_empty());
        assert_eq!(
            method
                .tariff(Money::from_cents(0).unwrap())
                .unwrap()
                .total
                .cents(),
            0
        );
        for bad in ["-1", "1.001", "NaN"] {
            i.common.minimum = bad.into();
            assert!(validate_for(&i.common, None).is_err());
        }
        i.common.minimum.clear();
        i.common.fee_kind = "percent".into();
        i.common.fee_value = "100.01".into();
        assert!(validate_for(&i.common, None).is_err());
    }
    #[test]
    fn fees_threshold_rounding_and_overflow() {
        let mut i = input();
        i.bank.beneficiary = "Example".into();
        i.bank.iban = "IT60X0542811101000000123456".into();
        i.common.active = true;
        i.common.fee_kind = "percent".into();
        i.common.fee_value = "2.50".into();
        i.common.minimum = "10".into();
        let mut m = validate(&i).unwrap();
        assert!(matches!(
            m.tariff(Money::from_cents(999).unwrap()),
            Err(PaymentError::Unavailable)
        ));
        assert_eq!(
            m.tariff(Money::from_cents(1000).unwrap())
                .unwrap()
                .fee
                .cents(),
            25
        );
        m.minimum_cents = 0;
        assert_eq!(
            m.tariff(Money::from_cents(20).unwrap())
                .unwrap()
                .fee
                .cents(),
            1
        );
        assert_eq!(
            m.tariff(Money::from_cents(19).unwrap())
                .unwrap()
                .fee
                .cents(),
            0
        );
        assert_eq!(
            m.tariff(Money::from_cents(0).unwrap()).unwrap().fee.cents(),
            0
        );
        assert!(matches!(
            m.tariff(Money::from_cents(i64::MAX).unwrap()),
            Err(PaymentError::Price(PriceError::Overflow))
        ));
        m.fee_kind = FeeKind::Fixed;
        m.fee_value = 200;
        assert_eq!(
            m.tariff(Money::from_cents(1000).unwrap())
                .unwrap()
                .total
                .cents(),
            1200
        );
        m.active = false;
        assert!(m.tariff(Money::from_cents(1000).unwrap()).is_err());
    }
}
