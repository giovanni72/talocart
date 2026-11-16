//! Fixed roles for implemented operations. Account mutations serialize on identity_state.
use super::*;
use sqlx::{Postgres, Transaction};

pub const ADMINISTRATOR_PAGE_SIZE: usize = 20;

#[derive(FromRow)]
pub struct Administrator {
    pub id: i64,
    pub email: String,
    pub role: String,
    pub role_name: String,
    pub active: bool,
}

impl Identity {
    /// A queued job is delegated to its creator, independent of the browser session.
    /// Recheck active account and current permission for each committed unit of work.
    pub async fn authorize_background(
        tx: &mut Transaction<'_, Postgres>,
        administrator_id: i64,
        permission: Permission,
    ) -> Result<()> {
        Self::lock_changes(tx).await?;
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators a WHERE a.id=$1 AND a.active AND (a.role='owner' OR EXISTS(SELECT 1 FROM admin_role_permissions p WHERE p.role_key=a.role AND p.permission=$2)))")
            .bind(administrator_id).bind(permission.key()).fetch_one(&mut **tx).await?;
        if allowed {
            Self::attribute_operation(tx, administrator_id).await?;
            Ok(())
        } else {
            Err(IdentityError::Forbidden)
        }
    }
    /// Authorizes a mutation on the shared pool, serializing against role/session revocation.
    /// Callers commit business queries on this transaction, never a second pool.
    /// Media publishers must acquire publish_lock before calling this method.
    pub async fn authorize_transaction(
        tx: &mut Transaction<'_, Postgres>,
        session: &Session,
        csrf: &str,
        permission: Permission,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        Self::lock_changes(tx).await?;
        Self::authorize_locked(tx, session, permission).await?;
        Ok(())
    }
    /// Authorizes a read-only snapshot on the caller's connection. Readers share
    /// the identity guard; configuration/role writers still take it exclusively.
    /// Shared account/session locks also exclude logout and password recovery.
    /// Do not upgrade this transaction to a mutation after acquiring read locks.
    pub async fn authorize_read_transaction(
        tx: &mut Transaction<'_, Postgres>,
        session: &Session,
        permission: Permission,
    ) -> Result<()> {
        let id = session.require(permission)?;
        sqlx::query("SELECT id FROM identity_state WHERE id=1 FOR SHARE")
            .fetch_one(&mut **tx)
            .await?;
        let allowed: Option<bool> = sqlx::query_scalar("SELECT true FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND s.token_hash=$2 AND a.active AND s.expires_at>now() AND (a.role='owner' OR EXISTS(SELECT 1 FROM admin_role_permissions p WHERE p.role_key=a.role AND p.permission=$3)) FOR SHARE OF a,s")
            .bind(id).bind(&session.token_hash).bind(permission.key())
            .fetch_optional(&mut **tx).await?;
        if allowed == Some(true) {
            Ok(())
        } else {
            Err(IdentityError::Forbidden)
        }
    }
    /// Self-service media actions require a live account, never an administrative permission.
    pub async fn authorize_account_transaction(
        tx: &mut Transaction<'_, Postgres>,
        session: &Session,
        csrf: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        Self::lock_changes(tx).await?;
        Self::own_locked(tx, session).await?;
        Ok(())
    }
    pub(super) async fn lock_changes(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
        sqlx::query("SELECT id FROM identity_state WHERE id=1 FOR UPDATE")
            .fetch_one(&mut **tx)
            .await?;
        Ok(())
    }
    /// Package execution is reserved to a current, active owner, including after long uploads.
    pub async fn authorize_owner(&self, session: &Session) -> Result<i64> {
        let id = session.require_owner()?;
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE a.id=$1 AND a.active AND a.role='owner' AND s.token_hash=$2 AND s.expires_at>now())")
            .bind(id).bind(&session.token_hash).fetch_one(&self.pool).await?;
        if allowed {
            Ok(id)
        } else {
            Err(IdentityError::Forbidden)
        }
    }
    pub async fn authorize(&self, session: &Session, permission: Permission) -> Result<i64> {
        self.authorize_all(session, &[permission]).await
    }
    /// Validate a composite read against one current session snapshot. Callers
    /// list every required permission; writes still use authorize_transaction.
    pub async fn authorize_all(
        &self,
        session: &Session,
        permissions: &[Permission],
    ) -> Result<i64> {
        let id = session.administrator_id.ok_or(IdentityError::Forbidden)?;
        for permission in permissions {
            session.require(*permission)?;
        }
        let current: Option<Session> = sqlx::query_as("SELECT s.token_hash,s.csrf,s.administrator_id,a.role,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=a.role ORDER BY permission) AS permissions,coalesce(a.display_name,'') AS display_name,a.email,a.avatar_key IS NOT NULL AS has_avatar,coalesce(a.avatar_version,0) AS avatar_version,a.locale FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE s.token_hash=$1 AND a.id=$2 AND a.active AND s.expires_at>now()")
            .bind(&session.token_hash).bind(id).fetch_optional(&self.pool).await?;
        current
            .filter(|s| permissions.iter().all(|permission| s.allows(*permission)))
            .map(|_| id)
            .ok_or(IdentityError::Forbidden)
    }
    pub(super) async fn authorize_locked(
        tx: &mut Transaction<'_, Postgres>,
        session: &Session,
        permission: Permission,
    ) -> Result<Session> {
        let id = session.require(permission)?;
        let current: Option<Session> = sqlx::query_as("SELECT s.token_hash,s.csrf,s.administrator_id,a.role,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=a.role ORDER BY permission) AS permissions,coalesce(a.display_name,'') AS display_name,a.email,a.avatar_key IS NOT NULL AS has_avatar,coalesce(a.avatar_version,0) AS avatar_version,a.locale FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE s.token_hash=$1 AND a.id=$2 AND a.active AND s.expires_at>now() FOR UPDATE OF a,s")
            .bind(&session.token_hash).bind(id).fetch_optional(&mut **tx).await?;
        let actor = current
            .filter(|s| s.allows(permission))
            .ok_or(IdentityError::Forbidden)?;
        Self::attribute_operation(tx, id).await?;
        Ok(actor)
    }
    // Transaction-local attribution cannot leak through a pooled connection.
    pub(super) async fn attribute_operation(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
    ) -> Result<()> {
        sqlx::query("SELECT set_config('talocart.administrator_id',$1,true)")
            .bind(id.to_string())
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub async fn administrators_counted(
        &self,
        session: &Session,
        page: i64,
    ) -> Result<crate::pagination::Page<Administrator>> {
        self.authorize(session, Permission::UsersRead).await?;
        if !(1..=1_000_000).contains(&page) {
            return Err(IdentityError::Invalid);
        }
        Ok(crate::pagination::read(&self.pool, sqlx::query("SELECT a.id,a.email,a.role,r.name AS role_name,a.active FROM administrators a JOIN admin_roles r ON r.key=a.role"), "a.id", page, ADMINISTRATOR_PAGE_SIZE as u16).await?)
    }
    /// Legacy cursor URLs are redirected to the corresponding numbered page.
    pub async fn administrator_cursor_page(&self, session: &Session, after: i64) -> Result<i64> {
        self.authorize(session, Permission::UsersRead).await?;
        if after < 0 {
            return Err(IdentityError::Invalid);
        }
        let preceding: i64 = sqlx::query_scalar("SELECT count(*) FROM administrators WHERE id<=$1")
            .bind(after)
            .fetch_one(&self.pool)
            .await?;
        Ok(preceding / ADMINISTRATOR_PAGE_SIZE as i64 + 1)
    }

    pub async fn administrators(
        &self,
        session: &Session,
        after: i64,
    ) -> Result<Vec<Administrator>> {
        self.authorize(session, Permission::UsersRead).await?;
        if after < 0 {
            return Err(IdentityError::Invalid);
        }
        Ok(sqlx::query_as(
            "SELECT a.id,a.email,a.role,r.name AS role_name,a.active FROM administrators a JOIN admin_roles r ON r.key=a.role WHERE a.id>$1 ORDER BY a.id LIMIT $2",
        )
        .bind(after)
        .bind((ADMINISTRATOR_PAGE_SIZE + 1) as i64)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn administrator(&self, session: &Session, id: i64) -> Result<Administrator> {
        self.authorize(session, Permission::UsersRead).await?;
        sqlx::query_as("SELECT a.id,a.email,a.role,r.name AS role_name,a.active FROM administrators a JOIN admin_roles r ON r.key=a.role WHERE a.id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(IdentityError::NotFound)
    }
    pub async fn create_administrator(
        &self,
        session: &Session,
        csrf: &str,
        address: &str,
        secret: String,
        role: Role,
    ) -> Result<i64> {
        session.check_csrf(csrf)?;
        self.authorize(session, Permission::UsersWrite).await?;
        let address = email(address)?;
        let hash = self.hash(secret).await?;
        let mut tx = self.pool.begin().await?;
        // Same first lock for all role/state mutations, including creation of another owner.
        Self::lock_changes(&mut tx).await?;
        let actor = Self::authorize_locked(&mut tx, session, Permission::UsersWrite).await?;
        let assigned = Self::role_locked(&mut tx, role.as_str()).await?;
        Self::ensure_grant(&actor, &assigned)?;
        let id: Option<i64> = sqlx::query_scalar("INSERT INTO administrators(email,password_hash,role,locale) VALUES ($1,$2,$3,$4) ON CONFLICT(email) DO NOTHING RETURNING id")
            .bind(address).bind(hash).bind(role.as_str()).bind(crate::locale::Locale::DEFAULT_CODE).fetch_optional(&mut *tx).await?;
        let id = id.ok_or(IdentityError::DuplicateEmail)?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn update_administrator(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        role: Role,
        active: bool,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        session.require(Permission::UsersWrite)?;
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let actor = Self::authorize_locked(&mut tx, session, Permission::UsersWrite).await?;
        let current: Administrator = sqlx::query_as(
            "SELECT a.id,a.email,a.role,r.name AS role_name,a.active FROM administrators a JOIN admin_roles r ON r.key=a.role WHERE a.id=$1 FOR UPDATE OF a",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(IdentityError::NotFound)?;
        let previous = Self::role_locked(&mut tx, &current.role).await?;
        let assigned = Self::role_locked(&mut tx, role.as_str()).await?;
        Self::ensure_grant(&actor, &previous)?;
        Self::ensure_grant(&actor, &assigned)?;
        let changed = current.role != role.as_str() || current.active != active;
        if current.role == "owner" && current.active && (role != Role::Owner || !active) {
            let owners: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM administrators WHERE role='owner' AND active",
            )
            .fetch_one(&mut *tx)
            .await?;
            if owners <= 1 {
                return Err(IdentityError::LastOwner);
            }
        }
        if changed {
            sqlx::query("UPDATE administrators SET role=$1,active=$2 WHERE id=$3")
                .bind(role.as_str())
                .bind(active)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM admin_sessions WHERE administrator_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            if !active {
                sqlx::query("DELETE FROM password_resets WHERE administrator_id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
