//! Persistent administrator identity. Raw session and recovery tokens are never stored.
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

mod account;
mod product_columns;
pub use product_columns::{PRODUCT_COLUMNS, ProductColumns};
pub mod customer_admin;
pub mod customer_groups;
pub mod customer_registration;
mod customers;
pub use customers::{
    CustomerAccount, CustomerAuthAction, CustomerError, CustomerProfile, CustomerSession, Customers,
};
mod branding;
pub use branding::{BRANDING_UPLOAD_LIMIT, Branding, BrandingKind, PreparedBranding};
mod settings;
pub use settings::{Settings, SettingsField, SettingsInput};
mod administrators;
pub use account::{AVATAR_UPLOAD_LIMIT, Account};
mod error;
pub use administrators::{ADMINISTRATOR_PAGE_SIZE, Administrator};
mod roles;
pub use error::IdentityError;
pub use roles::{PERMISSIONS, Permission, Role, RoleDefinition};

pub const ANONYMOUS_SESSION_SECONDS: i64 = 30 * 60;
pub const AUTHENTICATED_SESSION_SECONDS: i64 = 8 * 60 * 60;
pub const ONE_TIME_TOKEN_SECONDS: i64 = 30 * 60;
pub const RATE_WINDOW_SECONDS: i64 = 15 * 60;

pub type Result<T> = std::result::Result<T, IdentityError>;

#[derive(Clone)]
pub struct Identity {
    pool: PgPool,
    hashing: Arc<Semaphore>,
    imaging: Arc<Semaphore>,
    media: crate::media::MediaStore,
}
/// Validated bootstrap values with the password already hashed outside any transaction.
/// Fields stay private and Debug is deliberately absent: credentials are never diagnostic data.
pub struct PreparedBootstrap {
    address: String,
    password_hash: String,
    name: String,
    display_name: String,
    languages: crate::storefront::Languages,
}
impl PreparedBootstrap {
    pub fn with_languages(mut self, languages: crate::storefront::Languages) -> Self {
        self.languages = languages;
        self
    }
}
#[derive(Clone, FromRow)]
pub struct Session {
    pub token_hash: String,
    pub csrf: String,
    pub administrator_id: Option<i64>,
    pub role: Option<String>,
    pub permissions: Vec<String>,
    pub display_name: String,
    pub email: Option<String>,
    pub has_avatar: bool,
    pub avatar_version: i64,
    pub locale: String,
}
impl Session {
    pub fn check_csrf(&self, value: &str) -> Result<()> {
        check_csrf(&self.csrf, value)
    }
    pub fn is_owner(&self) -> bool {
        self.administrator_id.is_some() && self.role.as_deref() == Some("owner")
    }
    pub fn allows(&self, permission: Permission) -> bool {
        self.administrator_id.is_some()
            && (self.is_owner() || self.permissions.iter().any(|p| p == permission.key()))
    }
    pub fn require(&self, permission: Permission) -> Result<i64> {
        self.administrator_id
            .filter(|_| self.allows(permission))
            .ok_or(IdentityError::Forbidden)
    }
    pub fn require_owner(&self) -> Result<i64> {
        self.administrator_id
            .filter(|_| self.is_owner())
            .ok_or(IdentityError::Forbidden)
    }
}

/// Compare CSRF credentials in constant time for administrator and guest boundaries.
pub fn check_csrf(expected: &str, value: &str) -> Result<()> {
    if bool::from(expected.as_bytes().ct_eq(value.as_bytes())) {
        Ok(())
    } else {
        Err(IdentityError::Forbidden)
    }
}
pub fn token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| IdentityError::Unavailable)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub fn valid_token(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn email(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    let parts: Vec<_> = value.split('@').collect();
    if value.len() > 254
        || !value.is_ascii()
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
        || parts.len() != 2
        || parts[0].is_empty()
        || !parts[1].contains('.')
        || parts[1].starts_with('.')
        || parts[1].ends_with('.')
    {
        return Err(IdentityError::Invalid);
    }
    Ok(value)
}
fn password(value: &str) -> Result<()> {
    if !(12..=128).contains(&value.chars().count()) || value.len() > 512 {
        Err(IdentityError::Invalid)
    } else {
        Ok(())
    }
}
pub fn store_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        Err(IdentityError::Invalid)
    } else {
        Ok(value.to_owned())
    }
}
impl Identity {
    pub fn with_media(mut self, media: crate::media::MediaStore) -> Self {
        self.imaging = media.image_slots();
        self.media = media;
        self
    }

    pub fn new(pool: PgPool) -> Self {
        let media = crate::media::MediaStore::new("storage/media".into());
        let imaging = media.image_slots();
        Self {
            pool,
            hashing: Arc::new(Semaphore::new(2)),
            imaging,
            media,
        }
    }
    pub async fn installed(&self) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT installed FROM identity_state WHERE id = 1")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    async fn hash(&self, value: String) -> Result<String> {
        password(&value)?;
        let permit = self
            .hashing
            .clone()
            .try_acquire_owned()
            .map_err(|_| IdentityError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut salt = [0u8; 16];
            getrandom::fill(&mut salt).map_err(|_| IdentityError::Unavailable)?;
            let salt = SaltString::encode_b64(&salt).map_err(|_| IdentityError::Unavailable)?;
            Argon2::default()
                .hash_password(value.as_bytes(), &salt)
                .map(|h| h.to_string())
                .map_err(|_| IdentityError::Unavailable)
        })
        .await
        .map_err(|_| IdentityError::Unavailable)?
    }
    async fn verify(&self, value: String, hash: Option<String>) -> Result<bool> {
        let permit = self
            .hashing
            .clone()
            .try_acquire_owned()
            .map_err(|_| IdentityError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if let Some(hash) = hash {
                let parsed = PasswordHash::new(&hash).map_err(|_| IdentityError::Unavailable)?;
                Ok(Argon2::default()
                    .verify_password(value.as_bytes(), &parsed)
                    .is_ok())
            } else {
                // Perform the same expensive operation for unknown accounts.
                let salt = SaltString::encode_b64(b"talocart-dummy!!")
                    .map_err(|_| IdentityError::Unavailable)?;
                let _ = Argon2::default()
                    .hash_password(value.as_bytes(), &salt)
                    .map_err(|_| IdentityError::Unavailable)?;
                Ok(false)
            }
        })
        .await
        .map_err(|_| IdentityError::Unavailable)?
    }
    /// Creates the first owner atomically. The optional display name follows the account rules.
    pub async fn bootstrap(
        &self,
        address: &str,
        secret: String,
        name: &str,
        display_name: &str,
    ) -> Result<()> {
        let prepared = self
            .prepare_bootstrap(address, secret, name, display_name)
            .await?;
        let mut tx = self.pool.begin().await?;
        Self::bootstrap_transaction(&mut tx, prepared).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Prepare before acquiring database/media locks; password hashing has bounded concurrency.
    pub async fn prepare_bootstrap(
        &self,
        address: &str,
        secret: String,
        name: &str,
        display_name: &str,
    ) -> Result<PreparedBootstrap> {
        let address = email(address)?;
        let name = store_name(name)?;
        let display_name = account::profile_name(display_name)?;
        let password_hash = self.hash(secret).await?;
        Ok(PreparedBootstrap {
            address,
            password_hash,
            name,
            display_name,
            languages: crate::storefront::Languages::default(),
        })
    }
    /// Composes first-owner creation with optional module data in one transaction.
    /// Caller uses the same database as Identity and commits/rolls back all owners together.
    /// The conditional update locks identity_state and permits only one completed installation.
    pub async fn bootstrap_transaction(
        tx: &mut Transaction<'_, Postgres>,
        prepared: PreparedBootstrap,
    ) -> Result<()> {
        let accepted =
            sqlx::query("UPDATE identity_state SET installed=TRUE WHERE id=1 AND NOT installed")
                .execute(&mut **tx)
                .await?;
        if accepted.rows_affected() != 1 {
            return Err(IdentityError::Installed);
        }
        sqlx::query("INSERT INTO administrators(email,password_hash,role,display_name,locale) VALUES ($1,$2,'owner',$3,$4)")
            .bind(prepared.address).bind(prepared.password_hash).bind(prepared.display_name).bind(prepared.languages.primary().code()).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO store_settings(id,name) VALUES (1,$1)")
            .bind(prepared.name)
            .execute(&mut **tx)
            .await?;
        prepared
            .languages
            .persist(tx)
            .await
            .map_err(|error| match error {
                crate::storefront::StorefrontError::Identity(error) => error,
                crate::storefront::StorefrontError::Invalid(_) => IdentityError::InvalidLocale,
                crate::storefront::StorefrontError::Content(_) => IdentityError::Unavailable,
                crate::storefront::StorefrontError::Technical(e) => e.into(),
                crate::storefront::StorefrontError::Access(e) => e.into(),
            })?;
        sqlx::query("DELETE FROM admin_sessions")
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub async fn session(&self, raw: &str) -> Result<Option<Session>> {
        if !valid_token(raw) {
            return Ok(None);
        }
        Ok(sqlx::query_as("SELECT s.token_hash,s.csrf,s.administrator_id,a.role,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=a.role ORDER BY permission) AS permissions,coalesce(a.display_name,'') AS display_name,a.email,a.avatar_key IS NOT NULL AS has_avatar,coalesce(a.avatar_version,0) AS avatar_version,coalesce(a.locale,$2::text) AS locale FROM admin_sessions s LEFT JOIN administrators a ON a.id=s.administrator_id WHERE s.token_hash=$1 AND s.expires_at>now() AND (s.administrator_id IS NULL OR a.active)")
            .bind(digest(raw)).bind(crate::locale::Locale::DEFAULT_CODE).fetch_optional(&self.pool).await?)
    }
    pub async fn anonymous(&self) -> Result<(String, Session)> {
        self.limit("anonymous", 200).await?;
        sqlx::query("DELETE FROM admin_sessions WHERE expires_at<=now()")
            .execute(&self.pool)
            .await?;
        let raw = token()?;
        let csrf = token()?;
        let hash = digest(&raw);
        sqlx::query("INSERT INTO admin_sessions(token_hash,csrf,expires_at) VALUES ($1,$2,now()+$3::bigint*interval '1 second')")
            .bind(&hash).bind(&csrf).bind(ANONYMOUS_SESSION_SECONDS).execute(&self.pool).await?;
        Ok((
            raw,
            Session {
                token_hash: hash,
                csrf,
                administrator_id: None,
                role: None,
                permissions: Vec::new(),
                display_name: String::new(),
                email: None,
                has_avatar: false,
                avatar_version: 0,
                locale: crate::locale::Locale::DEFAULT_CODE.into(),
            },
        ))
    }
    async fn limit(&self, bucket: &str, maximum: i32) -> Result<()> {
        let count = self.record_attempt(bucket).await?;
        if count > maximum {
            Err(IdentityError::Limited)
        } else {
            Ok(())
        }
    }
    async fn record_attempt(&self, bucket: &str) -> Result<i32> {
        // Atomic fixed window: concurrent failures are counted without holding a lock while hashing.
        sqlx::query("DELETE FROM auth_limits WHERE expires_at<=now()")
            .execute(&self.pool)
            .await?;
        Ok(sqlx::query_scalar("INSERT INTO auth_limits(bucket,attempts,expires_at) VALUES ($1,1,now()+$2::bigint*interval '1 second') ON CONFLICT(bucket) DO UPDATE SET attempts=auth_limits.attempts+1 RETURNING attempts")
            .bind(bucket).bind(RATE_WINDOW_SECONDS).fetch_one(&self.pool).await?)
    }
    async fn check_login_failures(&self, bucket: &str) -> Result<()> {
        let blocked: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_limits WHERE bucket=$1 AND expires_at>now() AND attempts>=5)")
            .bind(bucket).fetch_one(&self.pool).await?;
        if blocked {
            Err(IdentityError::Limited)
        } else {
            Ok(())
        }
    }
    pub async fn login(
        &self,
        session: &Session,
        csrf: &str,
        address: &str,
        secret: String,
    ) -> Result<String> {
        session.check_csrf(csrf)?;
        self.limit("login-global", 100).await?;
        let address = email(address).map_err(|_| IdentityError::Unauthorized)?;
        // Separate namespace: legacy attempt counters must not become failure counters.
        let failure_bucket = format!("login-failed:{}", digest(&address));
        self.check_login_failures(&failure_bucket).await?;
        if secret.len() > 512 {
            self.record_attempt(&failure_bucket).await?;
            return Err(IdentityError::Unauthorized);
        }
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT id,password_hash FROM administrators WHERE email=$1 AND active")
                .bind(address)
                .fetch_optional(&self.pool)
                .await?;
        if !self
            .verify(secret, row.as_ref().map(|r| r.1.clone()))
            .await?
        {
            self.record_attempt(&failure_bucket).await?;
            return Err(IdentityError::Unauthorized);
        }
        let (id, verified_hash) = row.ok_or(IdentityError::Unauthorized)?;
        let raw = token()?;
        let mut tx = self.pool.begin().await?;
        // Lock the account against concurrent password reset, then invalidate the pre-login session.
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM administrators WHERE id=$1 AND password_hash=$2 AND active FOR UPDATE",
        )
        .bind(id)
        .bind(verified_hash)
        .fetch_optional(&mut *tx)
        .await?;
        if current.is_none() {
            return Err(IdentityError::Unauthorized);
        }
        let deleted =
            sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1 AND expires_at>now()")
                .bind(&session.token_hash)
                .execute(&mut *tx)
                .await?;
        if deleted.rows_affected() != 1 {
            return Err(IdentityError::Unauthorized);
        }
        sqlx::query("INSERT INTO admin_sessions(token_hash,csrf,administrator_id,expires_at) VALUES ($1,$2,$3,now()+$4::bigint*interval '1 second')")
            .bind(digest(&raw)).bind(token()?).bind(id).bind(AUTHENTICATED_SESSION_SECONDS).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(raw)
    }
    pub async fn logout(&self, session: &Session, csrf: &str) -> Result<()> {
        session.check_csrf(csrf)?;
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1")
            .bind(&session.token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Local operator-assisted recovery until email delivery is implemented.
    pub async fn issue_reset(&self, address: &str) -> Result<String> {
        let address = email(address)?;
        let raw = token()?;
        let mut tx = self.pool.begin().await?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM administrators WHERE email=$1 AND active FOR UPDATE",
        )
        .bind(address)
        .fetch_optional(&mut *tx)
        .await?;
        let id = id.ok_or(IdentityError::Unauthorized)?;
        sqlx::query("INSERT INTO password_resets(token_hash,administrator_id,expires_at) VALUES ($1,$2,now()+$3::bigint*interval '1 second') ON CONFLICT(administrator_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,expires_at=EXCLUDED.expires_at")
            .bind(digest(&raw)).bind(id).bind(ONE_TIME_TOKEN_SECONDS).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(raw)
    }
    pub async fn reset(
        &self,
        session: &Session,
        csrf: &str,
        raw: &str,
        secret: String,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.limit("reset-global", 30).await?;
        if !valid_token(raw) {
            return Err(IdentityError::Unauthorized);
        }
        let hash = self.hash(secret).await?;
        let mut tx = self.pool.begin().await?;
        // Account first, then recovery token, then sessions: same order as account management.
        let id: Option<i64> = sqlx::query_scalar("SELECT a.id FROM administrators a JOIN password_resets r ON r.administrator_id=a.id WHERE r.token_hash=$1 AND r.expires_at>now() AND a.active FOR UPDATE OF a")
            .bind(digest(raw)).fetch_optional(&mut *tx).await?;
        let id = id.ok_or(IdentityError::Unauthorized)?;
        let consumed = sqlx::query("DELETE FROM password_resets WHERE token_hash=$1 AND administrator_id=$2 AND expires_at>now()")
            .bind(digest(raw)).bind(id).execute(&mut *tx).await?;
        if consumed.rows_affected() != 1 {
            return Err(IdentityError::Unauthorized);
        }
        sqlx::query("UPDATE administrators SET password_hash=$1 WHERE id=$2")
            .bind(hash)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM admin_sessions WHERE administrator_id=$1 OR (administrator_id IS NULL AND token_hash=$2)")
            .bind(id)
            .bind(&session.token_hash)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn hash_capacity_is_not_a_fifteen_minute_lockout() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@localhost/unused")
            .unwrap();
        let service = Identity::new(pool);
        let _occupied = service.hashing.acquire_many(2).await.unwrap();
        let error = service
            .hash("correct horse battery".into())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("15 minuti"));
        assert!(matches!(error, IdentityError::Busy));
        assert!(matches!(
            service.verify("password".into(), None).await,
            Err(IdentityError::Busy)
        ));
    }

    #[test]
    fn inputs_and_tokens_obey_contract() {
        assert_eq!(email(" Owner@Example.com ").unwrap(), "owner@example.com");
        for bad in ["", "x", "@example.com", "x@y", "x@@y.z", "x@a.\nb"] {
            assert!(email(bad).is_err());
        }
        assert!(password("short").is_err());
        assert!(password(&"a".repeat(129)).is_err());
        assert!(store_name("\n").is_err());
        let a = token().unwrap();
        let b = token().unwrap();
        assert!(valid_token(&a));
        assert_ne!(a, b);
        assert_ne!(digest(&a), a);
    }
    #[test]
    fn permissions_and_csrf_fail_closed() {
        let s = Session {
            token_hash: String::new(),
            csrf: "expected".into(),
            administrator_id: Some(1),
            role: Some("viewer".into()),
            permissions: vec!["overview.read".into(), "settings.read".into()],
            display_name: String::new(),
            email: Some("reader@example.test".into()),
            has_avatar: false,
            avatar_version: 0,
            locale: crate::locale::Locale::DEFAULT_CODE.into(),
        };
        assert!(s.check_csrf("wrong").is_err());
        assert!(s.check_csrf("expected").is_ok());
        assert!(s.require_owner().is_err());
    }
    #[tokio::test]
    async fn password_hashes_are_salted_and_verified() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@localhost/unused")
            .unwrap();
        let service = Identity::new(pool);
        let a = service.hash("correct horse battery".into()).await.unwrap();
        let b = service.hash("correct horse battery".into()).await.unwrap();
        assert!(a.starts_with("$argon2id$"));
        assert_ne!(a, b);
        assert!(
            service
                .verify("correct horse battery".into(), Some(a.clone()))
                .await
                .unwrap()
        );
        assert!(!service.verify("incorrect".into(), Some(a)).await.unwrap());
        assert!(!service.verify("incorrect".into(), None).await.unwrap());
    }
}
