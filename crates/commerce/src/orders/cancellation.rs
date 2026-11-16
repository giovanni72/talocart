//! Idempotent cancellation, stock restoration and notice in one transaction.
use super::*;

impl Orders {
    pub async fn cancel(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> Result<Order> {
        crate::transaction_retry::run(
            "cancellation",
            || self.cancel_once(s, csrf, id, revision),
            OrderError::technical,
        )
        .await
    }
    async fn cancel_once(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> Result<Order> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::OrdersWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        let current: Option<(String, i64)> =
            sqlx::query_as("SELECT state,revision FROM orders WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let (state, current_revision) = current.ok_or(OrderError::NotFound)?;
        let order = load(&mut tx, id).await?;
        if state == "cancelled" {
            tx.commit().await?;
            return Ok(order);
        }
        if order.fulfillment.sent() {
            return Err(OrderError::Shipped);
        }
        if order.is_paid() {
            return Err(OrderError::PaidCancellation);
        }
        if current_revision != revision {
            return Err(CartError::Stale.into());
        }
        // Same catalog guard as confirmation and catalog editing; no provider call here.
        restore(&mut tx, &order).await?;
        sqlx::query("UPDATE orders SET state='cancelled',revision=revision+1,cancelled_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
        enqueue(&mut tx, &order, Event::Cancelled).await?;
        let order = load(&mut tx, id).await?;
        tx.commit().await?;
        Ok(order)
    }
}
