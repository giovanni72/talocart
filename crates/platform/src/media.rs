//! Shared immutable images. Owners authorize access; HTTP never generates files.
mod acquisition;
mod attachments;
mod cache;
mod files;
pub use acquisition::AcquisitionReport;
pub use cache::{CacheAction, CacheReport};
pub use files::DiscoveryReport;
mod images;
mod jobs;
mod profiles;
pub use attachments::AttachmentProfile;
pub use profiles::ImageProfile;
mod imported;
pub mod library;
mod locations;
pub use locations::{ImageFile, responsive_srcset};
mod maintenance;
mod publication;
mod repair;
mod variants;
pub(crate) use images::decode;
pub use imported::PreparedImport;
pub use maintenance::PruneReport;
use sqlx::{Postgres, Transaction};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};
pub use variants::{ImageVariant, PreparedImage};

const STORED_LIMIT: usize = 32 * 1024 * 1024;
// Shared by publishers and exclusive for maintenance, across processes using this database.
const MEDIA_LOCK: i64 = 0x4b4f414c414d4544;
#[derive(Debug, Error)]
pub enum MediaError {
    #[error("Choose a valid static PNG, JPEG or WebP image, up to 10 MiB and 8192 × 8192 pixels")]
    Invalid,
    #[error("Image unavailable")]
    Missing,
    #[error("Media storage temporarily unavailable")]
    Storage,
    #[error("Image processing is busy")]
    Busy,
}
impl From<std::io::Error> for MediaError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            Self::Missing
        } else {
            Self::Storage
        }
    }
}
impl From<sqlx::Error> for MediaError {
    fn from(error: sqlx::Error) -> Self {
        let _ = crate::technical::DatabaseFailure::capture(error, module_path!());
        Self::Storage
    }
}
#[derive(Clone)]
pub struct MediaStore {
    root: Arc<PathBuf>,
    imaging: Arc<Semaphore>,
    // Bounded hints only: HTTP never writes jobs; the shared background worker persists repairs.
    cache_repairs: Arc<Mutex<std::collections::HashSet<String>>>,
}
pub struct StoredMedia {
    pub key: String,
    pub bytes: i64,
    pub width: i32,
    pub height: i32,
    variants: Vec<(ImageVariant, StoredMedia)>,
    content_type: &'static str,
}
#[derive(sqlx::FromRow)]
pub struct ImageMetadata {
    pub id: i64,
    pub key: String,
    pub width: i32,
    pub height: i32,
    pub content_type: String,
}
pub struct MediaImage {
    pub metadata: ImageMetadata,
    pub png: Vec<u8>,
}
impl StoredMedia {
    async fn register_object(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        public: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO media_objects(key,content_type,byte_size,width,height,public,variants_ready) VALUES ($1,$6,$2,$3,$4,$5,true)")
            .bind(&self.key).bind(self.bytes).bind(self.width).bind(self.height).bind(public).bind(self.content_type).execute(&mut **tx).await?;
        Ok(())
    }
    /// Publisher must hold publish_lock until this transaction commits or rolls back.
    pub async fn register(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        public: bool,
    ) -> Result<(), sqlx::Error> {
        self.register_internal(tx, public).await?;
        if public {
            sqlx::query("SELECT media_adopt_source($1)")
                .bind(&self.key)
                .execute(&mut **tx)
                .await?;
            jobs::enqueue(tx, Some(&self.key), true, jobs::Enqueue::New).await?;
        }
        Ok(())
    }
    pub async fn register_internal(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        public: bool,
    ) -> Result<(), sqlx::Error> {
        self.register_object(tx, public).await?;
        self.register_variants(tx, public).await
    }
    async fn register_variants(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        public: bool,
    ) -> Result<(), sqlx::Error> {
        for (profile, image) in &self.variants {
            image.register_object(tx, public).await?;
            sqlx::query(
                "INSERT INTO media_variants(source_key,profile,object_key) VALUES ($1,$2,$3)",
            )
            .bind(&self.key)
            .bind(profile.key())
            .bind(&image.key)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }
}
impl MediaStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            imaging: Arc::new(Semaphore::new(2)),
            cache_repairs: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }
    pub(crate) fn image_slots(&self) -> Arc<Semaphore> {
        self.imaging.clone()
    }
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }
    pub async fn publish_lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), MediaError> {
        sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
            .bind(MEDIA_LOCK)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    fn valid_key(key: &str) -> bool {
        if Self::valid_import_key(key) {
            return true;
        }
        let name = key.strip_prefix("brand/").unwrap_or(key);
        (5..=124).contains(&key.len())
            && !name.is_empty()
            && name.ends_with(".png")
            && !key.contains("..")
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    }
    fn object_path(&self, key: &str) -> Result<PathBuf, MediaError> {
        if !Self::valid_key(key) {
            return Err(MediaError::Storage);
        }
        if Self::valid_import_key(key) {
            return Ok(self.root.join(key));
        }
        Ok(match key.strip_prefix("brand/") {
            Some(name) => self.root.join("brand").join(name),
            None => self.root.join("objects").join(key),
        })
    }
    /// Preserve a readable basename. Strip client paths and normalize the PNG extension.
    pub fn filename(input: &str) -> String {
        let base = input.rsplit(['/', '\\']).next().unwrap_or("image");
        let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
        let mut stem: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .take(96)
            .collect();
        stem = stem.trim_matches(['-', '_']).to_owned();
        if stem.is_empty() {
            stem = "image".into();
        }
        if matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            stem.insert_str(0, "image-");
        }
        format!("{stem}.png")
    }
    /// Low-level immutable publication; application callers use publish_lock and put_prepared.
    pub async fn put_png(
        &self,
        png: &[u8],
        width: u32,
        height: u32,
    ) -> Result<StoredMedia, MediaError> {
        self.put_named(png, width, height, "image.png").await
    }
    async fn put_named(
        &self,
        png: &[u8],
        width: u32,
        height: u32,
        name: &str,
    ) -> Result<StoredMedia, MediaError> {
        self.put_named_in(png, width, height, name, false).await
    }
    async fn put_named_in(
        &self,
        png: &[u8],
        width: u32,
        height: u32,
        name: &str,
        brand: bool,
    ) -> Result<StoredMedia, MediaError> {
        if png.is_empty() || png.len() > STORED_LIMIT || width == 0 || height == 0 {
            return Err(MediaError::Storage);
        }
        let width = i32::try_from(width).map_err(|_| MediaError::Storage)?;
        let height = i32::try_from(height).map_err(|_| MediaError::Storage)?;
        let name = Self::filename(name);
        let key = if brand { format!("brand/{name}") } else { name };
        let key = self.publish_bytes(&key, png, true).await?;
        Ok(StoredMedia {
            key,
            bytes: png.len() as i64,
            width,
            height,
            variants: Vec::new(),
            content_type: "image/png",
        })
    }
    pub async fn read(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        let path = self.object_path(key)?;
        self.check_parents(&path).await?;
        if tokio::fs::symlink_metadata(&path)
            .await?
            .file_type()
            .is_symlink()
        {
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
    pub async fn metadata<'e, E>(
        &self,
        executor: E,
        key: &str,
    ) -> Result<Option<ImageMetadata>, MediaError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Ok(sqlx::query_as(
            "SELECT id,key,width,height,content_type FROM media_objects WHERE key=$1",
        )
        .bind(key)
        .fetch_optional(executor)
        .await?)
    }
    pub async fn image(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
        variant: ImageVariant,
    ) -> Result<Option<MediaImage>, MediaError> {
        let Some(file) = self
            .available_rendition_file(pool, key, variant.key(), false)
            .await?
        else {
            return Ok(None);
        };
        match self.read_registered(pool, &file.key).await {
            Ok(png) => Ok(Some(MediaImage {
                metadata: file.metadata(),
                png,
            })),
            Err(MediaError::Missing) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub async fn public_image(
        &self,
        pool: &sqlx::PgPool,
        id: i64,
        key: &str,
        variant: ImageVariant,
    ) -> Result<Option<MediaImage>, MediaError> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM media_objects WHERE id=$1 AND key=$2 AND public)",
        )
        .bind(id)
        .bind(key)
        .fetch_one(pool)
        .await?;
        if allowed {
            self.image(pool, key, variant).await
        } else {
            Ok(None)
        }
    }
    /// Strict read-only audit; serve does not block startup on a missing image.
    pub async fn check(&self, pool: &sqlx::PgPool) -> Result<(), MediaError> {
        let mut after = String::new();
        loop {
            let rows:Vec<(String,i64)>=sqlx::query_as("SELECT key,byte_size FROM media_objects WHERE key>$1 AND key IN (SELECT key FROM media_live_keys) ORDER BY key LIMIT 100")
                .bind(&after).fetch_all(pool).await?;
            if rows.is_empty() {
                break;
            }
            for (key, size) in rows {
                let transformed: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM media_transforms WHERE object_key=$1)",
                )
                .bind(&key)
                .fetch_one(pool)
                .await?;
                if transformed {
                    if self
                        .available_rendition_file(pool, &key, "original", false)
                        .await?
                        .is_none()
                    {
                        return Err(MediaError::Missing);
                    }
                } else {
                    let file = self.registered_path(pool, &key).await?;
                    self.check_parents(&file).await?;
                    let metadata = tokio::fs::symlink_metadata(file).await?;
                    if !metadata.is_file()
                        || metadata.file_type().is_symlink()
                        || metadata.len() != size as u64
                    {
                        return Err(MediaError::Storage);
                    }
                }
                after = key;
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn readable_names_are_safe_and_concurrent_collisions_never_overwrite() {
        assert_eq!(MediaStore::filename("../../My logo.jpg"), "My-logo.png");
        assert_eq!(MediaStore::filename(r"C:\fakepath\logo.jpeg"), "logo.png");
        assert_eq!(MediaStore::filename(".."), "image.png");
        assert_eq!(MediaStore::filename("CON.png"), "image-CON.png");
        let root = std::env::temp_dir().join(format!("talocart_names_{}", std::process::id()));
        let store = MediaStore::new(root.clone());
        let (a, b) = tokio::join!(
            store.put_named(b"first", 1, 1, "logo.jpg"),
            store.put_named(b"second", 1, 1, "logo.jpg")
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_ne!(a.key, b.key);
        assert!(["logo.png", "logo-2.png"].contains(&a.key.as_str()));
        assert!(["logo.png", "logo-2.png"].contains(&b.key.as_str()));
        assert_eq!(store.read(&a.key).await.unwrap(), b"first");
        assert_eq!(store.read(&b.key).await.unwrap(), b"second");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[tokio::test]
    async fn files_are_immutable_persistent_and_paths_are_restricted() {
        let root = std::env::temp_dir().join(format!("talocart_media_{}", std::process::id()));
        let store = MediaStore::new(root.clone());
        let first = store
            .put_png(b"synthetic PNG payload", 16, 8)
            .await
            .unwrap();
        let second = store.put_png(b"replacement payload", 16, 8).await.unwrap();
        assert_ne!(first.key, second.key);
        assert_eq!(
            MediaStore::new(root.clone())
                .read(&first.key)
                .await
                .unwrap(),
            b"synthetic PNG payload"
        );
        for key in [
            "../../talocart.toml",
            "/etc/passwd",
            "x.png",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaé.png",
        ] {
            assert!(store.read(key).await.is_err());
        }
        assert!(store.put_png(b"", 16, 8).await.is_err());
        assert!(
            store
                .read(&format!("{}.png", "f".repeat(64)))
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
