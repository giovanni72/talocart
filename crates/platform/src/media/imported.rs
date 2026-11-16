//! FTP product files retain their bytes, filename and relative directories.
use super::*;
use image::ImageFormat;
use std::io::Cursor;

pub struct PreparedImport {
    pub(super) raw: Vec<u8>,
    pub(super) content_type: &'static str,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl MediaStore {
    /// Reuse a validated original at the exact import destination, without a remote read.
    /// Caller holds publish_lock through commit and has authorized the importing job.
    /// The local original is authoritative; numbered collisions require the FTP byte comparison.
    /// Never adopt private, trashed, retired, derived or aliased files, or follow symlinks.
    pub async fn reuse_local_import(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
    ) -> Result<Option<StoredMedia>, MediaError> {
        if !Self::valid_public_import_key(key) {
            return Err(MediaError::Invalid);
        }
        Self::lock_import_directory(tx, key).await?;
        let (reserved, retired, alias) = Self::import_path_reservations(tx, key).await?;
        if retired || alias {
            return Ok(None);
        }
        let registered: Option<(String, bool, String, i64, i32, i32)> = sqlx::query_as(
            "SELECT m.key,m.public AND l.owner_id IS NULL AND l.trashed_at IS NULL AND NOT EXISTS(SELECT 1 FROM media_variants WHERE object_key=m.key) AND NOT EXISTS(SELECT 1 FROM media_renditions WHERE object_key=m.key AND source_key<>m.key) AND NOT EXISTS(SELECT 1 FROM media_transforms WHERE object_key=m.key AND source_key<>m.key),m.content_type,m.byte_size,m.width,m.height FROM media_locations p JOIN media_objects m ON m.key=p.key LEFT JOIN media_library l ON l.key=m.key WHERE p.path=$1",
        ).bind(key).fetch_optional(&mut **tx).await?;
        if registered.as_ref().is_some_and(|row| !row.1) || (registered.is_none() && reserved) {
            return Ok(None);
        }
        let path = self.object_path(key)?;
        self.check_parents(&path).await?;
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(MediaError::Storage);
        }
        if metadata.len() == 0 || metadata.len() > 10 * 1024 * 1024 {
            return Ok(None);
        }
        let raw = self.read_path(&path).await?;
        let prepared = match self.prepare_import(raw).await {
            Ok(prepared) => prepared,
            Err(MediaError::Invalid) => return Ok(None),
            Err(error) => return Err(error),
        };
        if !Self::import_extension_matches(&prepared, key) {
            return Ok(None);
        }
        let existing_key = match registered {
            Some((existing_key, _, mime, bytes, width, height)) => {
                if mime != prepared.content_type
                    || bytes != prepared.raw.len() as i64
                    || width != prepared.width as i32
                    || height != prepared.height as i32
                {
                    return Ok(None);
                }
                Some(existing_key)
            }
            None => None,
        };
        let stored = StoredMedia {
            key: existing_key.clone().unwrap_or_else(|| key.to_owned()),
            bytes: prepared.raw.len() as i64,
            width: prepared.width as i32,
            height: prepared.height as i32,
            content_type: prepared.content_type,
            variants: Vec::new(),
        };
        if existing_key.is_none() {
            stored.register(tx, true).await?;
        }
        Ok(Some(stored))
    }

    fn valid_public_import_key(key: &str) -> bool {
        Self::valid_import_key(key)
            && !key.split('/').next().is_some_and(|root| {
                ["private", "staging", "cache", "library", "brand", "objects"]
                    .iter()
                    .any(|reserved| root.eq_ignore_ascii_case(reserved))
            })
    }

    fn import_extension_matches(prepared: &PreparedImport, key: &str) -> bool {
        key.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                (
                    prepared.content_type,
                    extension.to_ascii_lowercase().as_str()
                ),
                ("image/png", "png") | ("image/jpeg", "jpg" | "jpeg") | ("image/webp", "webp")
            )
        })
    }

    async fn lock_import_directory(
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
    ) -> Result<(), MediaError> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1280262988))")
            .bind(
                key.rsplit_once('/')
                    .ok_or(MediaError::Invalid)?
                    .0
                    .to_ascii_lowercase(),
            )
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn import_path_reservations(
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
    ) -> Result<(bool, bool, bool), MediaError> {
        Ok(sqlx::query_as("SELECT EXISTS(SELECT 1 FROM media_objects WHERE key=$1), EXISTS(SELECT 1 FROM media_retired_files WHERE lower(replace(replace(normalize(path, NFKC), 'ß', 'ss'), 'ẞ', 'ss') COLLATE \"C\")=lower(($1::text) COLLATE \"C\")), EXISTS(SELECT 1 FROM media_locations WHERE lower(replace(replace(normalize(path, NFKC), 'ß', 'ss'), 'ẞ', 'ss') COLLATE \"C\")=lower(($1::text) COLLATE \"C\") AND path COLLATE \"C\"<>($1::text) COLLATE \"C\")")
            .bind(key).fetch_one(&mut **tx).await?)
    }

    /// Publish and register an import, reusing byte-identical originals at its destination.
    /// Caller holds publish_lock through commit, before taking any identity/catalog locks.
    /// A per-directory transaction lock serializes importers; maintenance/moves take the
    /// exclusive media lock. Physical locations may differ from immutable object keys.
    pub async fn put_imported_reusing(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        prepared: PreparedImport,
        key: &str,
    ) -> Result<StoredMedia, MediaError> {
        if !Self::valid_public_import_key(key) || !Self::import_extension_matches(&prepared, key) {
            return Err(MediaError::Invalid);
        }
        Self::lock_import_directory(tx, key).await?;
        let (stem, extension) = key.rsplit_once('.').ok_or(MediaError::Invalid)?;
        for number in 1..=10000 {
            let candidate = if number == 1 {
                key.to_owned()
            } else {
                format!("{stem}-{number}.{extension}")
            };
            let path = self.object_path(&candidate)?;
            self.check_parents(&path).await?;
            match tokio::fs::symlink_metadata(&path).await {
                Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                    return Err(MediaError::Storage);
                }
                Ok(meta) if meta.len() != prepared.raw.len() as u64 => continue,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let registered: Option<(String, bool)> = sqlx::query_as(
                "SELECT m.key,m.public AND m.content_type=$2 AND m.byte_size=$3 AND m.width=$4 AND m.height=$5 AND l.owner_id IS NULL AND l.trashed_at IS NULL AND NOT EXISTS(SELECT 1 FROM media_variants WHERE object_key=m.key) AND NOT EXISTS(SELECT 1 FROM media_renditions WHERE object_key=m.key AND source_key<>m.key) AND NOT EXISTS(SELECT 1 FROM media_transforms WHERE object_key=m.key AND source_key<>m.key) FROM media_locations p JOIN media_objects m ON m.key=p.key LEFT JOIN media_library l ON l.key=m.key WHERE p.path=$1",
            )
            .bind(&candidate)
            .bind(prepared.content_type)
            .bind(prepared.raw.len() as i64)
            .bind(prepared.width as i32)
            .bind(prepared.height as i32)
            .fetch_optional(&mut **tx)
            .await?;
            let (reserved, retired, alias) = Self::import_path_reservations(tx, &candidate).await?;
            if retired || alias {
                continue;
            }
            match self.read_path(&path).await {
                Ok(raw) if raw == prepared.raw => {
                    let existing_key = match registered {
                        Some((existing_key, true)) => Some(existing_key),
                        Some((_, false)) => continue,
                        None if reserved => continue,
                        None => None,
                    };
                    let stored = StoredMedia {
                        key: existing_key.clone().unwrap_or(candidate),
                        bytes: prepared.raw.len() as i64,
                        width: prepared.width as i32,
                        height: prepared.height as i32,
                        content_type: prepared.content_type,
                        variants: Vec::new(),
                    };
                    if existing_key.is_none() {
                        stored.register(tx, true).await?;
                    }
                    return Ok(stored);
                }
                Ok(_) => continue,
                Err(MediaError::Missing) if registered.is_none() && !reserved => {
                    let stored = self.put_imported(prepared, &candidate).await?;
                    stored.register(tx, true).await?;
                    return Ok(stored);
                }
                Err(MediaError::Missing) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(MediaError::Storage)
    }

    /// Imported keys reserve the first segment for a safe local brand name.
    pub fn valid_import_key(key: &str) -> bool {
        let parts: Vec<_> = key.split('/').collect();
        parts.len() >= 2
            && key.len() <= 1024
            && !matches!(parts[0], "brand" | "objects")
            && parts[0].len() >= 2
            && parts[0]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            && parts.iter().all(|p| {
                !p.is_empty()
                    && !p.contains("..")
                    && p.as_bytes()[0].is_ascii_alphanumeric()
                    && p.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._- ".contains(&c))
            })
            && key.rsplit_once('.').is_some_and(|(_, e)| {
                matches!(
                    e,
                    "png" | "jpg" | "jpeg" | "webp" | "PNG" | "JPG" | "JPEG" | "WEBP"
                )
            })
    }
    /// Reject symlink parents even for reads and maintenance. The configured root is trusted.
    pub(crate) async fn check_parents(&self, path: &Path) -> Result<(), MediaError> {
        let relative = path
            .strip_prefix(self.root.as_path())
            .map_err(|_| MediaError::Storage)?;
        let mut parent = self.root.as_ref().clone();
        for part in relative
            .components()
            .take(relative.components().count().saturating_sub(1))
        {
            parent.push(part);
            match tokio::fs::symlink_metadata(&parent).await {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(MediaError::Storage),
            }
        }
        Ok(())
    }
    pub async fn prepare_import(&self, raw: Vec<u8>) -> Result<PreparedImport, MediaError> {
        self.prepare_source(raw).await
    }
    /// Validate a library source without creating legacy PNG copies.
    pub async fn prepare_source(&self, raw: Vec<u8>) -> Result<PreparedImport, MediaError> {
        let permit = self
            .imaging
            .clone()
            .try_acquire_owned()
            .map_err(|_| MediaError::Busy)?;
        Self::decode_source(raw, permit).await
    }
    /// Background discovery queues fairly behind existing image work, with a bounded wait.
    pub(super) async fn prepare_resident_source(
        &self,
        raw: Vec<u8>,
    ) -> Result<PreparedImport, MediaError> {
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.imaging.clone().acquire_owned(),
        )
        .await
        .map_err(|_| MediaError::Busy)?
        .map_err(|_| MediaError::Storage)?;
        Self::decode_source(raw, permit).await
    }
    async fn decode_source(
        raw: Vec<u8>,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<PreparedImport, MediaError> {
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let decoded = decode(&raw, 10 * 1024 * 1024, 8192, 128 * 1024 * 1024)
                .map_err(|_| MediaError::Invalid)?;
            let content_type = match image::guess_format(&raw).map_err(|_| MediaError::Invalid)? {
                ImageFormat::Png => {
                    if image::codecs::png::PngDecoder::new(Cursor::new(&raw))
                        .map_err(|_| MediaError::Invalid)?
                        .is_apng()
                        .map_err(|_| MediaError::Invalid)?
                    {
                        return Err(MediaError::Invalid);
                    }
                    "image/png"
                }
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::WebP => "image/webp",
                _ => return Err(MediaError::Invalid),
            };
            Ok(PreparedImport {
                raw,
                content_type,
                width: decoded.width(),
                height: decoded.height(),
            })
        })
        .await
        .map_err(|_| MediaError::Storage)?
    }
    /// Caller holds publish_lock. Immutable collision suffixes never overwrite local media.
    pub async fn put_imported(
        &self,
        prepared: PreparedImport,
        key: &str,
    ) -> Result<StoredMedia, MediaError> {
        if !Self::valid_import_key(key) {
            return Err(MediaError::Invalid);
        }
        let extension = key
            .rsplit_once('.')
            .ok_or(MediaError::Invalid)?
            .1
            .to_ascii_lowercase();
        if !matches!(
            (prepared.content_type, extension.as_str()),
            ("image/png", "png") | ("image/jpeg", "jpg" | "jpeg") | ("image/webp", "webp")
        ) {
            return Err(MediaError::Invalid);
        }
        let key = self.publish_bytes(key, &prepared.raw, true).await?;
        let stored = StoredMedia {
            key,
            bytes: prepared.raw.len() as i64,
            width: prepared.width as i32,
            height: prepared.height as i32,
            content_type: prepared.content_type,
            variants: Vec::new(),
        };
        Ok(stored)
    }
}

impl MediaStore {
    pub(super) async fn register_resident(
        &self,
        pool: &sqlx::PgPool,
        prepared: PreparedImport,
        key: &str,
    ) -> Result<(), MediaError> {
        let extension = key
            .rsplit_once('.')
            .ok_or(MediaError::Invalid)?
            .1
            .to_ascii_lowercase();
        if !matches!(
            (prepared.content_type, extension.as_str()),
            ("image/png", "png") | ("image/jpeg", "jpg" | "jpeg") | ("image/webp", "webp")
        ) {
            return Err(MediaError::Invalid);
        }
        let mut tx = pool.begin().await?;
        Self::exclusive_lock(&mut tx).await?;
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM media_locations WHERE path=$1)",
        )
        .bind(key)
        .fetch_one(&mut *tx)
        .await?
        {
            return Ok(());
        }
        let path = self.relative_path(key)?;
        self.check_parents(&path).await?;
        if self.read_path(&path).await? != prepared.raw {
            return Err(MediaError::Storage);
        }
        let stored = StoredMedia {
            key: key.into(),
            bytes: prepared.raw.len() as i64,
            width: prepared.width as i32,
            height: prepared.height as i32,
            variants: Vec::new(),
            content_type: prepared.content_type,
        };
        stored.register(&mut tx, true).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_compatibility_accepts_spaces_in_original_ftp_names() {
        assert!(MediaStore::valid_import_key(
            "babybugz/catalog/babybugz/varianti/mabz12_dusty blue.jpg"
        ));
        assert!(MediaStore::valid_import_key(
            "babybugz/catalog/photo set/mabz12_white.jpg"
        ));
        for path in [
            "babybugz/catalog/../photo.jpg",
            "babybugz/catalog/photo\n.jpg",
            "babybugz/catalog/%2e%2e/photo.jpg",
        ] {
            assert!(!MediaStore::valid_import_key(path));
        }
    }
    #[tokio::test]
    async fn imported_originals_preserve_bytes_directories_and_never_overwrite() {
        let root =
            std::env::temp_dir().join(format!("talocart_ftp_products_{}", std::process::id()));
        let store = MediaStore::new(root.clone());
        let raw = include_bytes!("../../../web/tests/fixtures/webp-lossless.webp").to_vec();
        let key = "fruit-of-the-loom/catalog/fruit_of_the_loom/varianti/white.webp";
        let first = store
            .put_imported(store.prepare_import(raw.clone()).await.unwrap(), key)
            .await
            .unwrap();
        let second = store
            .put_imported(store.prepare_import(raw.clone()).await.unwrap(), key)
            .await
            .unwrap();
        assert_eq!(first.key, key);
        assert_ne!(first.key, second.key);
        assert_eq!(store.read(key).await.unwrap(), raw);
        for (extension, format, mime) in [
            ("jpg", ImageFormat::Jpeg, "image/jpeg"),
            ("png", ImageFormat::Png, "image/png"),
        ] {
            let mut encoded = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(16, 8)
                .write_to(&mut encoded, format)
                .unwrap();
            let encoded = encoded.into_inner();
            let key = format!("fruit-of-the-loom/catalog/base/photo.{extension}");
            let stored = store
                .put_imported(store.prepare_import(encoded.clone()).await.unwrap(), &key)
                .await
                .unwrap();
            assert_eq!(stored.content_type, mime);
            assert_eq!(store.read(&key).await.unwrap(), encoded);
        }
        assert_eq!(first.content_type, "image/webp");
        assert!(root.join(key).is_file());
        assert!(
            first.variants.is_empty(),
            "Import queues rendition work after registration"
        );
        for key in [
            "brand/catalog/a.jpg",
            "objects/catalog/a.jpg",
            "fruit/../a.jpg",
            "fruit/catalog//a.jpg",
            "fruit/%2e%2e/a.jpg",
            "fruit/catalog/a.svg",
            "/fruit/a.jpg",
            "fruit/catalog/a.jpg\n",
            "fruit/catalog/a..jpg",
            "fruit/catalog/é.jpg",
        ] {
            assert!(!MediaStore::valid_import_key(key), "{key}");
        }
        assert!(
            store
                .put_imported(
                    store.prepare_import(raw).await.unwrap(),
                    "fruit/catalog/white.jpg"
                )
                .await
                .is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("objects"), root.join("fruit-of-the-loom/escape"))
                .unwrap();
            assert!(
                store
                    .read("fruit-of-the-loom/escape/image.png")
                    .await
                    .is_err()
            );
            assert!(
                store
                    .put_imported(
                        store
                            .prepare_import(
                                include_bytes!("../../../web/tests/fixtures/logo.png").to_vec()
                            )
                            .await
                            .unwrap(),
                        "fruit-of-the-loom/escape/image.png"
                    )
                    .await
                    .is_err()
            );
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}

#[cfg(test)]
mod sizing_regression {
    use super::*;
    #[tokio::test]
    async fn imported_small_sources_keep_original_dimensions_without_eager_copies() {
        let root =
            std::env::temp_dir().join(format!("talocart_import_size_{}", std::process::id()));
        let store = MediaStore::new(root.clone());
        let raw = include_bytes!("../../../web/tests/fixtures/webp-lossless.webp").to_vec();
        let stored = store
            .put_imported(
                store.prepare_import(raw).await.unwrap(),
                "sizing/source.webp",
            )
            .await
            .unwrap();
        assert_eq!((stored.width, stored.height), (512, 128));
        assert!(stored.variants.is_empty());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
