//! One enqueue policy for uploads, explicit maintenance, restoration and HTTP repair hints.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Enqueue {
    New,
    AutomaticRepair,
    Repair,
    Rebuild,
}

/// A source must already belong to the library. Pending/running claims are never changed.
/// Only explicit user actions retry failed work; new registration is strictly insert-only.
pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    source: Option<&str>,
    include_private: bool,
    policy: Enqueue,
) -> Result<(), sqlx::Error> {
    if matches!(policy, Enqueue::New) {
        sqlx::query("INSERT INTO media_cache_jobs(source_key) SELECT key FROM media_library WHERE ($1::text IS NULL OR key=$1) AND ($2 OR owner_id IS NULL) AND trashed_at IS NULL AND key IN(SELECT key FROM media_objects WHERE width>0 AND height>0) ON CONFLICT DO NOTHING")
            .bind(source).bind(include_private).execute(&mut **tx).await?;
        return Ok(());
    }
    let eligible: &[&str] = match policy {
        Enqueue::New => &[],
        Enqueue::AutomaticRepair => &["complete"],
        Enqueue::Repair | Enqueue::Rebuild => &["complete", "failed"],
    };
    sqlx::query(
        "INSERT INTO media_cache_jobs(source_key,rebuild)
         SELECT key,$3 FROM media_library
         WHERE ($1::text IS NULL OR key=$1) AND ($2 OR owner_id IS NULL) AND trashed_at IS NULL AND key IN(SELECT key FROM media_objects WHERE width>0 AND height>0)
         ON CONFLICT(source_key) DO UPDATE SET status='pending',attempts=0,
          lease_until=NULL,lease_token=NULL,rebuild=excluded.rebuild,updated_at=now()
         WHERE media_cache_jobs.status=ANY($4)",
    )
    .bind(source)
    .bind(include_private)
    .bind(matches!(policy, Enqueue::Rebuild))
    .bind(eligible)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl MediaStore {
    /// Owning mutation holds the publication lock and commits the selected source and job.
    pub(crate) async fn queue_selected_source(
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
    ) -> Result<(), sqlx::Error> {
        enqueue(tx, Some(key), false, Enqueue::Repair).await
    }
}
