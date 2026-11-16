//! Revision of the platform-owned settings editor. Row versions also detect ABA writes.
use crate::identity::IdentityError;
use sqlx::{Postgres, Transaction};

/// Read under the identity mutation lock. Includes translations and language policy;
/// the commerce coordinator adds its fiscal revision before comparing submissions.
pub async fn revision(tx: &mut Transaction<'_, Postgres>) -> Result<String, IdentityError> {
    let snapshot: String = sqlx::query_scalar(
        "SELECT jsonb_build_array(
          (SELECT jsonb_build_array(xmin::text,to_jsonb(s)) FROM store_settings s WHERE id=1),
          (SELECT jsonb_build_array(xmin::text,to_jsonb(s)) FROM storefront_settings s WHERE id=1),
          (SELECT jsonb_agg(jsonb_build_array(xmin::text,to_jsonb(t)) ORDER BY locale) FROM storefront_settings_translations t WHERE entity_id=1),
          (SELECT jsonb_agg(jsonb_build_array(xmin::text,code) ORDER BY code) FROM storefront_languages),
          (SELECT jsonb_build_array(xmin::text,to_jsonb(r)) FROM platform_regional_settings r WHERE singleton)
        )::text",
    ).fetch_one(&mut **tx).await?;
    Ok(crate::identity::digest(&snapshot))
}

/// The database choice overrides the installation default from the server file.
pub async fn module_install(
    pool: &sqlx::PgPool,
    fallback: crate::config::PackageInstall,
) -> Result<crate::config::PackageInstall, IdentityError> {
    let mode: Option<String> =
        sqlx::query_scalar("SELECT module_install_mode FROM store_settings WHERE id=1")
            .fetch_optional(pool)
            .await?
            .flatten();
    match mode.as_deref() {
        Some("panel") => Ok(crate::config::PackageInstall::Panel),
        Some("cli") => Ok(crate::config::PackageInstall::Cli),
        None => Ok(fallback),
        _ => Err(IdentityError::Unavailable),
    }
}

/// Password proof is prepared outside the settings transaction and cannot be fabricated by callers.
pub struct ModuleInstallChange {
    mode: String,
    fallback: crate::config::PackageInstall,
    verified: bool,
    actor: String,
}
impl ModuleInstallChange {
    pub async fn prepare(
        pool: &sqlx::PgPool,
        identity: &crate::identity::Identity,
        session: &crate::identity::Session,
        csrf: &str,
        mode: &str,
        fallback: crate::config::PackageInstall,
        password: String,
    ) -> Result<Self, IdentityError> {
        identity.authorize_owner(session).await?;
        session.check_csrf(csrf)?;
        if !matches!(mode, "cli" | "panel") {
            return Err(IdentityError::Invalid);
        }
        let current = module_install(pool, fallback).await?;
        let verified = (current == crate::config::PackageInstall::Panel) != (mode == "panel");
        if verified {
            identity
                .reauthenticate_owner(session, csrf, password)
                .await?;
        }
        Ok(Self {
            mode: mode.into(),
            fallback,
            verified,
            actor: session.token_hash.clone(),
        })
    }
    pub async fn persist(
        self,
        tx: &mut Transaction<'_, Postgres>,
        session: &crate::identity::Session,
        csrf: &str,
    ) -> Result<(), IdentityError> {
        crate::identity::Identity::authorize_transaction(
            tx,
            session,
            csrf,
            crate::identity::Permission::SettingsWrite,
        )
        .await?;
        if self.actor != session.token_hash {
            return Err(IdentityError::Forbidden);
        }
        let owner: bool = sqlx::query_scalar("SELECT role='owner' FROM administrators WHERE id=$1")
            .bind(session.administrator_id)
            .fetch_one(&mut **tx)
            .await?;
        if !owner {
            return Err(IdentityError::Forbidden);
        }
        let current: Option<String> =
            sqlx::query_scalar("SELECT module_install_mode FROM store_settings WHERE id=1")
                .fetch_one(&mut **tx)
                .await?;
        let panel = current
            .as_deref()
            .map_or(self.fallback == crate::config::PackageInstall::Panel, |v| {
                v == "panel"
            });
        if panel != (self.mode == "panel") && !self.verified {
            return Err(IdentityError::Forbidden);
        }
        sqlx::query("UPDATE store_settings SET module_install_mode=$1 WHERE id=1")
            .bind(self.mode)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
}
