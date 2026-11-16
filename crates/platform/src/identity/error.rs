use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("Choose an available language")]
    InvalidLocale,
    #[error("Invalid data: check email, name and password (12 to 128 characters)")]
    Invalid,
    #[error("Check the highlighted store settings fields")]
    InvalidSettings(Vec<super::SettingsField>),
    #[error("The current password is incorrect")]
    CurrentPassword,
    #[error("Choose a valid static PNG, JPEG or WebP image, up to 2 MiB and 4096 × 4096 pixels")]
    InvalidAvatar,
    #[error("Choose a valid static PNG, JPEG or WebP image, up to 10 MiB and 8192 × 8192 pixels")]
    InvalidBranding,
    #[error("Media storage temporarily unavailable")]
    Storage,
    #[error("The new passwords do not match")]
    PasswordMismatch,
    #[error("Invalid, expired or already used credentials or token")]
    Unauthorized,
    #[error("Operation not allowed")]
    Forbidden,
    #[error("Installation already completed")]
    Installed,
    #[error(
        "At least one active owner must remain. Assign the role to another administrator first"
    )]
    LastOwner,
    #[error("An administrator with this email already exists")]
    DuplicateEmail,
    #[error("Administrator not found")]
    NotFound,
    #[error("The Owner role is protected and cannot be edited or deleted")]
    ProtectedRole,
    #[error("This role is assigned to administrators. Reassign all accounts before deleting it")]
    RoleInUse,
    #[error("A role with this name already exists")]
    DuplicateRole,
    #[error("Role not found")]
    RoleNotFound,
    #[error("The limit of 100 roles has been reached")]
    RoleLimit,
    #[error("Too many attempts. Retry after the 15-minute window")]
    Limited,
    #[error("Service busy. Retry in a few seconds")]
    Busy,
    #[error("Service temporarily unavailable")]
    Unavailable,
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
}
impl From<sqlx::Error> for IdentityError {
    fn from(error: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(error, module_path!()).into())
    }
}
impl IdentityError {
    pub fn technical(&self) -> Option<crate::technical::TechnicalError> {
        use crate::technical::TechnicalError;
        match self {
            Self::Technical(error) => Some(error.clone()),
            Self::Busy => Some(TechnicalError::Busy),
            Self::Storage => Some(TechnicalError::Storage),
            Self::Unavailable => Some(TechnicalError::Unavailable),
            _ => None,
        }
    }
    pub fn access(&self) -> Option<crate::technical::AccessError> {
        use crate::technical::AccessError;
        match self {
            Self::Forbidden => Some(AccessError::Forbidden),
            Self::Unauthorized => Some(AccessError::Unauthorized),
            _ => None,
        }
    }
}
impl From<crate::technical::AccessError> for IdentityError {
    fn from(error: crate::technical::AccessError) -> Self {
        match error {
            crate::technical::AccessError::Forbidden => Self::Forbidden,
            crate::technical::AccessError::Unauthorized => Self::Unauthorized,
        }
    }
}

impl From<crate::media::MediaError> for IdentityError {
    fn from(error: crate::media::MediaError) -> Self {
        match error {
            crate::media::MediaError::Invalid => Self::InvalidBranding,
            crate::media::MediaError::Busy => Self::Busy,
            _ => Self::Storage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_preserves_category_without_secret_bearing_text() {
        let error = IdentityError::from(sqlx::Error::Protocol(
            "postgres://owner:secret_password@localhost/private".into(),
        ));
        assert!(matches!(
            error,
            IdentityError::Technical(crate::technical::TechnicalError::Database(_))
        ));
        assert!(!format!("{error:?} {error}").contains("secret_password"));
        assert_eq!(error.to_string(), "Service temporarily unavailable");
        assert!(matches!(
            IdentityError::from(sqlx::Error::PoolClosed),
            IdentityError::Technical(crate::technical::TechnicalError::Database(_))
        ));
    }
}
