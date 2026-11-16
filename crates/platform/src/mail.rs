//! PostgreSQL outbox. Enqueue in the caller's transaction; SMTP runs after claim commit.
//! Idempotent enqueue does not imply exactly-once SMTP delivery after a lost acknowledgement.
mod config;
mod health;
mod secrets;
mod transport;
pub use secrets::MailKey;
pub use transport::TransportSettings;
mod worker;
use crate::identity::{Identity, IdentityError, Permission, Session};
pub use config::{Security, SmtpConfig};
use lettre::message::{Mailbox, MultiPart};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error("Invalid email configuration or message")]
    Invalid,
    #[error("Email configuration changed")]
    Stale,
    #[error("Email record not found or not retryable")]
    NotFound,
    #[error("Conflicting email event")]
    Conflict,
    #[error("Email transport configuration invalid")]
    Configuration,
    #[error("Email encryption key unavailable or invalid")]
    Key,
    #[error("Email transport disabled")]
    Disabled,
    #[error("SMTP connection check failed")]
    Connection,
    #[error(transparent)]
    Identity(#[from] IdentityError),
}
impl From<sqlx::Error> for MailError {
    fn from(error: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(error, module_path!()).into())
    }
}
impl From<MailError> for IdentityError {
    fn from(error: MailError) -> Self {
        match error {
            MailError::Technical(cause) => cause.into(),
            MailError::Identity(cause) => cause,
            _ => Self::Unavailable,
        }
    }
}
pub type Result<T> = std::result::Result<T, MailError>;

#[derive(Clone, Default, FromRow)]
pub struct Settings {
    pub revision: i64,
    pub enabled: bool,
    pub from_name: String,
    pub from_email: String,
    pub reply_to: String,
    pub admin_recipients: Vec<String>,
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.from_name.trim().is_empty()
            || self.from_name.len() > 120
            || self.from_name.chars().any(char::is_control)
        {
            return Err(MailError::Invalid);
        }
        address(&self.from_email)?;
        if !self.reply_to.is_empty() {
            address(&self.reply_to)?;
        }
        if self.admin_recipients.is_empty() || self.admin_recipients.len() > 10 {
            return Err(MailError::Invalid);
        }
        let mut unique = std::collections::HashSet::new();
        for recipient in &self.admin_recipients {
            if !unique.insert(address(recipient)?) {
                return Err(MailError::Invalid);
            }
        }
        Ok(())
    }
}
pub fn address(input: &str) -> Result<String> {
    let normalized = input.trim().to_ascii_lowercase();
    if input.chars().any(char::is_control)
        || normalized.len() > 254
        || normalized.chars().any(char::is_control)
        || !normalized.contains('@')
    {
        return Err(MailError::Invalid);
    }
    let parsed: Mailbox = normalized.parse().map_err(|_| MailError::Invalid)?;
    // Only a bare address is accepted, never display names or multiple recipients.
    if parsed.name.is_some() || parsed.email.to_string() != normalized {
        return Err(MailError::Invalid);
    }
    Ok(normalized)
}
// Bounds include 200 maximum-length rows and worst-case HTML escaping.
pub const MAX_TEXT_BYTES: usize = 786_432;
pub const MAX_HTML_BYTES: usize = 8_388_608;
pub struct Content {
    pub locale: String,
    pub template: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}
/// Event mood of a message: colours the frame bar and status chip.
#[derive(Clone, Copy)]
pub enum Tone {
    Neutral,
    Positive,
    Alert,
}
/// Shared visual frame of every HTML email (`templates/mail-layout.html`).
/// Senders build it from trusted, already validated values; Askama escapes them.
pub struct Frame<'a> {
    pub locale: &'a str,
    pub heading: String,
    pub preheader: &'a str,
    pub brand: &'a str,
    pub initial: String,
    pub test: Option<&'a str>,
    pub footer: &'a str,
    pub ink: &'static str,
    pub surface: &'static str,
}
impl<'a> Frame<'a> {
    pub fn new(locale: &'a str, brand: &'a str, tone: Tone) -> Self {
        let (ink, surface) = match tone {
            Tone::Neutral => ("#243e32", "#e7eec6"),
            Tone::Positive => ("#345a2c", "#edf4e7"),
            Tone::Alert => ("#882f25", "#fff0ec"),
        };
        Self {
            locale,
            heading: String::new(),
            preheader: "",
            brand,
            initial: brand
                .trim()
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default(),
            test: None,
            footer: "",
            ink,
            surface,
        }
    }
}
impl Content {
    fn validate(&self) -> Result<()> {
        if !["it", "en"].contains(&self.locale.as_str())
            || ![
                "order_received",
                "order_updated",
                "order_admin",
                "payment_received",
                "payment_refunded",
                "order_shipped",
                "order_delivered",
                "order_tracking",
                "order_cancelled",
                "cancellation_admin",
                "customer_confirmation",
            ]
            .contains(&self.template.as_str())
            || self.subject.is_empty()
            || self.subject.chars().count() > 300
            || self.subject.chars().any(char::is_control)
            || self.text.is_empty()
            || self.text.len() > MAX_TEXT_BYTES
            || self.html.is_empty()
            || self.html.len() > MAX_HTML_BYTES
        {
            return Err(MailError::Invalid);
        }
        Ok(())
    }
}
#[derive(FromRow)]
pub struct Entry {
    pub id: i64,
    pub recipient: String,
    pub locale: String,
    pub template: String,
    pub subject: String,
    pub state: String,
    pub attempts: i32,
    pub created_at: String,
    pub accepted_at: String,
    pub last_outcome: String,
}
#[derive(FromRow)]
pub struct Attempt {
    pub attempt: i32,
    pub started_at: String,
    pub finished_at: String,
    pub outcome: String,
}
#[derive(Clone)]
pub struct Mail {
    pool: PgPool,
    identity: Identity,
}
impl Mail {
    /// A replacement token/activation makes older unsent links obsolete.
    pub async fn supersede_customer_confirmation(
        tx: &mut Transaction<'_, Postgres>,
        customer: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE email_outbox SET superseded=true,state=CASE WHEN state IN ('pending','failed') THEN 'superseded' ELSE state END WHERE template='customer_confirmation' AND event_key LIKE $1 AND state<>'accepted'")
            .bind(format!("customer-confirm:{customer}:%")).execute(&mut **tx).await?;
        Ok(())
    }
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn settings(&self, session: &Session) -> Result<Settings> {
        self.identity
            .authorize(session, Permission::SettingsRead)
            .await?;
        Ok(sqlx::query_as("SELECT revision,enabled,from_name,from_email,reply_to,admin_recipients FROM email_configuration WHERE id=1").fetch_one(&self.pool).await?)
    }
    pub async fn save(&self, session: &Session, csrf: &str, settings: &Settings) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        settings.validate()?;
        let admins = settings
            .admin_recipients
            .iter()
            .map(|r| address(r))
            .collect::<Result<Vec<_>>>()?;
        let changed=sqlx::query("UPDATE email_configuration SET revision=revision+1,enabled=$2,from_name=$3,from_email=$4,reply_to=$5,admin_recipients=$6 WHERE id=1 AND revision=$1")
            .bind(settings.revision).bind(settings.enabled).bind(settings.from_name.trim()).bind(address(&settings.from_email)?).bind(if settings.reply_to.trim().is_empty() { String::new() } else { address(&settings.reply_to)? }).bind(admins).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(MailError::Stale);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Snapshot sender configuration in the same transaction as the business event.
    /// An event_key is immutable and scoped to one notification kind/audience.
    /// Returns the existing id on a matching replay, rejects a changed payload.
    pub async fn enqueue(
        tx: &mut Transaction<'_, Postgres>,
        event_key: &str,
        recipient: &str,
        content: &Content,
    ) -> Result<i64> {
        content.validate()?;
        if event_key.is_empty() || event_key.len() > 200 || event_key.chars().any(char::is_control)
        {
            return Err(MailError::Invalid);
        }
        let recipient = address(recipient)?;
        let settings = Self::configuration(tx).await?;
        let mut hash = Sha256::new();
        for part in [
            &content.locale,
            &content.template,
            &content.subject,
            &content.text,
            &content.html,
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        let hash = hash.finalize().to_vec();
        let message_id = format!("<{}@talocart.invalid>", token()?);
        let id: Option<i64>=sqlx::query_scalar("INSERT INTO email_outbox(event_key,recipient,locale,template,subject,body_text,body_html,from_name,from_email,reply_to,message_id,content_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(event_key,recipient) DO NOTHING RETURNING id")
            .bind(event_key).bind(&recipient).bind(&content.locale).bind(&content.template).bind(&content.subject).bind(&content.text).bind(&content.html).bind(&settings.from_name).bind(&settings.from_email).bind(&settings.reply_to).bind(message_id).bind(&hash).fetch_optional(&mut **tx).await?;
        if let Some(id) = id {
            return Ok(id);
        }
        let (id, stored): (i64, Vec<u8>) = sqlx::query_as(
            "SELECT id,content_hash FROM email_outbox WHERE event_key=$1 AND recipient=$2",
        )
        .bind(event_key)
        .bind(recipient)
        .fetch_one(&mut **tx)
        .await?;
        if stored != hash {
            return Err(MailError::Conflict);
        }
        Ok(id)
    }
    /// Freeze the audience on first enqueue, including concurrent calls and transaction rollback.
    /// Replays use the original recipients even after the shop configuration changes.
    pub async fn event_audience(
        tx: &mut Transaction<'_, Postgres>,
        event_key: &str,
        recipients: &[String],
    ) -> Result<Vec<String>> {
        if event_key.is_empty()
            || event_key.len() > 200
            || event_key.chars().any(char::is_control)
            || recipients.is_empty()
            || recipients.len() > 10
        {
            return Err(MailError::Invalid);
        }
        let recipients = recipients
            .iter()
            .map(|r| address(r))
            .collect::<Result<Vec<_>>>()?;
        sqlx::query("INSERT INTO email_event_audiences(event_key,recipients) VALUES($1,$2) ON CONFLICT(event_key) DO NOTHING")
            .bind(event_key).bind(&recipients).execute(&mut **tx).await?;
        Ok(
            sqlx::query_scalar("SELECT recipients FROM email_event_audiences WHERE event_key=$1")
                .bind(event_key)
                .fetch_one(&mut **tx)
                .await?,
        )
    }
    /// Sender/admin snapshot for notification fan-out, locked until the caller commits.
    pub async fn configuration(tx: &mut Transaction<'_, Postgres>) -> Result<Settings> {
        Self::notification_configuration(tx)
            .await?
            .ok_or(MailError::Invalid)
    }
    /// Only the empty, disabled initial configuration omits optional notifications.
    /// Configured delivery can be paused without losing messages. Invalid or missing
    /// settings remain errors; explicit email operations use `configuration`.
    pub async fn notification_configuration(
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<Option<Settings>> {
        let settings: Settings = sqlx::query_as("SELECT revision,enabled,from_name,from_email,reply_to,admin_recipients FROM email_configuration WHERE id=1 FOR SHARE").fetch_one(&mut **tx).await?;
        if !settings.enabled
            && settings.from_name.is_empty()
            && settings.from_email.is_empty()
            && settings.reply_to.is_empty()
            && settings.admin_recipients.is_empty()
        {
            return Ok(None);
        }
        settings.validate()?;
        Ok(Some(settings))
    }
    /// Called in the cancellation transaction. Preserve in-flight leases so the
    /// cancellation notice waits for the attempt to finish (or its lease to expire).
    /// Pending/failed messages become terminal; SMTP already in flight cannot be recalled.
    pub async fn supersede_order(tx: &mut Transaction<'_, Postgres>, order: i64) -> Result<()> {
        sqlx::query("UPDATE email_outbox SET superseded=true,state=CASE WHEN state IN ('pending','failed') THEN 'superseded' ELSE state END WHERE order_id=$1 AND template IN ('order_received','order_updated','order_admin') AND state<>'accepted'")
            .bind(order).execute(&mut **tx).await?;
        Ok(())
    }
    /// Payment replaces the customer's unpaid confirmation with a full payment
    /// receipt. Keep the shop's original new-order notification and every attempt.
    pub async fn supersede_payment_request(
        tx: &mut Transaction<'_, Postgres>,
        order: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE email_outbox SET superseded=true,state=CASE WHEN state IN ('pending','failed') THEN 'superseded' ELSE state END WHERE order_id=$1 AND template IN ('order_received','order_updated') AND state<>'accepted'")
            .bind(order).execute(&mut **tx).await?;
        Ok(())
    }
    /// A confirmed refund supersedes unaccepted payment confirmations. Preserve
    /// in-flight leases; the refund notice waits until their attempt terminates.
    pub async fn supersede_payment_confirmation(
        tx: &mut Transaction<'_, Postgres>,
        order: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE email_outbox SET superseded=true,state=CASE WHEN state IN ('pending','failed') THEN 'superseded' ELSE state END WHERE order_id=$1 AND template IN ('order_received','order_updated','payment_received') AND state<>'accepted'").bind(order).execute(&mut **tx).await?;
        Ok(())
    }
    /// Keep the newest fulfillment notice, respecting any sender already in flight.
    pub async fn supersede_fulfillment(
        tx: &mut Transaction<'_, Postgres>,
        order: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE email_outbox SET superseded=true, state=CASE WHEN state IN ('pending','failed') THEN 'superseded' ELSE state END WHERE order_id=$1 AND template IN ('order_shipped','order_tracking','order_delivered') AND state<>'accepted'")
            .bind(order).execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn correlate_order(
        tx: &mut Transaction<'_, Postgres>,
        ids: &[i64],
        order: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE email_outbox SET order_id=$1 WHERE id=ANY($2::bigint[])")
            .bind(order)
            .bind(ids)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub async fn log(
        &self,
        session: &Session,
        page: i64,
    ) -> Result<crate::pagination::Page<Entry>> {
        self.identity
            .authorize(session, Permission::SettingsRead)
            .await?;
        Ok(crate::pagination::read(&self.pool,sqlx::query("SELECT id,recipient,locale,template,subject,state,attempts,platform_datetime(created_at,r.time_zone,r.date_format) AS created_at,COALESCE(platform_datetime(accepted_at,r.time_zone,r.date_format),'') AS accepted_at,last_outcome FROM email_outbox CROSS JOIN platform_regional_settings r WHERE r.singleton"),"id DESC",page,20).await?)
    }
    pub async fn attempts(&self, session: &Session, id: i64) -> Result<Vec<Attempt>> {
        self.identity
            .authorize(session, Permission::SettingsRead)
            .await?;
        Ok(sqlx::query_as("SELECT attempt,platform_datetime(started_at,r.time_zone,r.date_format) AS started_at,COALESCE(platform_datetime(finished_at,r.time_zone,r.date_format),'') AS finished_at,outcome FROM email_attempts CROSS JOIN platform_regional_settings r WHERE r.singleton AND message_id=$1 ORDER BY attempt DESC LIMIT 100").bind(id).fetch_all(&self.pool).await?)
    }
    pub async fn retry(&self, session: &Session, csrf: &str, id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        if sqlx::query("UPDATE email_outbox SET state='pending',cycle_attempts=0,available_at=clock_timestamp() WHERE id=$1 AND state='failed' AND NOT superseded").bind(id).execute(&mut *tx).await?.rows_affected()!=1 { return Err(MailError::NotFound); }
        tx.commit().await?;
        Ok(())
    }
    /// Authorization boundary for external probes; the SMTP call must follow commit.
    pub async fn authorize_probe(&self, session: &Session, csrf: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Explicit synthetic test, idempotent on the browser-generated key. Not a real order.
    pub async fn test_message(
        &self,
        session: &Session,
        csrf: &str,
        key: &str,
        recipient: &str,
        content: &Content,
    ) -> Result<i64> {
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        let id = Self::enqueue(&mut tx, &format!("test:{key}"), recipient, content).await?;
        tx.commit().await?;
        Ok(id)
    }
}
pub fn token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| MailError::Configuration)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
#[derive(FromRow)]
struct Claimed {
    id: i64,
    recipient: String,
    subject: String,
    body_text: String,
    body_html: String,
    from_name: String,
    from_email: String,
    reply_to: String,
    message_id: String,
    lease_token: String,
    attempts: i32,
    cycle_attempts: i16,
}
impl Claimed {
    fn message(&self) -> Result<lettre::Message> {
        let mut builder = lettre::Message::builder()
            .from(Mailbox::new(
                Some(self.from_name.clone()),
                self.from_email.parse().map_err(|_| MailError::Invalid)?,
            ))
            .to(self.recipient.parse().map_err(|_| MailError::Invalid)?)
            .subject(&self.subject)
            .message_id(Some(self.message_id.clone()));
        if !self.reply_to.is_empty() {
            builder = builder.reply_to(self.reply_to.parse().map_err(|_| MailError::Invalid)?);
        }
        builder
            .multipart(MultiPart::alternative_plain_html(
                self.body_text.clone(),
                self.body_html.clone(),
            ))
            .map_err(|_| MailError::Invalid)
    }
}

/// Email owns its operational state; dashboard receives only an authorized count.
pub async fn failed_email_count(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
) -> std::result::Result<i64, IdentityError> {
    identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM email_outbox WHERE state='failed'")
            .fetch_one(pool)
            .await?,
    )
}
