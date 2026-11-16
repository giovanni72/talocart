//! Self-service account changes never accept a target account, role or permissions.
use super::*;
use image::{ImageFormat, imageops::FilterType};
use std::io::Cursor;

pub const AVATAR_UPLOAD_LIMIT: usize = 2 * 1024 * 1024;
#[derive(FromRow)]
pub struct Account {
    pub id: i64,
    pub display_name: String,
    pub email: String,
    pub role_name: String,
    pub has_avatar: bool,
    pub avatar_version: i64,
    pub locale: String,
}
#[derive(FromRow)]
pub(super) struct Credentials {
    email: String,
    password_hash: String,
}
pub(super) fn profile_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.chars().count() > 80 || value.chars().any(char::is_control) {
        Err(IdentityError::Invalid)
    } else {
        Ok(value.to_owned())
    }
}
fn normalize_avatar(raw: &[u8]) -> Result<Vec<u8>> {
    let decoded = crate::media::decode(raw, AVATAR_UPLOAD_LIMIT, 4096, 64 * 1024 * 1024)
        .map_err(|_| IdentityError::InvalidAvatar)?;
    let avatar = decoded.resize_to_fill(256, 256, FilterType::Lanczos3);
    let mut output = Cursor::new(Vec::new());
    avatar
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| IdentityError::InvalidAvatar)?;
    let output = output.into_inner();
    if output.len() > 512 * 1024 {
        return Err(IdentityError::InvalidAvatar);
    }
    Ok(output)
}
impl Identity {
    /// Update only the authenticated caller; recheck active account and session
    /// under the same locks used by revocation and other account changes.
    pub async fn update_locale(&self, session: &Session, csrf: &str, value: &str) -> Result<()> {
        session.check_csrf(csrf)?;
        let locale = crate::locale::Locale::parse(value).ok_or(IdentityError::InvalidLocale)?;
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        Self::own_locked(&mut tx, session).await?;
        sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2")
            .bind(locale.code())
            .bind(session.administrator_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn account(&self, session: &Session) -> Result<Account> {
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        sqlx::query_as("SELECT a.id,a.display_name,a.email,r.name AS role_name,a.avatar_key IS NOT NULL AS has_avatar,a.avatar_version,a.locale FROM administrators a JOIN admin_roles r ON r.key=a.role JOIN admin_sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND s.token_hash=$2 AND s.expires_at>now() AND a.active")
            .bind(id).bind(&session.token_hash).fetch_optional(&self.pool).await?.ok_or(IdentityError::Forbidden)
    }
    async fn account_credentials(&self, session: &Session) -> Result<Credentials> {
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        sqlx::query_as("SELECT a.email,a.password_hash FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND s.token_hash=$2 AND s.expires_at>now() AND a.active")
            .bind(id).bind(&session.token_hash).fetch_optional(&self.pool).await?.ok_or(IdentityError::Forbidden)
    }
    pub(super) async fn own_locked(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
    ) -> Result<Credentials> {
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        let credentials = sqlx::query_as("SELECT a.email,a.password_hash FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND s.token_hash=$2 AND s.expires_at>now() AND a.active FOR UPDATE OF a,s")
            .bind(id).bind(&session.token_hash).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Forbidden)?;
        Self::attribute_operation(tx, id).await?;
        Ok(credentials)
    }
    async fn check_current_password(
        &self,
        session: &Session,
        secret: String,
        hash: &str,
    ) -> Result<()> {
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        // Shared budget for email/password reauthentication, including successful attempts.
        self.limit(&format!("account-change:{id}"), 10).await?;
        if secret.len() > 512 || !self.verify(secret, Some(hash.to_owned())).await? {
            return Err(IdentityError::CurrentPassword);
        }
        Ok(())
    }
    /// Reauthenticate a current owner without retaining a transaction during package I/O.
    /// Password work is bounded and shares the account reauthentication rate budget.
    pub async fn reauthenticate_owner(
        &self,
        session: &Session,
        csrf: &str,
        secret: String,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.authorize_owner(session).await?;
        let expected = self.account_credentials(session).await?;
        self.check_current_password(session, secret, &expected.password_hash)
            .await?;
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let current = Self::own_locked(&mut tx, session).await?;
        let owner: bool = sqlx::query_scalar("SELECT role='owner' FROM administrators WHERE id=$1")
            .bind(session.administrator_id)
            .fetch_one(&mut *tx)
            .await?;
        if !owner || current.password_hash != expected.password_hash {
            return Err(IdentityError::Forbidden);
        }
        tx.commit().await?;
        Ok(())
    }

    async fn revoke_account_credentials(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: i64,
    ) -> Result<()> {
        sqlx::query("DELETE FROM password_resets WHERE administrator_id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM admin_sessions WHERE administrator_id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    /// Returns true when changing email requires a fresh login. Name-only edits preserve sessions.
    pub async fn update_account(
        &self,
        session: &Session,
        csrf: &str,
        name: &str,
        address: &str,
        current_password: String,
    ) -> Result<bool> {
        session.check_csrf(csrf)?;
        let name = profile_name(name)?;
        let address = email(address)?;
        let expected = self.account_credentials(session).await?;
        let email_changed = expected.email != address;
        if email_changed {
            self.check_current_password(session, current_password, &expected.password_hash)
                .await?;
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let current = Self::own_locked(&mut tx, session).await?;
        if current.email != expected.email || current.password_hash != expected.password_hash {
            return Err(IdentityError::Forbidden);
        }
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        let result = sqlx::query("UPDATE administrators SET display_name=$1,email=$2 WHERE id=$3")
            .bind(name)
            .bind(address)
            .bind(id)
            .execute(&mut *tx)
            .await;
        match result {
            Err(e)
                if e.as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
            {
                return Err(IdentityError::DuplicateEmail);
            }
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        if email_changed {
            Self::revoke_account_credentials(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(email_changed)
    }
    pub async fn change_password(
        &self,
        session: &Session,
        csrf: &str,
        current_password: String,
        new_password: String,
        confirmation: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        password(&new_password)?;
        if new_password != confirmation {
            return Err(IdentityError::PasswordMismatch);
        }
        let expected = self.account_credentials(session).await?;
        self.check_current_password(session, current_password, &expected.password_hash)
            .await?;
        let hash = self.hash(new_password).await?;
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let current = Self::own_locked(&mut tx, session).await?;
        if current.password_hash != expected.password_hash {
            return Err(IdentityError::Forbidden);
        }
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        sqlx::query("UPDATE administrators SET password_hash=$1 WHERE id=$2")
            .bind(hash)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        Self::revoke_account_credentials(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn update_avatar(
        &self,
        session: &Session,
        csrf: &str,
        raw: Option<Vec<u8>>,
    ) -> Result<()> {
        self.update_avatar_named(session, csrf, raw, "avatar.png")
            .await
    }
    pub async fn update_avatar_named(
        &self,
        session: &Session,
        csrf: &str,
        raw: Option<Vec<u8>>,
        filename: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.account(session).await?;
        let png = if let Some(raw) = raw {
            let permit = self
                .imaging
                .clone()
                .try_acquire_owned()
                .map_err(|_| IdentityError::Busy)?;
            Some(
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    normalize_avatar(&raw)
                })
                .await
                .map_err(|_| IdentityError::Unavailable)??,
            )
        } else {
            None
        };
        let prepared = match png {
            Some(png) => Some(self.media.prepare_png(png).await?),
            None => None,
        };
        let mut tx = self.pool.begin().await?;
        crate::media::MediaStore::publish_lock(&mut tx).await?;
        Self::lock_changes(&mut tx).await?;
        Self::own_locked(&mut tx, session).await?;
        let stored = match prepared {
            Some(prepared) => Some(self.media.put_prepared(prepared, filename).await?),
            None => None,
        };
        if let Some(stored) = &stored {
            stored.register(&mut tx, false).await?;
            self.media
                .retain_private(
                    &mut tx,
                    &stored.key,
                    session.administrator_id.ok_or(IdentityError::Forbidden)?,
                )
                .await?;
        }
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        sqlx::query(
            "UPDATE administrators SET avatar_key=$1,avatar_version=avatar_version+1 WHERE id=$2",
        )
        .bind(stored.as_ref().map(|s| &s.key))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn select_avatar(&self, s: &Session, csrf: &str, media_id: i64) -> Result<()> {
        self.account(s).await?;
        s.check_csrf(csrf)?;
        let profile = crate::media::AttachmentProfile::Avatar;
        let prepared = self
            .media
            .prepare_attachment(&self.pool, media_id, profile, s.administrator_id)
            .await?;
        let mut tx = self.pool.begin().await?;
        crate::media::MediaStore::publish_lock(&mut tx).await?;
        Self::authorize_account_transaction(&mut tx, s, csrf).await?;
        let source =
            crate::media::MediaStore::selected_key(&mut tx, media_id, s.administrator_id).await?;
        let key = self
            .media
            .publish_attachment(&mut tx, &source, profile, prepared)
            .await?;
        sqlx::query(
            "UPDATE administrators SET avatar_key=$1,avatar_version=avatar_version+1 WHERE id=$2",
        )
        .bind(key)
        .bind(s.administrator_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn avatar(&self, session: &Session) -> Result<Option<Vec<u8>>> {
        Ok(self
            .avatar_image(session, crate::media::ImageVariant::Original)
            .await?
            .map(|i| i.png))
    }
    pub async fn avatar_image(
        &self,
        session: &Session,
        variant: crate::media::ImageVariant,
    ) -> Result<Option<crate::media::MediaImage>> {
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        let row: Option<(Option<String>,)> = sqlx::query_as("SELECT a.avatar_key FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND s.token_hash=$2 AND s.expires_at>now() AND a.active")
            .bind(id).bind(&session.token_hash).fetch_optional(&self.pool).await?;
        match row.ok_or(IdentityError::Forbidden)? {
            (Some(key),) => Ok(self.media.image(&self.pool, &key, variant).await?),
            (None,) => Ok(None),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn avatars_are_decoded_bounded_and_normalized() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(400, 100)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let normalized = normalize_avatar(png.get_ref()).unwrap();
        let decoded = image::load_from_memory(&normalized).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 256));
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(80, 100)
            .write_to(&mut jpeg, ImageFormat::Jpeg)
            .unwrap();
        assert!(
            normalize_avatar(jpeg.get_ref())
                .unwrap()
                .starts_with(b"\x89PNG\r\n\x1a\n")
        );
        for invalid in [
            b"<svg onload='alert(1)'/>".to_vec(),
            b"not an image".to_vec(),
            vec![0; AVATAR_UPLOAD_LIMIT + 1],
            png.get_ref()[..20].to_vec(),
        ] {
            assert!(matches!(
                normalize_avatar(&invalid),
                Err(IdentityError::InvalidAvatar)
            ));
        }
        let mut huge = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4097, 1)
            .write_to(&mut huge, ImageFormat::Png)
            .unwrap();
        assert!(normalize_avatar(huge.get_ref()).is_err());
    }
}
