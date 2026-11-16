//! Bounded persistent derivative work. GET only resolves existing immutable representations.
use super::*;
use crate::identity::{Identity, Permission, Session};
use image::{ImageEncoder, ImageFormat};
use std::io::Cursor;

struct Rendition {
    profiles: Vec<ImageProfile>,
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    mime: &'static str,
    extension: &'static str,
    children: Vec<(ImageVariant, variants::Encoded)>,
}
#[derive(Default, Debug)]
pub struct CacheReport {
    pub complete: u64,
    pub failed: u64,
    pub pending: u64,
    pub running: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheAction {
    Repair,
    Rebuild,
    Clear,
}

impl MediaStore {
    pub async fn queue_cache(
        &self,
        pool: &sqlx::PgPool,
        s: &Session,
        csrf: &str,
        clear: bool,
    ) -> Result<(), library::LibraryError> {
        self.queue_cache_action(
            pool,
            s,
            csrf,
            if clear {
                CacheAction::Clear
            } else {
                CacheAction::Rebuild
            },
        )
        .await
    }
    pub async fn queue_cache_action(
        &self,
        pool: &sqlx::PgPool,
        s: &Session,
        csrf: &str,
        action: CacheAction,
    ) -> Result<(), library::LibraryError> {
        let mut tx = pool.begin().await?;
        Self::exclusive_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::MediaWrite).await?;
        queue_jobs(&mut tx, false, action).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn queue_all_cache(
        &self,
        pool: &sqlx::PgPool,
        clear: bool,
    ) -> Result<(), MediaError> {
        self.queue_all_cache_action(
            pool,
            if clear {
                CacheAction::Clear
            } else {
                CacheAction::Rebuild
            },
        )
        .await
    }
    pub async fn queue_all_cache_action(
        &self,
        pool: &sqlx::PgPool,
        action: CacheAction,
    ) -> Result<(), MediaError> {
        let mut tx = pool.begin().await?;
        Self::exclusive_lock(&mut tx).await?;
        queue_jobs(&mut tx, true, action).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn cache_report(&self, pool: &sqlx::PgPool) -> Result<CacheReport, MediaError> {
        self.cache_report_scope(pool, true).await
    }
    pub async fn cache_report_public(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<CacheReport, MediaError> {
        self.cache_report_scope(pool, false).await
    }
    async fn cache_report_scope(
        &self,
        pool: &sqlx::PgPool,
        all: bool,
    ) -> Result<CacheReport, MediaError> {
        let rows:Vec<(String,i64)>=sqlx::query_as("SELECT j.status,count(*) FROM media_cache_jobs j LEFT JOIN media_library l ON l.key=j.source_key WHERE $1 OR (l.key IS NOT NULL AND l.owner_id IS NULL) GROUP BY j.status").bind(all).fetch_all(pool).await?;
        let mut report = CacheReport::default();
        for (status, count) in rows {
            match status.as_str() {
                "complete" => report.complete = count as u64,
                "pending" => report.pending = count as u64,
                "running" => report.running = count as u64,
                _ => report.failed = count as u64,
            }
        }
        Ok(report)
    }
    async fn prepare_renditions(
        &self,
        raw: Vec<u8>,
        profiles: Vec<ImageProfile>,
    ) -> Result<Vec<Rendition>, MediaError> {
        let permit = self
            .imaging
            .clone()
            .try_acquire_owned()
            .map_err(|_| MediaError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let decoded = decode(&raw, STORED_LIMIT, 8192, 128 * 1024 * 1024)
                .map_err(|_| MediaError::Invalid)?;
            let mut result: Vec<Rendition> = Vec::new();
            for profile in profiles {
                let image = profile.render(&decoded);
                // Same fitted dimensions and encoding policy have identical pixels.
                if !profile.attachment()
                    && let Some(existing) = result.iter_mut().find(|r| {
                        !r.profiles[0].attachment()
                            && r.width == image.width()
                            && r.height == image.height()
                    })
                {
                    existing.profiles.push(profile);
                    continue;
                }
                let rgba = image.to_rgba8();
                let alpha = rgba.pixels().any(|p| p.0[3] < 255);
                let (bytes, mime, extension) = if profile.attachment() {
                    let mut png = Cursor::new(Vec::new());
                    image
                        .write_to(&mut png, ImageFormat::Png)
                        .map_err(|_| MediaError::Storage)?;
                    (png.into_inner(), "image/png", "png")
                } else if alpha {
                    let mut png = Cursor::new(Vec::new());
                    image
                        .write_to(&mut png, ImageFormat::Png)
                        .map_err(|_| MediaError::Storage)?;
                    let mut webp = Vec::new();
                    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
                        .write_image(
                            rgba.as_raw(),
                            image.width(),
                            image.height(),
                            image::ExtendedColorType::Rgba8,
                        )
                        .map_err(|_| MediaError::Storage)?;
                    if webp.len() < png.get_ref().len() {
                        (webp, "image/webp", "webp")
                    } else {
                        (png.into_inner(), "image/png", "png")
                    }
                } else {
                    let rgb = image.to_rgb8();
                    let mut jpeg = Vec::new();
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 82)
                        .encode(
                            rgb.as_raw(),
                            image.width(),
                            image.height(),
                            image::ExtendedColorType::Rgb8,
                        )
                        .map_err(|_| MediaError::Storage)?;
                    (jpeg, "image/jpeg", "jpg")
                };
                let children = if profile.attachment() {
                    variants::prepared_from_decoded(bytes.clone(), image.clone())?.variants
                } else {
                    Vec::new()
                };
                result.push(Rendition {
                    children,
                    profiles: vec![profile],
                    bytes,
                    width: image.width(),
                    height: image.height(),
                    mime,
                    extension,
                });
            }
            Ok(result)
        })
        .await
        .map_err(|_| MediaError::Storage)?
    }
    /// Claims have unique tokens; a stale worker cannot publish or finish a newer claim.
    pub async fn process_cache_job(&self, pool: &sqlx::PgPool) -> Result<bool, MediaError> {
        self.persist_cache_repairs(pool).await?;
        let token = crate::identity::token().map_err(|_| MediaError::Storage)?;
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE media_cache_jobs SET status='failed',lease_until=NULL,lease_token=NULL,updated_at=now() WHERE status='running' AND lease_until<now() AND attempts>=3").execute(&mut *tx).await?;
        let job:Option<(String,bool)>=sqlx::query_as("UPDATE media_cache_jobs SET status='running',attempts=attempts+1,lease_until=now()+interval '5 minutes',lease_token=$1,updated_at=now() WHERE id=(SELECT id FROM media_cache_jobs WHERE attempts<3 AND (status='pending' OR status='running' AND lease_until<now()) ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING source_key,rebuild").bind(&token).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        let Some((key, rebuild)) = job else {
            return Ok(false);
        };
        let result = self.build_cache(pool, &key, rebuild, Some(&token)).await;
        let busy = matches!(result, Err(MediaError::Busy));
        let status = if busy {
            "pending"
        } else if result.is_ok() {
            "complete"
        } else {
            "failed"
        };
        sqlx::query("UPDATE media_cache_jobs SET status=$1,lease_until=NULL,lease_token=NULL,attempts=attempts-CASE WHEN $4 THEN 1 ELSE 0 END,updated_at=now() WHERE source_key=$2 AND status='running' AND lease_token=$3 AND (lease_until>now() OR $4)").bind(status).bind(&key).bind(&token).bind(busy).execute(pool).await?;
        if let Err(error) = result
            && !busy
        {
            tracing::warn!(%error,"Image regeneration failed");
        }
        Ok(!busy)
    }
    pub async fn build_renditions(&self, pool: &sqlx::PgPool, key: &str) -> Result<(), MediaError> {
        self.build_cache(pool, key, true, None).await
    }
    async fn build_cache(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
        rebuild: bool,
        claim: Option<&str>,
    ) -> Result<(), MediaError> {
        let profiles = self.required_profiles(pool, key).await?;
        let mut missing = Vec::new();
        for profile in profiles {
            if rebuild || !self.profile_available(pool, key, profile).await? {
                missing.push(profile);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let bytes = self.read_registered(pool, key).await?;
        let prepared = self.prepare_renditions(bytes, missing).await?;
        let mut tx = pool.begin().await?;
        Self::publish_lock(&mut tx).await?;
        if let Some(token) = claim {
            let valid:Option<String>=sqlx::query_scalar("SELECT source_key FROM media_cache_jobs WHERE source_key=$1 AND status='running' AND lease_token=$2 AND lease_until>now() FOR UPDATE").bind(key).bind(token).fetch_optional(&mut *tx).await?;
            if valid.is_none() {
                return Err(MediaError::Busy);
            }
        }
        let source: Option<(i64, bool)> =
            sqlx::query_as("SELECT id,public FROM media_objects WHERE key=$1 FOR SHARE")
                .bind(key)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((id, public)) = source else {
            return Ok(());
        };
        let revision = crate::identity::token().map_err(|_| MediaError::Storage)?;
        let directory = format!("cache/{id}/{revision}");
        self.ensure_directory(&directory).await?;
        for image in prepared {
            // A concurrent selection may have published a healthy special profile while decoding.
            if !rebuild
                && image.profiles[0].attachment()
                && self
                    .profile_available_on(&mut tx, key, image.profiles[0])
                    .await?
            {
                continue;
            }
            let object = format!(
                "{directory}/{}.{}",
                image.profiles[0].key(),
                image.extension
            );
            let object = self.publish_bytes(&object, &image.bytes, false).await?;
            let mut stored = StoredMedia {
                key: object,
                bytes: image.bytes.len() as i64,
                width: image.width as i32,
                height: image.height as i32,
                variants: Vec::new(),
                content_type: image.mime,
            };
            stored
                .register_object(&mut tx, public && image.profiles[0] != ImageProfile::Avatar)
                .await?;
            self.put_variants(&mut stored, image.children).await?;
            stored
                .register_variants(&mut tx, public && image.profiles[0] != ImageProfile::Avatar)
                .await?;
            for profile in image.profiles {
                sqlx::query("INSERT INTO media_renditions(source_key,profile,object_key) VALUES($1,$2,$3) ON CONFLICT(source_key,profile) DO UPDATE SET object_key=excluded.object_key").bind(key).bind(profile.key()).bind(&stored.key).execute(&mut *tx).await?;
            }
        }
        tokio::fs::File::open(self.relative_path(&directory)?)
            .await?
            .sync_all()
            .await?;
        if let Some(token) = claim {
            sqlx::query("UPDATE media_cache_jobs SET status='complete',lease_until=NULL,lease_token=NULL,updated_at=now() WHERE source_key=$1 AND lease_token=$2").bind(key).bind(token).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn required_profiles(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
    ) -> Result<Vec<ImageProfile>, MediaError> {
        let mut profiles = ImageProfile::STANDARD.to_vec();
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM media_transforms WHERE source_key=$1 ORDER BY kind",
        )
        .bind(key)
        .fetch_all(pool)
        .await?;
        for kind in kinds {
            profiles.push(ImageProfile::parse(&kind).ok_or(MediaError::Storage)?);
        }
        Ok(profiles)
    }
    pub(crate) async fn profile_available(
        &self,
        pool: &sqlx::PgPool,
        key: &str,
        profile: ImageProfile,
    ) -> Result<bool, MediaError> {
        self.profile_available_on(&mut *pool.acquire().await?, key, profile)
            .await
    }
    pub(crate) async fn profile_available_on(
        &self,
        connection: &mut sqlx::PgConnection,
        key: &str,
        profile: ImageProfile,
    ) -> Result<bool, MediaError> {
        let file:Option<ImageFile>=sqlx::query_as("SELECT m.id,m.key,m.width,m.height,m.content_type,m.byte_size,l.path FROM media_renditions r JOIN media_objects m ON m.key=r.object_key JOIN media_locations l ON l.key=m.key WHERE r.source_key=$1 AND r.profile=$2").bind(key).bind(profile.key()).fetch_optional(&mut *connection).await?;
        let Some(file) = file else { return Ok(false) };
        if !self.file_available(&file).await? {
            return Ok(false);
        }
        if profile.attachment() {
            for child in [
                ImageProfile::Thumbnail,
                ImageProfile::Card,
                ImageProfile::Detail,
            ] {
                if file.width.max(file.height) <= child.edge() as i32 {
                    continue;
                }
                let variant:Option<ImageFile>=sqlx::query_as("SELECT m.id,m.key,m.width,m.height,m.content_type,m.byte_size,l.path FROM media_variants v JOIN media_objects m ON m.key=v.object_key JOIN media_locations l ON l.key=m.key WHERE v.source_key=$1 AND v.profile=$2").bind(&file.key).bind(child.key()).fetch_optional(&mut *connection).await?;
                let Some(variant) = variant else {
                    return Ok(false);
                };
                if !self.file_available(&variant).await? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    /// Resolve existing bytes; missing files only notify the worker without HTTP database writes.
    pub async fn rendition_url(
        &self,
        pool: &sqlx::PgPool,
        id: i64,
        profile: &str,
    ) -> Result<Option<String>, MediaError> {
        Ok(self
            .rendition_file(pool, id, profile)
            .await?
            .map(|file| file.url()))
    }

    /// Resolve the current public representation before HTTP cache validation.
    /// Logical profile URLs can serve these bytes directly, without a redirect.
    pub async fn rendition_file(
        &self,
        pool: &sqlx::PgPool,
        id: i64,
        profile: &str,
    ) -> Result<Option<ImageFile>, MediaError> {
        if ImageProfile::parse(profile).is_none() || profile == "avatar" {
            return Ok(None);
        }
        let source: Option<String> =
            sqlx::query_scalar("SELECT key FROM media_objects WHERE id=$1 AND public")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some(source) = source else {
            return Ok(None);
        };
        self.available_rendition_file(pool, &source, profile, true)
            .await
    }

    /// Recover a registered cache URL after validating exact ID/key and public source scope.
    pub async fn cache_fallback_url(
        &self,
        pool: &sqlx::PgPool,
        id: i64,
        key: &str,
        variant: ImageVariant,
    ) -> Result<Option<String>, MediaError> {
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
        if variant != ImageVariant::Original {
            return self.rendition_url(pool, id, variant.key()).await;
        }
        let source:Option<(String,String)>=sqlx::query_as("SELECT source_key,profile FROM (SELECT source_key,profile FROM media_renditions WHERE object_key=$1 UNION ALL SELECT source_key,profile FROM media_variants WHERE object_key=$1 UNION ALL SELECT source_key,kind FROM media_transforms WHERE object_key=$1) r JOIN media_objects m ON m.key=r.source_key WHERE m.public ORDER BY profile LIMIT 1").bind(key).fetch_optional(pool).await?;
        if let Some((source, profile)) = source {
            self.request_cache_repair(&source);
            return Ok(self
                .available_rendition_file(pool, &source, &profile, true)
                .await?
                .map(|file| file.url()));
        }
        if let Some((source_id, profile)) = cache_source_profile(key) {
            return self.rendition_url(pool, source_id, profile).await;
        }
        Ok(None)
    }
}

fn cache_source_profile(key: &str) -> Option<(i64, &str)> {
    let mut parts = key.split('/');
    if parts.next()? != "cache" {
        return None;
    }
    let id = parts.next()?.parse::<i64>().ok().filter(|id| *id > 0)?;
    let revision = parts.next()?;
    if revision.len() != 64 || !revision.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let (profile, extension) = parts.next()?.rsplit_once('.')?;
    if parts.next().is_some()
        || !matches!(extension, "jpg" | "png" | "webp")
        || ImageProfile::parse(profile).is_none_or(|p| p == ImageProfile::Original)
    {
        return None;
    }
    Some((id, profile))
}

async fn queue_jobs(
    tx: &mut Transaction<'_, Postgres>,
    all: bool,
    action: CacheAction,
) -> Result<(), MediaError> {
    if action == CacheAction::Clear {
        // Do not invalidate work already claimed or queued. Old files retain their own URLs.
        let scope = "SELECT l.key FROM media_library l WHERE ($1 OR l.owner_id IS NULL) AND l.trashed_at IS NULL AND NOT EXISTS(SELECT 1 FROM media_cache_jobs j WHERE j.source_key=l.key AND j.status IN ('pending','running'))";
        for table in ["media_renditions", "media_variants"] {
            let query =
                format!("DELETE FROM {table} WHERE source_key IN({scope}) RETURNING object_key");
            let retired: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
                .bind(all)
                .fetch_all(&mut **tx)
                .await?;
            sqlx::query("UPDATE media_objects SET unused_since=now() WHERE key=ANY($1)")
                .bind(retired)
                .execute(&mut **tx)
                .await?;
        }
    }
    jobs::enqueue(
        tx,
        None,
        all,
        if action == CacheAction::Repair {
            jobs::Enqueue::Repair
        } else {
            jobs::Enqueue::Rebuild
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_source_keys_are_structural_and_bounded() {
        let revision = "a".repeat(64);
        assert_eq!(
            cache_source_profile(&format!("cache/42/{revision}/card.jpg")),
            Some((42, "card"))
        );
        for bad in [
            format!("cache/0/{revision}/card.jpg"),
            format!("cache/-1/{revision}/card.jpg"),
            format!("cache/42/{revision}/original.jpg"),
            format!("cache/42/{revision}/card.svg"),
            format!("private/42/{revision}/card.jpg"),
            format!("cache/42/{revision}/card.jpg/other"),
            "cache/42/short/card.jpg".into(),
            "cache/42/../../card.jpg".into(),
        ] {
            assert!(cache_source_profile(&bad).is_none());
        }
    }
}
