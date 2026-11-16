//! HTTP hints are bounded and coalesced. Only the background worker writes persistent jobs.
use super::*;

impl MediaStore {
    pub(crate) fn request_cache_repair(&self, key: &str) {
        match self.cache_repairs.try_lock() {
            Ok(mut hints) => {
                if hints.len() < 128 {
                    hints.insert(key.to_owned());
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
            Err(std::sync::TryLockError::Poisoned(_)) => {
                tracing::warn!("Cache repair notification temporarily unavailable");
            }
        }
    }

    pub(crate) async fn persist_cache_repairs(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<(), MediaError> {
        // No synchronous lock survives an await; contention never blocks the Tokio executor.
        let hints = {
            let mut pending = match self.cache_repairs.try_lock() {
                Ok(pending) => pending,
                Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
                Err(std::sync::TryLockError::Poisoned(_)) => return Err(MediaError::Storage),
            };
            let hints: Vec<_> = pending.iter().take(16).cloned().collect();
            for key in &hints {
                pending.remove(key);
            }
            hints
        };
        for (index, key) in hints.iter().enumerate() {
            if let Err(error) = self.persist_cache_repair(pool, key).await {
                for retry in &hints[index..] {
                    self.request_cache_repair(retry);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    async fn persist_cache_repair(&self, pool: &sqlx::PgPool, key: &str) -> Result<(), MediaError> {
        let mut tx = pool.begin().await?;
        Self::publish_lock(&mut tx).await?;
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM media_library WHERE key=$1 AND trashed_at IS NULL)",
        )
        .bind(key)
        .fetch_one(&mut *tx)
        .await?;
        if !live {
            return Ok(());
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM media_cache_jobs WHERE source_key=$1 FOR UPDATE",
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        // Keep pending/running work and failures intact: no unlimited retries from browser traffic.
        if status.as_deref().is_some_and(|status| status != "complete") {
            return Ok(());
        }
        let mut profiles = ImageProfile::STANDARD.to_vec();
        let kinds: Vec<String> =
            sqlx::query_scalar("SELECT kind FROM media_transforms WHERE source_key=$1")
                .bind(key)
                .fetch_all(&mut *tx)
                .await?;
        for kind in kinds {
            profiles.push(ImageProfile::parse(&kind).ok_or(MediaError::Storage)?);
        }
        let mut missing = false;
        for profile in profiles {
            if !self.profile_available_on(&mut tx, key, profile).await? {
                missing = true;
                break;
            }
        }
        if missing {
            jobs::enqueue(&mut tx, Some(key), true, jobs::Enqueue::AutomaticRepair).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
