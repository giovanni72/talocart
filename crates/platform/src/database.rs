pub mod commercial_lock;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use sqlx::PgPool;
use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use thiserror::Error;

use crate::config::DatabaseConfig;

pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Shared SQL/data semantics, bumped only for incompatible changes (including
/// destructive migrations added after the required historical prefix).
pub const CONTRACT_VERSION: u32 = 4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SchemaContract {
    pub version: u32,
    pub migrations: usize,
    pub sha256: String,
}
impl SchemaContract {
    pub fn valid(&self) -> bool {
        self.version > 0
            && self.migrations > 0
            && self.migrations <= 10000
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
fn schema_hash<'a>(migrations: impl Iterator<Item = (i64, &'a [u8])>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"talocart-base-schema-v1\0");
    for (version, checksum) in migrations {
        hash.update(version.to_be_bytes());
        hash.update((checksum.len() as u64).to_be_bytes());
        hash.update(checksum);
    }
    format!("{:x}", hash.finalize())
}
/// Compiled schema checkpoint, never inferred from a possibly corrupt database.
pub fn contract_for(migrator: &Migrator) -> SchemaContract {
    SchemaContract {
        version: CONTRACT_VERSION,
        migrations: migrator.iter().count(),
        sha256: schema_hash(migrator.iter().map(|m| (m.version, m.checksum.as_ref()))),
    }
}
pub fn contract() -> SchemaContract {
    contract_for(&MIGRATOR)
}
/// The required migration history must be an exact prefix of this host's history.
pub fn supports_contract(required: &SchemaContract) -> bool {
    required.valid()
        && required.version == CONTRACT_VERSION
        && required.migrations <= MIGRATOR.iter().count()
        && required.sha256
            == schema_hash(
                MIGRATOR
                    .iter()
                    .take(required.migrations)
                    .map(|m| (m.version, m.checksum.as_ref())),
            )
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error(
        "Configured theme is not included in this build: use a compatible release or explicitly select an available theme"
    )]
    Theme,
    #[error("PostgreSQL unavailable: check connection and credentials")]
    Unavailable,
    #[error("Incompatible schema: run `talocart migrate` and verify the release version")]
    Schema,
    #[error("Migration failed: check permissions, schema and release compatibility")]
    Migration,
}

/// Preserve connection/search-path settings while forcing verification sessions read-only.
/// This string contains credentials: callers must keep it on the private startup channel.
pub fn read_only_url(source: &str) -> Result<String, DatabaseError> {
    let mut parsed = url::Url::parse(source).map_err(|_| DatabaseError::Unavailable)?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        return Err(DatabaseError::Unavailable);
    }
    let pairs: Vec<_> = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut options = Vec::new();
    parsed.set_query(None);
    for (key, value) in pairs {
        if key == "options" {
            options.push(value);
        } else if key != "options[default_transaction_read_only]" {
            parsed.query_pairs_mut().append_pair(&key, &value);
        }
    }
    options.push("-c default_transaction_read_only=on".into());
    parsed
        .query_pairs_mut()
        .append_pair("options", &options.join(" "));
    Ok(parsed.into())
}

pub async fn connect(config: &DatabaseConfig) -> Result<PgPool, DatabaseError> {
    connect_mode(config, false).await
}
/// Dedicated verification connections: every session rejects writes by default.
pub async fn connect_read_only(config: &DatabaseConfig) -> Result<PgPool, DatabaseError> {
    connect_mode(config, true).await
}
async fn connect_mode(config: &DatabaseConfig, read_only: bool) -> Result<PgPool, DatabaseError> {
    tokio::time::timeout(
        Duration::from_secs(config.timeout_seconds),
        PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.timeout_seconds))
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    if read_only {
                        sqlx::query("SET default_transaction_read_only = on")
                            .execute(connection)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(&config.url),
    )
    .await
    .map_err(|_| DatabaseError::Unavailable)?
    .map_err(|_| DatabaseError::Unavailable)
}

pub async fn migrate(pool: &PgPool) -> Result<(), DatabaseError> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|_| DatabaseError::Migration)?;
    let mut tx = pool.begin().await.map_err(|_| DatabaseError::Migration)?;
    let codes: Vec<_> = crate::locale::Locale::SUPPORTED
        .iter()
        .map(|(code, _)| *code)
        .collect();
    sqlx::query("INSERT INTO administrator_languages(code) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING").bind(codes).execute(&mut *tx).await.map_err(|_| DatabaseError::Migration)?;
    // Migration is the explicit write boundary. Before bootstrap, defaults follow this build.
    let installed: bool =
        sqlx::query_scalar("SELECT installed FROM identity_state WHERE id=1 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| DatabaseError::Migration)?;
    if !installed {
        crate::storefront::Languages::default()
            .persist(&mut tx)
            .await
            .map_err(|_| DatabaseError::Migration)?;
        sqlx::query("DELETE FROM storefront_language_prefixes")
            .execute(&mut *tx)
            .await
            .map_err(|_| DatabaseError::Migration)?;
    }
    tx.commit().await.map_err(|_| DatabaseError::Migration)
}

/// Read-only: checks connectivity and every applied migration's checksum.
/// Unlike migrate(), this never creates or modifies the schema.
pub async fn check(pool: &PgPool) -> Result<(), DatabaseError> {
    tokio::time::timeout(Duration::from_secs(3), async {
        sqlx::query("SELECT 1")
            .execute(pool)
            .await
            .map_err(|_| DatabaseError::Unavailable)?;
        check_migrations(pool, &MIGRATOR).await?;
        check_configuration(pool).await?;
        Ok(())
    })
    .await
    .map_err(|_| DatabaseError::Unavailable)?
}

async fn check_configuration(pool: &PgPool) -> Result<(), DatabaseError> {
    let theme: String = sqlx::query_scalar("SELECT theme_id FROM storefront_settings WHERE id=1")
        .fetch_one(pool)
        .await
        .map_err(|_| DatabaseError::Schema)?;
    crate::themes::Theme::parse(&theme).map_err(|_| DatabaseError::Theme)?;
    sqlx::query("SELECT id FROM platform_metadata WHERE id = 1")
        .fetch_one(pool)
        .await
        .map_err(|_| DatabaseError::Schema)?;
    let codes: Vec<_> = crate::locale::Locale::SUPPORTED
        .iter()
        .map(|(code, _)| *code)
        .collect();
    let available: i64 =
        sqlx::query_scalar("SELECT count(*) FROM administrator_languages WHERE code=ANY($1)")
            .bind(&codes)
            .fetch_one(pool)
            .await
            .map_err(|_| DatabaseError::Schema)?;
    let unavailable: i64 =
        sqlx::query_scalar("SELECT count(*) FROM storefront_languages WHERE NOT(code=ANY($1))")
            .bind(&codes)
            .fetch_one(pool)
            .await
            .map_err(|_| DatabaseError::Schema)?;
    if available != codes.len() as i64 || unavailable != 0 {
        return Err(DatabaseError::Schema);
    }
    Ok(())
}

/// A standalone module checks the entire host checkpoint and its own required
/// prefix. Extra/missing/dirty/changed migrations remain errors; no writes occur.
pub async fn check_module(
    pool: &PgPool,
    host: &SchemaContract,
    required: &SchemaContract,
) -> Result<(), DatabaseError> {
    if !host.valid()
        || !required.valid()
        || host.version != CONTRACT_VERSION
        || required.version != host.version
        || required.migrations > host.migrations
    {
        return Err(DatabaseError::Schema);
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        let applied = applied_migrations(pool, MIGRATOR.table_name.as_ref())
            .await?
            .ok_or(DatabaseError::Schema)?;
        if applied.len() != host.migrations
            || schema_hash(applied.iter().map(|(v, c)| (*v, c.as_slice()))) != host.sha256
            || schema_hash(
                applied
                    .iter()
                    .take(required.migrations)
                    .map(|(v, c)| (*v, c.as_slice())),
            ) != required.sha256
        {
            return Err(DatabaseError::Schema);
        }
        check_configuration(pool).await
    })
    .await
    .map_err(|_| DatabaseError::Unavailable)?
}

/// Read-only validation of a compiled migration set in the pool's current schema.
/// Ledger identifiers are trusted module constants, limited to unqualified ASCII names.
/// This never creates a ledger, applies migrations or aligns checksums.
pub async fn check_migrations(pool: &PgPool, migrator: &Migrator) -> Result<(), DatabaseError> {
    let applied = applied_migrations(pool, migrator.table_name.as_ref())
        .await?
        .ok_or(DatabaseError::Schema)?;
    if applied.len() != migrator.iter().count() {
        return Err(DatabaseError::Schema);
    }
    for (actual, expected) in applied.iter().zip(migrator.iter()) {
        if actual.0 != expected.version || actual.1 != expected.checksum.as_ref() {
            return Err(DatabaseError::Schema);
        }
    }
    Ok(())
}

/// Read-only `(version, checksum)` list of a ledger, or `None` when it does not exist yet.
/// A dirty (partially failed) migration is reported as a schema error.
pub async fn applied_migrations(
    pool: &PgPool,
    name: &str,
) -> Result<Option<Vec<(i64, Vec<u8>)>>, DatabaseError> {
    use sqlx::migrate::Migrate;
    if name.is_empty()
        || name.len() > 63
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        || !name.as_bytes()[0].is_ascii_lowercase() && name.as_bytes()[0] != b'_'
    {
        return Err(DatabaseError::Schema);
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_tables WHERE schemaname=current_schema() AND tablename=$1)")
            .bind(name).fetch_one(pool).await.map_err(|_| DatabaseError::Schema)?;
        if !exists { return Ok(None); }
        let mut connection = pool.acquire().await.map_err(|_| DatabaseError::Unavailable)?;
        if connection.dirty_version(name).await.map_err(|_| DatabaseError::Schema)?.is_some() {
            return Err(DatabaseError::Schema);
        }
        let applied = connection.list_applied_migrations(name).await.map_err(|_| DatabaseError::Schema)?;
        Ok(Some(applied.into_iter().map(|m| (m.version, m.checksum.into_owned())).collect()))
    }).await.map_err(|_| DatabaseError::Unavailable)?
}

/// One repeatable-read snapshot of migration identities, never business rows.
#[derive(Serialize)]
pub struct MigrationLedger {
    pub name: String,
    pub applied: Vec<(i64, Vec<u8>)>,
}
pub async fn migration_snapshot(
    pool: &PgPool,
    names: &[String],
) -> Result<Vec<MigrationLedger>, DatabaseError> {
    if names.is_empty() || names.len() > 33 {
        return Err(DatabaseError::Schema);
    }
    let names: std::collections::BTreeSet<_> = names.iter().collect();
    for name in &names {
        if name.is_empty()
            || name.len() > 63
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            || !name.as_bytes()[0].is_ascii_lowercase() && name.as_bytes()[0] != b'_'
        {
            return Err(DatabaseError::Schema);
        }
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut tx = pool.begin().await.map_err(|_| DatabaseError::Unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|_| DatabaseError::Unavailable)?;
        let mut ledgers = Vec::new();
        for name in names {
            // Identifiers are bounded and validated above; no metadata can provide SQL.
            let rows: Vec<(i64, Vec<u8>, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT version, checksum, success FROM \"{name}\" ORDER BY version LIMIT 1001"
            )))
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DatabaseError::Schema)?;
            if rows.len() > 1000
                || rows
                    .iter()
                    .any(|(_, checksum, success)| !success || checksum.len() != 48)
            {
                return Err(DatabaseError::Schema);
            }
            ledgers.push(MigrationLedger {
                name: name.clone(),
                applied: rows.into_iter().map(|(v, c, _)| (v, c)).collect(),
            });
        }
        tx.commit().await.map_err(|_| DatabaseError::Unavailable)?;
        Ok(ledgers)
    })
    .await
    .map_err(|_| DatabaseError::Unavailable)?
}

/// Read-only: whether the base schema still lacks migrations compiled into this executable.
pub async fn pending(pool: &PgPool) -> Result<bool, DatabaseError> {
    let applied = applied_migrations(pool, MIGRATOR.table_name.as_ref())
        .await?
        .unwrap_or_default();
    Ok(applied.len() < MIGRATOR.iter().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_sql_checkpoint_requires_an_explicit_compatibility_review() {
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("database-contract.json")).unwrap();
        assert_eq!(
            serde_json::to_value(contract()).unwrap(),
            snapshot["contract"],
            "Review shared SQL semantics and previous native modules before updating database-contract.json; bump CONTRACT_VERSION for incompatible changes"
        );
    }

    #[test]
    fn module_schema_requirements_are_exact_prefixes_not_release_versions() {
        let previous = Migrator {
            migrations: std::borrow::Cow::Owned(
                MIGRATOR
                    .iter()
                    .take(MIGRATOR.iter().count())
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        let required = contract_for(&previous);
        assert!(supports_contract(&required));
        assert!(supports_contract(&contract()));
        let mut changed = required.clone();
        changed.version += 1;
        assert!(!supports_contract(&changed));
        changed = required.clone();
        changed.sha256 = "00".repeat(32);
        assert!(!supports_contract(&changed));
        changed = contract();
        changed.migrations += 1;
        assert!(!supports_contract(&changed));
        changed = required;
        changed.migrations -= 1;
        assert!(
            !supports_contract(&changed),
            "Changing a checkpoint count must not authorize a different prefix"
        );
    }

    #[test]
    fn base_migrations_do_not_own_koala_tables_or_queries() {
        for migration in MIGRATOR.iter() {
            assert!(
                !migration.sql.as_str().contains("catalog_import_"),
                "Base migration {} depends on Koala",
                migration.version
            );
        }
    }

    #[tokio::test]
    async fn module_ledger_identifiers_are_validated_before_any_database_access() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/unreachable")
            .expect("Synthetic lazy pool");
        for name in [
            "",
            "schema.ledger",
            "ledger; DROP TABLE users",
            "123ledger",
            "QuotedLedger",
        ] {
            let migrator = Migrator {
                table_name: std::borrow::Cow::Borrowed(name),
                ..Migrator::DEFAULT
            };
            assert!(matches!(
                check_migrations(&pool, &migrator).await,
                Err(DatabaseError::Schema)
            ));
        }
    }
}
