//! Administrative order drafts. Authorization and optimistic revisions cover every write.
use super::*;
use crate::pricing::{self, LineTotals, PriceMode, Rate};
use crate::products::{Products, PurchaseQuantities};
use talocart_platform::identity::customer_admin::CustomerAdmin;
#[path = "editor_commit.rs"]
mod publication;
#[path = "editor_quote.rs"]
mod quotation;
pub use publication::{Publish, RevisionSnapshot};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftLine {
    pub name: String,
    pub code: String,
    pub source_line_id: Option<i64>,
    pub product_id: i64,
    pub variant_id: i64,
    pub quantity: i32,
    /// Empty selects the current catalog/group price; otherwise an explicit final unit price.
    pub unit_price: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftData {
    pub customer_id: Option<i64>,
    pub customer: Details,
    pub locale: String,
    pub lines: Vec<DraftLine>,
    pub shipping_method_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub shipping_price: String,
    pub payment_price: String,
    pub coupon_code: String,
    pub reason: String,
}
impl DraftData {
    pub fn shipping_selected(&self, id: &i64) -> bool {
        self.shipping_method_id == Some(*id)
    }
    pub fn payment_selected(&self, id: &i64) -> bool {
        self.payment_method_id == Some(*id)
    }
}
impl Default for DraftData {
    fn default() -> Self {
        Self {
            customer_id: None,
            customer: Details::default(),
            locale: "it".into(),
            lines: vec![],
            shipping_method_id: None,
            payment_method_id: None,
            shipping_price: String::new(),
            payment_price: String::new(),
            coupon_code: String::new(),
            reason: String::new(),
        }
    }
}
pub struct Draft {
    pub id: i64,
    pub revision: i64,
    pub target_order_id: Option<i64>,
    pub target_revision: Option<i64>,
    pub committed_order_id: Option<i64>,
    pub data: DraftData,
}
pub const EDITOR_PAGE_SIZE: u16 = 50;
pub struct EditorPage<T> {
    pub entries: Vec<T>,
    pub total: i64,
    pub page: i64,
}
fn bounded_page(total: i64, requested: i64) -> i64 {
    requested.clamp(1, ((total - 1).max(0) / i64::from(EDITOR_PAGE_SIZE)) + 1)
}
#[derive(sqlx::FromRow)]
pub struct DraftSummary {
    pub id: i64,
    pub target_order_id: Option<i64>,
    pub updated_at: String,
}
#[derive(sqlx::FromRow)]
struct StoredDraft {
    id: i64,
    revision: i64,
    target_order_id: Option<i64>,
    target_revision: Option<i64>,
    committed_order_id: Option<i64>,
    payload: String,
}
impl StoredDraft {
    fn decode(self) -> Result<Draft> {
        Ok(Draft {
            id: self.id,
            revision: self.revision,
            target_order_id: self.target_order_id,
            target_revision: self.target_revision,
            committed_order_id: self.committed_order_id,
            data: serde_json::from_str(&self.payload).map_err(|_| OrderError::Invalid)?,
        })
    }
}
#[derive(sqlx::FromRow)]
pub struct EditEvent {
    pub administrator_id: i64,
    pub id: i64,
    pub revision: i64,
    pub administrator_name: String,
    pub reason: String,
    pub created_at: String,
    pub before_data: String,
    pub after_data: String,
}
#[derive(Serialize)]
pub struct PreviewLine {
    pub product_id: i64,
    pub variant_id: Option<i64>,
    pub quantity: i32,
    pub name: String,
    pub label: String,
    pub code: String,
    pub unit_cents: i64,
    pub catalog_unit_cents: i64,
    pub class_id: i64,
    pub tax_name: String,
    pub basis_points: i32,
    pub zone_name: Option<String>,
    pub price_mode: String,
    pub net_cents: i64,
    pub tax_cents: i64,
    pub gross_cents: i64,
    pub manage_stock: bool,
}
pub struct Preview {
    pub frozen: Frozen,
    pub currency: String,
    pub lines: Vec<PreviewLine>,
    pub merchandise_cents: i64,
    pub totals: LineTotals,
    pub review: String,
}
impl Preview {
    pub fn money(&self, cents: &i64) -> String {
        format!("{},{:02}", cents / 100, cents % 100)
    }
}
#[derive(sqlx::FromRow)]
pub struct ProductChoice {
    pub product_id: i64,
    pub variant_id: i64,
    pub name: String,
    pub code: String,
}
#[derive(sqlx::FromRow)]
pub struct MethodChoice {
    pub id: i64,
    pub name: String,
    pub code: String,
}

fn fields(names: &[&str]) -> OrderError {
    OrderError::EditorFields(names.iter().map(|s| (*s).to_owned()).collect())
}
fn payload(data: &DraftData) -> Result<String> {
    if data.lines.len() > 100
        || data.reason.chars().count() > 1000
        || data
            .reason
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        || !matches!(data.locale.as_str(), "it" | "en")
        || data.coupon_code.len() > 80
        || data.shipping_price.len() > 30
        || data.payment_price.len() > 30
        || data.lines.iter().any(|l| {
            l.unit_price.len() > 30
                || l.product_id <= 0
                || l.variant_id < 0
                || !(1..=999999).contains(&l.quantity)
        })
    {
        return Err(fields(&["lines", "reason"]));
    }
    let value = serde_json::to_string(data).map_err(|_| OrderError::Invalid)?;
    if value.len() > 262144 {
        return Err(OrderError::Invalid);
    }
    Ok(value)
}
async fn draft_in(tx: &mut Transaction<'_, Postgres>, id: i64, locked: bool) -> Result<Draft> {
    let sql = if locked {
        "SELECT * FROM order_drafts WHERE id=$1 FOR UPDATE"
    } else {
        "SELECT * FROM order_drafts WHERE id=$1"
    };
    sqlx::query_as::<_, StoredDraft>(sql)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(OrderError::NotFound)?
        .decode()
}
async fn editable_in(tx: &mut Transaction<'_, Postgres>, id: i64, revision: i64) -> Result<Order> {
    sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(OrderError::NotFound)?;
    let order = load(tx, id).await?;
    if !order.editable() {
        return Err(OrderError::EditBlocked);
    }
    if order.revision != revision {
        return Err(CartError::Stale.into());
    }
    let attempts: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_attempts WHERE order_id=$1)")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if attempts {
        return Err(OrderError::EditBlocked);
    }
    Ok(order)
}
impl Order {
    pub fn editable(&self) -> bool {
        self.state == "open"
            && self.payment_state == "unpaid"
            && !self.fulfillment.sent()
            && !self.is_online()
    }
}
impl Orders {
    pub async fn drafts(
        &self,
        session: &Session,
        requested: i64,
    ) -> Result<EditorPage<DraftSummary>> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_read_transaction(&mut tx, session, Permission::OrdersWrite).await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM order_drafts WHERE committed_order_id IS NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
        let page = bounded_page(total, requested);
        let entries = sqlx::query_as("SELECT d.id,d.target_order_id,platform_datetime(d.updated_at,r.time_zone,r.date_format) AS updated_at FROM order_drafts d CROSS JOIN platform_regional_settings r WHERE r.singleton AND d.committed_order_id IS NULL ORDER BY d.updated_at DESC,d.id DESC LIMIT $1 OFFSET $2")
            .bind(i64::from(EDITOR_PAGE_SIZE)).bind((page - 1) * i64::from(EDITOR_PAGE_SIZE)).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(EditorPage {
            entries,
            total,
            page,
        })
    }
    pub async fn draft(&self, session: &Session, id: i64) -> Result<Draft> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_read_transaction(&mut tx, session, Permission::OrdersWrite).await?;
        let draft = draft_in(&mut tx, id, false).await?;
        tx.commit().await?;
        Ok(draft)
    }
    pub async fn create_draft(
        &self,
        session: &Session,
        csrf: &str,
        target: Option<(i64, i64)>,
        locale: &str,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        let mut data = DraftData {
            locale: locale.into(),
            ..Default::default()
        };
        if let Some((id, revision)) = target {
            let order = editable_in(&mut tx, id, revision).await?;
            data.customer_id =
                sqlx::query_scalar("SELECT customer_id FROM customer_orders WHERE order_id=$1")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            data.customer = order.frozen.customer.clone();
            data.locale = order.frozen.customer_locale.clone();
            data.lines = order
                .lines
                .iter()
                .map(|l| DraftLine {
                    name: l.name.clone(),
                    code: l.code.clone(),
                    source_line_id: Some(l.id),
                    product_id: l.product_id,
                    variant_id: l.variant_id.unwrap_or(0),
                    quantity: l.quantity,
                    unit_price: order.money(&l.unit_cents),
                })
                .collect();
            data.shipping_method_id =
                sqlx::query_scalar("SELECT id FROM shipping_methods WHERE code=$1")
                    .bind(&order.frozen.shipping.code)
                    .fetch_optional(&mut *tx)
                    .await?;
            data.payment_method_id =
                sqlx::query_scalar("SELECT id FROM payment_methods WHERE code=$1 AND (provider_id IS NOT DISTINCT FROM $2 OR ($2 IS NULL AND id=3))")
                    .bind(&order.frozen.payment.code)
                    .bind(&order.frozen.payment.provider_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            data.shipping_price = order.money(&order.frozen.shipping.entered_cents);
            data.payment_price = order.money(&order.frozen.payment.entered_cents);
            data.coupon_code = order
                .frozen
                .coupon
                .as_ref()
                .map_or(String::new(), |c| c.code.clone());
        }
        let id=sqlx::query_scalar("INSERT INTO order_drafts(target_order_id,target_revision,payload,administrator_id) VALUES($1,$2,$3,$4) RETURNING id")
            .bind(target.map(|t|t.0)).bind(target.map(|t|t.1)).bind(payload(&data)?).bind(session.require(Permission::OrdersWrite)?).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn save_draft(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        data: &DraftData,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        let draft = draft_in(&mut tx, id, true).await?;
        if draft.revision != revision || draft.committed_order_id.is_some() {
            return Err(CartError::Stale.into());
        }
        if draft.target_order_id.is_some() && draft.data.customer_id != data.customer_id {
            return Err(fields(&["customer_id"]));
        }
        if let Some(customer) = data.customer_id.filter(|_| draft.target_order_id.is_none()) {
            CustomerAdmin::require_order_customer_in(&mut tx, session, customer)
                .await
                .map_err(customer_error)?;
        }
        sqlx::query("UPDATE order_drafts SET payload=$1,revision=revision+1,updated_at=statement_timestamp() WHERE id=$2").bind(payload(data)?).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn editor_products(
        &self,
        session: &Session,
        term: &str,
    ) -> Result<Vec<ProductChoice>> {
        self.identity
            .authorize(session, Permission::OrdersWrite)
            .await?;
        if term.chars().count() > 100 {
            return Err(fields(&["search"]));
        }
        Ok(sqlx::query_as("SELECT p.id AS product_id,coalesce(v.id,0) AS variant_id,p.name,coalesce(v.sku,p.model) AS code FROM catalog_products p LEFT JOIN catalog_variants v ON v.product_id=p.id AND v.active WHERE p.status='published' AND (NOT EXISTS(SELECT 1 FROM catalog_product_options a WHERE a.product_id=p.id) OR v.id IS NOT NULL) AND position(lower($1) in lower(p.name||' '||coalesce(v.sku,p.model)))>0 ORDER BY p.name,p.id,v.id LIMIT 25").bind(term.trim()).fetch_all(&self.pool).await?)
    }
    pub async fn editor_product(
        &self,
        session: &Session,
        product: i64,
        variant: i64,
        locale: &str,
    ) -> Result<DraftLine> {
        self.identity
            .authorize(session, Permission::OrdersWrite)
            .await?;
        let locale = talocart_platform::locale::Locale::parse(locale).ok_or(OrderError::Invalid)?;
        let mut tx = self.pool.begin().await?;
        let item = Products::cart_merchandise(
            &mut tx,
            &[(product, variant)],
            talocart_platform::content::Language::new(locale, locale),
        )
        .await
        .map_err(quotation::product_error)?
        .pop()
        .ok_or(CartError::Unavailable)?;
        let quantity = PurchaseQuantities::new(item.minimum_quantity, item.packs)
            .map_err(|_| CartError::Quantity)?
            .first_quantity();
        tx.commit().await?;
        Ok(DraftLine {
            name: if item.label.is_empty() {
                item.name
            } else {
                format!("{} / {}", item.name, item.label)
            },
            code: item.code,
            product_id: product,
            variant_id: variant,
            quantity,
            ..Default::default()
        })
    }
    pub async fn editor_methods(
        &self,
        session: &Session,
        locale: &str,
    ) -> Result<(Vec<MethodChoice>, Vec<MethodChoice>)> {
        self.identity
            .authorize(session, Permission::OrdersWrite)
            .await?;
        let shipping=sqlx::query_as("SELECT id,CASE WHEN $1='it' THEN name_it ELSE name_en END AS name,code FROM shipping_methods WHERE active ORDER BY sort_order,id LIMIT 2").bind(locale).fetch_all(&self.pool).await?;
        let payment=sqlx::query_as("SELECT id,CASE WHEN $1='it' THEN name_it ELSE name_en END AS name,code FROM payment_methods WHERE active AND code IN ('bank_transfer','cash_on_delivery') ORDER BY sort_order,id LIMIT 2").bind(locale).fetch_all(&self.pool).await?;
        Ok((shipping, payment))
    }
    pub async fn edit_history(
        &self,
        session: &Session,
        id: i64,
        requested: i64,
    ) -> Result<EditorPage<EditEvent>> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_read_transaction(&mut tx, session, Permission::OrdersRead).await?;
        let total: i64 =
            sqlx::query_scalar("SELECT count(*) FROM order_edit_events WHERE order_id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let page = bounded_page(total, requested);
        let entries = sqlx::query_as("SELECT e.id,e.revision,e.administrator_id,e.administrator_name,e.reason,e.before_data,e.after_data,platform_datetime(e.created_at,r.time_zone,r.date_format) AS created_at FROM order_edit_events e CROSS JOIN platform_regional_settings r WHERE r.singleton AND e.order_id=$1 ORDER BY e.revision DESC LIMIT $2 OFFSET $3")
            .bind(id).bind(i64::from(EDITOR_PAGE_SIZE)).bind((page - 1) * i64::from(EDITOR_PAGE_SIZE)).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(EditorPage {
            entries,
            total,
            page,
        })
    }
    pub async fn preview_draft(&self, session: &Session, id: i64) -> Result<Preview> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_read_transaction(&mut tx, session, Permission::OrdersWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        let draft = draft_in(&mut tx, id, false).await?;
        if draft.committed_order_id.is_some() {
            return Err(CartError::Stale.into());
        }
        let old = if let Some((id, revision)) = draft.target_order_id.zip(draft.target_revision) {
            Some(editable_in(&mut tx, id, revision).await?)
        } else {
            None
        };
        let quote = quotation::quote(&mut tx, session, &draft, old.as_ref()).await?;
        tx.commit().await?;
        Ok(quote)
    }
}
