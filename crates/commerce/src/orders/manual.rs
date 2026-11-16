//! The immutable receipt is separate from the commercial snapshot and stock.
use super::*;

#[derive(FromRow)]
pub struct ManualPayment {
    pub amount_cents: i64,
    pub currency: String,
    pub method_code: String,
    pub reference: String,
    pub administrator_id: i64,
    pub administrator_name: String,
    pub recorded_at: String,
}

fn reference(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 160 || value.chars().any(char::is_control) {
        return Err(OrderError::PaymentReference);
    }
    Ok(value)
}

impl Orders {
    /// One full receipt per order. Same-reference replay returns its original
    /// receipt regardless of the stale form revision; a different reference
    /// conflicts. Amount, currency, method, actor and timestamp are server-owned.
    pub async fn record_payment(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        payment_reference: &str,
    ) -> Result<Order> {
        crate::transaction_retry::run(
            "manual_payment",
            || self.record_payment_once(session, csrf, id, revision, payment_reference),
            OrderError::technical,
        )
        .await
    }

    async fn record_payment_once(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        payment_reference: &str,
    ) -> Result<Order> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::OrdersWrite).await?;
        let actor = session.require(Permission::OrdersWrite)?;
        let payment_reference = reference(payment_reference)?;
        // Same order lock used by cancellation; exactly one competing effect wins.
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(OrderError::NotFound)?;
        let order = load(&mut tx, id).await?;
        if let Some(receipt) = &order.manual_payment {
            if receipt.reference != payment_reference {
                return Err(OrderError::AlreadyPaid);
            }
            tx.commit().await?;
            return Ok(order);
        }
        if order.state == "cancelled" {
            return Err(OrderError::PaymentCancelled);
        }
        if order.revision != revision {
            return Err(CartError::Stale.into());
        }
        if !matches!(
            order.frozen.payment.code.as_str(),
            "bank_transfer" | "cash_on_delivery"
        ) {
            return Err(OrderError::PaymentMethod);
        }
        sqlx::query(
            "INSERT INTO order_manual_payments(order_id,amount_cents,currency,method_code,reference,\
                    administrator_id,administrator_name) \
                SELECT $1,$2,$3,$4,$5,id,display_name \
                FROM administrators \
                WHERE id=$6",
        )
        .bind(id)
        .bind(order.total_cents)
        .bind(&order.currency)
        .bind(&order.frozen.payment.code)
        .bind(payment_reference)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        let order = load(&mut tx, id).await?;
        enqueue(&mut tx, &order, Event::PaymentRecorded).await?;
        tx.commit().await?;
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_payment_reference_is_bounded_and_normalized() {
        assert_eq!(reference("  TRX-123  ").unwrap(), "TRX-123");
        assert!(reference(&"é".repeat(160)).is_ok());
        for value in [
            "".to_owned(),
            " ".to_owned(),
            "é".repeat(161),
            "A\nB".to_owned(),
            "A\0B".to_owned(),
        ] {
            assert!(matches!(
                reference(&value),
                Err(OrderError::PaymentReference)
            ));
        }
    }
}
