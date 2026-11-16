//! Atomic setup of the shop. Password hashing and image preparation precede the transaction.
use crate::demo::{DemoSeeder, SeedError};
use sqlx::PgPool;
use talocart_platform::{
    identity::{Identity, IdentityError, PreparedBootstrap},
    media::MediaStore,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Identity(IdentityError),
    #[error(transparent)]
    Technical(talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Demo(SeedError),
}
impl From<IdentityError> for Error {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else {
            Self::Identity(error)
        }
    }
}
impl From<SeedError> for Error {
    fn from(error: SeedError) -> Self {
        match error {
            SeedError::Identity(error) => Self::from(error),
            error => Self::Demo(error),
        }
    }
}

/// Bootstrap remains single-use under concurrency. All owner writes share one commit.
/// Lock order is media publication (with demo), identity, then fiscal defaults.
pub async fn install(
    pool: &PgPool,
    identity: PreparedBootstrap,
    demo: Option<&DemoSeeder>,
) -> Result<(), Error> {
    let prepared = match demo {
        Some(demo) => Some(demo.prepare().await?),
        None => None,
    };
    let mut tx = pool.begin().await.map_err(|error| {
        Error::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    })?;
    if prepared.is_some() {
        MediaStore::publish_lock(&mut tx)
            .await
            .map_err(SeedError::from)?;
    }
    Identity::bootstrap_transaction(&mut tx, identity).await?;
    crate::taxes::Taxes::initialize_defaults(&mut tx)
        .await
        .map_err(SeedError::from)?;
    if let Some((demo, prepared)) = demo.zip(prepared) {
        demo.install_in_transaction(&mut tx, prepared).await?;
    }
    tx.commit().await.map_err(|error| {
        Error::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    })?;
    Ok(())
}
