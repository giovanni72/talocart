//! Append-only operation metadata, never snapshots or credential values.
use crate::{
    identity::{Identity, IdentityError, Permission, Session},
    pagination,
};
use sqlx::{FromRow, PgPool};

pub const AREAS: [&str; 5] = ["orders", "catalog", "pages", "settings", "access"];
#[derive(FromRow)]
pub struct Event {
    pub id: i64,
    pub created_at: String,
    pub area: String,
    pub resource: String,
    pub resource_key: String,
    pub action: String,
    pub administrator_id: Option<i64>,
}
pub async fn list(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
    area: &str,
    page: i64,
) -> Result<pagination::Page<Event>, IdentityError> {
    identity
        .authorize(session, Permission::OperationsRead)
        .await?;
    if (!area.is_empty() && !AREAS.contains(&area)) || !(1..=1_000_000).contains(&page) {
        return Err(IdentityError::Invalid);
    }
    // Separate bounded predicates allow the composite area/id index to serve filtered reads.
    let columns = "SELECT e.id,platform_datetime(e.created_at,r.time_zone,r.date_format) AS created_at,e.area,e.resource,e.resource_key,e.action,e.administrator_id FROM operation_events e CROSS JOIN platform_regional_settings r WHERE r.singleton";
    let query = if area.is_empty() {
        sqlx::query(sqlx::AssertSqlSafe(columns.to_owned()))
    } else {
        sqlx::query(sqlx::AssertSqlSafe(format!("{columns} AND e.area=$1"))).bind(area)
    };
    Ok(pagination::read(pool, query, "e.id DESC", page, 20).await?)
}
