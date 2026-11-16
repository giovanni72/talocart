//! Background intake of resident sources. Physical bytes and commercial references are untouched.
use super::*;
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub acquired: u64,
    pub existing: u64,
    pub deferred: u64,
    pub rejected: u64,
}

pub(super) fn document_type(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}
pub(super) fn join_path(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.into()
    } else {
        format!("{folder}/{name}")
    }
}
pub(super) fn excluded(path: &str) -> bool {
    matches!(path.split('/').next(), Some("cache" | "staging"))
        || path.split('/').any(|part| {
            part.starts_with('.')
                || part.ends_with(".part")
                || part.ends_with(".tmp")
                || part.ends_with(".pending")
        })
}

impl MediaStore {
    /// Read-only visibility check for registered tree candidates. Never follows symlinks.
    pub(super) async fn library_directory_exists(
        &self,
        relative: &str,
    ) -> Result<bool, MediaError> {
        if relative.is_empty() || relative == "private" || excluded(relative) {
            return Ok(false);
        }
        let path = self.relative_path(relative)?;
        if self.check_parents(&path.join("check.png")).await.is_err() {
            return Ok(false);
        }
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    pub(super) async fn prepare_library_source(
        &self,
        raw: Vec<u8>,
        name: &str,
    ) -> Result<PreparedImport, MediaError> {
        let ext = name
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
            return self.prepare_source(raw).await;
        }
        if raw.is_empty() || raw.len() > 10 * 1024 * 1024 {
            return Err(MediaError::Invalid);
        }
        Ok(PreparedImport {
            raw,
            content_type: document_type(name),
            width: 0,
            height: 0,
        })
    }
    pub(super) async fn put_library_source(
        &self,
        prepared: PreparedImport,
        token: &str,
    ) -> Result<StoredMedia, MediaError> {
        let key = format!("staging/{token}/source.png");
        let key = self.publish_bytes(&key, &prepared.raw, false).await?;
        Ok(StoredMedia {
            key,
            bytes: i64::try_from(prepared.raw.len()).map_err(|_| MediaError::Invalid)?,
            width: prepared.width as i32,
            height: prepared.height as i32,
            content_type: prepared.content_type,
            variants: Vec::new(),
        })
    }
    /// Validated open for bounded-memory authenticated downloads, including large documents.
    pub async fn open_registered(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
    ) -> Result<tokio::fs::File, MediaError> {
        let path = self.registered_path(pool, key).await?;
        self.check_parents(&path).await?;
        let metadata = tokio::fs::symlink_metadata(&path).await?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(MediaError::Storage);
        }
        let file = tokio::fs::File::open(&path).await?;
        self.check_parents(&path).await?;
        if !file.metadata().await?.is_file() {
            return Err(MediaError::Storage);
        }
        Ok(file)
    }
    /// One scan at startup and periodically thereafter. Reads occur outside short publication
    /// transactions; candidate lookups are batched, and repeated scans do not decode known files.
    pub async fn discover_files(&self, pool: &sqlx::PgPool) -> Result<DiscoveryReport, MediaError> {
        let owners: Vec<i64> = sqlx::query_scalar("SELECT id FROM administrators")
            .fetch_all(pool)
            .await?;
        let retired_directories: std::collections::HashSet<String> =
            sqlx::query_scalar("SELECT path FROM media_retired_directories")
                .fetch_all(pool)
                .await?
                .into_iter()
                .collect();
        let mut report = DiscoveryReport::default();
        let mut folders = vec![self.root.as_ref().clone()];
        let mut batch = Vec::new();
        while let Some(folder) = folders.pop() {
            self.check_parents(&folder.join("check.png")).await?;
            let mut entries = match tokio::fs::read_dir(&folder).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    report.rejected += 1;
                    continue;
                }
            };
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let Some(relative) = path
                    .strip_prefix(self.root.as_path())
                    .ok()
                    .and_then(Path::to_str)
                else {
                    report.rejected += 1;
                    continue;
                };
                if excluded(relative) {
                    continue;
                }
                let owner = match source_owner(relative, &owners) {
                    Ok(owner) => owner,
                    Err(()) => {
                        report.rejected += 1;
                        continue;
                    }
                };
                if self.relative_path(relative).is_err() {
                    report.rejected += 1;
                    continue;
                }
                let kind = match entry.file_type().await {
                    Ok(kind) => kind,
                    Err(_) => {
                        report.deferred += 1;
                        continue;
                    }
                };
                if kind.is_symlink() {
                    report.rejected += 1;
                    continue;
                }
                if kind.is_dir() {
                    // The private namespace itself is never a shared folder.
                    if relative != "private" && !retired_directories.contains(relative) {
                        self.discovered_folder(pool, relative, owner).await?;
                    }
                    folders.push(path);
                } else if kind.is_file() {
                    batch.push((relative.to_owned(), owner));
                    if batch.len() == 100 {
                        self.discover_batch(pool, &batch, &mut report).await?;
                        batch.clear();
                    }
                }
            }
        }
        self.discover_batch(pool, &batch, &mut report).await?;
        Ok(report)
    }
    async fn discover_batch(
        &self,
        pool: &sqlx::PgPool,
        batch: &[(String, Option<i64>)],
        report: &mut DiscoveryReport,
    ) -> Result<(), MediaError> {
        if batch.is_empty() {
            return Ok(());
        }
        let paths: Vec<&str> = batch.iter().map(|(path, _)| path.as_str()).collect();
        let known: Vec<(String,String,bool,bool,bool)> = sqlx::query_as("SELECT p.path,m.key,m.public,l.key IS NOT NULL,EXISTS(SELECT 1 FROM media_variants WHERE object_key=m.key) OR EXISTS(SELECT 1 FROM media_renditions WHERE object_key=m.key AND source_key<>m.key) OR EXISTS(SELECT 1 FROM media_transforms WHERE object_key=m.key AND source_key<>m.key) FROM media_locations p JOIN media_objects m ON m.key=p.key LEFT JOIN media_library l ON l.key=m.key WHERE p.path=ANY($1) UNION ALL SELECT path,''::text,false,false,true FROM media_retired_files WHERE path=ANY($1)").bind(paths).fetch_all(pool).await?;
        let known: HashMap<_, _> = known
            .into_iter()
            .map(|(path, key, public, library, technical)| {
                (path, (key, public, library, technical))
            })
            .collect();
        for (relative, owner) in batch {
            if let Some((key, public, library, technical)) = known.get(relative) {
                if *public && !library && !technical {
                    let mut tx = pool.begin().await?;
                    Self::exclusive_lock(&mut tx).await?;
                    // Recheck under the publication lock: purge may have retired the path
                    // since this batch read its known sources.
                    sqlx::query("SELECT media_adopt_source($1) WHERE NOT EXISTS(SELECT 1 FROM media_retired_files WHERE path=$2)")
                        .bind(key)
                        .bind(relative)
                        .execute(&mut *tx)
                        .await?;
                    jobs::enqueue(&mut tx, Some(key), false, jobs::Enqueue::New).await?;
                    tx.commit().await?;
                }
                report.existing += 1;
                continue;
            }
            match self.discover_file(pool, relative, *owner).await {
                Ok(true) => report.acquired += 1,
                Ok(false) | Err(MediaError::Busy | MediaError::Missing) => report.deferred += 1,
                Err(_) => report.rejected += 1,
            }
        }
        Ok(())
    }
    async fn discover_file(
        &self,
        pool: &sqlx::PgPool,
        relative: &str,
        owner: Option<i64>,
    ) -> Result<bool, MediaError> {
        let path = self.relative_path(relative)?;
        self.check_parents(&path).await?;
        let before = tokio::fs::symlink_metadata(&path).await?;
        if !before.is_file() || before.file_type().is_symlink() {
            return Err(MediaError::Invalid);
        }
        if SystemTime::now()
            .duration_since(before.modified()?)
            .unwrap_or_default()
            < Duration::from_secs(2)
        {
            return Ok(false);
        }
        let mut mime = document_type(relative);
        let mut width = 0;
        let mut height = 0;
        if before.len() > 0 && before.len() <= 10 * 1024 * 1024 {
            let raw = self.read_path(&path).await?;
            match self.prepare_resident_source(raw).await {
                Ok(prepared) => {
                    mime = prepared.content_type;
                    width = prepared.width as i32;
                    height = prepared.height as i32;
                }
                Err(MediaError::Busy) => return Err(MediaError::Busy),
                Err(MediaError::Invalid) => {}
                Err(error) => return Err(error),
            }
        }
        let mut tx = pool.begin().await?;
        Self::exclusive_lock(&mut tx).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_locations WHERE path=$1) OR EXISTS(SELECT 1 FROM media_retired_files WHERE path=$1)")
                .bind(relative)
                .fetch_one(&mut *tx)
                .await?;
        if exists {
            return Ok(false);
        }
        self.check_parents(&path).await?;
        let after = tokio::fs::symlink_metadata(&path).await?;
        if after.file_type().is_symlink()
            || !after.is_file()
            || before.len() != after.len()
            || before.modified()? != after.modified()?
        {
            return Ok(false);
        }
        let key = format!(
            "{}.png",
            crate::identity::token().map_err(|_| MediaError::Storage)?
        );
        let stored = StoredMedia {
            key: key.clone(),
            bytes: i64::try_from(after.len()).map_err(|_| MediaError::Invalid)?,
            width,
            height,
            content_type: mime,
            variants: Vec::new(),
        };
        stored.register_internal(&mut tx, owner.is_none()).await?;
        sqlx::query("UPDATE media_locations SET path=$1 WHERE key=$2")
            .bind(relative)
            .bind(&key)
            .execute(&mut *tx)
            .await?;
        if let Some(owner) = owner {
            let (directory, name) = relative.rsplit_once('/').ok_or(MediaError::Invalid)?;
            let folder = adopt_folder(&mut tx, directory, Some(owner)).await?;
            sqlx::query(
                "INSERT INTO media_library(key,folder_id,name,owner_id) VALUES($1,$2,$3,$4)",
            )
            .bind(&key)
            .bind(folder)
            .bind(name)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("SELECT media_adopt_source($1)")
                .bind(&key)
                .execute(&mut *tx)
                .await?;
        }
        jobs::enqueue(&mut tx, Some(&key), true, jobs::Enqueue::New).await?;
        tx.commit().await?;
        Ok(true)
    }
    async fn discovered_folder(
        &self,
        pool: &sqlx::PgPool,
        path: &str,
        owner: Option<i64>,
    ) -> Result<(), MediaError> {
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_folders WHERE path=$1 AND owner_id IS NOT DISTINCT FROM $2)").bind(path).bind(owner).fetch_one(pool).await?;
        if !exists {
            let mut tx = pool.begin().await?;
            Self::exclusive_lock(&mut tx).await?;
            let retired: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM media_retired_directories WHERE path=$1)",
            )
            .bind(path)
            .fetch_one(&mut *tx)
            .await?;
            if !retired && self.library_directory_exists(path).await? {
                adopt_folder(&mut tx, path, owner).await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }
}
fn source_owner(path: &str, owners: &[i64]) -> Result<Option<i64>, ()> {
    let parts: Vec<_> = path.split('/').collect();
    if parts[0] != "private" || parts.len() == 1 {
        return Ok(None);
    }
    let owner = parts[1]
        .strip_prefix("account-")
        .and_then(|id| id.parse::<i64>().ok())
        .filter(|id| owners.contains(id))
        .ok_or(())?;
    Ok(Some(owner))
}
async fn adopt_folder(
    tx: &mut Transaction<'_, Postgres>,
    path: &str,
    owner: Option<i64>,
) -> Result<i64, MediaError> {
    let parts: Vec<_> = path.split('/').collect();
    let mut parent = None;
    let mut current = String::new();
    for (index, part) in parts.iter().enumerate() {
        current = join_path(&current, part);
        if owner.is_some() && index == 0 {
            continue;
        }
        sqlx::query("INSERT INTO media_folders(parent_id,name,path,owner_id,protected) VALUES($1,$2,$3,$4,$5) ON CONFLICT(path) DO NOTHING")
            .bind(parent).bind(part).bind(&current).bind(owner).bind(index==0 && matches!(*part,"objects"|"brand"|"library") || owner.is_some() && index==1).execute(&mut **tx).await?;
        parent = Some(
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM media_folders WHERE path=$1 AND owner_id IS NOT DISTINCT FROM $2",
            )
            .bind(&current)
            .bind(owner)
            .fetch_one(&mut **tx)
            .await?,
        );
    }
    parent.ok_or(MediaError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn resident_decode_waits_for_a_slot_without_rejecting_a_valid_source() {
        let store = MediaStore::new(PathBuf::from("unused-media-root"));
        let occupied = store.imaging.clone().acquire_many_owned(2).await.unwrap();
        let decode = store.prepare_resident_source(
            include_bytes!("../../../web/tests/fixtures/avatar.png").to_vec(),
        );
        let release = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(occupied);
        };
        let (result, ()) = tokio::join!(decode, release);
        assert!(result.unwrap().width > 0);
    }
}
