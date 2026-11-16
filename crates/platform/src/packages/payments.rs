//! Optional native payment capability. Lifecycle handshake v3 and manifest v4 form one current contract;
//! this authenticated RPC has its own version and uses the current native SDK.
use super::{Error, Manager, Result};
use crate::payment_gateway::{self as gateway, Call, Gateway, Identifier};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

pub const VERSION: u32 = 2;
pub const LIMIT: usize = 1024 * 1024;
pub const RPC_PATH: &str = "/__talocart_package/payments";
pub const UI_PATH: &str = "/__talocart_package/payment-ui";
/// Checkout pages ask for availability on every render; one answer serves them briefly.
const STATUS_MAX_AGE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub protocol: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_limits: Option<gateway::AmountLimits>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retry_window_seconds: u32,
    pub webhook_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webhook_headers: Vec<String>,
    /// Read-only RPC v1 manifest fields. Never used by the current gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefixes: Option<[String; 5]>,
    pub checkout_host: String,
    pub receipt_policy: String,
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}
impl Descriptor {
    /// Old signed manifests remain readable for inventory and replacement, never execution.
    pub fn validate_structure(&self, id: &str) -> Result<()> {
        if self.protocol != 1 {
            return self.validate(id);
        }
        let header = self.signature_header.as_ref().ok_or(Error::Invalid)?;
        let prefixes = self.prefixes.as_ref().ok_or(Error::Invalid)?;
        if self.amount_limits.is_some()
            || self.retry_window_seconds != 0
            || !self.webhook_headers.is_empty()
            || prefixes.iter().any(|p| {
                p.is_empty()
                    || p.len() > 32
                    || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
        {
            return Err(Error::Invalid);
        }
        let mut current = self.clone();
        current.protocol = VERSION;
        current.retry_window_seconds = 1;
        current.webhook_headers = vec![header.clone()];
        current.signature_header = None;
        current.prefixes = None;
        current.validate(id)
    }

    pub fn validate(&self, id: &str) -> Result<()> {
        if self.protocol != VERSION
            || self
                .amount_limits
                .as_ref()
                .is_some_and(|limits| !limits.valid())
            || self.signature_header.is_some()
            || self.prefixes.is_some()
            || self.retry_window_seconds == 0
            || self.retry_window_seconds > 604_800
            || self.webhook_path != format!("/payments/{id}/webhook")
            || self.webhook_headers.is_empty()
            || self.webhook_headers.len() > 16
            || self.webhook_headers.iter().any(|h| {
                h.is_empty()
                    || h.len() > 64
                    || !h
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || self.checkout_host.is_empty()
            || self.checkout_host.len() > 253
            || !self
                .checkout_host
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
            || self.receipt_policy.len() > 4096
            || self.receipt_policy.is_empty()
            || self.receipt_policy.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn identifier(&self, value: &str, _kind: Identifier) -> bool {
        gateway::valid_identifier(value)
    }
    pub fn checkout_url(&self, value: &str) -> bool {
        value.len() <= 4096
            && !value.chars().any(char::is_control)
            && url::Url::parse(value).is_ok_and(|u| {
                u.scheme() == "https"
                    && u.host_str() == Some(self.checkout_host.as_str())
                    && u.username().is_empty()
                    && u.password().is_none()
                    && u.port_or_known_default() == Some(443)
            })
    }
}
// No Debug: requests and responses may contain customer information or capabilities.
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Asset {
        name: String,
    },
    Create {
        input: gateway::Create,
    },
    Session {
        id: String,
    },
    Refund {
        payment_id: String,
        amount: i64,
        key: String,
    },
    Refunds {
        payment_id: String,
    },
    RefundStatus {
        id: String,
    },
    Event {
        body: Vec<u8>,
        headers: std::collections::BTreeMap<String, String>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    pub protocol: u32,
    pub value: T,
}

impl Manager {
    /// Resolve only the single compatible, fully installed, enabled provider.
    pub fn payment(&self) -> Result<Option<super::Installed>> {
        let mut items = self.payments()?.into_iter();
        let first = items.next();
        Ok(if items.next().is_none() { first } else { None })
    }
    pub fn payments(&self) -> Result<Vec<super::Installed>> {
        Ok(self
            .inventory()?
            .into_iter()
            .filter(|p| p.enabled && p.available() && p.manifest.payment.is_some())
            .collect())
    }
    pub fn payment_for(&self, provider: Option<&str>) -> Result<Option<super::Installed>> {
        match provider {
            Some(id) => Ok(self.payments()?.into_iter().find(|p| p.manifest.id == id)),
            None => self.payment(),
        }
    }
    /// Registration preserves merchant settings and the legacy method ID on upgrade.
    pub async fn register_payment_methods(&self, pool: &sqlx::PgPool) -> Result<()> {
        for item in self
            .inventory()?
            .iter()
            .filter(|p| p.manifest.payment.is_some())
        {
            Self::register_payment_method(pool, &item.manifest).await?;
        }
        Ok(())
    }
    /// Register existing owners first: an offline install after migration must not
    /// let a new provider claim the legacy method of an already-installed package.
    pub(super) async fn register_new_payment_method(
        &self,
        pool: &sqlx::PgPool,
        manifest: &super::Manifest,
    ) -> Result<()> {
        self.register_payment_methods(pool).await?;
        Self::register_payment_method(pool, manifest).await
    }
    pub(super) async fn register_payment_method(
        pool: &sqlx::PgPool,
        manifest: &super::Manifest,
    ) -> Result<()> {
        if manifest.payment.is_none() {
            return Ok(());
        }
        // Manifest titles allow longer labels than editable checkout method names.
        let name_it: String = manifest.title.it.trim().chars().take(120).collect();
        let name_en: String = manifest.title.en.trim().chars().take(120).collect();
        let mut tx = pool.begin().await.map_err(|_| super::Error::Schema)?;
        crate::database::commercial_lock::exclusive(&mut tx)
            .await
            .map_err(|_| super::Error::Schema)?;
        let bound = sqlx::query("UPDATE payment_methods SET provider_id=$1, name_it=CASE WHEN name_it='Pagamento online' THEN $2 ELSE name_it END, name_en=CASE WHEN name_en='Online payment' THEN $3 ELSE name_en END WHERE code='online_checkout' AND provider_id IS NULL AND NOT EXISTS(SELECT 1 FROM payment_methods WHERE provider_id=$1)")
            .bind(&manifest.id).bind(&name_it).bind(&name_en).execute(&mut *tx).await.map_err(|_| super::Error::Schema)?;
        let created = sqlx::query("INSERT INTO payment_methods(code,provider_id,name_it,name_en,sort_order,initial_order_status_id) SELECT 'online_checkout',$1,$2,$3,20,initial_order_status_id FROM payment_methods ORDER BY id LIMIT 1 ON CONFLICT (provider_id) WHERE provider_id IS NOT NULL DO NOTHING")
            .bind(&manifest.id).bind(&name_it).bind(&name_en).execute(&mut *tx).await.map_err(|_| super::Error::Schema)?;
        if bound.rows_affected() + created.rows_affected() > 0 {
            sqlx::query("UPDATE payment_configuration SET revision=revision+1 WHERE id=1")
                .execute(&mut *tx)
                .await
                .map_err(|_| super::Error::Schema)?;
        }
        tx.commit().await.map_err(|_| super::Error::Schema)
    }
    /// Provider calls share the module gate: a slow provider request never blocks
    /// other payments, while deactivation and updates still wait for all of them.
    async fn payment_guard(&self, id: &str) -> gateway::Result<super::ModuleGuard> {
        self.request_guard(id)
            .await
            .map_err(|_| gateway::Error::Unavailable)
    }
    /// Availability for presentation only, reused briefly within one process generation.
    /// Confirmation uses `lease_status`, which always asks the provider, and the provider
    /// rejects creation itself when it is not accepting. Failures are never cached.
    async fn payment_status(&self, provider: Option<&str>) -> gateway::Result<bool> {
        let item = self
            .payment_for(provider)
            .map_err(|_| gateway::Error::Unavailable)?
            .ok_or(gateway::Error::Unavailable)?;
        if provider.is_some_and(|id| id != item.manifest.id) {
            return Err(gateway::Error::Unavailable);
        }
        let id = item.manifest.id.as_str();
        if let Some(accepting) = self.cached_payment_status(id, STATUS_MAX_AGE) {
            return Ok(accepting);
        }
        let _slot = self.admit_payment()?;
        let guard = self.payment_guard(id).await?;
        let accepting = self
            .payment_call_guarded(&guard, RPC_PATH, &Request::Status)
            .await?;
        // Still under the request guard: this generation cannot have been replaced.
        self.remember_payment_status(id, accepting);
        Ok(accepting)
    }
    pub async fn payment_call<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        path: &str,
        value: &T,
    ) -> gateway::Result<R> {
        self.payment_call_for(None, path, value).await
    }
    pub async fn payment_call_for<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        provider: Option<&str>,
        path: &str,
        value: &T,
    ) -> gateway::Result<R> {
        let _slot = self.admit_payment()?;
        let item = self
            .payment_for(provider)
            .map_err(|_| gateway::Error::Unavailable)?
            .ok_or(gateway::Error::Unavailable)?;
        if provider.is_some_and(|id| id != item.manifest.id) {
            return Err(gateway::Error::Unavailable);
        }
        let guard = self.payment_guard(&item.manifest.id).await?;
        self.payment_call_guarded(&guard, path, value).await
    }
    async fn payment_call_guarded<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        guard: &super::ModuleGuard,
        path: &str,
        value: &T,
    ) -> gateway::Result<R> {
        if !matches!(path, RPC_PATH | UI_PATH) {
            return Err(gateway::Error::Invalid);
        }
        let transport = async {
            let (port, token) = guard
                .endpoint()
                .await
                .map_err(|_| gateway::Error::Unavailable)?;
            let bytes = serde_json::to_vec(&Envelope {
                protocol: VERSION,
                value,
            })
            .map_err(|_| gateway::Error::Invalid)?;
            if bytes.len() > LIMIT {
                return Err(gateway::Error::Invalid);
            }
            let mut auth = reqwest::header::HeaderValue::from_str(&token)
                .map_err(|_| gateway::Error::Invalid)?;
            auth.set_sensitive(true);
            let mut response = self
                .payment_client()?
                .post(format!("http://127.0.0.1:{port}{path}"))
                .header("x-talocart-module", auth)
                .header("content-type", "application/json")
                .timeout(Duration::from_secs(45))
                .body(bytes)
                .send()
                .await
                .map_err(|_| gateway::Error::Unavailable)?;
            if !response.status().is_success() {
                return Err(gateway::Error::Unavailable);
            }
            if response.content_length().is_some_and(|n| n > LIMIT as u64) {
                return Err(gateway::Error::Invalid);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| gateway::Error::Unavailable)?
            {
                if bytes.len().saturating_add(chunk.len()) > LIMIT {
                    return Err(gateway::Error::Invalid);
                }
                bytes.extend_from_slice(&chunk);
            }
            let reply: Envelope<gateway::Result<R>> =
                serde_json::from_slice(&bytes).map_err(|_| gateway::Error::Invalid)?;
            if reply.protocol != VERSION {
                return Err(gateway::Error::Invalid);
            }
            Ok(reply.value)
        }
        .await;
        match transport {
            Ok(value) => value,
            Err(error) => {
                guard.record_request_failure();
                Err(error)
            }
        }
    }
}

#[derive(Clone)]
pub struct NativeGateway {
    manager: Manager,
    provider: Option<String>,
}
impl NativeGateway {
    pub fn new(manager: Manager) -> Self {
        Self {
            manager,
            provider: None,
        }
    }
    fn payment(&self) -> super::Result<Option<super::Installed>> {
        self.manager.payment_for(self.provider.as_deref())
    }
    fn descriptor(&self) -> Option<Descriptor> {
        self.payment().ok().flatten()?.manifest.payment
    }
    async fn call<T: DeserializeOwned>(&self, request: Request) -> gateway::Result<T> {
        self.manager
            .payment_call_for(self.provider.as_deref(), RPC_PATH, &request)
            .await
    }
}
impl Gateway for NativeGateway {
    fn providers(&self) -> Vec<String> {
        if let Some(id) = &self.provider {
            return vec![id.clone()];
        }
        self.manager
            .payments()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.manifest.id)
            .collect()
    }
    fn select(&self, provider: &str) -> Option<std::sync::Arc<dyn Gateway>> {
        Some(std::sync::Arc::new(Self {
            manager: self.manager.clone(),
            provider: Some(provider.to_owned()),
        }))
    }
    fn pinned(&self) -> Option<std::sync::Arc<dyn Gateway>> {
        Some(std::sync::Arc::new(Self {
            manager: self.manager.clone(),
            provider: Some(self.provider_id()),
        }))
    }
    fn provider_id(&self) -> String {
        self.provider.clone().unwrap_or_else(|| {
            self.payment()
                .ok()
                .flatten()
                .map(|item| item.manifest.id)
                .unwrap_or_default()
        })
    }
    fn amount_limits(&self) -> Option<gateway::AmountLimits> {
        self.descriptor().and_then(|d| d.amount_limits)
    }
    fn retry_window_seconds(&self) -> u32 {
        self.descriptor().map_or(0, |d| d.retry_window_seconds)
    }

    fn available(&self) -> bool {
        self.descriptor().is_some()
    }
    fn accepting(&self) -> Call<'_, bool> {
        Box::pin(async {
            if !self.available() {
                return Ok(false);
            }
            self.manager.payment_status(self.provider.as_deref()).await
        })
    }
    fn lease_status(&self) -> Call<'_, (Box<dyn Send>, bool)> {
        Box::pin(async {
            let slot = self.manager.admit_payment()?;
            let item = self
                .payment()
                .map_err(|_| gateway::Error::Unavailable)?
                .ok_or(gateway::Error::Unavailable)?;
            let guard = self.manager.payment_guard(&item.manifest.id).await?;
            let accepting = self
                .manager
                .payment_call_guarded(&guard, RPC_PATH, &Request::Status)
                .await?;
            Ok((Box::new((guard, slot)) as Box<dyn Send>, accepting))
        })
    }
    fn lease(&self) -> Call<'_, Box<dyn Send>> {
        Box::pin(async {
            let slot = self.manager.admit_payment()?;
            let item = self
                .payment()
                .map_err(|_| gateway::Error::Unavailable)?
                .ok_or(gateway::Error::Unavailable)?;
            let guard = self.manager.payment_guard(&item.manifest.id).await?;
            // Re-read activation after waiting for the gate. Endpoint verifies the generation.
            guard
                .endpoint()
                .await
                .map_err(|_| gateway::Error::Unavailable)?;
            Ok(Box::new((guard, slot)) as Box<dyn Send>)
        })
    }
    fn webhook_path(&self) -> &'static str {
        "/payments/{provider}/webhook"
    }
    fn signature_header(&self) -> &'static str {
        ""
    }
    fn webhook_headers(&self, path: &str) -> Option<Vec<String>> {
        self.descriptor()
            .filter(|d| d.webhook_path == path)
            .map(|d| d.webhook_headers)
    }
    fn checkout_url(&self, value: &str) -> bool {
        self.descriptor().is_some_and(|d| d.checkout_url(value))
    }
    fn identifier(&self, value: &str, kind: Identifier) -> bool {
        self.descriptor().is_some_and(|d| d.identifier(value, kind))
    }
    // Native event verification and decoding are a single authenticated asynchronous operation.
    fn verify_webhook(&self, _: &[u8], _: &str) -> gateway::Result<()> {
        Err(gateway::Error::Invalid)
    }
    fn decode_event(&self, _: &[u8]) -> gateway::Result<gateway::WakeEvent> {
        Err(gateway::Error::Invalid)
    }
    fn authenticated_headers<'a>(
        &'a self,
        body: &'a [u8],
        headers: &'a std::collections::BTreeMap<String, String>,
    ) -> Call<'a, gateway::WakeEvent> {
        Box::pin(async move {
            self.call(Request::Event {
                body: body.to_vec(),
                headers: headers.clone(),
            })
            .await
        })
    }
    fn create<'a>(&'a self, input: &'a gateway::Create) -> Call<'a, gateway::Session> {
        Box::pin(async move {
            self.call(Request::Create {
                input: input.clone(),
            })
            .await
        })
    }
    fn session<'a>(&'a self, id: &'a str) -> Call<'a, gateway::Session> {
        Box::pin(async move { self.call(Request::Session { id: id.into() }).await })
    }
    fn refund<'a>(
        &'a self,
        payment_id: &'a str,
        amount: i64,
        key: &'a str,
    ) -> Call<'a, gateway::Refund> {
        Box::pin(async move {
            self.call(Request::Refund {
                payment_id: payment_id.into(),
                amount,
                key: key.into(),
            })
            .await
        })
    }
    fn refunds<'a>(&'a self, payment_id: &'a str) -> Call<'a, gateway::RefundList> {
        Box::pin(async move {
            self.call(Request::Refunds {
                payment_id: payment_id.into(),
            })
            .await
        })
    }
    fn refund_status<'a>(&'a self, id: &'a str) -> Call<'a, gateway::Refund> {
        Box::pin(async move { self.call(Request::RefundStatus { id: id.into() }).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn descriptor() -> Descriptor {
        Descriptor {
            protocol: VERSION,
            amount_limits: None,
            retry_window_seconds: 3600,
            webhook_path: "/payments/fixture/webhook".into(),
            webhook_headers: vec!["fixture-signature".into()],
            signature_header: None,
            prefixes: None,
            checkout_host: "checkout.example.test".into(),
            receipt_policy: "default-src 'none'".into(),
        }
    }
    #[test]
    fn declared_amount_bounds_are_validated_and_require_the_current_sdk() {
        let mut d = descriptor();
        d.amount_limits = Some(gateway::AmountLimits(std::collections::BTreeMap::from([(
            "EUR".into(),
            gateway::AmountRange {
                minimum: 50,
                maximum: 1000,
            },
        )])));
        assert!(d.validate("fixture").is_ok());
        let mut manifest = super::super::format::fixture_manifest();
        manifest.payment = Some(d.clone());
        assert!(manifest.validate().is_ok());
        manifest.sdk_version = "5.1.0".into();
        assert!(manifest.validate_structure().is_err());
        let encoded = serde_json::to_vec(&d).unwrap();
        assert_eq!(serde_json::from_slice::<Descriptor>(&encoded).unwrap(), d);
        for (currency, minimum, maximum) in [
            ("eur", 50, 1000),
            ("EU", 50, 1000),
            ("EUR", 0, 1000),
            ("EUR", 50, 49),
        ] {
            d.amount_limits = Some(gateway::AmountLimits(std::collections::BTreeMap::from([(
                currency.into(),
                gateway::AmountRange { minimum, maximum },
            )])));
            assert!(d.validate("fixture").is_err());
        }
    }
    #[test]
    fn signed_capability_limits_paths_identifiers_urls_and_protocol() {
        let d = descriptor();
        assert!(d.validate("fixture").is_ok());
        for url in [
            "http://checkout.example.test/pay",
            "https://checkout.example.test.evil/pay",
            "https://user@checkout.example.test/pay",
            "https://checkout.example.test:444/pay",
            "https://checkout.example.test/\n",
        ] {
            assert!(!d.checkout_url(url), "{url}");
        }
        assert!(d.checkout_url("https://checkout.example.test/pay"));
        assert!(d.identifier("order:123-ABC", Identifier::Session));
        assert!(d.identifier(&"x".repeat(255), Identifier::Payment));
        assert!(!d.identifier(&"x".repeat(256), Identifier::Payment));
        for id in ["", "bad\nvalue", "has space", "è"] {
            assert!(!d.identifier(id, Identifier::Session));
        }
        for mutate in [0, 1, 2, 3, 4, 5] {
            let mut bad = d.clone();
            match mutate {
                0 => bad.protocol += 1,
                1 => bad.webhook_path = "/admin/users".into(),
                2 => bad.webhook_headers = vec!["authorization\n".into()],
                3 => bad.receipt_policy.clear(),
                4 => bad.retry_window_seconds = 0,
                _ => bad.protocol = 1,
            }
            assert!(bad.validate("fixture").is_err());
        }
    }
    #[test]
    fn previous_signed_payment_descriptor_is_readable_but_never_executable() {
        let legacy = serde_json::json!({
            "protocol": 1, "webhook_path": "/payments/fixture/webhook",
            "signature_header": "stripe-signature", "checkout_host": "checkout.example.test",
            "prefixes": ["cs_test_", "pi_", "ch_", "re_", "evt_"],
            "receipt_policy": "default-src 'none'"
        });
        let descriptor: Descriptor = serde_json::from_value(legacy.clone()).unwrap();
        assert!(descriptor.validate_structure("fixture").is_ok());
        assert!(descriptor.validate("fixture").is_err());
        assert_eq!(serde_json::to_value(&descriptor).unwrap(), legacy);
        let mut manifest = super::super::format::fixture_manifest();
        manifest.sdk_version = "3.0.0".into();
        manifest.database.version = 1;
        manifest.payment = Some(descriptor);
        assert!(manifest.validate_structure().is_ok());
        assert!(!manifest.compatible());
        assert!(manifest.validate().is_err());
        manifest.sdk_version = super::super::sdk::VERSION.into();
        manifest.database = crate::database::contract();
        assert!(
            !manifest.compatible(),
            "RPC v1 stays disabled even with current SDK metadata"
        );
    }
    #[test]
    fn current_contract_accepts_admin_and_payment_packages_and_rejects_previous_generation() {
        let old = super::super::format::fixture_manifest();
        let mut value = serde_json::to_value(&old).unwrap();
        value.as_object_mut().unwrap().remove("payment");
        let mut m: super::super::Manifest = serde_json::from_value(value).unwrap();
        assert!(m.validate().is_ok());
        m.payment = Some(descriptor());
        assert!(m.validate().is_ok());
        m.sdk_version = "3.0.0".into();
        assert!(m.validate().is_err());
        m.sdk_version = "4.0.0".into();
        assert!(m.validate().is_err());
        m.sdk_version = super::super::sdk::VERSION.into();
        m.protocol = 2;
        assert!(m.validate().is_err());
        m.protocol = super::super::protocol::VERSION;
        m.format = 3;
        assert!(m.validate().is_err());
    }
}
