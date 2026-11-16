//! Transactional notification outbox.
use super::*;

pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    order: &Order,
    event: Event,
) -> Result<()> {
    let customer = order.notice(&order.frozen.customer_locale)?;
    let shop = order.notice(&order.frozen.shop_locale)?;
    let suffix = match event {
        Event::Created | Event::OnlineCreated | Event::AdminCreated => "created",
        Event::Updated => "updated",
        Event::Shipped => "shipped",
        Event::Delivered => "delivered",
        Event::Tracking => "tracking",
        Event::Cancelled => {
            talocart_platform::mail::Mail::supersede_order(tx, order.id)
                .await
                .map_err(OrderError::from)?;
            "cancelled"
        }
        Event::Refunded => {
            talocart_platform::mail::Mail::supersede_payment_confirmation(tx, order.id)
                .await
                .map_err(OrderError::from)?;
            "refund"
        }
        Event::PaymentRecorded | Event::OnlinePaymentRecorded => {
            talocart_platform::mail::Mail::supersede_payment_request(tx, order.id)
                .await
                .map_err(OrderError::from)?;
            if order.is_online() {
                "online-payment"
            } else {
                "manual-payment"
            }
        }
    };
    let key = if matches!(event, Event::Shipped | Event::Delivered | Event::Tracking) {
        talocart_platform::mail::Mail::supersede_fulfillment(tx, order.id)
            .await
            .map_err(OrderError::from)?;
        format!("order:{}:{suffix}:{}", order.id, order.fulfillment.revision)
    } else if matches!(event, Event::Updated) {
        format!("order:{}:{suffix}:{}", order.id, order.revision)
    } else {
        format!("order:{}:{suffix}", order.id)
    };
    let ids = notifications::enqueue_snapshots(
        tx,
        &key,
        event,
        (&customer, &shop),
        &order.frozen.customer.email,
        (&order.frozen.customer_locale, &order.frozen.shop_locale),
    )
    .await
    .map_err(OrderError::from)?;
    talocart_platform::mail::Mail::correlate_order(tx, &ids, order.id)
        .await
        .map_err(OrderError::from)?;
    Ok(())
}
