use super::*;
#[derive(Debug, Default)]
pub struct AcquisitionReport {
    pub acquired: u64,
    pub existing: u64,
    pub rejected: u64,
}
impl MediaStore {
    /// Explicit intake only. Technical and private namespaces cannot be published by acquisition.
    pub async fn acquire(
        &self,
        pool: &sqlx::PgPool,
        directory: &str,
        dry_run: bool,
    ) -> Result<AcquisitionReport, MediaError> {
        if matches!(
            directory.split('/').next(),
            Some("objects" | "brand" | "cache" | "private" | "staging")
        ) {
            return Err(MediaError::Invalid);
        }
        let root = self.relative_path(directory)?;
        self.check_parents(&root.join("check.png")).await?;
        let mut folders = vec![root];
        let mut report = AcquisitionReport::default();
        while let Some(folder) = folders.pop() {
            let mut entries = tokio::fs::read_dir(&folder).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let relative = path
                    .strip_prefix(self.root.as_path())
                    .map_err(|_| MediaError::Storage)?
                    .to_string_lossy()
                    .into_owned();
                let kind = entry.file_type().await?;
                if kind.is_symlink() {
                    report.rejected += 1;
                    continue;
                }
                if kind.is_dir() {
                    if self.relative_path(&relative).is_ok() {
                        folders.push(path);
                    } else {
                        report.rejected += 1;
                    }
                    continue;
                }
                if !kind.is_file() || !Self::valid_import_key(&relative) {
                    report.rejected += 1;
                    continue;
                }
                if sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM media_locations WHERE path=$1)",
                )
                .bind(&relative)
                .fetch_one(pool)
                .await?
                {
                    report.existing += 1;
                    continue;
                }
                let raw = match self.read_path(&path).await {
                    Ok(raw) => raw,
                    Err(_) => {
                        report.rejected += 1;
                        continue;
                    }
                };
                let prepared = match self.prepare_source(raw).await {
                    Ok(image) => image,
                    Err(_) => {
                        report.rejected += 1;
                        continue;
                    }
                };
                if !dry_run {
                    self.register_resident(pool, prepared, &relative).await?;
                }
                report.acquired += 1;
            }
        }
        Ok(report)
    }
}

impl MediaStore {
    /// Caller holds the media publication lock and identity lock. Retain a legacy private avatar
    /// without making it public; shared historical objects are copied for the second owner.
    pub async fn retain_private(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
        owner: i64,
    ) -> Result<String, MediaError> {
        let existing: Option<Option<i64>> =
            sqlx::query_scalar("SELECT owner_id FROM media_library WHERE key=$1")
                .bind(key)
                .fetch_optional(&mut **tx)
                .await?;
        if existing == Some(Some(owner)) {
            return Ok(key.into());
        }
        let (old,bytes,width,height,mime):(String,i64,i32,i32,String)=sqlx::query_as("SELECT l.path,m.byte_size,m.width,m.height,m.content_type FROM media_objects m JOIN media_locations l ON l.key=m.key WHERE m.key=$1 AND NOT m.public").bind(key).fetch_optional(&mut **tx).await?.ok_or(MediaError::Invalid)?;
        let directory = format!("private/account-{owner}");
        self.ensure_directory(&directory).await?;
        sqlx::query("INSERT INTO media_folders(name,path,owner_id,protected) VALUES('Le mie immagini',$1,$2,true) ON CONFLICT(path) DO NOTHING").bind(&directory).bind(owner).execute(&mut **tx).await?;
        let folder: i64 =
            sqlx::query_scalar("SELECT id FROM media_folders WHERE path=$1 AND owner_id=$2")
                .bind(&directory)
                .bind(owner)
                .fetch_one(&mut **tx)
                .await?;
        let name = key.rsplit('/').next().ok_or(MediaError::Invalid)?;
        let (stem, extension) = name.rsplit_once('.').ok_or(MediaError::Invalid)?;
        for number in 1..=10000 {
            let name = if number == 1 {
                name.into()
            } else {
                format!("{stem}-{number}.{extension}")
            };
            let target = format!("{directory}/{name}");
            match self.link_file(&old, &target).await {
                Ok(()) => {}
                Err(MediaError::Invalid) => continue,
                Err(e) => return Err(e),
            }
            let effective_key = if existing.is_some() {
                let content_type = match mime.as_str() {
                    "image/png" => "image/png",
                    "image/jpeg" => "image/jpeg",
                    "image/webp" => "image/webp",
                    _ => return Err(MediaError::Storage),
                };
                let stored = StoredMedia {
                    key: target.clone(),
                    bytes,
                    width,
                    height,
                    variants: Vec::new(),
                    content_type,
                };
                stored.register_internal(tx, false).await?;
                stored.key
            } else {
                sqlx::query("UPDATE media_locations SET path=$1 WHERE key=$2")
                    .bind(&target)
                    .bind(key)
                    .execute(&mut **tx)
                    .await?;
                sqlx::query(
                    "INSERT INTO media_retired_files(path) VALUES($1) ON CONFLICT(path) DO NOTHING",
                )
                .bind(&old)
                .execute(&mut **tx)
                .await?;
                key.into()
            };
            sqlx::query(
                "INSERT INTO media_library(key,folder_id,name,owner_id) VALUES($1,$2,$3,$4)",
            )
            .bind(&effective_key)
            .bind(folder)
            .bind(name)
            .bind(owner)
            .execute(&mut **tx)
            .await?;
            jobs::enqueue(tx, Some(&effective_key), true, jobs::Enqueue::New).await?;
            return Ok(effective_key);
        }
        Err(MediaError::Storage)
    }
    /// Explicit post-migration acquisition of current private avatars; idempotent and bounded.
    pub async fn index_private_avatars(&self, pool: &sqlx::PgPool) -> Result<u64, MediaError> {
        let mut after = 0;
        let mut count = 0;
        loop {
            let rows:Vec<(i64,String)>=sqlx::query_as("SELECT a.id,a.avatar_key FROM administrators a JOIN media_objects m ON m.key=a.avatar_key WHERE a.id>$1 AND NOT m.public AND NOT EXISTS(SELECT 1 FROM media_library l WHERE l.key=a.avatar_key AND l.owner_id=a.id) AND NOT EXISTS(SELECT 1 FROM media_transforms t WHERE t.object_key=a.avatar_key) ORDER BY a.id LIMIT 100").bind(after).fetch_all(pool).await?;
            if rows.is_empty() {
                break;
            }
            for (owner, key) in rows {
                after = owner;
                let mut tx = pool.begin().await?;
                Self::exclusive_lock(&mut tx).await?;
                sqlx::query("SELECT id FROM identity_state WHERE id=1 FOR UPDATE")
                    .fetch_one(&mut *tx)
                    .await?;
                let current:Option<String>=sqlx::query_scalar("SELECT avatar_key FROM administrators WHERE id=$1 AND avatar_key=$2 FOR UPDATE").bind(owner).bind(&key).fetch_optional(&mut *tx).await?;
                if current.is_none() {
                    continue;
                }
                let effective = self.retain_private(&mut tx, &key, owner).await?;
                if effective != key {
                    sqlx::query("UPDATE administrators SET avatar_key=$1 WHERE id=$2")
                        .bind(&effective)
                        .bind(owner)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                count += 1;
            }
        }
        Ok(count)
    }
}
