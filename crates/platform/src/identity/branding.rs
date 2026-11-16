//! Small public store images, independent of general settings and private avatars.
use super::*;
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::io::Cursor;

pub const BRANDING_UPLOAD_LIMIT: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum BrandingKind {
    Logo,
    Favicon,
}
impl BrandingKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Favicon => "favicon",
        }
    }
}
#[derive(FromRow)]
pub struct Branding {
    pub has_logo: bool,
    pub logo_version: i64,
    pub has_favicon: bool,
    pub favicon_version: i64,
}

fn normalize(raw: &[u8], kind: BrandingKind) -> Result<(Vec<u8>, u32, u32)> {
    let decoded = crate::media::decode(raw, BRANDING_UPLOAD_LIMIT, 8192, 128 * 1024 * 1024)
        .map_err(|_| IdentityError::InvalidBranding)?;
    let image = match kind {
        BrandingKind::Logo => {
            if decoded.width() <= 2048 && decoded.height() <= 1024 {
                decoded
            } else {
                decoded.thumbnail(2048, 1024)
            }
        }
        BrandingKind::Favicon => {
            let fit = decoded.thumbnail(64, 64).to_rgba8();
            let mut square = RgbaImage::new(64, 64);
            image::imageops::overlay(
                &mut square,
                &fit,
                i64::from((64 - fit.width()) / 2),
                i64::from((64 - fit.height()) / 2),
            );
            DynamicImage::ImageRgba8(square)
        }
    };
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| IdentityError::InvalidBranding)?;
    let png = output.into_inner();
    Ok((png, image.width(), image.height()))
}

/// Prepared before opening the configuration transaction; no setting is changed.
pub struct PreparedBranding {
    kind: BrandingKind,
    media_id: Option<i64>,
    image: Option<crate::media::PreparedImage>,
}
impl Identity {
    pub async fn prepare_branding_selection(
        &self,
        s: &Session,
        kind: BrandingKind,
        selection: &str,
    ) -> Result<Option<PreparedBranding>> {
        self.authorize(s, Permission::SettingsWrite).await?;
        if selection.is_empty() {
            return Ok(None);
        }
        if selection == "remove" {
            return Ok(Some(PreparedBranding {
                kind,
                media_id: None,
                image: None,
            }));
        }
        let id = selection
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or(IdentityError::InvalidBranding)?;
        self.authorize(s, Permission::MediaRead).await?;
        let profile = match kind {
            BrandingKind::Logo => crate::media::AttachmentProfile::Logo,
            BrandingKind::Favicon => crate::media::AttachmentProfile::Favicon,
        };
        let image = self
            .media
            .prepare_attachment(&self.pool, id, profile, None)
            .await?;
        Ok(Some(PreparedBranding {
            kind,
            media_id: Some(id),
            image: Some(image),
        }))
    }
    /// Caller holds publish_lock before identity authorization; never commits.
    pub async fn save_branding_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        s: &Session,
        csrf: &str,
        prepared: PreparedBranding,
    ) -> Result<()> {
        Self::authorize_transaction(tx, s, csrf, Permission::SettingsWrite).await?;
        let key = match (prepared.media_id, prepared.image) {
            (Some(id), Some(image)) => {
                Self::authorize_transaction(tx, s, csrf, Permission::MediaRead).await?;
                let source = crate::media::MediaStore::selected_key(tx, id, None).await?;
                let profile = match prepared.kind {
                    BrandingKind::Logo => crate::media::AttachmentProfile::Logo,
                    BrandingKind::Favicon => crate::media::AttachmentProfile::Favicon,
                };
                Some(
                    self.media
                        .publish_attachment(tx, &source, profile, image)
                        .await?,
                )
            }
            (None, None) => None,
            _ => return Err(IdentityError::InvalidBranding),
        };
        let query = match prepared.kind {
            BrandingKind::Logo => {
                "UPDATE store_settings SET logo_key=$1,logo_version=logo_version+1 WHERE id=1"
            }
            BrandingKind::Favicon => {
                "UPDATE store_settings SET favicon_key=$1,favicon_version=favicon_version+1 WHERE id=1"
            }
        };
        sqlx::query(query).bind(key).execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn select_branding(
        &self,
        s: &Session,
        csrf: &str,
        kind: BrandingKind,
        media_id: i64,
    ) -> Result<()> {
        self.authorize(s, Permission::SettingsWrite).await?;
        self.authorize(s, Permission::MediaRead).await?;
        s.check_csrf(csrf)?;
        let profile = match kind {
            BrandingKind::Logo => crate::media::AttachmentProfile::Logo,
            BrandingKind::Favicon => crate::media::AttachmentProfile::Favicon,
        };
        let prepared = self
            .media
            .prepare_attachment(&self.pool, media_id, profile, None)
            .await?;
        let mut tx = self.pool.begin().await?;
        crate::media::MediaStore::publish_lock(&mut tx).await?;
        Self::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        Self::authorize_transaction(&mut tx, s, csrf, Permission::MediaRead).await?;
        let source = crate::media::MediaStore::selected_key(&mut tx, media_id, None).await?;
        let key = self
            .media
            .publish_attachment(&mut tx, &source, profile, prepared)
            .await?;
        let query = match kind {
            BrandingKind::Logo => {
                "UPDATE store_settings SET logo_key=$1,logo_version=logo_version+1 WHERE id=1"
            }
            BrandingKind::Favicon => {
                "UPDATE store_settings SET favicon_key=$1,favicon_version=favicon_version+1 WHERE id=1"
            }
        };
        sqlx::query(query).bind(key).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn public_image(
        &self,
        id: i64,
        key: &str,
        variant: crate::media::ImageVariant,
    ) -> Result<Option<crate::media::MediaImage>> {
        Ok(self
            .media
            .public_image(&self.pool, id, key, variant)
            .await?)
    }

    /// Metadata only; regular page rendering never reads image bytes.
    pub async fn branding(&self) -> Result<Option<Branding>> {
        Self::read_branding(&self.pool).await
    }
    async fn read_branding<'e, E>(executor: E) -> Result<Option<Branding>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Ok(sqlx::query_as("SELECT logo_key IS NOT NULL AS has_logo,logo_version,favicon_key IS NOT NULL AS has_favicon,favicon_version FROM store_settings WHERE id=1")
            .fetch_optional(executor).await?)
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn branding_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Option<Branding>> {
        Self::read_branding(&mut **tx).await
    }
    /// Public store assets. An absent image or uninstalled store uses the web fallback.
    pub async fn branding_image(&self, kind: BrandingKind) -> Result<Option<Vec<u8>>> {
        let Some(metadata) = self.branding_metadata(kind).await? else {
            return Ok(None);
        };
        Ok(self
            .media
            .image(
                &self.pool,
                &metadata.key,
                crate::media::ImageVariant::Original,
            )
            .await?
            .map(|i| i.png))
    }
    pub async fn branding_metadata(
        &self,
        kind: BrandingKind,
    ) -> Result<Option<crate::media::ImageMetadata>> {
        let mut connection = self.pool.acquire().await?;
        self.branding_metadata_on(&mut connection, kind).await
    }
    /// Metadata lookup on an existing connection; no filesystem access.
    pub async fn branding_metadata_on(
        &self,
        connection: &mut sqlx::PgConnection,
        kind: BrandingKind,
    ) -> Result<Option<crate::media::ImageMetadata>> {
        let query = match kind {
            BrandingKind::Logo => "SELECT logo_key FROM store_settings WHERE id=1",
            BrandingKind::Favicon => "SELECT favicon_key FROM store_settings WHERE id=1",
        };
        let row: Option<(Option<String>,)> = sqlx::query_as(query)
            .fetch_optional(&mut *connection)
            .await?;
        match row.and_then(|r| r.0) {
            Some(key) => Ok(self.media.metadata(&mut *connection, &key).await?),
            None => Ok(None),
        }
    }
    pub async fn update_branding(
        &self,
        session: &Session,
        csrf: &str,
        kind: BrandingKind,
        raw: Option<Vec<u8>>,
    ) -> Result<()> {
        self.update_branding_named(session, csrf, kind, raw, "branding.png")
            .await
    }
    pub async fn update_branding_named(
        &self,
        session: &Session,
        csrf: &str,
        kind: BrandingKind,
        raw: Option<Vec<u8>>,
        filename: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.authorize(session, Permission::SettingsWrite).await?;
        let png = if let Some(raw) = raw {
            let permit = self
                .imaging
                .clone()
                .try_acquire_owned()
                .map_err(|_| IdentityError::Busy)?;
            Some(
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    normalize(&raw, kind)
                })
                .await
                .map_err(|_| IdentityError::Unavailable)??,
            )
        } else {
            None
        };
        let prepared = match png {
            Some((png, _, _)) => Some(self.media.prepare_png(png).await?),
            None => None,
        };
        let mut tx = self.pool.begin().await?;
        crate::media::MediaStore::publish_lock(&mut tx).await?;
        Self::lock_changes(&mut tx).await?;
        Self::authorize_locked(&mut tx, session, Permission::SettingsWrite).await?;
        let stored = match prepared {
            Some(prepared) => Some(self.media.put_prepared(prepared, filename).await?),
            None => None,
        };
        if let Some(stored) = &stored {
            stored.register(&mut tx, true).await?;
        }
        let query = match kind {
            BrandingKind::Logo => {
                "UPDATE store_settings SET logo_key=$1,logo_version=logo_version+1 WHERE id=1"
            }
            BrandingKind::Favicon => {
                "UPDATE store_settings SET favicon_key=$1,favicon_version=favicon_version+1 WHERE id=1"
            }
        };
        let result = sqlx::query(query)
            .bind(stored.as_ref().map(|s| &s.key))
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() != 1 {
            return Err(IdentityError::Unavailable);
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logo_keeps_aspect_and_transparency_favicon_is_padded() {
        let input = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            1024,
            256,
            image::Rgba([80, 100, 60, 128]),
        ));
        let mut raw = Cursor::new(Vec::new());
        input.write_to(&mut raw, ImageFormat::Png).unwrap();
        let logo =
            image::load_from_memory(&normalize(raw.get_ref(), BrandingKind::Logo).unwrap().0)
                .unwrap()
                .to_rgba8();
        assert_eq!(logo.dimensions(), (1024, 256));
        assert_eq!(logo.get_pixel(0, 0).0[3], 128);
        let icon =
            image::load_from_memory(&normalize(raw.get_ref(), BrandingKind::Favicon).unwrap().0)
                .unwrap()
                .to_rgba8();
        assert_eq!(icon.dimensions(), (64, 64));
        assert_eq!(icon.get_pixel(32, 0).0[3], 0);
        assert_eq!(icon.get_pixel(32, 32).0[3], 128);
    }
    #[test]
    fn large_logo_is_not_limited_to_avatar_size() {
        let mut seed = 0x12345678u32;
        let pixels = image::RgbImage::from_fn(800, 400, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8])
        });
        let mut raw = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(pixels)
            .write_to(&mut raw, ImageFormat::Png)
            .unwrap();
        assert!(raw.get_ref().len() > 512 * 1024);
        let (png, width, height) = normalize(raw.get_ref(), BrandingKind::Logo).unwrap();
        assert!(png.len() > 512 * 1024);
        assert_eq!((width, height), (800, 400));
    }
    #[test]
    fn invalid_branding_and_jpeg_are_checked_by_content() {
        for raw in [
            vec![],
            b"<svg onload='alert(1)'/>".to_vec(),
            vec![0; BRANDING_UPLOAD_LIMIT + 1],
            include_bytes!("../../../web/tests/fixtures/avatar.png")[..25].to_vec(),
        ] {
            for kind in [BrandingKind::Logo, BrandingKind::Favicon] {
                assert!(matches!(
                    normalize(&raw, kind),
                    Err(IdentityError::InvalidBranding)
                ));
            }
        }
        let mut raw = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(100, 50)
            .write_to(&mut raw, ImageFormat::Jpeg)
            .unwrap();
        assert!(
            normalize(raw.get_ref(), BrandingKind::Logo)
                .unwrap()
                .0
                .starts_with(b"\x89PNG")
        );
        raw = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(8193, 1)
            .write_to(&mut raw, ImageFormat::Png)
            .unwrap();
        assert!(normalize(raw.get_ref(), BrandingKind::Logo).is_err());
    }
}
