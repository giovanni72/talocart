//! Persisted SMTP settings. Public views contain only a password-presence flag.
use super::*;

#[derive(Clone, FromRow)]
pub struct TransportSettings {
    pub revision: i64,
    pub enabled: bool,
    pub host: String,
    pub port: i32,
    pub security: String,
    pub username: String,
    pub has_password: bool,
    pub timeout_seconds: i32,
}
impl TransportSettings {
    fn config(&self, password: String) -> Result<SmtpConfig> {
        let config = SmtpConfig {
            enabled: self.enabled,
            host: self.host.trim().into(),
            port: self.port.try_into().map_err(|_| MailError::Invalid)?,
            security: match self.security.as_str() {
                "starttls" => Security::Starttls,
                "tls" => Security::Tls,
                "local" => Security::Local,
                _ => return Err(MailError::Invalid),
            },
            username: self.username.clone(),
            password,
            timeout_seconds: self
                .timeout_seconds
                .try_into()
                .map_err(|_| MailError::Invalid)?,
        };
        config.validate().map_err(|_| MailError::Invalid)?;
        if config.security == Security::Local
            && !matches!(config.host.as_str(), "127.0.0.1" | "::1")
        {
            return Err(MailError::Invalid);
        }
        Ok(config)
    }
}
const VIEW: &str = "SELECT revision,enabled,host,port,security,username,octet_length(password_ciphertext)>0 AS has_password,timeout_seconds FROM email_transport WHERE id=1 FOR SHARE";
impl Mail {
    pub async fn transport_settings(&self, session: &Session) -> Result<TransportSettings> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_read_transaction(&mut tx, session, Permission::SettingsRead).await?;
        let settings = sqlx::query_as(VIEW).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(settings)
    }
    pub async fn save_transport(
        &self,
        session: &Session,
        csrf: &str,
        settings: &TransportSettings,
        password: &str,
        clear_password: bool,
        key: &MailKey,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        let (revision, old): (i64, Vec<u8>) = sqlx::query_as(
            "SELECT revision,password_ciphertext FROM email_transport WHERE id=1 FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if revision != settings.revision {
            return Err(MailError::Stale);
        }
        if clear_password && !password.is_empty() {
            return Err(MailError::Invalid);
        }
        let plain = if clear_password {
            String::new()
        } else if !password.is_empty() {
            password.into()
        } else if old.is_empty() {
            String::new()
        } else {
            key.decrypt(&old)?
        };
        let config = settings.config(plain)?;
        let encrypted = if config.password.is_empty() {
            vec![]
        } else {
            key.encrypt(&config.password)?
        };
        sqlx::query("UPDATE email_transport SET revision=revision+1,enabled=$1,host=$2,port=$3,security=$4,username=$5,password_ciphertext=$6,timeout_seconds=$7 WHERE id=1")
            .bind(config.enabled).bind(config.host).bind(i32::from(config.port)).bind(config.security.key()).bind(config.username).bind(encrypted).bind(settings.timeout_seconds).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Snapshot for one operation. No transaction remains open during SMTP I/O.
    pub async fn smtp_config(&self, key: &MailKey) -> Result<SmtpConfig> {
        let (settings, password): (TransportSettings, Vec<u8>) = {
            let mut tx = self.pool.begin().await?;
            let settings = sqlx::query_as(VIEW).fetch_one(&mut *tx).await?;
            let password: Vec<u8> =
                sqlx::query_scalar("SELECT password_ciphertext FROM email_transport WHERE id=1")
                    .fetch_one(&mut *tx)
                    .await?;
            tx.commit().await?;
            (settings, password)
        };
        let plain = if password.is_empty() {
            String::new()
        } else {
            key.decrypt(&password)?
        };
        settings.config(plain)
    }
    pub async fn has_smtp_password(pool: &PgPool) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT octet_length(password_ciphertext)>0 FROM email_transport WHERE id=1",
        )
        .fetch_one(pool)
        .await?)
    }
    pub async fn smtp_enabled(tx: &mut Transaction<'_, Postgres>) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT enabled FROM email_transport WHERE id=1 FOR SHARE")
                .fetch_one(&mut **tx)
                .await?,
        )
    }
    pub async fn process_pending(&self, key: &MailKey) -> Result<bool> {
        self.process_one(&self.smtp_config(key).await?).await
    }
}
