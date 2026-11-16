//! Role-specific projections keep the selected library source intact.
use super::*;
#[derive(Clone, Copy)]
pub enum AttachmentProfile {
    Logo,
    Favicon,
    Avatar,
}
impl AttachmentProfile {
    pub fn key(self) -> &'static str {
        ImageProfile::from(self).key()
    }
}
impl MediaStore {
    pub async fn prepare_attachment(
        &self,
        pool: &sqlx::PgPool,
        id: i64,
        profile: AttachmentProfile,
        owner: Option<i64>,
    ) -> Result<PreparedImage, MediaError> {
        let key:Option<String>=sqlx::query_scalar("SELECT m.key FROM media_objects m JOIN media_library l ON l.key=m.key WHERE m.id=$1 AND m.width>0 AND m.height>0 AND l.trashed_at IS NULL AND (m.public OR l.owner_id=$2)").bind(id).bind(owner).fetch_optional(pool).await?;
        let key = key.ok_or(MediaError::Invalid)?;
        let raw = self.read_registered(pool, &key).await?;
        let permit = self
            .imaging
            .clone()
            .try_acquire_owned()
            .map_err(|_| MediaError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let decoded = decode(&raw, STORED_LIMIT, 8192, 128 * 1024 * 1024)
                .map_err(|_| MediaError::Invalid)?;
            let image = ImageProfile::from(profile).render(&decoded);
            let mut output = std::io::Cursor::new(Vec::new());
            image
                .write_to(&mut output, image::ImageFormat::Png)
                .map_err(|_| MediaError::Storage)?;
            variants::prepared_from_decoded(output.into_inner(), image)
        })
        .await
        .map_err(|_| MediaError::Storage)?
    }
    pub async fn publish_attachment(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        source: &str,
        profile: AttachmentProfile,
        prepared: PreparedImage,
    ) -> Result<String, MediaError> {
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT object_key FROM media_transforms WHERE source_key=$1 AND kind=$2",
        )
        .bind(source)
        .bind(profile.key())
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(stable) = &existing
            && self
                .profile_available_on(&mut *tx, source, ImageProfile::from(profile))
                .await?
        {
            return Ok(stable.clone());
        }
        // Technical projections always get a fresh identity, even when an older file was removed.
        let name = format!(
            "{}-{}.png",
            crate::identity::token().map_err(|_| MediaError::Storage)?,
            profile.key()
        );
        let stored = self.put_prepared(prepared, &name).await?;
        stored
            .register_internal(tx, !matches!(profile, AttachmentProfile::Avatar))
            .await?;
        sqlx::query("INSERT INTO media_renditions(source_key,profile,object_key) VALUES($1,$2,$3) ON CONFLICT(source_key,profile) DO UPDATE SET object_key=excluded.object_key")
            .bind(source).bind(profile.key()).bind(&stored.key).execute(&mut **tx).await?;
        if let Some(stable) = existing {
            return Ok(stable);
        }
        sqlx::query("INSERT INTO media_transforms(source_key,kind,object_key) VALUES($1,$2,$3)")
            .bind(source)
            .bind(profile.key())
            .bind(&stored.key)
            .execute(&mut **tx)
            .await?;
        Ok(stored.key)
    }
}
