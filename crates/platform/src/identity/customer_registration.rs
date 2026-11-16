//! Public registration policy and single-use activation; never authenticates a link holder.
use super::*;
use crate::mail::{Content, Frame, Mail, Tone};
use askama::Template;

#[derive(Clone, FromRow)]
pub struct RegistrationSettings {
    pub revision: i64,
    pub require_email_confirmation: bool,
    pub allow_guest_checkout: bool,
}
impl Default for RegistrationSettings {
    fn default() -> Self {
        Self {
            revision: 0,
            require_email_confirmation: false,
            allow_guest_checkout: true,
        }
    }
}
/// Trusted application configuration and localized route, never request Host/forwarded headers.
pub struct RegistrationEmail<'a> {
    pub origin: &'a str,
    pub path: &'a str,
    pub locale: &'a str,
}
impl RegistrationSettings {
    pub async fn load(pool: &PgPool) -> Result<Self> {
        Ok(sqlx::query_as("SELECT revision,require_email_confirmation,allow_guest_checkout FROM customer_registration_settings WHERE singleton").fetch_one(pool).await?)
    }
    /// Held through checkout completion so a policy change cannot race a new order.
    pub async fn guests_allowed_in(tx: &mut Transaction<'_, Postgres>) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT allow_guest_checkout FROM customer_registration_settings WHERE singleton FOR SHARE")
            .fetch_one(&mut **tx).await?)
    }
    pub async fn save(
        &self,
        pool: &PgPool,
        session: &Session,
        csrf: &str,
    ) -> std::result::Result<(), CustomerError> {
        let mut tx = pool.begin().await?;
        crate::database::commercial_lock::exclusive(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        if self.require_email_confirmation {
            ready(&mut tx).await?;
        }
        let changed=sqlx::query("UPDATE customer_registration_settings SET require_email_confirmation=$1,allow_guest_checkout=$3,revision=revision+1 WHERE singleton AND revision=$2")
            .bind(self.require_email_confirmation).bind(self.revision).bind(self.allow_guest_checkout).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(CustomerError::Stale);
        }
        tx.commit().await?;
        Ok(())
    }
}
pub(super) async fn required(tx: &mut Transaction<'_, Postgres>) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT require_email_confirmation FROM customer_registration_settings WHERE singleton FOR SHARE").fetch_one(&mut **tx).await?)
}
/// Returns the sender snapshot whose name brands the confirmation email.
async fn ready(tx: &mut Transaction<'_, Postgres>) -> Result<crate::mail::Settings> {
    let settings = Mail::configuration(tx).await.map_err(IdentityError::from)?;
    if !settings.enabled || !Mail::smtp_enabled(tx).await.map_err(IdentityError::from)? {
        return Err(IdentityError::Unavailable);
    }
    Ok(settings)
}
#[derive(serde::Deserialize)]
struct Words {
    subject: String,
    intro: String,
    action: String,
    expires: String,
    ignore: String,
    fallback: String,
}
#[derive(Template)]
#[template(path = "customer-confirm.html")]
struct Html<'a> {
    frame: Frame<'a>,
    words: &'a Words,
    url: &'a str,
}
fn content(delivery: &RegistrationEmail<'_>, raw: &str, brand: &str) -> Result<Content> {
    let mut url = url::Url::parse(delivery.origin).map_err(|_| IdentityError::Invalid)?;
    let loopback = url.host_str().is_some_and(|h| {
        h.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !matches!(
            delivery.path,
            "/account/confirm" | "/it/account/confirm" | "/en/account/confirm"
        )
    {
        return Err(IdentityError::Invalid);
    }
    let source = match delivery.locale {
        "it" => include_str!("../../templates/customer-confirm-it.toml"),
        "en" => include_str!("../../templates/customer-confirm-en.toml"),
        _ => return Err(IdentityError::Invalid),
    };
    let w: Words = toml::from_str(source).map_err(|_| IdentityError::Unavailable)?;
    url.set_path(delivery.path);
    url.query_pairs_mut().append_pair("token", raw);
    let text = format!(
        "{brand}\n\n{}\n{}\n\n{}\n\n{}:\n{url}\n\n{}\n\n--\n{}\n",
        w.subject,
        "=".repeat(60),
        w.intro,
        w.action,
        w.expires,
        w.ignore
    );
    let mut frame = Frame::new(delivery.locale, brand, Tone::Neutral);
    frame.heading = w.subject.clone();
    frame.preheader = &w.intro;
    frame.footer = &w.ignore;
    let html = Html {
        frame,
        words: &w,
        url: url.as_str(),
    }
    .render()
    .map_err(|_| IdentityError::Unavailable)?;
    Ok(Content {
        locale: delivery.locale.into(),
        template: "customer_confirmation".into(),
        subject: w.subject,
        text,
        html,
    })
}
pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    address: &str,
    delivery: &RegistrationEmail<'_>,
) -> Result<()> {
    let settings = ready(tx).await?;
    let raw = token()?;
    let hash = digest(&raw);
    let content = content(delivery, &raw, &settings.from_name)?;
    sqlx::query("INSERT INTO customer_email_confirmations(customer_id,email,token_hash,expires_at) VALUES($1,$2,$3,clock_timestamp()+interval '24 hours') ON CONFLICT(customer_id) DO UPDATE SET email=EXCLUDED.email,token_hash=EXCLUDED.token_hash,expires_at=EXCLUDED.expires_at,last_sent_at=clock_timestamp()")
        .bind(id).bind(address).bind(&hash).execute(&mut **tx).await?;
    Mail::supersede_customer_confirmation(tx, id)
        .await
        .map_err(IdentityError::from)?;
    Mail::enqueue(
        tx,
        &format!("customer-confirm:{id}:{hash}"),
        address,
        &content,
    )
    .await
    .map_err(IdentityError::from)?;
    Ok(())
}
impl Customers {
    pub(super) async fn anonymous_in(
        tx: &mut Transaction<'_, Postgres>,
        session: &CustomerSession,
    ) -> std::result::Result<(), CustomerError> {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM customer_sessions WHERE token_hash=$1 AND customer_id IS NULL AND expires_at>clock_timestamp() FOR SHARE)")
            .bind(&session.token_hash).fetch_one(&mut **tx).await?;
        if !valid {
            return Err(IdentityError::Unauthorized.into());
        }
        Ok(())
    }
    /// Explicit POST after a read-only landing page. Requires a live anonymous
    /// browser session and CSRF, but never grants a session from an email token.
    pub async fn confirm_email(
        &self,
        session: &CustomerSession,
        csrf: &str,
        raw: &str,
    ) -> std::result::Result<(), CustomerError> {
        session.check_csrf(csrf)?;
        if session.customer_id.is_some() {
            return Err(IdentityError::Forbidden.into());
        }
        self.limit_session(session, CustomerAuthAction::Reset)
            .await?;
        if raw.len() != 64 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CustomerError::ConfirmationInvalid);
        }
        let mut tx = self.pool.begin().await?;
        Self::anonymous_in(&mut tx, session).await?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT customer_id FROM customer_email_confirmations WHERE token_hash=$1",
        )
        .bind(digest(raw))
        .fetch_optional(&mut *tx)
        .await?;
        let id = id.ok_or(CustomerError::ConfirmationInvalid)?;
        // Match the account-first order used by administration and resend.
        let account: Option<(String, bool)> =
            sqlx::query_as("SELECT email,active FROM customers WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let (email, active) = account.ok_or(CustomerError::ConfirmationInvalid)?;
        let valid:Option<i64>=sqlx::query_scalar("DELETE FROM customer_email_confirmations WHERE customer_id=$1 AND token_hash=$2 AND email=$3 AND expires_at>clock_timestamp() RETURNING customer_id")
            .bind(id).bind(digest(raw)).bind(email).fetch_optional(&mut *tx).await?;
        if active || valid.is_none() {
            return Err(CustomerError::ConfirmationInvalid);
        }
        sqlx::query("UPDATE customers SET active=true,revision=revision+1 WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        Mail::supersede_customer_confirmation(&mut tx, id)
            .await
            .map_err(IdentityError::from)?;
        tx.commit().await?;
        Ok(())
    }
    /// Generic response for unknown, active, administratively disabled or cooldown
    /// accounts. Only a still-pending registration can issue a replacement token.
    pub async fn resend_confirmation(
        &self,
        session: &CustomerSession,
        csrf: &str,
        address: &str,
        delivery: &RegistrationEmail<'_>,
    ) -> std::result::Result<(), CustomerError> {
        session.check_csrf(csrf)?;
        if session.customer_id.is_some() {
            return Err(IdentityError::Forbidden.into());
        }
        self.limit_session(session, CustomerAuthAction::Reset)
            .await?;
        let address = email(address).map_err(|_| CustomerError::Fields(vec!["email"]))?;
        let address =
            crate::mail::address(&address).map_err(|_| CustomerError::Fields(vec!["email"]))?;
        self.identity
            .limit(&format!("customer-confirm-resend:{}", digest(&address)), 5)
            .await?;
        let mut tx = self.pool.begin().await?;
        Self::anonymous_in(&mut tx, session).await?;
        let id: Option<i64> =
            sqlx::query_scalar("SELECT id FROM customers WHERE email=$1 AND NOT active FOR UPDATE")
                .bind(&address)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(id) = id {
            let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM customer_email_confirmations WHERE customer_id=$1 AND email=$2 AND last_sent_at<=clock_timestamp()-interval '60 seconds')")
                .bind(id).bind(&address).fetch_one(&mut *tx).await?;
            if pending {
                enqueue(&mut tx, id, &address, delivery).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_mail_locales_origin_and_escaping() {
        for locale in ["en", "it"] {
            let delivery = RegistrationEmail {
                origin: "https://shop.example.test",
                path: "/account/confirm",
                locale,
            };
            let mail = content(&delivery, &"a".repeat(64), "<Shop & Co>").unwrap();
            assert!(mail.html.contains(&format!("lang=\"{locale}\"")));
            assert!(mail.html.contains("&#60;Shop &#38; Co&#62;"));
            assert!(!mail.html.contains("<Shop"));
            assert!(mail.text.starts_with("<Shop & Co>\n"));
            assert!(mail.text.contains("24"));
            assert!(
                mail.html
                    .contains("https://shop.example.test/account/confirm?token=")
            );
        }
        for origin in [
            "http://evil.test",
            "https://user:pass@shop.test",
            "https://shop.test/path",
            "https://shop.test?x=1",
        ] {
            assert!(
                content(
                    &RegistrationEmail {
                        origin,
                        path: "/account/confirm",
                        locale: "en",
                    },
                    "token",
                    "Shop"
                )
                .is_err()
            );
        }
    }
}
