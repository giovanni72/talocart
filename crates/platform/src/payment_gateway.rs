//! Typed payment-provider boundary. No credentials, network clients or SQL.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, future::Future, pin::Pin};
use thiserror::Error;
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum Error {
    #[error("Payment provider temporarily unavailable")]
    Unavailable,
    #[error("Payment provider rejected the request")]
    Rejected,
    #[error("Payment object was not found in this account")]
    NotFound,
    #[error("Payment provider response or signature is invalid")]
    Invalid,
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
pub type Call<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Provider-owned bounds, captured before entering commercial transactions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmountRange {
    pub minimum: i64,
    pub maximum: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AmountLimits(pub BTreeMap<String, AmountRange>);
impl AmountLimits {
    pub fn valid(&self) -> bool {
        !self.0.is_empty()
            && self.0.len() <= 64
            && self.0.iter().all(|(currency, range)| {
                currency.len() == 3
                    && currency.bytes().all(|b| b.is_ascii_uppercase())
                    && range.minimum > 0
                    && range.maximum >= range.minimum
            })
    }
    pub fn allows(&self, currency: &str, amount: i64) -> bool {
        self.valid()
            && self
                .0
                .get(currency)
                .is_some_and(|range| (range.minimum..=range.maximum).contains(&amount))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Create {
    pub email: String,
    pub ui_mode: String,
    pub key: String,
    pub reference: String,
    pub title: String,
    pub currency: String,
    pub amount: i64,
    pub return_url: String,
    pub locale: String,
}
/// The concrete payment-provider boundary also permits deterministic test gateways.
pub trait Gateway: Send + Sync {
    /// Dynamic registries pin an immutable provider identity for a complete operation.
    /// Static gateways already have immutable identity.
    fn pinned(&self) -> Option<std::sync::Arc<dyn Gateway>> {
        None
    }

    /// Dynamic provider catalogs resolve identities explicitly; static adapters accept only themselves.
    fn providers(&self) -> Vec<String> {
        vec![self.provider_id()]
    }
    fn select(&self, _provider: &str) -> Option<std::sync::Arc<dyn Gateway>> {
        None
    }
    fn provider_id(&self) -> String;
    /// Guaranteed positive deduplication window for create and refund requests.
    fn retry_window_seconds(&self) -> u32;

    /// Providers without static bounds use the host's generic method thresholds.
    fn amount_limits(&self) -> Option<AmountLimits> {
        None
    }

    fn available(&self) -> bool {
        true
    }
    /// Hold across enqueue/commit so package deactivation cannot race new durable work.
    fn lease(&self) -> Call<'_, Box<dyn Send>> {
        Box::pin(async { Ok(Box::new(()) as Box<dyn Send>) })
    }
    fn lease_status(&self) -> Call<'_, (Box<dyn Send>, bool)> {
        Box::pin(async {
            let accepting = self.accepting().await?;
            Ok((self.lease().await?, accepting))
        })
    }
    fn accepting(&self) -> Call<'_, bool> {
        Box::pin(async { Ok(self.available()) })
    }
    fn webhook_headers(&self, path: &str) -> Option<Vec<String>> {
        self.webhook_header(path).map(|header| vec![header])
    }
    fn authenticated_headers<'a>(
        &'a self,
        body: &'a [u8],
        headers: &'a BTreeMap<String, String>,
    ) -> Call<'a, WakeEvent> {
        self.authenticated_event(
            body,
            headers
                .get(self.signature_header())
                .map_or("", String::as_str),
        )
    }
    fn webhook_header(&self, path: &str) -> Option<String> {
        (path == self.webhook_path()).then(|| self.signature_header().to_owned())
    }
    fn authenticated_event<'a>(
        &'a self,
        body: &'a [u8],
        signature: &'a str,
    ) -> Call<'a, WakeEvent> {
        Box::pin(async move {
            self.verify_webhook(body, signature)?;
            self.decode_event(body)
        })
    }

    fn decode_event(&self, body: &[u8]) -> Result<WakeEvent>;
    fn checkout_url(&self, value: &str) -> bool;
    fn identifier(&self, value: &str, kind: Identifier) -> bool;
    fn verify_webhook(&self, body: &[u8], signature: &str) -> Result<()>;
    fn webhook_path(&self) -> &'static str;
    fn signature_header(&self) -> &'static str;

    fn create<'a>(&'a self, input: &'a Create) -> Call<'a, Session>;
    fn session<'a>(&'a self, id: &'a str) -> Call<'a, Session>;
    fn refund<'a>(&'a self, intent: &'a str, amount: i64, key: &'a str) -> Call<'a, Refund>;
    fn refunds<'a>(&'a self, intent: &'a str) -> Call<'a, RefundList>;
    fn refund_status<'a>(&'a self, id: &'a str) -> Call<'a, Refund>;
}

/// Provider-neutral authoritative result. Provider-specific objects never cross RPC.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub ui_mode: String,
    pub client_secret: Option<String>,
    pub id: String,
    pub state: PaymentState,
    pub amount: i64,
    pub currency: String,
    pub reference: String,
    pub request_key: String,
    pub url: Option<String>,
    /// Stable provider payment identifier used for reconciliation and refunds.
    pub payment_id: Option<String>,
    pub capture: Option<Capture>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentState {
    Open,
    Processing,
    Paid,
    Expired,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Capture {
    pub amount: i64,
    pub amount_refunded: i64,
    pub currency: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Refund {
    pub id: String,
    pub payment_id: String,
    pub amount: i64,
    pub currency: String,
    pub state: RefundState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundState {
    Pending,
    Succeeded,
    Failed,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RefundList {
    pub data: Vec<Refund>,
    pub has_more: bool,
}

/// Storage/transport envelope only. The adapter validates its own ID semantics.
pub fn valid_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && value.bytes().all(|b| b.is_ascii_graphic())
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Identifier {
    Session,
    Payment,
    Refund,
    Event,
}
pub const BODY_LIMIT: usize = 262_144;
pub fn request_key() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Unavailable)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Serialize, Deserialize)]
pub struct WakeEvent {
    pub id: String,
    pub kind: String,
    pub request_key: String,
    pub object_id: String,
    #[serde(default)]
    pub object_kind: Option<Identifier>,
    pub payment_id: String,
    pub payload_hash: String,
}
