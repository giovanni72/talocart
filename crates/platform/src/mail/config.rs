use crate::config::ConfigError;
use lettre::{AsyncSmtpTransport, Tokio1Executor, transport::smtp::authentication::Credentials};
use serde::Deserialize;
use std::time::Duration;

// No Debug: credentials must never enter diagnostic output or HTML.
#[derive(Clone)]
pub struct SmtpConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub password: String,
    pub timeout_seconds: u64,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    Starttls,
    Tls,
    Local,
}
impl Security {
    pub fn key(self) -> &'static str {
        match self {
            Self::Starttls => "starttls",
            Self::Tls => "tls",
            Self::Local => "local",
        }
    }
}
impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".into(),
            port: 1025,
            security: Security::Local,
            username: String::new(),
            password: String::new(),
            timeout_seconds: 10,
        }
    }
}
impl SmtpConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.host.is_empty()
            || self.host.len() > 253
            || self
                .host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || "/@".contains(c))
            || self.port == 0
            || !(1..=20).contains(&self.timeout_seconds)
            || self.username.is_empty() != self.password.is_empty()
            || self.username.len() > 320
            || self.password.len() > 4096
        {
            return Err(ConfigError::Invalid("mail"));
        }
        // Cleartext is confined to numeric loopback, without authentication or DNS rebinding.
        if self.security == Security::Local
            && (!matches!(self.host.parse::<std::net::IpAddr>(), Ok(ip) if ip.is_loopback())
                || !self.username.is_empty())
        {
            return Err(ConfigError::Invalid("mail.security"));
        }
        Ok(())
    }
    pub(super) fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, super::MailError> {
        self.validate()
            .map_err(|_| super::MailError::Configuration)?;
        let builder = match self.security {
            Security::Local => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.host),
            Security::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
                .map_err(|_| super::MailError::Configuration)?,
            Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&self.host)
                .map_err(|_| super::MailError::Configuration)?,
        }
        .port(self.port)
        .timeout(Some(Duration::from_secs(self.timeout_seconds)));
        let builder = if self.username.is_empty() {
            builder
        } else {
            builder.credentials(Credentials::new(
                self.username.clone(),
                self.password.clone(),
            ))
        };
        Ok(builder.build())
    }
    /// Connection, TLS and authentication only; no MAIL/RCPT/DATA and no message.
    pub async fn check_connection(&self) -> Result<(), super::MailError> {
        if !self.enabled {
            return Err(super::MailError::Disabled);
        }
        let smtp = self.transport()?;
        match tokio::time::timeout(
            Duration::from_secs(self.timeout_seconds),
            smtp.test_connection(),
        )
        .await
        {
            Ok(Ok(true)) => Ok(()),
            _ => Err(super::MailError::Connection),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smtp_rejects_cleartext_remote_auth_and_invalid_limits() {
        let mut c = SmtpConfig::default();
        assert!(c.validate().is_ok());
        for host in [
            "localhost",
            "smtp.example.test",
            "127.0.0.1@evil",
            "host\r\n",
        ] {
            c.host = host.into();
            assert!(c.validate().is_err());
        }
        c.host = "127.0.0.1".into();
        c.username = "u".into();
        c.password = "secret".into();
        assert!(c.validate().is_err());
        c.security = Security::Starttls;
        assert!(c.validate().is_ok());
        c.timeout_seconds = 121;
        assert!(c.validate().is_err());
        c.timeout_seconds = 10;
        c.password.clear();
        assert!(c.validate().is_err());
    }
}
