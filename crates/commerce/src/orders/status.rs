//! Current status is mutable; checkout snapshots and financial truth are independent.
use super::*;
use crate::order_statuses::OrderStatus;

#[derive(FromRow)]
pub struct StatusEvent {
    pub name_it: String,
    pub name_en: String,
    pub color: String,
    pub created_at: String,
}
impl StatusEvent {
    pub fn name(&self, locale: &str) -> &str {
        if locale == "it" {
            &self.name_it
        } else {
            &self.name_en
        }
    }
}
impl Orders {
    pub async fn status_choices(&self, s: &Session) -> Result<Vec<OrderStatus>> {
        self.identity.authorize(s, Permission::OrdersRead).await?;
        Ok(sqlx::query_as(
            "SELECT s.*,coalesce(r.role,'custom') AS role \
            FROM order_statuses s \
            LEFT \
            JOIN order_status_roles r ON r.status_id=s.id \
            ORDER BY s.sort_order,s.id \
            LIMIT 1000",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn status_history(
        &self,
        s: &Session,
        id: i64,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<StatusEvent>> {
        self.identity.authorize(s, Permission::OrdersRead).await?;
        if !(1..=1000000).contains(&page) {
            return Err(OrderError::Invalid);
        }
        Ok(talocart_platform::pagination::read(&self.pool,sqlx::query("SELECT e.id,s.name_it,s.name_en,s.color,platform_datetime(e.created_at,r.time_zone,\
                r.date_format) AS created_at \
            FROM order_status_events e \
            JOIN order_statuses s ON s.id=e.status_id \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE r.singleton \
            AND e.order_id=$1").bind(id),"id DESC",page,20).await?)
    }
    pub async fn set_status(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        status_id: i64,
    ) -> Result<Order> {
        s.check_csrf(csrf)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::OrdersWrite).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(OrderError::NotFound)?;
        let order = load(&mut tx, id).await?;
        if order.status_revision != revision {
            return Err(CartError::Stale.into());
        }
        let target = OrderStatuses::require(&mut tx, status_id)
            .await
            .map_err(OrderError::from)?;
        if order.current_status.id == status_id {
            tx.commit().await?;
            return Ok(order);
        }
        if order.state != "open"
            || order.is_refunded()
            || order.fulfillment.sent()
            || !matches!(target.role.as_str(), "custom" | "preparing")
        {
            return Err(OrderError::FulfillmentTransition);
        }
        if target.role == "preparing" && order.fulfillment.state == "pending" {
            sqlx::query(
                "INSERT INTO order_fulfillment(order_id) VALUES($1) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE order_fulfillment SET state='preparing',revision=revision+1 WHERE order_id=$1").bind(id).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE order_current_status SET status_id=$2,revision=revision+1 WHERE order_id=$1").bind(id).bind(status_id).execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO order_fulfillment(order_id) VALUES($1) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            // Invalidate concurrently opened shipping/note forms too.
            sqlx::query("UPDATE order_fulfillment SET revision=revision+1 WHERE order_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let result = load(&mut tx, id).await?;
        tx.commit().await?;
        Ok(result)
    }
}
pub async fn check_current_status(pool: &PgPool) -> Result<()> {
    let missing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 \
            FROM orders o \
            WHERE NOT EXISTS(SELECT 1 \
                FROM order_current_status s \
                WHERE s.order_id=o.id))",
    )
    .fetch_one(pool)
    .await?;
    if missing {
        return Err(OrderError::Invalid);
    }
    Ok(())
}
