//! Platform date/time presentation and the zone used for annual order numbering.
use crate::identity::{Identity, IdentityError, Permission, Session};
use sqlx::{FromRow, PgPool};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegionalError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error(transparent)]
    Identity(IdentityError),
    #[error("Invalid regional settings")]
    Invalid,
    #[error("Regional settings changed; reload before saving")]
    Conflict,
}
impl From<sqlx::Error> for RegionalError {
    fn from(error: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(error, module_path!()).into())
    }
}
#[derive(FromRow)]
pub struct Settings {
    pub time_zone: String,
    pub date_format: String,
    pub revision: i64,
}
pub const DATE_FORMATS: [&str; 3] = ["dmy", "ymd", "mdy"];

pub async fn settings(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
) -> Result<Settings, IdentityError> {
    identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    read_settings(pool).await
}
async fn read_settings<'e, E>(executor: E) -> Result<Settings, IdentityError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as(
        "SELECT time_zone,date_format,revision FROM platform_regional_settings WHERE singleton",
    )
    .fetch_one(executor)
    .await?)
}
/// Reads on the caller transaction without another pool checkout.
pub async fn settings_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session: &Session,
) -> Result<Settings, IdentityError> {
    Identity::authorize_read_transaction(tx, session, Permission::SettingsRead).await?;
    read_settings(&mut **tx).await
}
/// Bounded inventory of supported zone identifiers, not browser-provided SQL.
pub async fn time_zones(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
) -> Result<Vec<String>, IdentityError> {
    identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    read_time_zones(pool).await
}
async fn read_time_zones<'e, E>(executor: E) -> Result<Vec<String>, IdentityError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_scalar("SELECT name FROM pg_timezone_names WHERE name='UTC' OR name ~ '^(Africa|America|Antarctica|Arctic|Asia|Atlantic|Australia|Europe|Indian|Pacific)/' ORDER BY name LIMIT 1000")
        .fetch_all(executor).await?)
}
/// Reads on the caller transaction without another pool checkout.
pub async fn time_zones_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session: &Session,
) -> Result<Vec<String>, IdentityError> {
    Identity::authorize_read_transaction(tx, session, Permission::SettingsRead).await?;
    read_time_zones(&mut **tx).await
}
pub async fn save(
    pool: &PgPool,
    session: &Session,
    csrf: &str,
    time_zone: &str,
    date_format: &str,
    revision: i64,
) -> Result<(), RegionalError> {
    let mut tx = pool.begin().await?;
    save_in(&mut tx, session, csrf, time_zone, date_format, revision).await?;
    tx.commit().await?;
    Ok(())
}
/// Validates and saves within a caller-owned transaction.
pub async fn save_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session: &Session,
    csrf: &str,
    time_zone: &str,
    date_format: &str,
    revision: i64,
) -> Result<(), RegionalError> {
    Identity::authorize_transaction(tx, session, csrf, Permission::SettingsWrite).await?;
    if revision < 0 || time_zone.len() > 100 || !DATE_FORMATS.contains(&date_format) {
        return Err(RegionalError::Invalid);
    }
    let valid: bool = sqlx::query_scalar("SELECT platform_time_zone_valid($1)")
        .bind(time_zone)
        .fetch_one(&mut **tx)
        .await?;
    if !valid {
        return Err(RegionalError::Invalid);
    }
    let changed = sqlx::query("UPDATE platform_regional_settings SET time_zone=$1,date_format=$2,revision=revision+1 WHERE singleton AND revision=$3")
        .bind(time_zone).bind(date_format).bind(revision).execute(&mut **tx).await?;
    if changed.rows_affected() != 1 {
        return Err(RegionalError::Conflict);
    }
    Ok(())
}
/// Public diagnostics expose the current clock converted to the configured zone, without its label.
pub async fn current_time(pool: &PgPool) -> Result<String, IdentityError> {
    read_current_time(pool).await
}
async fn read_current_time<'e, E>(executor: E) -> Result<String, IdentityError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_scalar("SELECT platform_datetime(statement_timestamp(),time_zone,date_format) FROM platform_regional_settings WHERE singleton")
        .fetch_one(executor).await?)
}
/// Reads on the caller transaction without another pool checkout.
pub async fn current_time_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<String, IdentityError> {
    read_current_time(&mut **tx).await
}

/// Accepts a real Gregorian calendar day in the native HTML date input format.
pub fn valid_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let Ok(year) = value[..4].parse::<u16>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u8>() else {
        return false;
    };
    let Ok(day) = value[8..].parse::<u8>() else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}

impl From<IdentityError> for RegionalError {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}
