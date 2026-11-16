//! Persistent provider work. Claim/commit, external I/O, then fenced application.
use super::*;
use std::sync::Arc;
use talocart_platform::payment_gateway::{
    self as gateway, Gateway, Identifier, PaymentState, RefundState,
};

#[derive(Clone)]
pub struct OnlinePayments {
    pool: PgPool,
    gateway: Arc<dyn Gateway>,
    enabled: bool,
}
#[derive(FromRow)]
pub struct Attempt {
    pub id: i64,
    pub order_id: i64,
    pub state: String,
    pub last_error: String,
    pub session_id: Option<String>,
    pub intent_id: Option<String>,
    pub refund_id: Option<String>,
    pub created_at: String,
}
#[derive(FromRow)]
struct Job {
    provider_id: String,
    ui_mode: String,
    id: i64,
    order_id: i64,
    request_key: String,
    amount_cents: i64,
    currency: String,
    title: String,
    return_url: String,
    locale: String,
    session_id: Option<String>,
    intent_id: Option<String>,
    refund_id: Option<String>,
    action: String,
    refund_reason: Option<String>,
    retry_safe: bool,
    refund_safe: bool,
    lease_token: String,
}
enum Outcome {
    Session(Box<gateway::Session>),
    Refund(gateway::Refund),
    Review(&'static str),
}
impl OnlinePayments {
    fn pinned(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            gateway: self
                .gateway
                .pinned()
                .unwrap_or_else(|| self.gateway.clone()),
            enabled: self.enabled,
        }
    }

    pub fn new(pool: PgPool, gateway: Arc<dyn Gateway>, enabled: bool) -> Self {
        Self {
            pool,
            gateway,
            enabled,
        }
    }
    /// Provider configuration must preserve access to its durable payment history.
    pub async fn has_provider_history(pool: &PgPool, provider: &str) -> Result<bool> {
        Ok(
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM payment_attempts WHERE provider_id=$1)",
            )
            .bind(provider)
            .fetch_one(pool)
            .await?,
        )
    }
    /// A package may stop only when its own orders and attempts no longer need it.
    pub async fn provider_busy(pool: &PgPool, provider: &str) -> Result<bool> {
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_attempts WHERE provider_id=$1 AND (lease_token IS NOT NULL OR state IN ('creating','open','processing','review','refund_pending') OR action IN ('refund','refund_fetch')))").bind(provider).fetch_one(pool).await?;
        if pending {
            return Ok(true);
        }
        let legacy: Option<String> = sqlx::query_scalar(
            "SELECT provider_id FROM payment_methods WHERE id=3 AND code='online_checkout'",
        )
        .fetch_optional(pool)
        .await?
        .flatten();
        let mut after = 0i64;
        loop {
            let rows: Vec<(i64,String,Option<String>)> = sqlx::query_as("SELECT id,snapshot,(SELECT provider_id FROM payment_attempts WHERE order_id=orders.id ORDER BY id LIMIT 1) FROM orders WHERE id>$1 AND state='open' AND payment_state='unpaid' AND snapshot LIKE '%online_checkout%' ORDER BY id LIMIT 250").bind(after).fetch_all(pool).await?;
            if rows.is_empty() {
                return Ok(false);
            }
            for (id, snapshot, attempt_provider) in rows {
                let frozen: Frozen = toml::from_str(&snapshot).map_err(|_| OrderError::Invalid)?;
                if crate::payments::is_online_code(&frozen.payment.code)
                    && frozen
                        .payment
                        .provider_id
                        .as_deref()
                        .or(attempt_provider.as_deref())
                        .or(legacy.as_deref())
                        .is_none_or(|id| id == provider)
                {
                    return Ok(true);
                }
                after = id;
            }
        }
    }
    pub async fn require_providers(pool: &PgPool, available: &[String]) -> Result<()> {
        let mut after = String::new();
        loop {
            let identities: Vec<String> = sqlx::query_scalar("SELECT provider_id FROM (SELECT provider_id FROM payment_methods WHERE provider_id IS NOT NULL UNION SELECT provider_id FROM payment_attempts) providers WHERE provider_id>$1 ORDER BY provider_id LIMIT 250").bind(&after).fetch_all(pool).await?;
            if identities.is_empty() {
                break;
            }
            for provider in identities {
                if !available.contains(&provider) && Self::provider_busy(pool, &provider).await? {
                    return Err(OrderError::ProviderRequired);
                }
                after = provider;
            }
        }
        Self::require_provider(pool, !available.is_empty()).await
    }
    pub async fn require_provider(pool: &PgPool, available: bool) -> Result<()> {
        if available {
            return Ok(());
        }
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_attempts WHERE lease_token IS NOT NULL OR state IN ('creating','open','processing','review','refund_pending') OR action IN ('refund','refund_fetch'))").fetch_one(pool).await?;
        if pending {
            return Err(OrderError::ProviderRequired);
        }
        let mut after = 0i64;
        loop {
            let rows:Vec<(i64,String)>=sqlx::query_as("SELECT id,snapshot FROM orders WHERE id>$1 AND state='open' AND payment_state='unpaid' AND snapshot LIKE '%online_checkout%' ORDER BY id LIMIT 250").bind(after).fetch_all(pool).await?;
            if rows.is_empty() {
                return Ok(());
            }
            for (id, snapshot) in rows {
                let frozen: Frozen = toml::from_str(&snapshot).map_err(|_| OrderError::Invalid)?;
                if crate::payments::is_online_code(&frozen.payment.code) {
                    return Err(OrderError::ProviderRequired);
                }
                after = id;
            }
        }
    }
    pub fn for_provider(&self, id: &str) -> Result<Self> {
        let gateway = match self.gateway.select(id) {
            Some(gateway) => gateway,
            None if self.gateway.provider_id() == id => self.gateway.clone(),
            None => return Err(OrderError::ProviderRequired),
        };
        Ok(Self {
            pool: self.pool.clone(),
            gateway,
            enabled: self.enabled,
        })
    }
    pub async fn for_method(&self, id: Option<i64>) -> Result<Self> {
        let provider: Option<String> = sqlx::query_scalar(
            "SELECT provider_id FROM payment_methods WHERE id=$1 AND code='online_checkout'",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        match provider {
            Some(id) => self.for_provider(&id),
            None => Ok(self.pinned()),
        }
    }
    pub async fn for_order(&self, id: i64) -> Result<Self> {
        let snapshot: String = sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        let frozen: Frozen = toml::from_str(&snapshot).map_err(|_| OrderError::Invalid)?;
        if !crate::payments::is_online_code(&frozen.payment.code) {
            return Err(OrderError::PaymentMethod);
        }
        let provider = match frozen.payment.provider_id {
            Some(id) => Some(id),
            None => {
                let attempt: Option<String> = sqlx::query_scalar("SELECT provider_id FROM payment_attempts WHERE order_id=$1 ORDER BY id LIMIT 1").bind(id).fetch_optional(&self.pool).await?;
                match attempt {
                    Some(id) => Some(id),
                    None => sqlx::query_scalar("SELECT provider_id FROM payment_methods WHERE id=3 AND code='online_checkout'").fetch_optional(&self.pool).await?.flatten(),
                }
            }
        };
        match provider {
            Some(id) => self.for_provider(&id),
            None => Ok(self.pinned()),
        }
    }
    pub async fn available_providers(&self) -> crate::payments::OnlineProviders {
        let mut providers = crate::payments::OnlineProviders::new();
        for id in self.gateway.providers() {
            if let Ok(service) = self.for_provider(&id)
                && service.accepting().await
            {
                providers.insert(id, service.amount_limits());
            }
        }
        providers
    }
    fn for_webhook(&self, path: &str) -> Option<Self> {
        self.gateway
            .providers()
            .into_iter()
            .filter_map(|id| self.for_provider(&id).ok())
            .find(|s| s.gateway.webhook_headers(path).is_some())
    }
    pub fn multiple_providers(&self) -> bool {
        self.gateway.providers().len() > 1
    }
    pub fn provider_id(&self) -> String {
        self.gateway.provider_id()
    }
    pub fn amount_limits(&self) -> Option<gateway::AmountLimits> {
        self.gateway.amount_limits()
    }
    pub fn enabled(&self) -> bool {
        self.enabled && !self.gateway.providers().is_empty()
    }
    pub async fn accepting(&self) -> bool {
        self.enabled() && self.gateway.accepting().await.unwrap_or(false)
    }
    /// Resolve immutable method identity before provider I/O. Confirmation still
    /// validates availability, prices and selection in its own transaction.
    pub async fn confirmation_gate_for(
        &self,
        payment_method_id: Option<i64>,
    ) -> Result<(Box<dyn Send>, bool)> {
        let online: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM payment_methods WHERE id=$1 AND code='online_checkout')",
        )
        .bind(payment_method_id)
        .fetch_one(&self.pool)
        .await?;
        if !online {
            return Ok((Box::new(()), false));
        }
        self.for_method(payment_method_id)
            .await?
            .confirmation_gate()
            .await
    }
    pub async fn confirmation_gate(&self) -> Result<(Box<dyn Send>, bool)> {
        let (guard, accepting) = self
            .gateway
            .lease_status()
            .await
            .map_err(|_| OrderError::PaymentMethod)?;
        Ok((guard, self.enabled && accepting))
    }
    pub fn webhook_path(&self) -> &'static str {
        self.gateway.webhook_path()
    }
    pub fn signature_header(&self) -> &'static str {
        self.gateway.signature_header()
    }
    pub fn verify_webhook(&self, body: &[u8], signature: &str) -> bool {
        self.gateway.verify_webhook(body, signature).is_ok()
    }
    /// The guest and order snapshot are checked again under the same order lock.
    /// No caller-supplied price, reference or return destination reaches the provider.
    pub async fn start(
        &self,
        guest: &Guest,
        csrf: &str,
        reference: &str,
        revision: i64,
        origin: &str,
    ) -> Result<i64> {
        self.start_mode(guest, csrf, reference, revision, origin, "hosted")
            .await
    }
    pub async fn start_elements(
        &self,
        guest: &Guest,
        csrf: &str,
        reference: &str,
        revision: i64,
        origin: &str,
    ) -> Result<i64> {
        self.start_mode(guest, csrf, reference, revision, origin, "elements")
            .await
    }
    async fn start_mode(
        &self,
        guest: &Guest,
        csrf: &str,
        reference: &str,
        revision: i64,
        origin: &str,
        ui_mode: &str,
    ) -> Result<i64> {
        let order = Orders::new(self.pool.clone(), Identity::new(self.pool.clone()))
            .for_guest(guest, reference)
            .await?;
        self.for_order(order.id)
            .await?
            .start_mode_selected(guest, csrf, reference, revision, origin, ui_mode)
            .await
    }
    async fn start_mode_selected(
        &self,
        guest: &Guest,
        csrf: &str,
        reference: &str,
        revision: i64,
        origin: &str,
        ui_mode: &str,
    ) -> Result<i64> {
        guest.check_csrf(csrf)?;
        let (_provider, accepting) = self.confirmation_gate().await?;
        if !accepting {
            return Err(OrderError::PaymentMethod);
        }
        let limits = self.amount_limits();
        let mut tx = self.pool.begin().await?;
        crate::cart::Carts::authorize(&mut tx, guest).await?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT o.id \
                FROM orders o \
                JOIN guest_carts g ON g.id=$1 \
                AND g.token_hash=o.owner_hash \
                AND g.expires_at>now() \
                WHERE o.reference=$2 \
                AND o.owner_hash=$3 FOR UPDATE OF o",
        )
        .bind(guest.id)
        .bind(reference)
        .bind(&guest.token_hash)
        .fetch_optional(&mut *tx)
        .await?;
        let order = load(&mut tx, id.ok_or(OrderError::NotFound)?).await?;
        if order
            .frozen
            .payment
            .provider_id
            .as_ref()
            .is_some_and(|id| *id != self.gateway.provider_id())
        {
            return Err(OrderError::PaymentMethod);
        }
        if !order.is_online() {
            return Err(OrderError::PaymentMethod);
        }
        if order.state != "open" {
            return Err(OrderError::PaymentCancelled);
        }
        if order.payment_state != "unpaid" {
            return Err(OrderError::AlreadyPaid);
        }
        if let Some(id) = sqlx::query_scalar(
            "SELECT id \
            FROM payment_attempts \
            WHERE order_id=$1 \
            AND state IN ('creating','open','processing','review','refund_pending') \
            ORDER BY id DESC \
            LIMIT 1",
        )
        .bind(order.id)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(id);
        }
        if order.revision != revision {
            return Err(CartError::Stale.into());
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payment_attempts WHERE order_id=$1")
                .bind(order.id)
                .fetch_one(&mut *tx)
                .await?;
        if count >= 10
            || !crate::payments::online_amount(&order.currency, order.total_cents)
            || limits
                .as_ref()
                .is_some_and(|limits| !limits.allows(&order.currency, order.total_cents))
        {
            return Err(OrderError::PaymentMethod);
        }
        let key = gateway::request_key().map_err(|_| IdentityError::Unavailable)?;
        let origin = url_origin(origin)?;
        let locale = &order.frozen.customer_locale;
        let primary: String =
            sqlx::query_scalar("SELECT primary_locale FROM storefront_settings WHERE id=1")
                .fetch_one(&mut *tx)
                .await?;
        let path = if locale == &primary {
            String::new()
        } else {
            format!("/{locale}")
        };
        // A return is only a UI hint to wait for verification, never proof of payment.
        let return_url = format!(
            "{origin}{path}/checkout/orders/{}{}?payment_return=1",
            order.reference,
            if ui_mode == "elements" {
                "/payment"
            } else {
                ""
            }
        );
        let title = format!("{} · {}", order.frozen.store_name, order.reference);
        let id=sqlx::query_scalar("INSERT INTO payment_attempts(order_id,request_key,amount_cents,currency,title,return_url,\
                locale,ui_mode,provider_id) \
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) \
            RETURNING id").bind(order.id).bind(key).bind(order.total_cents).bind(&order.currency).bind(title).bind(return_url).bind(locale).bind(ui_mode).bind(self.gateway.provider_id()).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }
    /// Recover an uncertain create after its safe retry window, using an existing
    /// provider session verified against every immutable attempt parameter.
    pub async fn attach_session(
        &self,
        session: &Session,
        csrf: &str,
        order_id: i64,
        attempt_id: i64,
        session_id: &str,
    ) -> Result<()> {
        self.for_order(order_id)
            .await?
            .attach_session_selected(session, csrf, order_id, attempt_id, session_id)
            .await
    }
    async fn attach_session_selected(
        &self,
        session: &Session,
        csrf: &str,
        order_id: i64,
        attempt_id: i64,
        session_id: &str,
    ) -> Result<()> {
        let pinned = self.pinned();
        let service = &pinned;
        Identity::new(service.pool.clone())
            .authorize(session, Permission::OrdersWrite)
            .await?;
        session.check_csrf(csrf)?;
        if !service.gateway.identifier(session_id, Identifier::Session) {
            return Err(OrderError::Invalid);
        }
        let j:Job=sqlx::query_as("SELECT provider_id,ui_mode,id,order_id,request_key,amount_cents,currency,title,return_url,locale,\
            session_id,intent_id,refund_id,action,refund_reason,true AS retry_safe,false AS refund_safe,\
            coalesce(lease_token,'') AS lease_token \
            FROM payment_attempts \
            WHERE id=$1 \
            AND order_id=$2").bind(attempt_id).bind(order_id).fetch_optional(&service.pool).await?.ok_or(OrderError::NotFound)?;
        if j.provider_id != service.gateway.provider_id() {
            return Err(OrderError::PaymentMethod);
        }
        let reference: String = sqlx::query_scalar("SELECT reference FROM orders WHERE id=$1")
            .bind(order_id)
            .fetch_one(&service.pool)
            .await?;
        let provider = service
            .gateway
            .session(session_id)
            .await
            .map_err(|_| OrderError::Invalid)?;
        if !valid_session(service.gateway.as_ref(), &j, &provider, &reference)
            || j.session_id.as_ref().is_some_and(|id| id != session_id)
        {
            return Err(OrderError::Invalid);
        }
        let mut tx = service.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await?;
        let n = sqlx::query(
            "UPDATE payment_attempts \
                SET session_id=$3,action='fetch',error_count=0,last_error='',next_at=clock_timestamp(),\
                updated_at=clock_timestamp() \
                WHERE id=$1 \
                AND order_id=$2 \
                AND (session_id IS NULL \
                    OR session_id=$3) \
                AND state<>'refunded' \
                AND (lease_until IS NULL \
                    OR lease_until<clock_timestamp())",
        )
        .bind(attempt_id)
        .bind(order_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n != 1 {
            return Err(CartError::Stale.into());
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn checkout(&self, guest: &Guest, reference: &str) -> Result<Option<String>> {
        let order = Orders::new(self.pool.clone(), Identity::new(self.pool.clone()))
            .for_guest(guest, reference)
            .await?;
        self.for_order(order.id)
            .await?
            .checkout_selected(guest, reference)
            .await
    }
    async fn checkout_selected(&self, guest: &Guest, reference: &str) -> Result<Option<String>> {
        let order = Orders::new(self.pool.clone(), Identity::new(self.pool.clone()))
            .for_guest(guest, reference)
            .await?;
        if order.state != "open" || order.payment_state != "unpaid" {
            return Ok(None);
        }
        let url: Option<String> = sqlx::query_scalar(
            "SELECT checkout_url \
            FROM payment_attempts \
            WHERE order_id=$1 \
            AND state='open' \
            ORDER BY id DESC \
            LIMIT 1",
        )
        .bind(order.id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        Ok(url.filter(|u| self.gateway.checkout_url(u)))
    }
    /// The client secret is a transient capability: only the owning, unpaid order can receive it.
    /// Fetching the provider happens outside a transaction and never marks the order paid.
    pub async fn element_secret(&self, guest: &Guest, reference: &str) -> Result<Option<String>> {
        let order = Orders::new(self.pool.clone(), Identity::new(self.pool.clone()))
            .for_guest(guest, reference)
            .await?;
        self.for_order(order.id)
            .await?
            .element_secret_selected(guest, reference)
            .await
    }
    async fn element_secret_selected(
        &self,
        guest: &Guest,
        reference: &str,
    ) -> Result<Option<String>> {
        let pinned = self.pinned();
        let service = &pinned;
        let orders = Orders::new(service.pool.clone(), Identity::new(service.pool.clone()));
        let order = orders.for_guest(guest, reference).await?;
        if order.state != "open" || order.payment_state != "unpaid" || order.online_review {
            return Ok(None);
        }
        let job: Option<Job> = sqlx::query_as("SELECT provider_id,ui_mode,id,order_id,request_key,amount_cents,currency,title,return_url,locale,\
            session_id,intent_id,refund_id,action,refund_reason,true AS retry_safe,false AS refund_safe,\
            coalesce(lease_token,'') AS lease_token \
            FROM payment_attempts \
            WHERE order_id=$1 \
            AND ui_mode='elements' \
            AND state='open' \
            ORDER BY id DESC \
            LIMIT 1").bind(order.id).fetch_optional(&service.pool).await?;
        let Some(job) = job else {
            return Ok(None);
        };
        if job.provider_id != service.gateway.provider_id() {
            return Ok(None);
        }
        let provider = service
            .gateway
            .session(job.session_id.as_deref().ok_or(OrderError::Invalid)?)
            .await
            .map_err(|_| OrderError::PaymentMethod)?;
        if !valid_session(service.gateway.as_ref(), &job, &provider, reference)
            || provider.state != PaymentState::Open
        {
            return Ok(None);
        }
        let current = orders.for_guest(guest, reference).await?;
        if current.state != "open" || current.payment_state != "unpaid" || current.online_review {
            return Ok(None);
        }
        Ok(provider.client_secret.filter(|secret| {
            !secret.is_empty() && secret.len() <= 4096 && !secret.chars().any(char::is_whitespace)
        }))
    }
    pub async fn reconcile(
        &self,
        session: &Session,
        csrf: &str,
        order_id: i64,
        attempt_id: i64,
    ) -> Result<()> {
        self.for_order(order_id)
            .await?
            .reconcile_selected(session, csrf, order_id, attempt_id)
            .await
    }
    async fn reconcile_selected(
        &self,
        session: &Session,
        csrf: &str,
        order_id: i64,
        attempt_id: i64,
    ) -> Result<()> {
        let _provider = self
            .gateway
            .lease()
            .await
            .map_err(|_| OrderError::PaymentMethod)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(order_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(OrderError::NotFound)?;
        // Preserve the exact request and its original idempotency window.
        let n = sqlx::query(
            "UPDATE payment_attempts \
                SET error_count=0,last_error='',action=CASE \
                WHEN refund_id IS NOT NULL THEN 'refund_fetch' \
                WHEN refund_reason IS NOT NULL THEN 'refund' \
                WHEN session_id IS NULL THEN 'create' \
                ELSE 'fetch' END,next_at=clock_timestamp(),updated_at=clock_timestamp() \
                WHERE id=$1 \
                AND order_id=$2 \
                AND state<>'refunded' \
                AND (lease_until IS NULL \
                    OR lease_until<clock_timestamp())",
        )
        .bind(attempt_id)
        .bind(order_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n != 1 {
            return Err(CartError::Stale.into());
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn request_refund(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        self.for_order(id)
            .await?
            .request_refund_selected(session, csrf, id, revision)
            .await
    }
    async fn request_refund_selected(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        let _provider = self
            .gateway
            .lease()
            .await
            .map_err(|_| OrderError::PaymentMethod)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(OrderError::NotFound)?;
        let order = load(&mut tx, id).await?;
        if order.is_refunded() {
            tx.commit().await?;
            return Ok(());
        }
        if order.fulfillment.sent() {
            return Err(OrderError::Shipped);
        }
        if !order.is_online() || !order.is_paid() {
            return Err(OrderError::PaymentMethod);
        }
        let provider: String = sqlx::query_scalar("SELECT a.provider_id FROM payment_attempts a JOIN order_online_payments p ON p.attempt_id=a.id WHERE p.order_id=$1")
            .bind(id).fetch_one(&mut *tx).await?;
        if provider != self.gateway.provider_id() {
            return Err(OrderError::PaymentMethod);
        }
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 \
                FROM payment_attempts a \
                JOIN order_online_payments p ON p.attempt_id=a.id \
                WHERE p.order_id=$1 \
                AND a.refund_reason IS NOT NULL)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !pending && order.revision != revision {
            return Err(CartError::Stale.into());
        }
        sqlx::query("UPDATE payment_attempts a \
            SET state='refund_pending',refund_reason=coalesce(refund_reason,'admin_cancel'),refund_requested_at=coalesce(refund_requested_at,\
                clock_timestamp()),action=CASE \
            WHEN refund_id IS NULL THEN 'refund' \
            ELSE 'refund_fetch' END,error_count=0,next_at=clock_timestamp(),updated_at=clock_timestamp() \
            FROM order_online_payments p \
            WHERE p.order_id=$1 \
            AND p.attempt_id=a.id").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub fn webhook_headers(&self, path: &str) -> Option<Vec<String>> {
        self.for_webhook(path)
            .and_then(|s| s.gateway.webhook_headers(path))
    }
    pub async fn webhook_with_headers(
        &self,
        path: &str,
        body: &[u8],
        headers: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let pinned = self.for_webhook(path).ok_or(OrderError::Invalid)?;
        let service = &pinned;
        if service.gateway.webhook_headers(path).is_none() {
            return Err(OrderError::Invalid);
        }
        let event = service
            .gateway
            .authenticated_headers(body, headers)
            .await
            .map_err(|e| match e {
                gateway::Error::Invalid | gateway::Error::Rejected => OrderError::Invalid,
                _ => OrderError::ProviderRequired,
            })?;
        service.record_event(event).await
    }
    pub fn webhook_header(&self, path: &str) -> Option<String> {
        self.gateway.webhook_header(path)
    }
    pub async fn webhook(&self, body: &[u8], signature: &str) -> Result<()> {
        let pinned = self.pinned();
        let service = &pinned;
        let event = service
            .gateway
            .authenticated_event(body, signature)
            .await
            .map_err(|e| match e {
                gateway::Error::Invalid | gateway::Error::Rejected => OrderError::Invalid,
                _ => OrderError::ProviderRequired,
            })?;
        service.record_event(event).await
    }
    /// Valid signatures are checked by the HTTP boundary. Store no raw webhook data.
    /// Events only wake authoritative API reconciliation; payload status is never money evidence.
    pub async fn event(&self, body: &[u8]) -> Result<()> {
        let pinned = self.pinned();
        let service = &pinned;
        let event = service
            .gateway
            .decode_event(body)
            .map_err(|_| OrderError::Invalid)?;
        service.record_event(event).await
    }
    async fn record_event(&self, event: gateway::WakeEvent) -> Result<()> {
        if !self.gateway.identifier(&event.id, Identifier::Event) {
            return Err(OrderError::Invalid);
        }
        let gateway::WakeEvent {
            id,
            kind,
            request_key: key,
            object_id: object,
            object_kind,
            payment_id: intent,
            payload_hash: digest,
        } = event;
        let object_type = match object_kind {
            Some(Identifier::Session) => "session",
            Some(Identifier::Payment) => "payment",
            Some(Identifier::Refund) => "refund",
            Some(Identifier::Event) => return Err(OrderError::Invalid),
            None => "",
        };
        if object_kind.is_some_and(|kind| !self.gateway.identifier(&object, kind))
            || (!intent.is_empty() && !self.gateway.identifier(&intent, Identifier::Payment))
        {
            return Err(OrderError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let attempts: Vec<i64> = sqlx::query_scalar(
            "SELECT id \
            FROM payment_attempts \
            WHERE provider_id=$4 AND (request_key=$1 \
            OR ($5='session' AND session_id=$2) \
            OR ($5='payment' AND intent_id=$2) \
            OR ($5='refund' AND refund_id=$2) \
            OR (intent_id=$3 \
                AND $3<>'')) \
            ORDER BY id \
            LIMIT 2",
        )
        .bind(key)
        .bind(object)
        .bind(intent)
        .bind(self.gateway.provider_id())
        .bind(object_type)
        .fetch_all(&mut *tx)
        .await?;
        if attempts.len() > 1 {
            return Err(OrderError::Invalid);
        }
        let attempt = attempts.first().copied();
        let inserted = sqlx::query(
            "INSERT INTO online_payment_events(id,kind,payload_hash,attempt_id,provider_id) \
            VALUES($1,$2,$3,$4,$5) \
            ON CONFLICT(provider_id,id) DO NOTHING",
        )
        .bind(&id)
        .bind(&kind)
        .bind(&digest)
        .bind(attempt)
        .bind(self.gateway.provider_id())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        let existing: String = sqlx::query_scalar(
            "SELECT payload_hash FROM online_payment_events WHERE id=$1 AND provider_id=$2",
        )
        .bind(&id)
        .bind(self.gateway.provider_id())
        .fetch_one(&mut *tx)
        .await?;
        if existing != digest {
            return Err(OrderError::Invalid);
        }
        if let Some(attempt) = attempt.filter(|_| inserted) {
            sqlx::query(
                "UPDATE payment_attempts \
                    SET next_at=clock_timestamp(),action=CASE \
                    WHEN action='none' \
                    AND session_id IS NOT NULL \
                    AND state IN ('paid','expired','failed') THEN 'fetch' \
                    ELSE action END \
                    WHERE id=$1 \
                    AND state<>'refunded'",
            )
            .bind(attempt)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn process_one(&self) -> Result<bool> {
        let mut worked = false;
        for provider in self.gateway.providers() {
            worked |= self.for_provider(&provider)?.process(None).await?;
        }
        Ok(worked)
    }
    /// Start only the requested attempt; never delay a browser behind unrelated jobs.
    pub async fn process_attempt(&self, id: i64) -> Result<bool> {
        let provider: String =
            sqlx::query_scalar("SELECT provider_id FROM payment_attempts WHERE id=$1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        self.for_provider(&provider)?.process(Some(id)).await
    }
    async fn process(&self, id: Option<i64>) -> Result<bool> {
        let pinned = self.pinned();
        let service = &pinned;
        let token = gateway::request_key().map_err(|_| IdentityError::Unavailable)?;
        let job:Option<Job>=sqlx::query_as("UPDATE payment_attempts \
            SET lease_token=$1,lease_until=clock_timestamp()+interval '2 minutes' \
            WHERE id=(SELECT id \
                FROM payment_attempts \
                WHERE ($2::bigint IS NULL \
                    OR id=$2) \
                AND provider_id=$3 AND action<>'none' \
                AND next_at<=clock_timestamp() \
                AND (lease_until IS NULL \
                    OR lease_until<clock_timestamp()) \
                ORDER BY next_at,id FOR UPDATE SKIP LOCKED \
                LIMIT 1) \
            RETURNING *,created_at>clock_timestamp()-make_interval(secs => $4) AS retry_safe,coalesce(refund_requested_at>clock_timestamp()-make_interval(secs => $4),false) AS refund_safe")
            .bind(token).bind(id).bind(service.gateway.provider_id()).bind(f64::from(service.gateway.retry_window_seconds())).fetch_optional(&service.pool).await?;
        let Some(job) = job else {
            return Ok(false);
        };
        let outcome = service.perform(&job).await;
        let result = service.apply(&job, outcome).await;
        if result.is_err() {
            // A failed commercial transaction rolls back all effects, then makes the
            // same provider object retryable. A crash instead leaves an expiring lease.
            sqlx::query("UPDATE payment_attempts \
                SET lease_token=NULL,lease_until=NULL,error_count=least(error_count+1,5),last_error='temporary',\
                state=CASE \
                WHEN error_count>=4 THEN 'review' \
                ELSE state END,action=CASE \
                WHEN error_count>=4 THEN 'none' \
                ELSE action END,next_at=clock_timestamp()+interval '30 seconds' \
                WHERE id=$1 \
                AND lease_token=$2").bind(job.id).bind(&job.lease_token).execute(&service.pool).await?;
        }
        result?;
        Ok(true)
    }
    async fn perform(&self, j: &Job) -> gateway::Result<Outcome> {
        if j.provider_id != self.gateway.provider_id() {
            return Err(gateway::Error::Unavailable);
        }
        match j.action.as_str() {
            "create" => {
                if !j.retry_safe {
                    return Ok(Outcome::Review("retry_window"));
                }
                let (reference, snapshot): (String, String) =
                    sqlx::query_as("SELECT reference,snapshot FROM orders WHERE id=$1")
                        .bind(j.order_id)
                        .fetch_one(&self.pool)
                        .await
                        .map_err(|_| gateway::Error::Unavailable)?;
                let frozen: Frozen =
                    toml::from_str(&snapshot).map_err(|_| gateway::Error::Invalid)?;
                let i = gateway::Create {
                    email: frozen.customer.email,
                    ui_mode: j.ui_mode.clone(),
                    key: j.request_key.clone(),
                    reference,
                    title: j.title.clone(),
                    currency: j.currency.clone(),
                    amount: j.amount_cents,
                    return_url: j.return_url.clone(),
                    locale: j.locale.clone(),
                };
                let mut s = self.gateway.create(&i).await?;
                if s.state == PaymentState::Paid {
                    s = self.gateway.session(&s.id).await?;
                }
                Ok(Outcome::Session(Box::new(s)))
            }
            "fetch" => Ok(Outcome::Session(Box::new(
                self.gateway
                    .session(j.session_id.as_deref().ok_or(gateway::Error::Invalid)?)
                    .await?,
            ))),
            "refund_fetch" => Ok(Outcome::Refund(
                self.gateway
                    .refund_status(j.refund_id.as_deref().ok_or(gateway::Error::Invalid)?)
                    .await?,
            )),
            "refund" => {
                let intent = j.intent_id.as_deref().ok_or(gateway::Error::Invalid)?;
                let refunds = self.gateway.refunds(intent).await?;
                if refunds.has_more || refunds.data.iter().any(|r| r.amount != j.amount_cents) {
                    return Ok(Outcome::Review("partial_refund"));
                }
                if let Some(r) = refunds
                    .data
                    .into_iter()
                    .find(|r| r.state == RefundState::Succeeded || r.state == RefundState::Pending)
                {
                    return Ok(Outcome::Refund(r));
                }
                if !j.refund_safe {
                    return Ok(Outcome::Review("retry_window"));
                }
                Ok(Outcome::Refund(
                    self.gateway
                        .refund(intent, j.amount_cents, &j.request_key)
                        .await?,
                ))
            }
            _ => Err(gateway::Error::Invalid),
        }
    }
    async fn apply(&self, j: &Job, outcome: gateway::Result<Outcome>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(j.order_id)
            .fetch_one(&mut *tx)
            .await?;
        let current: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT action,refund_reason \
            FROM payment_attempts \
            WHERE id=$1 \
            AND lease_token=$2 \
            AND lease_until>clock_timestamp() FOR UPDATE",
        )
        .bind(j.id)
        .bind(&j.lease_token)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((action, reason)) = current else {
            tx.rollback().await?;
            return Ok(());
        };
        // An administrator may have requested a refund while a fetch was in flight.
        if action != j.action || reason != j.refund_reason {
            sqlx::query("UPDATE payment_attempts SET lease_token=NULL,lease_until=NULL,next_at=clock_timestamp() WHERE id=$1").bind(j.id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        }
        let order = load(&mut tx, j.order_id).await?;
        match outcome {
            Ok(Outcome::Review(reason)) => review(&mut tx, j.id, reason).await?,
            Err(e) => {
                let reason = match e {
                    gateway::Error::Unavailable => "temporary",
                    gateway::Error::Rejected => "rejected",
                    gateway::Error::NotFound => "missing",
                    gateway::Error::Invalid => "invalid",
                };
                // Never declare a transport failure an unpaid terminal outcome.
                sqlx::query("UPDATE payment_attempts \
                    SET error_count=least(error_count+1,5),last_error=$2,state=CASE \
                    WHEN error_count>=4 \
                    OR $2<>'temporary' THEN 'review' \
                    ELSE state END,action=CASE \
                    WHEN error_count>=4 \
                    OR $2<>'temporary' THEN 'none' \
                    ELSE action END,next_at=clock_timestamp()+interval '30 seconds'*(error_count+1) \
                    WHERE id=$1").bind(j.id).bind(reason).execute(&mut *tx).await?;
            }
            Ok(Outcome::Session(s)) => {
                if !valid_session(self.gateway.as_ref(), j, &s, &order.reference) {
                    review(&mut tx, j.id, "invalid").await?;
                } else {
                    let intent = s.payment_id.as_deref();
                    let compatible = j.session_id.as_ref().is_none_or(|id| id == &s.id)
                        && j.intent_id.as_deref().is_none_or(|id| Some(id) == intent);
                    if !compatible {
                        review(&mut tx, j.id, "invalid").await?;
                    } else {
                        sqlx::query("UPDATE payment_attempts \
                            SET session_id=$2,intent_id=coalesce(intent_id,$3),checkout_url=$4,error_count=0,\
                            last_error='' \
                            WHERE id=$1").bind(j.id).bind(&s.id).bind(intent).bind(&s.url).execute(&mut *tx).await?;
                        if s.state == PaymentState::Paid {
                            if let Some(charge) = paid_charge(self.gateway.as_ref(), j, &s) {
                                let intent = intent.ok_or(OrderError::Invalid)?;
                                sqlx::query("INSERT INTO online_captures(attempt_id,intent_id,amount_cents,currency,provider_id) \
                                    VALUES($1,$2,$3,$4,$5) \
                                    ON CONFLICT(attempt_id) DO NOTHING").bind(j.id).bind(intent).bind(j.amount_cents).bind(&j.currency).bind(&j.provider_id).execute(&mut *tx).await?;
                                let canonical:Option<i64>=sqlx::query_scalar("SELECT attempt_id FROM order_online_payments WHERE order_id=$1").bind(j.order_id).fetch_optional(&mut *tx).await?;
                                let first = canonical.is_none();
                                if first {
                                    sqlx::query("INSERT INTO order_online_payments(order_id,attempt_id) VALUES($1,$2)").bind(j.order_id).bind(j.id).execute(&mut *tx).await?;
                                    if order.state == "open" && charge.amount_refunded == 0 {
                                        let paid = load(&mut tx, j.order_id).await?;
                                        enqueue(&mut tx, &paid, Event::OnlinePaymentRecorded)
                                            .await?;
                                    }
                                }
                                let reason = if !first && canonical != Some(j.id) {
                                    Some("duplicate")
                                } else if order.state == "cancelled" {
                                    Some("late")
                                } else if charge.amount_refunded > 0 {
                                    Some("external")
                                } else {
                                    None
                                };
                                if let Some(reason) = reason {
                                    if charge.amount_refunded > 0
                                        && charge.amount_refunded != j.amount_cents
                                    {
                                        review(&mut tx, j.id, "partial_refund").await?;
                                    } else {
                                        sqlx::query("UPDATE payment_attempts \
                                            SET state='refund_pending',action='refund',refund_reason=coalesce(refund_reason,$2),\
                                            refund_requested_at=coalesce(refund_requested_at,clock_timestamp()),next_at=clock_timestamp() \
                                            WHERE id=$1").bind(j.id).bind(reason).execute(&mut *tx).await?;
                                    }
                                } else {
                                    // A webhook received during this two-minute lease advances
                                    // next_at beyond the claim time. Preserve that wake-up;
                                    // otherwise paid orders only need a daily fallback.
                                    sqlx::query("UPDATE payment_attempts \
                                        SET state='paid',action='fetch',next_at=CASE \
                                        WHEN next_at > lease_until - interval '2 minutes' THEN clock_timestamp() \
                                        ELSE clock_timestamp()+interval '24 hours' END \
                                        WHERE id=$1").bind(j.id).execute(&mut *tx).await?;
                                }
                            } else {
                                review(&mut tx, j.id, "invalid").await?;
                            }
                        } else if s.state == PaymentState::Expired {
                            sqlx::query("UPDATE payment_attempts SET state='expired',action='none',checkout_url=NULL WHERE id=$1").bind(j.id).execute(&mut *tx).await?;
                        } else {
                            sqlx::query("UPDATE payment_attempts \
                                SET state=$2,action='fetch',next_at=clock_timestamp()+interval '30 seconds' \
                                WHERE id=$1").bind(j.id).bind(if s.state == PaymentState::Processing {"processing"} else {"open"}).execute(&mut *tx).await?;
                        }
                    }
                }
            }
            Ok(Outcome::Refund(r)) => {
                if !self.gateway.identifier(&r.id, Identifier::Refund)
                    || Some(r.payment_id.as_str()) != j.intent_id.as_deref()
                    || r.amount != j.amount_cents
                    || !r.currency.eq_ignore_ascii_case(&j.currency)
                    || j.refund_id.as_ref().is_some_and(|id| id != &r.id)
                {
                    review(&mut tx, j.id, "invalid").await?;
                } else {
                    sqlx::query("UPDATE payment_attempts SET refund_id=$2,error_count=0,last_error='' WHERE id=$1").bind(j.id).bind(&r.id).execute(&mut *tx).await?;
                    if r.state == RefundState::Succeeded {
                        let reason = j.refund_reason.as_deref().ok_or(OrderError::Invalid)?;
                        let inserted=sqlx::query("INSERT INTO online_refunds(attempt_id,refund_id,amount_cents,currency,reason,provider_id) \
                            VALUES($1,$2,$3,$4,$5,$6) \
                            ON CONFLICT(attempt_id) DO NOTHING").bind(j.id).bind(&r.id).bind(j.amount_cents).bind(&j.currency).bind(reason).bind(&j.provider_id).execute(&mut *tx).await?.rows_affected();
                        let canonical:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM order_online_payments WHERE order_id=$1 AND attempt_id=$2)").bind(j.order_id).bind(j.id).fetch_one(&mut *tx).await?;
                        if inserted == 1 && canonical {
                            if order.state == "open" && !order.fulfillment.sent() {
                                restore(&mut tx, &order).await?;
                                sqlx::query("UPDATE orders SET state='cancelled',revision=revision+1,cancelled_at=clock_timestamp() WHERE id=$1").bind(order.id).execute(&mut *tx).await?;
                            }
                            let refunded = load(&mut tx, order.id).await?;
                            enqueue(&mut tx, &refunded, Event::Refunded).await?;
                        }
                        sqlx::query("UPDATE payment_attempts SET state='refunded',action='none',next_at=clock_timestamp() WHERE id=$1").bind(j.id).execute(&mut *tx).await?;
                    } else if r.state == RefundState::Pending {
                        sqlx::query("UPDATE payment_attempts \
                            SET state='refund_pending',action='refund_fetch',next_at=clock_timestamp()+interval '30 seconds' \
                            WHERE id=$1").bind(j.id).execute(&mut *tx).await?;
                    } else {
                        review(&mut tx, j.id, "rejected").await?;
                    }
                }
            }
        }
        sqlx::query(
            "UPDATE payment_attempts \
            SET lease_token=NULL,lease_until=NULL,updated_at=clock_timestamp() \
            WHERE id=$1",
        )
        .bind(j.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
async fn review(tx: &mut Transaction<'_, Postgres>, id: i64, reason: &str) -> Result<()> {
    sqlx::query(
        "UPDATE payment_attempts SET state='review',action='none',last_error=$2 WHERE id=$1",
    )
    .bind(id)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
fn valid_session(provider: &dyn Gateway, j: &Job, s: &gateway::Session, reference: &str) -> bool {
    j.provider_id == provider.provider_id()
        && s.ui_mode == j.ui_mode
        && provider.identifier(&s.id, Identifier::Session)
        && j.session_id.as_ref().is_none_or(|id| id == &s.id)
        && (j.ui_mode != "elements" || s.url.is_none())
        && s.amount == j.amount_cents
        && s.currency.eq_ignore_ascii_case(&j.currency)
        && s.reference == reference
        && s.request_key == j.request_key
        && s.url.as_ref().is_none_or(|u| provider.checkout_url(u))
        && s.payment_id
            .as_ref()
            .is_none_or(|id| provider.identifier(id, Identifier::Payment))
}
fn paid_charge<'a>(
    _provider: &dyn Gateway,
    j: &Job,
    s: &'a gateway::Session,
) -> Option<&'a gateway::Capture> {
    let c = s.capture.as_ref()?;
    (s.state == PaymentState::Paid
        && s.payment_id.is_some()
        && c.amount == j.amount_cents
        && c.currency.eq_ignore_ascii_case(&j.currency)
        && (0..=j.amount_cents).contains(&c.amount_refunded))
    .then_some(c)
}
fn url_origin(origin: &str) -> Result<&str> {
    // Origin comes from trusted configuration and the HTTP-layer authority check.
    let parsed = url::Url::parse(origin).map_err(|_| OrderError::Invalid)?;
    if matches!(parsed.scheme(), "http" | "https")
        && (parsed.scheme() == "https"
            || parsed
                .host_str()
                .is_some_and(|h| matches!(h, "127.0.0.1" | "[::1]" | "localhost")))
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
    {
        Ok(origin.trim_end_matches('/'))
    } else {
        Err(OrderError::Invalid)
    }
}

impl Orders {
    pub async fn attempts(&self, session: &Session, id: i64) -> Result<Vec<Attempt>> {
        Identity::new(self.pool.clone())
            .authorize(session, Permission::OrdersRead)
            .await?;
        Ok(sqlx::query_as("SELECT id,order_id,state,last_error,session_id,intent_id,refund_id,platform_datetime(created_at,\
                r.time_zone,r.date_format) AS created_at \
            FROM payment_attempts \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE r.singleton \
            AND order_id=$1 \
            ORDER BY id DESC \
            LIMIT 10").bind(id).fetch_all(&self.pool).await?)
    }
}
