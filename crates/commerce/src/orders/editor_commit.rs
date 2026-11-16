//! Publish a draft atomically with its audit record and stock effects.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct AuditLine {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub product_id: i64,
    #[serde(default)]
    pub variant_id: Option<i64>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub class_id: i64,
    #[serde(default)]
    pub tax_name: String,
    #[serde(default)]
    pub basis_points: i32,
    #[serde(default)]
    pub zone_name: Option<String>,
    #[serde(default)]
    pub price_mode: String,
    #[serde(default)]
    pub net_cents: i64,
    #[serde(default)]
    pub tax_cents: i64,
    pub name: String,
    pub code: String,
    pub quantity: i32,
    pub unit_cents: i64,
    pub gross_cents: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RevisionSnapshot {
    #[serde(default)]
    pub merchandise_cents: i64,
    #[serde(default)]
    pub net_cents: i64,
    #[serde(default)]
    pub tax_cents: i64,
    pub frozen: Frozen,
    pub lines: Vec<AuditLine>,
    pub currency: String,
    pub total_cents: i64,
}
impl RevisionSnapshot {
    pub fn money(&self, cents: &i64) -> String {
        format!("{},{:02}", cents / 100, cents % 100)
    }
    fn from_order(order: &Order) -> Self {
        Self {
            merchandise_cents: order.merchandise_cents,
            net_cents: order.net_cents,
            tax_cents: order.tax_cents,
            frozen: order.frozen.clone(),
            lines: order
                .lines
                .iter()
                .map(|l| AuditLine {
                    id: Some(l.id),
                    product_id: l.product_id,
                    variant_id: l.variant_id,
                    label: l.label.clone(),
                    class_id: l.class_id,
                    tax_name: l.tax_name.clone(),
                    basis_points: l.basis_points,
                    zone_name: l.zone_name.clone(),
                    price_mode: l.price_mode.clone(),
                    net_cents: l.net_cents,
                    tax_cents: l.tax_cents,
                    name: l.name.clone(),
                    code: l.code.clone(),
                    quantity: l.quantity,
                    unit_cents: l.unit_cents,
                    gross_cents: l.gross_cents,
                })
                .collect(),
            currency: order.currency.clone(),
            total_cents: order.total_cents,
        }
    }
    fn from_preview(preview: &Preview) -> Self {
        Self {
            merchandise_cents: preview.merchandise_cents,
            net_cents: preview.totals.net().cents(),
            tax_cents: preview.totals.tax().cents(),
            frozen: preview.frozen.clone(),
            lines: preview
                .lines
                .iter()
                .map(|l| AuditLine {
                    id: None,
                    product_id: l.product_id,
                    variant_id: l.variant_id,
                    label: l.label.clone(),
                    class_id: l.class_id,
                    tax_name: l.tax_name.clone(),
                    basis_points: l.basis_points,
                    zone_name: l.zone_name.clone(),
                    price_mode: l.price_mode.clone(),
                    net_cents: l.net_cents,
                    tax_cents: l.tax_cents,
                    name: l.name.clone(),
                    code: l.code.clone(),
                    quantity: l.quantity,
                    unit_cents: l.unit_cents,
                    gross_cents: l.gross_cents,
                })
                .collect(),
            currency: preview.currency.clone(),
            total_cents: preview.totals.gross().cents(),
        }
    }
}
pub struct Publish<'a> {
    pub session: &'a Session,
    pub csrf: &'a str,
    pub draft_id: i64,
    pub revision: i64,
    pub review: &'a str,
    pub notify: bool,
}
impl Orders {
    pub async fn publish_draft(&self, input: Publish<'_>) -> Result<Order> {
        crate::transaction_retry::run(
            "admin_order_publish",
            || self.publish_draft_once(&input),
            OrderError::technical,
        )
        .await
    }
    async fn publish_draft_once(&self, input: &Publish<'_>) -> Result<Order> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(
            &mut tx,
            input.session,
            input.csrf,
            Permission::OrdersWrite,
        )
        .await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        let draft = draft_in(&mut tx, input.draft_id, true).await?;
        // A committed draft is the idempotency identity, even after later order actions.
        if let Some(id) = draft.committed_order_id {
            let order = load(&mut tx, id).await?;
            tx.commit().await?;
            return Ok(order);
        }
        if draft.revision != input.revision {
            return Err(CartError::Stale.into());
        }
        if draft.data.reason.trim().is_empty() {
            return Err(fields(&["reason"]));
        }
        let previous =
            if let Some((id, revision)) = draft.target_order_id.zip(draft.target_revision) {
                Some(editable_in(&mut tx, id, revision).await?)
            } else {
                None
            };
        let preview = quotation::quote(&mut tx, input.session, &draft, previous.as_ref()).await?;
        if input.review.is_empty() || preview.review != input.review {
            return Err(OrderError::Review);
        }
        let snapshot = toml::to_string(&preview.frozen).map_err(|_| OrderError::Invalid)?;
        let before = previous
            .as_ref()
            .map(|o| serde_json::to_string(&RevisionSnapshot::from_order(o)))
            .transpose()
            .map_err(|_| OrderError::Invalid)?
            .unwrap_or_default();
        let after = serde_json::to_string(&RevisionSnapshot::from_preview(&preview))
            .map_err(|_| OrderError::Invalid)?;
        let id = if let Some(order) = &previous {
            order.id
        } else {
            let owner_hash =
                talocart_platform::identity::digest(&talocart_platform::identity::token()?);
            let id:i64=sqlx::query_scalar("INSERT INTO orders(reference,origin,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at) VALUES(next_order_reference(statement_timestamp()),'admin',NULL,0,$1,$2,$3,$4,$5,$6,$7,$8,$9,statement_timestamp()) RETURNING id")
                .bind(owner_hash).bind(&snapshot).bind(&preview.currency).bind(preview.merchandise_cents).bind(preview.frozen.shipping.gross_cents).bind(preview.frozen.payment.gross_cents).bind(preview.totals.net().cents()).bind(preview.totals.tax().cents()).bind(preview.totals.gross().cents()).fetch_one(&mut *tx).await?;
            list::insert_customer(&mut tx, id, &preview.frozen.customer).await?;
            if let Some(customer) = draft.data.customer_id {
                sqlx::query("INSERT INTO customer_orders(order_id,customer_id) VALUES($1,$2)")
                    .bind(id)
                    .bind(customer)
                    .execute(&mut *tx)
                    .await?;
            }
            if preview.frozen.status_id != 0 {
                let status = OrderStatuses::require(&mut tx, preview.frozen.status_id).await?;
                if status.role == "preparing" {
                    sqlx::query("INSERT INTO order_fulfillment(order_id) VALUES($1)")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("UPDATE order_fulfillment SET state='preparing',revision=revision+1 WHERE order_id=$1").bind(id).execute(&mut *tx).await?;
                }
            }
            if let Some(coupon) = &preview.frozen.coupon {
                sqlx::query(
                    "INSERT INTO coupon_uses(order_id,coupon_id,discount_cents) VALUES($1,$2,$3)",
                )
                .bind(id)
                .bind(coupon.id)
                .bind(coupon.discount_cents)
                .execute(&mut *tx)
                .await?;
            }
            id
        };
        sqlx::query("INSERT INTO order_edit_events(order_id,draft_id,revision,administrator_id,administrator_name,reason,before_data,after_data) SELECT $1,$2,$3,id,display_name,$4,$5,$6 FROM administrators WHERE id=$7")
            .bind(id).bind(draft.id).bind(preview.frozen.commercial_revision).bind(draft.data.reason.trim()).bind(before).bind(after).bind(input.session.require(Permission::OrdersWrite)?).execute(&mut *tx).await?;
        if let Some(order) = &previous {
            restore(&mut tx, order).await?;
            sqlx::query("UPDATE order_lines SET active=false WHERE order_id=$1 AND active")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE orders SET snapshot=$1,merchandise_cents=$2,shipping_cents=$3,fee_cents=$4,net_cents=$5,tax_cents=$6,total_cents=$7,revision=revision+1 WHERE id=$8")
                .bind(snapshot).bind(preview.merchandise_cents).bind(preview.frozen.shipping.gross_cents).bind(preview.frozen.payment.gross_cents).bind(preview.totals.net().cents()).bind(preview.totals.tax().cents()).bind(preview.totals.gross().cents()).bind(id).execute(&mut *tx).await?;
            sqlx::query("UPDATE order_customer_summaries SET first_name=$1,last_name=$2,email=$3 WHERE order_id=$4")
                .bind(&preview.frozen.customer.first_name).bind(&preview.frozen.customer.last_name).bind(&preview.frozen.customer.email).bind(id).execute(&mut *tx).await?;
            // Pending notices containing the previous amount/address must never be sent later.
            talocart_platform::mail::Mail::supersede_order(&mut tx, id).await?;
        }
        for line in &preview.lines {
            let line_id:i64=sqlx::query_scalar("INSERT INTO order_lines(order_id,product_id,variant_id,quantity,name,label,code,unit_cents,class_id,tax_name,basis_points,zone_name,price_mode,net_cents,tax_cents,gross_cents) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id")
                .bind(id).bind(line.product_id).bind(line.variant_id).bind(line.quantity).bind(&line.name).bind(&line.label).bind(&line.code).bind(line.unit_cents).bind(line.class_id).bind(&line.tax_name).bind(line.basis_points).bind(&line.zone_name).bind(&line.price_mode).bind(line.net_cents).bind(line.tax_cents).bind(line.gross_cents).fetch_one(&mut *tx).await?;
            if line.manage_stock {
                let stock = stock_change(
                    &mut tx,
                    line.product_id,
                    line.variant_id,
                    line.quantity,
                    false,
                )
                .await?;
                movement(&mut tx, line_id, "debit", line.quantity, stock).await?;
            }
        }
        sqlx::query("UPDATE order_drafts SET committed_order_id=$1,updated_at=statement_timestamp() WHERE id=$2").bind(id).bind(draft.id).execute(&mut *tx).await?;
        let order = load(&mut tx, id).await?;
        if input.notify {
            if talocart_platform::mail::Mail::notification_configuration(&mut tx)
                .await?
                .is_none()
            {
                return Err(OrderError::Notification);
            }
            enqueue(
                &mut tx,
                &order,
                if previous.is_some() {
                    Event::Updated
                } else {
                    Event::Created
                },
            )
            .await?;
        } else if previous.is_none() {
            enqueue(&mut tx, &order, Event::AdminCreated).await?;
        }
        tx.commit().await?;
        Ok(order)
    }
    /// Explicit, idempotent delivery of the current summary, separate from saving.
    pub async fn send_summary(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(OrderError::NotFound)?;
        let order = load(&mut tx, id).await?;
        if order.revision != revision {
            return Err(CartError::Stale.into());
        }
        if order.state != "open" || order.is_online() {
            return Err(OrderError::EditBlocked);
        }
        if talocart_platform::mail::Mail::notification_configuration(&mut tx)
            .await?
            .is_none()
        {
            return Err(OrderError::Notification);
        }
        enqueue(&mut tx, &order, Event::Updated).await?;
        tx.commit().await?;
        Ok(())
    }
}
