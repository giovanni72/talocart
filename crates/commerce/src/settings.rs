//! Coordinates the single configuration commit; owner modules retain validation and SQL.
use crate::{
    countries::{CheckoutCountries, CountryError},
    taxes::{Preferences, TaxError, Taxes},
};
use sqlx::PgPool;
use talocart_platform::{
    content::Draft,
    identity::{BrandingKind, Identity, IdentityError, Permission, Session, SettingsInput},
    media::MediaStore,
    regional::{self, RegionalError},
    storefront::{Languages, Settings, Storefront, StorefrontError},
};
use thiserror::Error;

pub struct ModuleInstall {
    pub mode: String,
    pub fallback: talocart_platform::config::PackageInstall,
    pub password: String,
}

pub struct Input {
    pub module_install: Option<ModuleInstall>,
    pub general: SettingsInput,
    pub storefront: Settings,
    pub content: Option<Draft>,
    pub primary: String,
    pub locales: Vec<String>,
    pub time_zone: String,
    pub date_format: String,
    pub regional_revision: i64,
    pub fiscal_revision: i64,
    pub revision: String,
    pub preferences: Preferences,
    pub countries: Vec<String>,
    pub logo: String,
    pub favicon: String,
}
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error(transparent)]
    Identity(IdentityError),
    #[error("Configuration changed")]
    Conflict,
    #[error(transparent)]
    General(IdentityError),
    #[error(transparent)]
    Regional(#[from] RegionalError),
    #[error(transparent)]
    Storefront(#[from] StorefrontError),
    #[error(transparent)]
    Taxes(#[from] TaxError),
    #[error(transparent)]
    Countries(#[from] CountryError),
    #[error("Invalid image selection: {0}")]
    Branding(&'static str, IdentityError),
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}

/// Read failures distinguish technical/access causes from identity domain rules.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<IdentityError> for ReadError {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}
impl From<sqlx::Error> for ReadError {
    fn from(error: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    }
}
impl From<TaxError> for ReadError {
    fn from(error: TaxError) -> Self {
        match error {
            TaxError::Technical(error) => error.into(),
            TaxError::Access(error) => error.into(),
            TaxError::Identity(error) => error.into(),
            _ => Self::Technical(talocart_platform::technical::TechnicalError::Unavailable),
        }
    }
}
impl From<CountryError> for ReadError {
    fn from(error: CountryError) -> Self {
        match error {
            CountryError::Technical(error) => error.into(),
            CountryError::Access(error) => error.into(),
            CountryError::Identity(error) => error.into(),
            _ => Self::Technical(talocart_platform::technical::TechnicalError::Unavailable),
        }
    }
}
impl From<talocart_platform::content::ContentError> for ReadError {
    fn from(error: talocart_platform::content::ContentError) -> Self {
        match error {
            talocart_platform::content::ContentError::Technical(error) => error.into(),
            talocart_platform::content::ContentError::Access(error) => error.into(),
            talocart_platform::content::ContentError::Identity(error) => error.into(),
            _ => Self::Technical(talocart_platform::technical::TechnicalError::Unavailable),
        }
    }
}
/// Owner models and revisions observed while all supported configuration writers
/// are excluded by a shared identity/settings guard. Readers can coexist.
/// No transaction escapes to HTTP.
pub struct Snapshot {
    pub store: talocart_platform::identity::Settings,
    pub theme: Settings,
    pub regional: regional::Settings,
    pub revision: String,
    pub fiscal_revision: i64,
    pub countries: Vec<String>,
    pub preferences: Option<Preferences>,
    pub defaults: Vec<crate::taxes::Entry>,
    pub translations: Vec<talocart_platform::content::Translation>,
    pub branding: talocart_platform::identity::Branding,
    pub logo: Option<talocart_platform::media::ImageMetadata>,
    pub regional_preview: String,
    pub time_zones: Vec<String>,
}

/// Read through owner APIs on one transaction/connection. The identity lock keeps
/// values and revisions coherent; no nested pool checkout can starve its holder.
/// The guard is released before view-model construction and template rendering.
pub async fn snapshot(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
) -> Result<Snapshot, ReadError> {
    let mut guard = pool.begin().await?;
    Identity::authorize_read_transaction(&mut guard, session, Permission::SettingsRead).await?;
    let revision = talocart_platform::settings::revision(&mut guard).await?;
    let store = Identity::settings_in(&mut guard)
        .await?
        .ok_or(IdentityError::Unavailable)?;
    let mut theme = Storefront::settings_in(&mut guard).await?;
    let primary = talocart_platform::locale::Locale::parse(&theme.primary_locale)
        .ok_or(IdentityError::Unavailable)?;
    Storefront::localize_in(
        &mut guard,
        &mut theme,
        talocart_platform::content::Language::new(primary, primary),
    )
    .await?;
    let regional = regional::settings_in(&mut guard, session).await?;
    let fiscal_revision = Taxes::revision_in(&mut guard)
        .await
        .map_err(ReadError::from)?;
    let countries = CheckoutCountries::selected_in(&mut guard, session)
        .await
        .map_err(ReadError::from)?;
    let (preferences, defaults) = if session.allows(Permission::TaxesRead) {
        (
            Some(
                Taxes::settings_in(&mut guard, session)
                    .await
                    .map_err(ReadError::from)?
                    .values(),
            ),
            Taxes::choices_in(&mut guard, session, crate::taxes::Kind::Class)
                .await
                .map_err(ReadError::from)?,
        )
    } else {
        (None, Vec::new())
    };
    let translations = Storefront::CONTENT
        .load_in(&mut guard, session, 1)
        .await
        .map_err(ReadError::from)?;
    let branding = Identity::branding_in(&mut guard)
        .await?
        .ok_or(IdentityError::Unavailable)?;
    let logo = identity
        .branding_metadata_on(&mut guard, BrandingKind::Logo)
        .await?;
    let regional_preview = regional::current_time_in(&mut guard).await?;
    let time_zones = regional::time_zones_in(&mut guard, session).await?;
    guard.rollback().await?;
    Ok(Snapshot {
        store,
        theme,
        regional,
        revision,
        fiscal_revision,
        countries,
        preferences,
        defaults,
        translations,
        branding,
        logo,
        regional_preview,
        time_zones,
    })
}

/// Images are prepared before the transaction. Every database change shares its commit.
/// Lock order: media publication, identity, fiscal/product, destinations, owner rows.
pub async fn save(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
    csrf: &str,
    input: Input,
) -> Result<(), Error> {
    session.check_csrf(csrf)?;
    let platform_write = session.allows(Permission::SettingsWrite);
    let permission = if platform_write {
        Permission::SettingsWrite
    } else {
        Permission::TaxesWrite
    };
    identity.authorize(session, permission).await?;
    let module_change = match &input.module_install {
        Some(policy) => Some(
            talocart_platform::settings::ModuleInstallChange::prepare(
                pool,
                identity,
                session,
                csrf,
                &policy.mode,
                policy.fallback,
                policy.password.clone(),
            )
            .await?,
        ),
        None => None,
    };
    let (logo, favicon, languages) = if platform_write {
        (
            identity
                .prepare_branding_selection(session, BrandingKind::Logo, &input.logo)
                .await
                .map_err(|e| Error::Branding("logo", e))?,
            identity
                .prepare_branding_selection(session, BrandingKind::Favicon, &input.favicon)
                .await
                .map_err(|e| Error::Branding("favicon", e))?,
            Some(Languages::new(&input.primary, &input.locales)?),
        )
    } else {
        (None, None, None)
    };
    let mut tx = pool.begin().await?;
    if platform_write {
        MediaStore::publish_lock(&mut tx)
            .await
            .map_err(IdentityError::from)?;
    }
    Identity::authorize_transaction(&mut tx, session, csrf, permission).await?;
    crate::taxes::check_revision(&mut tx, input.fiscal_revision).await?;
    if talocart_platform::settings::revision(&mut tx).await? != input.revision {
        return Err(Error::Conflict);
    }
    if let Some(change) = module_change {
        change.persist(&mut tx, session, csrf).await?;
    }
    if platform_write {
        Storefront::set_maintenance_in(
            &mut tx,
            session,
            csrf,
            input.storefront.maintenance_enabled,
        )
        .await?;
        identity
            .save_settings_in(&mut tx, session, csrf, &input.general)
            .await
            .map_err(Error::General)?;
        regional::save_in(
            &mut tx,
            session,
            csrf,
            &input.time_zone,
            &input.date_format,
            input.regional_revision,
        )
        .await?;
        Storefront::new(pool.clone())
            .save_in(
                &mut tx,
                session,
                csrf,
                input.storefront,
                input.content.as_ref(),
            )
            .await?;
        if let Some(languages) = languages {
            languages.persist(&mut tx).await?;
        }
    }
    // Only submit the tax area when it was rendered writable. Never grant TaxesWrite
    // through SettingsWrite, including crafted requests.
    if session.allows(Permission::TaxesWrite) {
        Taxes::new(pool.clone(), identity.clone())
            .save_preferences_in(
                &mut tx,
                session,
                csrf,
                &input.preferences,
                input.fiscal_revision,
            )
            .await?;
    }
    if platform_write {
        CheckoutCountries::new(pool.clone(), identity.clone())
            .save_in(&mut tx, session, csrf, &input.countries)
            .await?;
        for prepared in [logo, favicon].into_iter().flatten() {
            identity
                .save_branding_in(&mut tx, session, csrf, prepared)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

impl From<IdentityError> for Error {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}
