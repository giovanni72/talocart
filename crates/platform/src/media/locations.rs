use super::*;
impl MediaStore {
    pub(crate) async fn file_available(&self, file: &ImageFile) -> Result<bool, MediaError> {
        let path = self.relative_path(&file.path)?;
        self.check_parents(&path).await?;
        match tokio::fs::symlink_metadata(path).await {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                Ok(meta.len() == file.byte_size as u64)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            _ => Err(MediaError::Storage),
        }
    }

    // Requested current/legacy profile, another usable derivative, then the original.
    // This only inspects existing representations; missing cache sends a bounded memory hint.
    pub(crate) async fn available_rendition_file(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
        profile: &str,
        public: bool,
    ) -> Result<Option<ImageFile>, MediaError> {
        let Some(requested_profile) = ImageProfile::parse(profile) else {
            return Ok(None);
        };
        let transformed: Option<(String, String)> = sqlx::query_as(
            "SELECT source_key,kind FROM media_transforms WHERE object_key=$1 LIMIT 1",
        )
        .bind(key)
        .fetch_optional(pool)
        .await?;
        if let Some((source, kind)) = transformed {
            return self
                .attachment_file(pool, &source, &kind, profile, public)
                .await;
        }
        if requested_profile.attachment() {
            return self
                .attachment_file(pool, key, profile, "original", public)
                .await;
        }
        #[derive(sqlx::FromRow)]
        struct Candidate {
            #[sqlx(flatten)]
            file: ImageFile,
            generation: i32,
            profile: String,
        }
        let mut candidates:Vec<Candidate>=sqlx::query_as("WITH candidates AS (SELECT object_key AS key,profile,0 AS generation FROM media_renditions WHERE source_key=$1 AND profile=ANY($4) UNION ALL SELECT object_key,profile,1 FROM media_variants WHERE source_key=$1 UNION ALL SELECT $1::text,'original',2) SELECT m.id,m.key,m.width,m.height,m.content_type,m.byte_size,l.path,c.generation,c.profile FROM candidates c JOIN media_objects m ON m.key=c.key JOIN media_locations l ON l.key=m.key WHERE m.width>0 AND m.height>0 AND (NOT $3 OR m.public) AND ($2<>'original' OR c.generation=2)")
            .bind(key).bind(profile).bind(public).bind(ImageProfile::STANDARD.iter().map(|p|p.key()).collect::<Vec<_>>()).fetch_all(pool).await?;
        // At most eight representations. Keep preference rules alongside the typed profile registry.
        candidates.sort_by_key(|candidate| {
            let priority = if candidate.generation == 2 {
                3
            } else if candidate.profile == profile {
                candidate.generation
            } else {
                2
            };
            let edge = ImageProfile::parse(&candidate.profile).map_or(0, ImageProfile::edge);
            let wanted = requested_profile.edge();
            let distance = if edge >= wanted {
                edge - wanted
            } else {
                10000 + wanted - edge
            };
            (priority, distance, candidate.generation, candidate.file.id)
        });
        let mut requested_seen = profile == "original";
        for candidate in candidates {
            let requested = candidate.generation == 0 && candidate.profile == profile;
            if requested {
                requested_seen = true;
            }
            if !self.file_available(&candidate.file).await? {
                if requested {
                    self.request_cache_repair(key);
                }
                continue;
            }
            if !requested_seen {
                self.request_cache_repair(key);
            }
            return Ok(Some(candidate.file));
        }
        if !requested_seen {
            self.request_cache_repair(key);
        }
        Ok(None)
    }

    async fn attachment_file(
        &self,
        pool: &sqlx::PgPool,
        source: &str,
        profile: &str,
        requested: &str,
        public: bool,
    ) -> Result<Option<ImageFile>, MediaError> {
        // Never fall back to an uncropped source or a different attachment kind.
        let files:Vec<ImageFile>=sqlx::query_as("SELECT m.id,m.key,m.width,m.height,m.content_type,m.byte_size,l.path FROM (SELECT object_key,0 AS rank FROM media_renditions WHERE source_key=$1 AND profile=$2 UNION ALL SELECT object_key,1 FROM media_transforms WHERE source_key=$1 AND kind=$2) r JOIN media_objects m ON m.key=r.object_key JOIN media_locations l ON l.key=m.key WHERE NOT $3 OR m.public ORDER BY r.rank").bind(source).bind(profile).bind(public).fetch_all(pool).await?;
        for file in files {
            let main_available = self.file_available(&file).await?;
            if !main_available {
                self.request_cache_repair(source);
            }
            if matches!(requested, "thumbnail" | "card" | "detail") {
                let child:Option<ImageFile>=sqlx::query_as("SELECT m.id,m.key,m.width,m.height,m.content_type,m.byte_size,l.path FROM media_variants v JOIN media_objects m ON m.key=v.object_key JOIN media_locations l ON l.key=m.key WHERE v.source_key=$1 AND v.profile=$2 AND (NOT $3 OR m.public)").bind(&file.key).bind(requested).bind(public).fetch_optional(pool).await?;
                if let Some(child) = child
                    && self.file_available(&child).await?
                {
                    return Ok(Some(child));
                }
                if ImageProfile::parse(requested)
                    .is_some_and(|p| file.width.max(file.height) > p.edge() as i32)
                {
                    self.request_cache_repair(source);
                }
            }
            if main_available {
                return Ok(Some(file));
            }
        }
        // Only real registered transforms may request a repair; arbitrary profiles cannot.
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_transforms t JOIN media_objects m ON m.key=t.object_key WHERE source_key=$1 AND kind=$2 AND (NOT $3 OR m.public))").bind(source).bind(profile).bind(public).fetch_one(pool).await?;
        if exists {
            self.request_cache_repair(source);
        }
        Ok(None)
    }

    pub(crate) async fn exclusive_lock(
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<(), MediaError> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MEDIA_LOCK)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub(crate) fn relative_path(&self, path: &str) -> Result<PathBuf, MediaError> {
        if path.is_empty()
            || path.len() > 1024
            || path.split('/').any(|p| {
                p.is_empty()
                    || p.contains("..")
                    || p.starts_with('.')
                    || p.chars().any(|c| c.is_control() || c == '\\')
            })
        {
            return Err(MediaError::Invalid);
        }
        Ok(self.root.join(path))
    }
    pub(crate) async fn ensure_directory(&self, path: &str) -> Result<(), MediaError> {
        let path = if path.is_empty() {
            self.root.as_ref().clone()
        } else {
            self.relative_path(path)?
        };
        self.check_parents(&path.join("check.png")).await?;
        tokio::fs::create_dir_all(path).await?;
        Ok(())
    }
    pub async fn registered_path(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
    ) -> Result<PathBuf, MediaError> {
        let path: Option<String> =
            sqlx::query_scalar("SELECT path FROM media_locations WHERE key=$1")
                .bind(key)
                .fetch_optional(pool)
                .await?;
        match path {
            Some(path) => self.relative_path(&path),
            None => Err(MediaError::Missing),
        }
    }
    pub async fn read_registered(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
    ) -> Result<Vec<u8>, MediaError> {
        let path = self.registered_path(pool, key).await?;
        self.read_path(&path).await
    }
    pub(crate) async fn read_path(&self, path: &Path) -> Result<Vec<u8>, MediaError> {
        self.check_parents(path).await?;
        let metadata = tokio::fs::symlink_metadata(path).await?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(MediaError::Storage);
        }
        let mut file = tokio::fs::File::open(path).await?;
        let mut bytes = Vec::new();
        (&mut file)
            .take((STORED_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.is_empty() || bytes.len() > STORED_LIMIT {
            return Err(MediaError::Storage);
        }
        Ok(bytes)
    }
    pub(crate) async fn link_file(&self, old: &str, target: &str) -> Result<(), MediaError> {
        let old = self.relative_path(old)?;
        let target = self.relative_path(target)?;
        self.check_parents(&old).await?;
        self.check_parents(&target).await?;
        let meta = tokio::fs::symlink_metadata(&old).await?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(MediaError::Storage);
        }
        let parent = target.parent().ok_or(MediaError::Storage)?;
        tokio::fs::create_dir_all(parent).await?;
        match tokio::fs::hard_link(&old, &target).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    let existing = tokio::fs::symlink_metadata(&target).await?;
                    if existing.is_file()
                        && !existing.file_type().is_symlink()
                        && existing.dev() == meta.dev()
                        && existing.ino() == meta.ino()
                    {
                        return Ok(());
                    }
                }
                return Err(MediaError::Invalid);
            }
            Err(e) => return Err(e.into()),
        }
        tokio::fs::File::open(parent).await?.sync_all().await?;
        Ok(())
    }
    /// An attachment is selected by immutable object ID and checked again under the publish lock.
    pub async fn selected_key(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        owner: Option<i64>,
    ) -> Result<String, MediaError> {
        sqlx::query_scalar("SELECT m.key FROM media_objects m JOIN media_library l ON l.key=m.key WHERE m.id=$1 AND m.width>0 AND m.height>0 AND l.trashed_at IS NULL AND (m.public OR l.owner_id=$2) AND l.key NOT IN(SELECT object_key FROM media_variants) FOR SHARE OF l,m")
         .bind(id).bind(owner).fetch_optional(&mut **tx).await?.ok_or(MediaError::Invalid)
    }
}

#[derive(sqlx::FromRow)]
pub struct ImageFile {
    pub id: i64,
    pub key: String,
    pub width: i32,
    pub height: i32,
    pub content_type: String,
    pub byte_size: i64,
    pub path: String,
}
impl ImageFile {
    pub fn url(&self) -> String {
        format!(
            "/media/public/{}/{}/original",
            self.id,
            self.key.replace(' ', "%20")
        )
    }

    pub fn metadata(self) -> ImageMetadata {
        ImageMetadata {
            id: self.id,
            key: self.key,
            width: self.width,
            height: self.height,
            content_type: self.content_type,
        }
    }
}
impl MediaStore {
    pub async fn public_file(
        &self,
        pool: &sqlx::PgPool,
        id: i64,
        key: &str,
        variant: ImageVariant,
    ) -> Result<Option<ImageFile>, MediaError> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM media_objects WHERE id=$1 AND key=$2 AND public)",
        )
        .bind(id)
        .bind(key)
        .fetch_one(pool)
        .await?;
        if !allowed {
            return Ok(None);
        }
        let file:Option<ImageFile>=sqlx::query_as("SELECT m.id,m.key,m.width,m.height,m.content_type,m.byte_size,l.path FROM media_objects m JOIN media_locations l ON l.key=m.key WHERE m.public AND m.width>0 AND m.height>0 AND m.key=CASE WHEN $2='original' THEN $1 ELSE (SELECT object_key FROM media_variants WHERE source_key=$1 AND profile=$2) END").bind(key).bind(variant.key()).fetch_optional(pool).await?;
        if let Some(file) = file
            && self.file_available(&file).await?
        {
            return Ok(Some(file));
        }
        Ok(None)
    }
    /// The owner must authorize the source before resolving its best available preview.
    pub async fn preview_file(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
    ) -> Result<Option<ImageFile>, MediaError> {
        self.available_rendition_file(pool, key, "card", false)
            .await
    }
}

/// Responsive candidates report actual widths, including portrait sources and no-upscale caps.
/// Dimensions use the same nearest rounding as image 0.25's bounded thumbnail operation.
pub fn responsive_srcset(id: Option<i64>, width: Option<i32>, height: Option<i32>) -> String {
    let (Some(id), Some(width), Some(height)) = (id, width, height) else {
        return String::new();
    };
    if id <= 0 || width <= 0 || height <= 0 {
        return String::new();
    }
    let major = i64::from(width.max(height));
    let mut previous = 0;
    let mut candidates = Vec::new();
    for profile in [
        ImageProfile::Card,
        ImageProfile::Detail,
        ImageProfile::Large,
    ] {
        let edge = i64::from(profile.edge());
        let pixels = if major <= edge {
            i64::from(width)
        } else {
            ((i64::from(width) * edge + major / 2) / major).max(1)
        };
        if pixels != previous {
            candidates.push(format!("/media/image/{id}/{} {pixels}w", profile.key()));
            previous = pixels;
        }
    }
    candidates.join(", ")
}
#[cfg(test)]
mod responsive_tests {
    use super::*;
    #[test]
    fn widths_preserve_portraits_caps_and_missing_metadata() {
        assert_eq!(
            responsive_srcset(Some(1), Some(1500), Some(3000)),
            "/media/image/1/card 256w, /media/image/1/detail 512w, /media/image/1/large 1024w"
        );
        assert_eq!(
            responsive_srcset(Some(1), Some(32), Some(32)),
            "/media/image/1/card 32w"
        );
        assert!(responsive_srcset(Some(1), Some(0), Some(3)).is_empty());
    }
}
