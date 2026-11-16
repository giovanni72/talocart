//! Bounded sales history, grouped in the shop time zone by current order status.
use super::*;
use crate::{order_statuses::OrderStatus, orders::Orders};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SalesPeriod {
    Week,
    #[default]
    Month,
    Quarter,
    Year,
}
impl SalesPeriod {
    pub fn key(self) -> &'static str {
        match self {
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Quarter => "90d",
            Self::Year => "12m",
        }
    }
    fn buckets(self) -> i32 {
        match self {
            Self::Week => 7,
            Self::Month => 30,
            Self::Quarter => 90,
            Self::Year => 12,
        }
    }
}
#[derive(sqlx::FromRow)]
pub struct SalesBucket {
    pub date: String,
    pub currency: String,
    pub orders: i64,
    pub cents: i64,
}
pub struct SalesHistory {
    pub statuses: Vec<OrderStatus>,
    pub selected: Vec<i64>,
    pub buckets: Vec<SalesBucket>,
}
/// None selects every status; Some(empty) deliberately counts no orders.
/// Amounts are order totals (tax, shipping and fees included), never payment receipts.
pub async fn sales_history(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
    period: SalesPeriod,
    selected: Option<Vec<i64>>,
) -> Result<SalesHistory, IdentityError> {
    identity
        .authorize_all(session, &[Permission::AdminRead, Permission::OrdersRead])
        .await?;
    let statuses = Orders::new(pool.clone(), identity.clone())
        .status_choices(session)
        .await
        .map_err(|error| match error {
            crate::orders::OrderError::Identity(error) => error,
            crate::orders::OrderError::Technical(error) => error.into(),
            crate::orders::OrderError::Access(error) => error.into(),
            _ => IdentityError::Unavailable,
        })?;
    let mut selected = selected.unwrap_or_else(|| statuses.iter().map(|s| s.id).collect());
    if selected.len() > 1000
        || selected
            .iter()
            .any(|id| !statuses.iter().any(|s| s.id == *id))
    {
        return Err(IdentityError::Invalid);
    }
    selected.sort_unstable();
    selected.dedup();
    // Make the singleton and calendar cardinalities explicit even before ANALYZE.
    // Aggregate once before zero-filling; the final joins operate on bucket totals,
    // not on every matching order. Keep historical currencies separate.
    let buckets: Vec<SalesBucket> = sqlx::query_as(
        "WITH config AS (SELECT time_zone, CASE WHEN $1 THEN 'month' ELSE 'day' END AS unit \
            FROM platform_regional_settings WHERE singleton LIMIT 1), \
        bounds AS (SELECT *,date_trunc(unit,statement_timestamp() AT TIME ZONE time_zone) AS finish FROM config), \
        span AS (SELECT *,finish-($2-1)*CASE WHEN $1 THEN interval '1 month' ELSE interval '1 day' END AS start FROM bounds), \
        totals AS MATERIALIZED (SELECT date_trunc(w.unit,o.created_at AT TIME ZONE w.time_zone) AS bucket,o.currency, \
            count(*) AS orders,sum(o.total_cents)::bigint AS cents \
            FROM orders o JOIN order_current_status cs ON cs.order_id=o.id CROSS JOIN span w \
            WHERE cs.status_id=ANY($3) AND o.created_at >= (w.start AT TIME ZONE w.time_zone) \
            AND o.created_at <= statement_timestamp() GROUP BY 1,2), \
        currencies AS (SELECT DISTINCT currency FROM totals UNION SELECT '' WHERE NOT EXISTS(SELECT 1 FROM totals)) \
        SELECT to_char(d,CASE WHEN $1 THEN 'YYYY-MM' ELSE 'YYYY-MM-DD' END) AS date,c.currency, \
            coalesce(t.orders,0)::bigint AS orders,coalesce(t.cents,0)::bigint AS cents \
        FROM span w CROSS JOIN LATERAL generate_series(0,$2::int-1) n \
        CROSS JOIN LATERAL (SELECT w.start+n*CASE WHEN $1 THEN interval '1 month' ELSE interval '1 day' END AS d) dates \
        CROSS JOIN currencies c LEFT JOIN totals t ON t.bucket=d AND t.currency=c.currency \
        ORDER BY c.currency,d LIMIT 10001"
    ).bind(period == SalesPeriod::Year).bind(period.buckets()).bind(&selected).fetch_all(pool).await?;
    if buckets.len() > 10000 {
        return Err(IdentityError::Unavailable);
    }
    Ok(SalesHistory {
        statuses,
        selected,
        buckets,
    })
}
