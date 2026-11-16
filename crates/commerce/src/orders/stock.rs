//! Stock effects within the transaction owned by the order operation.
use super::*;

pub(super) async fn restore(tx: &mut Transaction<'_, Postgres>, order: &Order) -> Result<()> {
    if order.fulfillment.sent() {
        return Err(OrderError::Shipped);
    }
    // Immutable debits record the policy at confirmation, including historical orders.
    // Never consult the product's current policy when restoring an earlier order.
    let debited: Vec<i64> = sqlx::query_scalar(
        "SELECT m.line_id FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id \
         WHERE l.order_id=$1 AND l.active AND m.kind='debit'",
    )
    .bind(order.id)
    .fetch_all(&mut **tx)
    .await?;
    for line in &order.lines {
        if !debited.contains(&line.id) {
            continue;
        }
        let stock = stock_change(tx, line.product_id, line.variant_id, line.quantity, true).await?;
        movement(tx, line.id, "credit", line.quantity, stock).await?;
    }
    Ok(())
}
pub(super) async fn stock_change(
    tx: &mut Transaction<'_, Postgres>,
    product: i64,
    variant: Option<i64>,
    quantity: i32,
    restore: bool,
) -> Result<i32> {
    let delta = if restore { quantity } else { -quantity };
    let stock = if let Some(id) = variant {
        sqlx::query_scalar(
            "UPDATE catalog_variants \
            SET stock=stock+$1 \
            WHERE id=$2 \
            AND product_id=$3 \
            AND stock::bigint+$1 BETWEEN 0 \
            AND 999999 \
            RETURNING stock",
        )
        .bind(delta)
        .bind(id)
        .bind(product)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            "UPDATE catalog_products \
            SET stock=stock+$1 \
            WHERE id=$2 \
            AND stock::bigint+$1 BETWEEN 0 \
            AND 999999 \
            RETURNING stock",
        )
        .bind(delta)
        .bind(product)
        .fetch_optional(&mut **tx)
        .await?
    };
    stock.ok_or(if restore {
        OrderError::Restore
    } else {
        CartError::Stock.into()
    })
}
pub(super) async fn movement(
    tx: &mut Transaction<'_, Postgres>,
    line: i64,
    kind: &str,
    quantity: i32,
    stock: i32,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO order_stock_moves(line_id,kind,quantity,stock_after) VALUES($1,$2,$3,$4)",
    )
    .bind(line)
    .bind(kind)
    .bind(quantity)
    .bind(stock)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
