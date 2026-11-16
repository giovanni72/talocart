//! Read-only operational counts. Each area retains its own permission boundary.
use sqlx::PgPool;
use talocart_platform::identity::{Identity, IdentityError, Permission, Session};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricKind {
    OrdersTotal,
    OrdersAwaitingPayment,
    OrdersAwaitingShipment,
    ProductsTotal,
    ProductsPublished,
    PagesPublished,
}
pub struct Metric {
    pub kind: MetricKind,
    pub value: i64,
}
pub async fn metrics(
    pool: &PgPool,
    identity: &Identity,
    s: &Session,
) -> Result<Vec<Metric>, IdentityError> {
    let mut permissions = vec![Permission::AdminRead];
    for permission in [
        Permission::OrdersRead,
        Permission::ProductsRead,
        Permission::PagesRead,
    ] {
        if s.allows(permission) {
            permissions.push(permission);
        }
    }
    identity.authorize_all(s, &permissions).await?;
    let mut rows = Vec::new();
    if s.allows(Permission::OrdersRead) {
        let (total, unpaid, unshipped): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*),count(*) FILTER(WHERE o.state='open' \
                    AND o.payment_state='unpaid' \
                    AND o.total_cents>0),count(*) FILTER(WHERE o.state='open' \
                    AND coalesce(f.state,'pending') IN ('pending','preparing')) \
                FROM orders o \
                LEFT \
                JOIN order_fulfillment f ON f.order_id=o.id",
        )
        .fetch_one(pool)
        .await?;
        for (kind, value) in [
            (MetricKind::OrdersTotal, total),
            (MetricKind::OrdersAwaitingPayment, unpaid),
            (MetricKind::OrdersAwaitingShipment, unshipped),
        ] {
            rows.push(Metric { kind, value });
        }
    }
    if s.allows(Permission::ProductsRead) {
        let (total, published): (i64, i64) = sqlx::query_as(
            "SELECT count(*),count(*) FILTER(WHERE status='published') FROM catalog_products",
        )
        .fetch_one(pool)
        .await?;
        for (kind, value) in [
            (MetricKind::ProductsTotal, total),
            (MetricKind::ProductsPublished, published),
        ] {
            rows.push(Metric { kind, value });
        }
    }
    if s.allows(Permission::PagesRead) {
        let value = sqlx::query_scalar("SELECT count(*) FROM information_pages WHERE published")
            .fetch_one(pool)
            .await?;
        rows.push(Metric {
            kind: MetricKind::PagesPublished,
            value,
        });
    }
    Ok(rows)
}

mod sales;
pub use sales::{SalesBucket, SalesHistory, SalesPeriod, sales_history};
