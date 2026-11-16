//! Database-wide commercial guard shared by platform, commerce and SQL triggers.
//!
//! The key is a persistent SQL contract: migrations contain the same bigint literal.
//! Do not change it independently of the triggers. Locks last until transaction end.
//! Acquire at the existing call-site position, before protected reads/row locks;
//! checkout needs exclusive mode upfront because stock-write triggers also acquire it.
//! A shared-to-exclusive upgrade mid-checkout would change the lock ordering.

use sqlx::{Postgres, Transaction};

/// Stable bigint key used by commercial guards in the base SQL migrations.
pub const KEY: i64 = 5426645964561798229;

/// Serialize commercial writers, including checkout and its stock-write triggers.
/// Errors are translated by the calling domain, preserving its diagnostic context.
pub async fn exclusive(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Keep fiscal/currency configuration stable for readers until transaction end.
/// Multiple shared holders coexist; exclusive holders conflict with both modes.
pub async fn shared(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
        .bind(KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
