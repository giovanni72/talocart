use super::*;
use image::ImageFormat;
use std::io::Cursor;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageVariant {
    #[default]
    Original,
    Thumbnail,
    Card,
    Detail,
}
impl ImageVariant {
    pub fn key(self) -> &'static str {
        ImageProfile::from(self).key()
    }
}
pub(super) struct Encoded {
    png: Vec<u8>,
    width: u32,
    height: u32,
}
pub struct PreparedImage {
    original: Encoded,
    pub(super) variants: Vec<(ImageVariant, Encoded)>,
}
impl PreparedImage {
    pub(super) fn source(png: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            original: Encoded { png, width, height },
            variants: Vec::new(),
        }
    }
}
pub(super) fn prepared_from_decoded(
    png: Vec<u8>,
    decoded: image::DynamicImage,
) -> Result<PreparedImage, MediaError> {
    let mut variants = Vec::new();
    for profile in [
        ImageVariant::Thumbnail,
        ImageVariant::Card,
        ImageVariant::Detail,
    ] {
        let spec = ImageProfile::from(profile);
        let edge = spec.edge();
        if decoded.width().max(decoded.height()) <= edge {
            continue;
        }
        let resized = spec.render(&decoded);
        let mut output = Cursor::new(Vec::new());
        resized
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|_| MediaError::Storage)?;
        variants.push((
            profile,
            Encoded {
                png: output.into_inner(),
                width: resized.width(),
                height: resized.height(),
            },
        ));
    }
    Ok(PreparedImage {
        original: Encoded {
            png,
            width: decoded.width(),
            height: decoded.height(),
        },
        variants,
    })
}
impl MediaStore {
    pub async fn prepare_png(&self, png: Vec<u8>) -> Result<PreparedImage, MediaError> {
        let permit = self
            .imaging
            .clone()
            .try_acquire_owned()
            .map_err(|_| MediaError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let decoded = decode(&png, STORED_LIMIT, 8192, 128 * 1024 * 1024)
                .map_err(|_| MediaError::Storage)?;
            prepared_from_decoded(png, decoded)
        })
        .await
        .map_err(|_| MediaError::Storage)?
    }
    pub async fn put_prepared(
        &self,
        prepared: PreparedImage,
        name: &str,
    ) -> Result<StoredMedia, MediaError> {
        self.put_prepared_in(prepared, name, false).await
    }
    /// Publish a catalog brand logo and all derived images in the reserved brand directory.
    pub async fn put_brand_prepared(
        &self,
        prepared: PreparedImage,
        name: &str,
    ) -> Result<StoredMedia, MediaError> {
        self.put_prepared_in(prepared, name, true).await
    }
    async fn put_prepared_in(
        &self,
        prepared: PreparedImage,
        name: &str,
        brand: bool,
    ) -> Result<StoredMedia, MediaError> {
        let mut stored = self
            .put_named_in(
                &prepared.original.png,
                prepared.original.width,
                prepared.original.height,
                name,
                brand,
            )
            .await?;
        self.put_variants(&mut stored, prepared.variants).await?;
        Ok(stored)
    }
    pub(super) async fn put_variants(
        &self,
        stored: &mut StoredMedia,
        variants: Vec<(ImageVariant, Encoded)>,
    ) -> Result<(), MediaError> {
        for (profile, image) in variants {
            let name = if stored.key.starts_with("cache/") {
                let revision = stored.key.split('/').nth(2).ok_or(MediaError::Storage)?;
                let basename = stored
                    .key
                    .rsplit('/')
                    .next()
                    .ok_or(MediaError::Storage)?
                    .trim_end_matches(".png");
                format!("{revision}-{basename}-{}.png", profile.key())
            } else {
                format!(
                    "{}-{}.png",
                    stored.key.trim_end_matches(".png"),
                    profile.key()
                )
            };
            stored.variants.push((
                profile,
                self.put_named_in(
                    &image.png,
                    image.width,
                    image.height,
                    &name,
                    stored.key.starts_with("brand/"),
                )
                .await?,
            ));
        }
        Ok(())
    }
    /// Explicit backfill, never performed by GET or server startup.
    pub async fn rebuild_variants(&self, pool: &sqlx::PgPool) -> Result<u64, MediaError> {
        let mut count = 0;
        let mut after = String::new();
        loop {
            let rows:Vec<(String,bool)>=sqlx::query_as("SELECT key,public FROM media_objects WHERE NOT variants_ready AND key>$1 AND key IN (SELECT key FROM media_roots) ORDER BY key LIMIT 100")
                .bind(&after).fetch_all(pool).await?;
            if rows.is_empty() {
                break;
            }
            for (key, public) in rows {
                after = key.clone();
                let png = match self.read_registered(pool, &key).await {
                    Ok(png) => png,
                    Err(MediaError::Missing) => continue,
                    Err(e) => return Err(e),
                };
                let prepared = self.prepare_png(png).await?;
                let mut tx = pool.begin().await?;
                Self::publish_lock(&mut tx).await?;
                let eligible:Option<String>=sqlx::query_scalar("SELECT key FROM media_objects WHERE key=$1 AND NOT variants_ready AND key IN (SELECT key FROM media_roots) FOR UPDATE")
                    .bind(&key).fetch_optional(&mut *tx).await?;
                if eligible.is_none() {
                    continue;
                }
                let mut stored = StoredMedia {
                    key,
                    bytes: prepared.original.png.len() as i64,
                    width: prepared.original.width as i32,
                    height: prepared.original.height as i32,
                    variants: Vec::new(),
                    content_type: "image/png",
                };
                self.put_variants(&mut stored, prepared.variants).await?;
                stored.register_variants(&mut tx, public).await?;
                sqlx::query("UPDATE media_objects SET variants_ready=true WHERE key=$1")
                    .bind(&stored.key)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn brand_images_and_variants_use_reserved_directory_with_safe_names() {
        let root =
            std::env::temp_dir().join(format!("talocart_brand_media_{}", std::process::id()));
        let store = MediaStore::new(root.clone());
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            800,
            400,
            image::Rgba([20, 40, 60, 128]),
        ))
        .write_to(&mut png, ImageFormat::Png)
        .unwrap();
        let png = png.into_inner();
        let legacy = store
            .put_prepared(store.prepare_png(png.clone()).await.unwrap(), "logo.png")
            .await
            .unwrap();
        let first = store
            .put_brand_prepared(
                store.prepare_png(png.clone()).await.unwrap(),
                "../../logo.webp",
            )
            .await
            .unwrap();
        let second = store
            .put_brand_prepared(store.prepare_png(png.clone()).await.unwrap(), "logo.png")
            .await
            .unwrap();
        assert_eq!(first.key, "brand/logo.png");
        assert_eq!(second.key, "brand/logo-2.png");
        assert_eq!(store.read(&legacy.key).await.unwrap(), png);
        assert!(root.join("objects/logo.png").is_file());
        for stored in [&first, &second] {
            assert_eq!(stored.variants.len(), 2);
            for key in
                std::iter::once(&stored.key).chain(stored.variants.iter().map(|(_, v)| &v.key))
            {
                assert!(key.starts_with("brand/"));
                assert!(root.join(key).is_file());
                assert!(store.read(key).await.unwrap().starts_with(b"\x89PNG"));
            }
        }
        for key in [
            "brand/../logo.png",
            "brand/nested/logo.png",
            "objects/logo.png",
            "brand//logo.png",
            "/brand/logo.png",
            "brand/.logo.png",
            "brand/",
            "brand\\logo.png",
        ] {
            assert!(!MediaStore::valid_key(key), "{key}");
            assert!(store.read(key).await.is_err());
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[tokio::test]
    async fn variants_preserve_aspect_transparency_and_never_upscale() {
        let store = MediaStore::new(std::env::temp_dir().join("unused-variant-test"));
        for (width, height, expected) in [(800, 400, vec![(128, 64), (512, 256)]), (20, 10, vec![])]
        {
            let mut png = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([20, 40, 60, 128]),
            ))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
            let result = store.prepare_png(png.into_inner()).await.unwrap();
            assert_eq!(
                result
                    .variants
                    .iter()
                    .map(|(_, v)| (v.width, v.height))
                    .collect::<Vec<_>>(),
                expected
            );
            for (_, v) in result.variants {
                let decoded = image::load_from_memory(&v.png).unwrap().to_rgba8();
                assert_eq!(decoded.get_pixel(0, 0).0[3], 128);
            }
        }
        assert!(store.prepare_png(b"not png".to_vec()).await.is_err());
    }
}
