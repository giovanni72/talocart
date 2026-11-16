//! Counted, bounded reads with consistent totals and rows. Domain services own
//! authorization, input validation, filtering and the allowlisted SQL ordering.
use sqlx::{
    Arguments, Execute, FromRow, PgPool, Postgres,
    postgres::{PgArguments, PgRow},
    query::Query,
};

pub struct Page<T> {
    pub entries: Vec<T>,
    pub total: i64,
    pub page: i64,
}

/// `query` must be an authorized SELECT without top-level LIMIT/OFFSET/ORDER BY; `order`
/// must come from static application SQL, never browser input. Values stay bound.
/// Callers validate page input and pass their fixed, nonzero page size.
pub async fn read<T>(
    pool: &PgPool,
    mut query: Query<'_, Postgres, PgArguments>,
    order: &'static str,
    requested: i64,
    size: u16,
) -> Result<Page<T>, sqlx::Error>
where
    T: for<'r> FromRow<'r, PgRow> + Send + Unpin,
{
    let persistent = Execute::persistent(&query);
    let mut arguments = query
        .take_arguments()
        .map_err(sqlx::Error::Encode)?
        .unwrap_or_default();
    let sql = query.sql();
    let base = sql.as_str();
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let total: i64 = sqlx::query_scalar_with(
        sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ({base}) counted_rows")),
        arguments.clone(),
    )
    .persistent(persistent)
    .fetch_one(&mut *tx)
    .await?;
    let size = i64::from(size.max(1));
    let pages = if total == 0 {
        1
    } else {
        (total - 1) / size + 1
    };
    let page = requested.clamp(1, pages);
    let limit = arguments.len() + 1;
    let offset = limit + 1;
    arguments.add(size).map_err(sqlx::Error::Encode)?;
    arguments
        .add((page - 1) * size)
        .map_err(sqlx::Error::Encode)?;
    let entries = sqlx::query_as_with(
        sqlx::AssertSqlSafe(format!(
            "{base} ORDER BY {order} LIMIT ${limit} OFFSET ${offset}"
        )),
        arguments,
    )
    .persistent(persistent)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Page {
        entries,
        total,
        page,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid administrative page size")]
pub struct InvalidPageSize;

pub const ADMIN_PAGE_SIZES: [u16; 4] = [10, 20, 50, 100];
pub const PRODUCT_PAGE_SIZES: [u16; 5] = [10, 20, 50, 100, 500];
/// Validated administrative page size, independent of storefront pagination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdminPageSize(u16);
impl Default for AdminPageSize {
    fn default() -> Self {
        Self(20)
    }
}
impl AdminPageSize {
    pub fn parse(value: &str) -> Result<Self, InvalidPageSize> {
        Self::parse_allowed(value, &ADMIN_PAGE_SIZES)
    }
    /// Product lists also allow 500 rows; other admin lists keep their own limit.
    pub fn parse_products(value: &str) -> Result<Self, InvalidPageSize> {
        Self::parse_allowed(value, &PRODUCT_PAGE_SIZES)
    }
    fn parse_allowed(value: &str, allowed: &[u16]) -> Result<Self, InvalidPageSize> {
        if value.is_empty() {
            return Ok(Self::default());
        }
        value
            .parse::<u16>()
            .ok()
            .filter(|size| allowed.contains(size))
            .map(Self)
            .ok_or(InvalidPageSize)
    }
    pub fn value(self) -> u16 {
        self.0
    }
}
