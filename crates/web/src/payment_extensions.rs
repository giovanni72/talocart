//! Narrow composition contract for an optional payment provider's presentation.
use talocart_commerce::orders::Order;

pub struct PaymentBody<'a> {
    pub order: &'a Order,
    pub csrf: &'a str,
    pub online_available: bool,
    pub waiting: bool,
    pub submitted: bool,
    pub resumable: bool,
}
pub trait PaymentUi: Send + Sync {
    fn embedded(&self) -> bool;
    /// Trusted module templates must escape every customer/imported value.
    fn render<'a>(&'a self, context: PaymentBody<'a>) -> Render<'a>;
    fn receipt_policy(&self) -> String;
    fn receipt_policy_for(&self, _provider: Option<&str>) -> String {
        self.receipt_policy()
    }
}
/// Shared storefront paths for provider-owned templates, scoped to the current request.
pub fn path(value: impl AsRef<str>) -> String {
    crate::store_language::path(value)
}

/// Minimal presentation snapshot: no session, owner capability, payment history or credentials.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentOrder {
    pub reference: String,
    pub revision: i64,
    pub currency: String,
    pub merchandise_cents: i64,
    pub shipping_cents: i64,
    pub fee_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
    pub email: String,
    pub shipping_name: String,
    pub shipping_postal: String,
    pub shipping_city: String,
    pub shipping_country: String,
    pub lines: Vec<PaymentLine>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentLine {
    pub quantity: i32,
    pub name: String,
    pub gross_cents: i64,
}
impl PaymentOrder {
    pub fn money(&self, cents: &i64) -> String {
        format!("{},{:02}", cents / 100, cents % 100)
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRequest {
    pub order: PaymentOrder,
    pub csrf: String,
    pub locale: String,
    pub prefix: String,
    pub online_available: bool,
    pub waiting: bool,
    pub submitted: bool,
    pub resumable: bool,
}
impl From<PaymentBody<'_>> for UiRequest {
    fn from(c: PaymentBody<'_>) -> Self {
        let o = c.order;
        Self {
            order: PaymentOrder {
                reference: o.reference.clone(),
                revision: o.revision,
                currency: o.currency.clone(),
                merchandise_cents: o.merchandise_cents,
                shipping_cents: o.shipping_cents,
                fee_cents: o.fee_cents,
                tax_cents: o.tax_cents,
                total_cents: o.total_cents,
                email: o.frozen.customer.email.clone(),
                shipping_name: o.frozen.shipping.name(crate::i18n::locale()).into(),
                shipping_postal: o.frozen.customer.shipping_postal.clone(),
                shipping_city: o.frozen.customer.shipping_city.clone(),
                shipping_country: o.frozen.customer.shipping_country.clone(),
                lines: o
                    .lines
                    .iter()
                    .map(|l| PaymentLine {
                        quantity: l.quantity,
                        name: l.name.clone(),
                        gross_cents: l.gross_cents,
                    })
                    .collect(),
            },
            csrf: c.csrf.into(),
            locale: crate::i18n::locale().into(),
            prefix: path("/").trim_end_matches('/').into(),
            online_available: c.online_available,
            waiting: c.waiting,
            submitted: c.submitted,
            resumable: c.resumable,
        }
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rendered {
    pub body: String,
    pub scripts: Vec<String>,
    pub title: String,
    pub policy: String,
}
pub type Render<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Rendered, askama::Error>> + Send + 'a>,
>;

pub struct NativePresentation {
    manager: talocart_platform::packages::Manager,
}
impl NativePresentation {
    pub fn new(manager: talocart_platform::packages::Manager) -> Self {
        Self { manager }
    }
}
impl PaymentUi for NativePresentation {
    fn embedded(&self) -> bool {
        self.manager.payments().is_ok_and(|p| !p.is_empty())
    }
    fn receipt_policy(&self) -> String {
        self.manager
            .payment()
            .ok()
            .flatten()
            .and_then(|i| i.manifest.payment)
            .map_or_else(|| "default-src 'none'".into(), |d| d.receipt_policy)
    }
    fn receipt_policy_for(&self, provider: Option<&str>) -> String {
        self.manager
            .payment_for(provider)
            .ok()
            .flatten()
            .and_then(|p| p.manifest.payment)
            .map_or_else(|| "default-src 'none'".into(), |p| p.receipt_policy)
    }
    fn render<'a>(&'a self, context: PaymentBody<'a>) -> Render<'a> {
        let provider = context.order.frozen.payment.provider_id.clone();
        let request = UiRequest::from(context);
        Box::pin(async move {
            self.manager
                .payment_call_for(
                    provider.as_deref(),
                    talocart_platform::packages::payments::UI_PATH,
                    &request,
                )
                .await
                .map_err(|_| askama::Error::Fmt)
        })
    }
}

/// Assets are available only from the currently enabled signed provider, via private RPC.
pub(crate) fn native_assets() -> axum::Router<crate::AppState> {
    axum::Router::new().route(
        "/assets/modules/{provider}/{asset}",
        axum::routing::get(native_asset),
    )
}
async fn native_asset(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path((provider, asset)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use talocart_platform::packages::payments::{RPC_PATH, Request};
    let Some(manager) = state.packages.as_ref() else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    if manager
        .payment_for(Some(&provider))
        .ok()
        .flatten()
        .is_none_or(|i| i.manifest.id != provider)
        || asset != "checkout.js"
    {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    match manager
        .payment_call_for::<_, String>(Some(&provider), RPC_PATH, &Request::Asset { name: asset })
        .await
    {
        Ok(content) => (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/javascript; charset=utf-8",
                ),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            content,
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
