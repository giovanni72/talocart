//! Shared host checks. No migrations, worker startup or external connectivity.
use std::{path::Path, sync::Arc};
use talocart_platform::{
    database::PgPool,
    identity::Identity,
    mail::{Mail, MailError, MailKey},
};

/// Verification serving intentionally omits the online-provider requirement.
pub(crate) async fn database(
    pool: &PgPool,
    provider: Option<Vec<String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::schema::check_schema(pool).await?;
    talocart_commerce::orders::check_list_customers(pool).await?;
    talocart_commerce::orders::check_current_status(pool).await?;
    if let Some(present) = provider {
        talocart_commerce::orders::OnlinePayments::require_providers(pool, &present).await?;
    }
    Ok(())
}

pub(crate) enum MailPolicy {
    /// Doctor validates encrypted credentials without creating files.
    Inspect,
    /// Serving prepares a key only when no stored password needs the old key.
    Prepare,
}

pub(crate) async fn mail_key(
    pool: &PgPool,
    media_root: &Path,
    policy: MailPolicy,
) -> Result<Option<Arc<MailKey>>, MailError> {
    let password = Mail::has_smtp_password(pool).await?;
    let prepare = matches!(policy, MailPolicy::Prepare);
    if !prepare && !password {
        return Ok(None);
    }
    let key = MailKey::load(MailKey::directory(media_root), prepare && !password).await?;
    Mail::new(pool.clone(), Identity::new(pool.clone()))
        .smtp_config(&key)
        .await?;
    Ok(Some(key))
}
