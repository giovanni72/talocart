//! Persistent guest and customer carts. Prices are observations, never browser-authorized values.
use crate::{
    pricing::{self, LineTotals, Money, PriceMode, Rate},
    products::{CartMerchandise, Products, PurchaseQuantities},
};
use sqlx::{PgPool, Postgres, Transaction};
use talocart_platform::{
    content::Language,
    identity::{self, IdentityError},
};
use thiserror::Error;
mod customer;

pub const ITEM_LIMIT: usize = 100;
pub const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;
#[derive(Debug, Error)]
pub enum CartError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Coupon unavailable")]
    Coupon,
    #[error("Invalid cart input")]
    Invalid,
    #[error("Cart session expired or invalid")]
    Forbidden,
    #[error("Cart changed; reload before retrying")]
    Stale,
    #[error("Item is no longer available")]
    Unavailable,
    #[error("Selected checkout method is unavailable")]
    Methods,
    #[error("Quantity must respect minimum and packs")]
    Quantity,
    #[error("Insufficient stock")]
    Stock,
    #[error("Cart line limit reached")]
    Limit,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for CartError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
/// Classifies failures for transaction retry without inspecting user-visible text.
impl CartError {
    pub fn technical(&self) -> Option<talocart_platform::technical::TechnicalError> {
        match self {
            Self::Technical(error) => Some(error.clone()),
            Self::Identity(error) => error.technical(),
            _ => None,
        }
    }
}
pub type Result<T> = std::result::Result<T, CartError>;
/// No Debug: neither session credentials nor CSRF values belong in diagnostics.
#[derive(Clone, sqlx::FromRow)]
pub struct Guest {
    pub id: i64,
    pub csrf: String,
    pub revision: i64,
    pub count: i64,
    pub(crate) token_hash: String,
    #[sqlx(skip)]
    customer: Option<talocart_platform::identity::CustomerSession>,
}
impl Guest {
    pub(crate) fn customer_id(&self) -> Option<i64> {
        self.customer.as_ref().and_then(|s| s.customer_id())
    }
    pub fn check_csrf(&self, csrf: &str) -> Result<()> {
        identity::check_csrf(&self.csrf, csrf).map_err(|_| CartError::Forbidden)
    }
}
#[derive(Clone)]
pub struct Carts {
    pub(crate) online_available: bool,
    pub(crate) online_providers: Option<crate::payments::OnlineProviders>,
    pub(crate) online_limits: Option<talocart_platform::payment_gateway::AmountLimits>,
    pub(crate) pool: PgPool,
}
#[derive(sqlx::FromRow)]
struct StoredItem {
    id: i64,
    product_id: Option<i64>,
    variant_id: Option<i64>,
    had_variant: bool,
    quantity: i32,
    name: String,
    code: String,
    unit_cents: i64,
    basis_points: i32,
    price_mode: String,
    currency: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Issue {
    Unavailable,
    Quantity,
    Stock,
}
pub struct CartLine {
    pub id: i64,
    pub quantity: i32,
    pub name: String,
    pub code: String,
    pub current: Option<CartMerchandise>,
    pub totals: Option<LineTotals>,
    pub issue: Option<Issue>,
    pub changed: bool,
}
pub struct Quote {
    pub group_discount: Option<crate::group_discounts::Snapshot>,
    pub coupon: Option<crate::coupons::Snapshot>,
    pub coupon_code: String,
    pub coupon_invalid: bool,
    pub overflow: bool,
    pub revision: i64,
    pub lines: Vec<CartLine>,
    pub totals: Option<LineTotals>,
    pub currency: String,
}
/// Backlog count is capped at 1000 so diagnostics remain bounded.
pub struct PruneReport {
    pub deleted: u64,
    pub remaining_at_least: i64,
}
impl Carts {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            online_available: false,
            online_limits: None,
            online_providers: None,
        }
    }
    pub fn with_online(mut self, available: bool) -> Self {
        self.online_available = available;
        self
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
    /// Read only, bound to the current unexpired guest identity.
    pub async fn details(&self, guest: &Guest) -> Result<Option<crate::checkout_details::Details>> {
        let mut tx = self.pool.begin().await?;
        match Self::authorize(&mut tx, guest).await {
            Ok(()) => {}
            Err(CartError::Forbidden) => return Ok(None),
            Err(e) => return Err(e),
        }
        let data = sqlx::query_as(
            "SELECT d.* \
            FROM checkout_details d \
            JOIN guest_carts c ON c.id=d.cart_id \
            WHERE c.id=$1 \
            AND c.token_hash=$2 \
            AND c.expires_at>now()",
        )
        .bind(guest.id)
        .bind(&guest.token_hash)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(data)
    }
    /// Caller validates typed input; repeat validation at the persistence boundary.
    pub async fn save_details(
        &self,
        guest: &Guest,
        csrf: &str,
        revision: i64,
        data: crate::checkout_details::Details,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock(&mut tx, guest, csrf, revision).await?;
        Self::require_checkout_account(&mut tx, guest).await?;
        Self::store_details(&mut tx, guest, data).await?;
        Self::changed(&mut tx, guest.id).await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn store_details(
        tx: &mut Transaction<'_, Postgres>,
        guest: &Guest,
        data: crate::checkout_details::Details,
    ) -> Result<()> {
        Self::store_customer_details(tx, guest, data, true).await
    }
    /// Only address-book selection calls this: all values originate in authorized
    /// stored records. It may leave the draft incomplete; confirmation always validates.
    pub(crate) async fn store_address_details(
        tx: &mut Transaction<'_, Postgres>,
        guest: &Guest,
        data: crate::checkout_details::Details,
    ) -> Result<()> {
        Self::store_customer_details(tx, guest, data, false).await
    }
    async fn store_customer_details(
        tx: &mut Transaction<'_, Postgres>,
        guest: &Guest,
        data: crate::checkout_details::Details,
        complete: bool,
    ) -> Result<()> {
        let policy = talocart_platform::customer_fields::load_in(tx).await?;
        let mut data = if complete {
            data.validate_with(&policy)
                .map_err(|_| CartError::Invalid)?
        } else {
            data.normalize_with(&policy)
        };
        let previous: Option<crate::checkout_details::Details> =
            sqlx::query_as("SELECT * FROM checkout_details WHERE cart_id=$1")
                .bind(guest.id)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(previous) = previous {
            for f in talocart_platform::customer_fields::FIELDS {
                if (policy.overridden(f.name, &data.customer_type)
                    || (f.scope == "billing"
                        && data.invoice_requested
                        && previous.customer_type == data.customer_type))
                    && policy.mode(f.name, &data.customer_type)
                        == talocart_platform::customer_fields::Mode::Hidden
                    && let Some(value) = data.value_mut(f.name)
                {
                    *value = previous.value(f.name).to_owned();
                }
            }
        }
        crate::countries::CheckoutCountries::require_destination(tx, &data.shipping_country)
            .await
            .map_err(|e| match e {
                crate::countries::CountryError::Technical(e) => e.into(),
                crate::countries::CountryError::Access(e) => e.into(),
                crate::countries::CountryError::Identity(e) => CartError::Identity(e),
                _ => CartError::Unavailable,
            })?;
        sqlx::query("INSERT INTO checkout_details(cart_id,email,first_name,last_name,phone,recipient_first_name,\
                recipient_last_name,shipping_company,shipping_country,shipping_street,shipping_number,\
                shipping_extra,shipping_postal,shipping_city,shipping_region,delivery_notes,customer_type,\
                billing_first_name,billing_last_name,billing_company,billing_country,billing_street,\
                billing_number,billing_extra,billing_postal,billing_city,billing_region,vat_id,tax_id,\
                invoice_channel,recipient_code,pec,invoice_requested,billing_same) \
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,\
                $22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34) \
            ON CONFLICT(cart_id) DO UPDATE \
            SET email=EXCLUDED.email,first_name=EXCLUDED.first_name,last_name=EXCLUDED.last_name,\
            phone=EXCLUDED.phone,recipient_first_name=EXCLUDED.recipient_first_name,recipient_last_name=EXCLUDED.recipient_last_name,\
            shipping_company=EXCLUDED.shipping_company,shipping_country=EXCLUDED.shipping_country,\
            shipping_street=EXCLUDED.shipping_street,shipping_number=EXCLUDED.shipping_number,\
            shipping_extra=EXCLUDED.shipping_extra,shipping_postal=EXCLUDED.shipping_postal,shipping_city=EXCLUDED.shipping_city,\
            shipping_region=EXCLUDED.shipping_region,delivery_notes=EXCLUDED.delivery_notes,customer_type=EXCLUDED.customer_type,\
            billing_first_name=EXCLUDED.billing_first_name,billing_last_name=EXCLUDED.billing_last_name,\
            billing_company=EXCLUDED.billing_company,billing_country=EXCLUDED.billing_country,\
            billing_street=EXCLUDED.billing_street,billing_number=EXCLUDED.billing_number,billing_extra=EXCLUDED.billing_extra,\
            billing_postal=EXCLUDED.billing_postal,billing_city=EXCLUDED.billing_city,billing_region=EXCLUDED.billing_region,\
            vat_id=EXCLUDED.vat_id,tax_id=EXCLUDED.tax_id,invoice_channel=EXCLUDED.invoice_channel,\
            recipient_code=EXCLUDED.recipient_code,pec=EXCLUDED.pec,invoice_requested=EXCLUDED.invoice_requested,\
            billing_same=EXCLUDED.billing_same")
            .bind(guest.id).bind(&data.email).bind(&data.first_name).bind(&data.last_name).bind(&data.phone).bind(&data.recipient_first_name).bind(&data.recipient_last_name).bind(&data.shipping_company).bind(&data.shipping_country).bind(&data.shipping_street).bind(&data.shipping_number).bind(&data.shipping_extra).bind(&data.shipping_postal).bind(&data.shipping_city).bind(&data.shipping_region).bind(&data.delivery_notes).bind(&data.customer_type).bind(&data.billing_first_name).bind(&data.billing_last_name).bind(&data.billing_company).bind(&data.billing_country).bind(&data.billing_street).bind(&data.billing_number).bind(&data.billing_extra).bind(&data.billing_postal).bind(&data.billing_city).bind(&data.billing_region).bind(&data.vat_id).bind(&data.tax_id).bind(&data.invoice_channel).bind(&data.recipient_code).bind(&data.pec).bind(data.invoice_requested).bind(data.billing_same).execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn session(&self, raw: &str) -> Result<Option<Guest>> {
        if !identity::valid_token(raw) {
            return Ok(None);
        }
        Ok(sqlx::query_as(
            "SELECT c.id,c.csrf,c.revision,c.token_hash,(SELECT coalesce(sum(quantity),0)::bigint \
                    FROM guest_cart_items \
                    WHERE cart_id=c.id) AS count \
                FROM guest_carts c \
                WHERE token_hash=$1 \
                AND expires_at>now() \
                AND c.customer_id IS NULL",
        )
        .bind(identity::digest(raw))
        .fetch_optional(&self.pool)
        .await?)
    }
    pub async fn create(&self) -> std::result::Result<(String, Guest), IdentityError> {
        let raw = identity::token()?;
        let csrf = identity::token()?;
        let guest = sqlx::query_as(
            "INSERT INTO guest_carts(token_hash,csrf) \
            VALUES($1,$2) \
            RETURNING id,csrf,revision,token_hash,0::bigint AS count",
        )
        .bind(identity::digest(&raw))
        .bind(csrf)
        .fetch_one(&self.pool)
        .await?;
        Ok((raw, guest))
    }
    pub(crate) async fn lock(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        guest: &Guest,
        csrf: &str,
        revision: i64,
    ) -> Result<()> {
        guest.check_csrf(csrf)?;
        Self::authorize(tx, guest).await?;
        let current: Option<(String,i64)> = sqlx::query_as("SELECT csrf,revision FROM guest_carts WHERE id=$1 AND token_hash=$2 AND expires_at>now() FOR UPDATE")
            .bind(guest.id).bind(&guest.token_hash).fetch_optional(&mut **tx).await?;
        let (current_csrf, current_revision) = current.ok_or(CartError::Forbidden)?;
        guest.check_csrf(&current_csrf)?;
        if revision != current_revision {
            return Err(CartError::Stale);
        }
        Ok(())
    }
    pub(crate) async fn changed(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<()> {
        sqlx::query("UPDATE guest_carts SET revision=revision+1 WHERE id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    fn quantity(item: &CartMerchandise, quantity: i32) -> Result<LineTotals> {
        if !(1..=999999).contains(&quantity) {
            return Err(CartError::Quantity);
        }
        PurchaseQuantities::new(item.minimum_quantity, item.packs)
            .map_err(|_| CartError::Unavailable)?
            .validate(quantity)
            .map_err(|_| CartError::Quantity)?;
        if item.manage_stock && quantity > item.stock {
            return Err(CartError::Stock);
        }
        // Reject an unrepresentable quote before persisting any quantity.
        quote_line(item, quantity)
    }
    pub async fn add(
        &self,
        guest: &Guest,
        csrf: &str,
        revision: i64,
        selection: (i64, i64),
        quantity: i32,
        language: Language,
    ) -> Result<()> {
        if selection.0 <= 0 || selection.1 < 0 {
            return Err(CartError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        self.lock(&mut tx, guest, csrf, revision).await?;
        let item = Products::cart_merchandise(&mut tx, &[selection], language)
            .await
            .map_err(catalog_error)?
            .pop()
            .ok_or(CartError::Unavailable)?;
        // Minimum/packs belong to the final line, as defined by PurchaseQuantities.
        // A smaller positive increment can complete or extend an existing line.
        if !(1..=999999).contains(&quantity) {
            return Err(CartError::Quantity);
        }
        let key = format!("{}:{}", selection.0, selection.1);
        let previous: Option<(i64, i32)> = sqlx::query_as(
            "SELECT id,quantity FROM guest_cart_items WHERE cart_id=$1 AND item_key=$2",
        )
        .bind(guest.id)
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?;
        let final_quantity = quantity
            .checked_add(previous.map_or(0, |p| p.1))
            .ok_or(CartError::Quantity)?;
        Self::quantity(&item, final_quantity)?;
        if let Some((id, _)) = previous {
            save_item(&mut tx, id, final_quantity, &item).await?;
        } else {
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM guest_cart_items WHERE cart_id=$1")
                    .bind(guest.id)
                    .fetch_one(&mut *tx)
                    .await?;
            if count >= ITEM_LIMIT as i64 {
                return Err(CartError::Limit);
            }
            sqlx::query(
                "INSERT INTO guest_cart_items(cart_id,item_key,product_id,variant_id,had_variant,quantity,\
                        name,code,unit_cents,basis_points,price_mode,currency) \
                    VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            )
            .bind(guest.id)
            .bind(key)
            .bind(selection.0)
            .bind((selection.1 > 0).then_some(selection.1))
            .bind(selection.1 > 0)
            .bind(final_quantity)
            .bind(&item.name)
            .bind(&item.code)
            .bind(item.unit_cents)
            .bind(item.basis_points)
            .bind(&item.price_mode)
            .bind(&item.currency)
            .execute(&mut *tx)
            .await?;
        }
        Self::changed(&mut tx, guest.id).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn update(
        &self,
        guest: &Guest,
        csrf: &str,
        revision: i64,
        id: i64,
        quantity: Option<i32>,
        language: Language,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock(&mut tx, guest, csrf, revision).await?;
        let stored: Option<StoredItem> =
            sqlx::query_as("SELECT * FROM guest_cart_items WHERE cart_id=$1 AND id=$2")
                .bind(guest.id)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let stored = stored.ok_or(CartError::Unavailable)?;
        if let Some(quantity) = quantity {
            let pair = stored_pair(&stored).ok_or(CartError::Unavailable)?;
            let item = Products::cart_merchandise(&mut tx, &[pair], language)
                .await
                .map_err(catalog_error)?
                .pop()
                .ok_or(CartError::Unavailable)?;
            Self::quantity(&item, quantity)?;
            save_item(&mut tx, id, quantity, &item).await?;
        } else {
            sqlx::query("DELETE FROM guest_cart_items WHERE cart_id=$1 AND id=$2")
                .bind(guest.id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        Self::changed(&mut tx, guest.id).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn quote(&self, guest: &Guest, language: Language) -> Result<Quote> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        let quote = Self::quote_in(&mut tx, guest, language, None).await?;
        tx.commit().await?;
        Ok(quote)
    }
    pub(crate) async fn quote_in(
        tx: &mut Transaction<'_, Postgres>,
        guest: &Guest,
        language: Language,
        destination: Option<&str>,
    ) -> Result<Quote> {
        Self::authorize(tx, guest).await?;
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM guest_carts WHERE id=$1 AND token_hash=$2 AND expires_at>now()",
        )
        .bind(guest.id)
        .bind(&guest.token_hash)
        .fetch_optional(&mut **tx)
        .await?;
        let revision = revision.ok_or(CartError::Forbidden)?;
        let stored: Vec<StoredItem> =
            sqlx::query_as("SELECT * FROM guest_cart_items WHERE cart_id=$1 ORDER BY id LIMIT 100")
                .bind(guest.id)
                .fetch_all(&mut **tx)
                .await?;
        let pairs: Vec<_> = stored.iter().filter_map(stored_pair).collect();
        let current = Products::cart_merchandise_for(tx, &pairs, language, destination)
            .await
            .map_err(catalog_error)?;
        let currency: String = sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
            .fetch_one(&mut **tx)
            .await?;
        let mut lines = Vec::with_capacity(stored.len());
        let mut amounts = vec![];
        let mut valid = true;
        for row in stored {
            let item = stored_pair(&row)
                .and_then(|pair| {
                    current
                        .iter()
                        .find(|i| (i.product_id, i.variant_id) == pair)
                })
                .cloned();
            let (issue, totals, changed) = if let Some(item) = &item {
                let (issue, totals) = match Self::quantity(item, row.quantity) {
                    Ok(totals) => (None, totals),
                    Err(CartError::Stock) => (Some(Issue::Stock), quote_line(item, row.quantity)?),
                    Err(CartError::Quantity) => {
                        (Some(Issue::Quantity), quote_line(item, row.quantity)?)
                    }
                    Err(e) => return Err(e),
                };
                amounts.push(totals);
                (
                    issue,
                    Some(totals),
                    item.unit_cents != row.unit_cents
                        || item.basis_points != row.basis_points
                        || item.price_mode != row.price_mode
                        || item.currency != row.currency,
                )
            } else {
                (Some(Issue::Unavailable), None, false)
            };
            valid &= issue.is_none();
            lines.push(CartLine {
                id: row.id,
                quantity: row.quantity,
                name: item.as_ref().map_or(row.name, |i| i.name.clone()),
                code: item.as_ref().map_or(row.code, |i| i.code.clone()),
                current: item,
                totals,
                issue,
                changed,
            });
        }
        let (totals, overflow) = if valid {
            match pricing::total(&amounts) {
                Ok(totals) => (Some(totals), false),
                Err(pricing::PriceError::Overflow) => (None, true),
                Err(pricing::PriceError::Invalid) => return Err(CartError::Invalid),
            }
        } else {
            (None, false)
        };
        let mut quote = Quote {
            group_discount: None,
            coupon: None,
            coupon_code: String::new(),
            coupon_invalid: false,
            overflow,
            revision,
            lines,
            totals,
            currency,
        };
        crate::group_discounts::apply(tx, guest, &mut quote).await?;
        crate::coupons::apply(tx, guest, &mut quote).await?;
        Ok(quote)
    }
    /// Small independently committed batches; stop after 20 batches or a cooperative
    /// two-second budget. SQL timeouts bound waits inside each batch. Locked carts
    /// are skipped and the next maintenance tick retries the remaining backlog.
    pub async fn prune(&self) -> Result<PruneReport> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut deleted = 0;
        for _ in 0..20 {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET LOCAL statement_timeout = '500ms'")
                .execute(&mut *tx)
                .await?;
            sqlx::query("SET LOCAL lock_timeout = '250ms'")
                .execute(&mut *tx)
                .await?;
            let count = sqlx::query(
                "DELETE \
                FROM guest_carts \
                WHERE id IN (SELECT id \
                    FROM guest_carts \
                    WHERE expires_at<=now() \
                    ORDER BY expires_at,id \
                    LIMIT 100 FOR UPDATE SKIP LOCKED)",
            )
            .execute(&mut *tx)
            .await?
            .rows_affected();
            tx.commit().await?;
            deleted += count;
            if count < 100 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let remaining_at_least = sqlx::query_scalar(
            "SELECT count(*) \
            FROM (SELECT id \
                FROM guest_carts \
                WHERE expires_at<=now() \
                ORDER BY expires_at,id \
                LIMIT 1000) expired",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(PruneReport {
            deleted,
            remaining_at_least,
        })
    }
}
fn stored_pair(row: &StoredItem) -> Option<(i64, i64)> {
    Some((
        row.product_id?,
        if row.had_variant { row.variant_id? } else { 0 },
    ))
}
fn catalog_error(e: crate::products::ProductError) -> CartError {
    match e {
        crate::products::ProductError::Technical(e) => e.into(),
        crate::products::ProductError::Access(e) => e.into(),
        crate::products::ProductError::Identity(e) => CartError::Identity(e),
        _ => CartError::Unavailable,
    }
}
fn quote_line(item: &CartMerchandise, quantity: i32) -> Result<LineTotals> {
    pricing::line(
        Money::from_cents(item.unit_cents).map_err(|_| CartError::Invalid)?,
        u32::try_from(quantity).map_err(|_| CartError::Quantity)?,
        Rate::from_basis_points(item.basis_points).map_err(|_| CartError::Invalid)?,
        PriceMode::parse(&item.price_mode).map_err(|_| CartError::Invalid)?,
    )
    .map_err(|_| CartError::Invalid)
}
async fn save_item(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    quantity: i32,
    item: &CartMerchandise,
) -> Result<()> {
    sqlx::query(
        "UPDATE guest_cart_items \
        SET quantity=$1,name=$2,code=$3,unit_cents=$4,basis_points=$5,price_mode=$6,currency=$7 \
        WHERE id=$8",
    )
    .bind(quantity)
    .bind(&item.name)
    .bind(&item.code)
    .bind(item.unit_cents)
    .bind(item.basis_points)
    .bind(&item.price_mode)
    .bind(&item.currency)
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl From<IdentityError> for CartError {
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
