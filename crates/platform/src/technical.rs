//! Safe infrastructure failures, independent of identity and HTTP presentation.
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("Service temporarily unavailable")]
pub struct DatabaseFailure {
    category: &'static str,
    sqlstate: Option<String>,
}
impl DatabaseFailure {
    /// Never retain or log raw SQLx text: it may contain credentials, SQL or user data.
    /// component is trusted static code context, normally module_path!().
    pub fn capture(error: sqlx::Error, component: &'static str) -> Self {
        let category = match &error {
            sqlx::Error::Database(_) => "database",
            sqlx::Error::PoolTimedOut => "pool_timeout",
            sqlx::Error::PoolClosed => "pool_closed",
            sqlx::Error::Io(_) => "io",
            sqlx::Error::Tls(_) => "tls",
            sqlx::Error::Configuration(_) => "configuration",
            sqlx::Error::Protocol(_) => "protocol",
            sqlx::Error::RowNotFound => "row_missing",
            sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode",
            _ => "query",
        };
        let code = error.as_database_error().and_then(|error| error.code());
        let failure = Self::from_metadata(category, code.as_deref());
        tracing::error!(
            component,
            category = failure.category,
            sqlstate = failure.sqlstate.as_deref().unwrap_or("none"),
            "Database operation failed"
        );
        failure
    }
    /// Only bounded, credential-free metadata may enter a diagnostic.
    pub(crate) fn from_metadata(category: &'static str, code: Option<&str>) -> Self {
        let category = if [
            "database",
            "pool_timeout",
            "pool_closed",
            "io",
            "tls",
            "configuration",
            "protocol",
            "row_missing",
            "decode",
            "query",
        ]
        .contains(&category)
        {
            category
        } else {
            "query"
        };
        let sqlstate = code
            .filter(|code| {
                code.len() == 5
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            })
            .map(str::to_owned);
        Self { category, sqlstate }
    }
    pub fn category(&self) -> &'static str {
        self.category
    }
    pub fn sqlstate(&self) -> Option<&str> {
        self.sqlstate.as_deref()
    }
    pub fn retryable(&self) -> bool {
        matches!(self.sqlstate(), Some("40001" | "40P01"))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TechnicalError {
    #[error(transparent)]
    Database(#[from] DatabaseFailure),
    #[error("Service busy")]
    Busy,
    #[error("Media storage unavailable")]
    Storage,
    #[error("Service temporarily unavailable")]
    Unavailable,
}
impl TechnicalError {
    pub fn retryable(&self) -> bool {
        matches!(self,Self::Database(error) if error.retryable())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum AccessError {
    #[error("Operation not allowed")]
    Forbidden,
    #[error("Invalid credentials or session")]
    Unauthorized,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlx_secrets_never_enter_error_representation() {
        let error = DatabaseFailure::capture(
            sqlx::Error::Protocol("postgres://owner:synthetic_secret@localhost/private".into()),
            module_path!(),
        );
        assert_eq!(error.category(), "protocol");
        assert_eq!(error.sqlstate(), None);
        assert!(!format!("{error:?} {error}").contains("synthetic_secret"));
    }
    #[test]
    fn only_transaction_failures_are_retryable_and_metadata_is_bounded() {
        for (code, expected) in [
            ("40001", true),
            ("40P01", true),
            ("23505", false),
            ("08006", false),
            ("bad://secret", false),
        ] {
            let error = DatabaseFailure::from_metadata("database", Some(code));
            assert_eq!(error.retryable(), expected);
        }
        let error = DatabaseFailure::from_metadata("not-a-category", Some("bad://secret"));
        assert_eq!(error.category(), "query");
        assert_eq!(error.sqlstate(), None);
        assert!(!TechnicalError::Busy.retryable());
    }
    #[test]
    fn identity_errors_preserve_access_and_technical_classification() {
        use crate::identity::IdentityError;
        for access in [AccessError::Forbidden, AccessError::Unauthorized] {
            let error: IdentityError = access.into();
            assert_eq!(error.access(), Some(access));
            assert!(error.technical().is_none());
        }
        let error: IdentityError =
            TechnicalError::Database(DatabaseFailure::from_metadata("database", Some("40001")))
                .into();
        assert!(error.technical().expect("technical cause").retryable());
        assert!(error.access().is_none());
        assert!(IdentityError::LastOwner.technical().is_none());
        assert!(IdentityError::LastOwner.access().is_none());
    }
}
