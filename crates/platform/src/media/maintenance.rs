use super::*;
use std::{
    collections::HashSet,
    time::{Duration, SystemTime},
};
#[derive(Debug, Default)]
pub struct PruneReport {
    pub objects: u64,
    pub files: u64,
    pub busy: bool,
}
impl MediaStore {
    /// Registered deletion and orphan checks use short, separate publication locks.
    /// Filesystem discovery runs outside transactions; unknown source files are preserved.
    pub async fn prune(
        &self,
        pool: &sqlx::PgPool,
        dry_run: bool,
    ) -> Result<PruneReport, MediaError> {
        let mut report = self.prune_registered(pool, dry_run).await?;
        if report.busy {
            return Ok(report);
        }
        let mut candidates = Vec::new();
        let mut folders = vec![self.root.as_ref().clone()];
        while let Some(folder) = folders.pop() {
            self.check_parents(&folder.join("check.png")).await?;
            let mut directory = match tokio::fs::read_dir(&folder).await {
                Ok(d) => d,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            while let Some(entry) = directory.next_entry().await? {
                let kind = match entry.file_type().await {
                    Ok(kind) => kind,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                let relative = entry
                    .path()
                    .strip_prefix(self.root.as_path())
                    .map_err(|_| MediaError::Storage)?
                    .to_string_lossy()
                    .into_owned();
                let name = entry.file_name().to_string_lossy().into_owned();
                if matches!(relative.split('/').next(), Some("library" | "private")) {
                    continue;
                }
                if kind.is_dir() && self.relative_path(&relative).is_ok() {
                    folders.push(entry.path());
                } else if kind.is_file() && technical_file(&relative, &name) {
                    let metadata = match entry.metadata().await {
                        Ok(metadata) => metadata,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(e.into()),
                    };
                    let age = SystemTime::now()
                        .duration_since(metadata.modified()?)
                        .unwrap_or_default();
                    if age >= Duration::from_secs(86400) {
                        let key = relative
                            .strip_prefix("objects/")
                            .unwrap_or(&relative)
                            .to_owned();
                        candidates.push((key, entry.path()));
                    }
                    if candidates.len() == 100 {
                        report.files += self.prune_orphans(pool, &candidates, dry_run).await?;
                        candidates.clear();
                    }
                }
            }
        }
        report.files += self.prune_orphans(pool, &candidates, dry_run).await?;
        Ok(report)
    }

    async fn prune_registered(
        &self,
        pool: &sqlx::PgPool,
        dry_run: bool,
    ) -> Result<PruneReport, MediaError> {
        let mut tx = pool.begin().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(MEDIA_LOCK)
            .fetch_one(&mut *tx)
            .await?;
        if !locked {
            return Ok(PruneReport {
                busy: true,
                ..PruneReport::default()
            });
        }
        let mut report = PruneReport::default();
        let keys:Vec<String>=sqlx::query_scalar("SELECT key FROM media_objects WHERE unused_since<now()-interval '24 hours' AND key NOT IN (SELECT key FROM media_live_keys) ORDER BY unused_since,key LIMIT 100")
            .fetch_all(&mut *tx).await?;
        for key in &keys {
            let path: String = sqlx::query_scalar("SELECT path FROM media_locations WHERE key=$1")
                .bind(key)
                .fetch_one(&mut *tx)
                .await?;
            let physical = self.relative_path(&path)?;
            if !dry_run {
                sqlx::query("DELETE FROM media_objects WHERE key=$1")
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
            report.objects += 1;
            self.check_parents(&physical).await?;
            match tokio::fs::symlink_metadata(&physical).await {
                Ok(m) if m.is_file() => {
                    if !dry_run {
                        tokio::fs::remove_file(&physical).await?;
                    }
                    report.files += 1;
                }
                Ok(_) => return Err(MediaError::Storage),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            if !dry_run {
                // A purged original's retirement marker is complete with this removal;
                // do not process or count the same physical path again as an alias below.
                sqlx::query("DELETE FROM media_retired_files WHERE path=$1")
                    .bind(&path)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        let retired: Vec<String> = sqlx::query_scalar("SELECT path FROM media_retired_files WHERE retired_at<now()-interval '24 hours' AND lower(path) NOT IN(SELECT lower(path) FROM media_locations) LIMIT 100").fetch_all(&mut *tx).await?;
        for path in retired {
            let physical = self.relative_path(&path)?;
            self.check_parents(&physical).await?;
            if !dry_run {
                match tokio::fs::remove_file(&physical).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                sqlx::query("DELETE FROM media_retired_files WHERE path=$1")
                    .bind(&path)
                    .execute(&mut *tx)
                    .await?;
            }
            report.files += 1;
        }
        // Remove only empty retired directory aliases, deepest first. New FTP
        // originals and folders explicitly adopted again must remain untouched.
        let directories: Vec<(String, bool)> = sqlx::query_as(
            "SELECT d.path,EXISTS(SELECT 1 FROM media_folders f WHERE f.path=d.path)
             FROM media_retired_directories d WHERE retired_at<now()-interval '24 hours'
             ORDER BY length(path) DESC LIMIT 100",
        )
        .fetch_all(&mut *tx)
        .await?;
        for (relative, adopted) in directories {
            let path = self.relative_path(&relative)?;
            self.check_parents(&path).await?;
            if !dry_run {
                if !adopted {
                    match tokio::fs::remove_dir(path).await {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => continue,
                        Err(e) => return Err(e.into()),
                    }
                }
                sqlx::query("DELETE FROM media_retired_directories WHERE path=$1")
                    .bind(relative)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(report)
    }
    async fn prune_orphans(
        &self,
        pool: &sqlx::PgPool,
        candidates: &[(String, PathBuf)],
        dry_run: bool,
    ) -> Result<u64, MediaError> {
        if candidates.is_empty() {
            return Ok(0);
        }
        let mut tx = pool.begin().await?;
        Self::exclusive_lock(&mut tx).await?;
        let registered: HashSet<String> =
            sqlx::query_scalar::<_, String>("SELECT key FROM media_objects WHERE key=ANY($1) UNION SELECT path FROM media_locations WHERE path=ANY($2) UNION SELECT path FROM media_retired_files WHERE path=ANY($2)")
                .bind(
                    candidates
                        .iter()
                        .map(|(key, _)| key.as_str())
                        .collect::<Vec<_>>(),
                )
                .bind(candidates.iter().map(|(_,path)| path.strip_prefix(self.root.as_path()).map(|p|p.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>())
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect();
        let mut count = 0;
        for (key, path) in candidates {
            let relative = path
                .strip_prefix(self.root.as_path())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            if registered.contains(key) || registered.contains(&relative) {
                continue;
            }
            // Recheck availability and age under the lock, after discovery and registration checks.
            self.check_parents(path).await?;
            let metadata = match tokio::fs::symlink_metadata(path).await {
                Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta,
                Ok(_) => return Err(MediaError::Storage),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if SystemTime::now()
                .duration_since(metadata.modified()?)
                .unwrap_or_default()
                < Duration::from_secs(86400)
            {
                continue;
            }
            if !dry_run {
                tokio::fs::remove_file(path).await?;
            }
            count += 1;
        }
        tx.commit().await?;
        Ok(count)
    }
}

// Only application-generated names identify orphan bytes. A readable source filename is
// insufficient evidence for deletion, even inside objects/brand or an import directory.
fn technical_file(relative: &str, name: &str) -> bool {
    let token = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
    if name
        .strip_prefix(".upload-")
        .and_then(|s| s.strip_suffix(".pending"))
        .is_some_and(token)
    {
        return true;
    }
    let parts: Vec<_> = relative.split('/').collect();
    match parts.as_slice() {
        ["cache", id, revision, _] => {
            id.parse::<i64>().is_ok_and(|id| id > 0)
                && token(revision)
                && MediaStore::valid_import_key(relative)
        }
        ["staging", revision, _] => token(revision) && MediaStore::valid_import_key(relative),
        _ => false,
    }
}
