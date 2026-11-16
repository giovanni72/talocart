//! Permitted purchase destinations; independent from fiscal zones and catalog visibility.
use sqlx::{PgPool, Postgres, Transaction};
use talocart_platform::{
    geography,
    identity::{Identity, IdentityError, Permission, Session},
};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum CountryError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Scegli nazioni valide senza duplicati")]
    Invalid,
    #[error("Configurazione modificata. Ricarica la pagina prima di salvare")]
    Stale,
    #[error("Acquisti non abilitati per questa nazione")]
    NotEnabled,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for CountryError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
pub type Result<T> = std::result::Result<T, CountryError>;
#[derive(Clone)]
pub struct CheckoutCountries {
    pool: PgPool,
    identity: Identity,
}
impl CheckoutCountries {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn revision(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT revision FROM tax_configuration WHERE id=1")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn selected(&self, s: &Session) -> Result<Vec<String>> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        self.destinations().await
    }
    /// Authorized destinations on the caller transaction.
    pub async fn selected_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
    ) -> Result<Vec<String>> {
        Identity::authorize_read_transaction(tx, session, Permission::SettingsRead).await?;
        Self::destinations_in(tx).await
    }
    /// Bounded public country choices for the future checkout, without administrator permissions.
    pub async fn destinations(&self) -> Result<Vec<String>> {
        Self::read_destinations(&self.pool).await
    }
    async fn read_destinations<'e, E>(executor: E) -> Result<Vec<String>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Ok(sqlx::query_scalar(
            "SELECT country_code FROM checkout_countries ORDER BY country_code LIMIT 249",
        )
        .fetch_all(executor)
        .await?)
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn destinations_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Vec<String>> {
        Self::read_destinations(&mut **tx).await
    }
    /// Replaces the selected destinations atomically; empty selection intentionally blocks purchase.
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        countries: &[String],
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        crate::taxes::check_revision(&mut tx, revision)
            .await
            .map_err(|e| match e {
                crate::taxes::TaxError::Stale => CountryError::Stale,
                crate::taxes::TaxError::Technical(e) => e.into(),
                crate::taxes::TaxError::Access(e) => e.into(),
                crate::taxes::TaxError::Identity(e) => CountryError::Identity(e),
                _ => CountryError::Invalid,
            })?;
        self.save_in(&mut tx, s, csrf, countries).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Caller must check the shared fiscal revision before any fiscal writes.
    pub async fn save_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        s: &Session,
        csrf: &str,
        countries: &[String],
    ) -> Result<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let countries = validate(countries)?;
        Identity::authorize_transaction(tx, s, csrf, Permission::SettingsWrite).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(5426645964561798230)")
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM checkout_countries WHERE NOT (country_code=ANY($1::text[]))")
            .bind(&countries)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO checkout_countries(country_code) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING")
            .bind(countries)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    /// Future checkout must call this inside its order transaction before committing.
    /// The shared guard prevents a destination removal until this transaction ends.
    pub async fn require_destination(
        tx: &mut Transaction<'_, Postgres>,
        country: &str,
    ) -> Result<()> {
        if !geography::country_valid(country) {
            return Err(CountryError::Invalid);
        }
        sqlx::query("SELECT pg_advisory_xact_lock_shared(5426645964561798230)")
            .execute(&mut **tx)
            .await?;
        let enabled: Option<String> = sqlx::query_scalar(
            "SELECT country_code FROM checkout_countries WHERE country_code=$1 FOR KEY SHARE",
        )
        .bind(country)
        .fetch_optional(&mut **tx)
        .await?;
        if enabled.is_some() {
            Ok(())
        } else {
            Err(CountryError::NotEnabled)
        }
    }
    /// Read-only readiness/probe boundary; order creation uses require_destination instead.
    pub async fn check_destination(&self, country: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::require_destination(&mut tx, country).await?;
        tx.commit().await?;
        Ok(())
    }
}
fn validate(countries: &[String]) -> Result<Vec<String>> {
    if countries.len() > geography::COUNTRIES.len() {
        return Err(CountryError::Invalid);
    }
    let mut unique = std::collections::BTreeSet::new();
    for code in countries {
        if !geography::country_valid(code) || !unique.insert(code.clone()) {
            return Err(CountryError::Invalid);
        }
    }
    Ok(unique.into_iter().collect())
}
impl From<IdentityError> for CountryError {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_destinations_are_explicit_bounded_unique_and_allow_empty() {
        assert_eq!(validate(&[]).unwrap(), Vec::<String>::new());
        assert_eq!(validate(&["US".into(), "IT".into()]).unwrap(), ["IT", "US"]);
        for values in [
            vec!["IT".into(), "IT".into()],
            vec!["ZZ".into()],
            vec!["it".into()],
            vec!["US".into(); 250],
        ] {
            assert!(validate(&values).is_err());
        }
    }
}
