//! Explicit base/module schema operations, shared by doctor and serve.
use talocart_platform::database;

pub(crate) async fn check_schema(
    pool: &talocart_platform::database::PgPool,
) -> Result<(), database::DatabaseError> {
    database::check(pool).await?;
    Ok(())
}

pub(crate) async fn migrate_schema(
    pool: &talocart_platform::database::PgPool,
) -> Result<(), database::DatabaseError> {
    database::migrate(pool).await
}
