//! Fiscal classes, exact price configuration and deterministic country zones.
mod zones;
use crate::pricing::{self, LineTotals, Money, PriceError, PriceMode, Rate};
use sqlx::{FromRow, PgPool};
use talocart_platform::identity::{Identity, IdentityError, Permission, Session};
use thiserror::Error;
pub use zones::{Zone, ZoneValues};
mod coverage;
pub use coverage::Coverage;
pub mod charges;

pub const PAGE_SIZE: usize = 20;
pub const CONFIGURATION_LIMIT: i64 = 100;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rate,
    Class,
}
impl Kind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Rate => "/admin/settings/taxes/rates",
            Self::Class => "/admin/settings/taxes/classes",
        }
    }
    fn table(self) -> &'static str {
        match self {
            Self::Rate => "tax_rates",
            Self::Class => "tax_classes",
        }
    }
    fn select(self) -> &'static str {
        match self {
            Self::Rate => {
                "SELECT r.id,r.name,r.description,r.active,r.basis_points,NULL::bigint AS rate_id,\
                    r.name AS rate_name,r.active AS rate_active \
                    FROM tax_rates r"
            }
            Self::Class => {
                "SELECT r.id,r.name,r.description,r.active,t.basis_points,r.rate_id,t.name AS rate_name,\
                    t.active AS rate_active \
                    FROM tax_classes r \
                    JOIN tax_rates t ON t.id=r.rate_id"
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Description,
    Percentage,
    Rate,
    Mode,
    DefaultClass,
    Amount,
    Quantity,
    Class,
    Currency,
    Country,
    Countries,
}
#[derive(Debug, Error)]
pub enum TaxError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Controlla i campi evidenziati")]
    Invalid(Vec<Field>),
    #[error("Esiste già una voce con questo nome")]
    Duplicate,
    #[error("La voce è collegata ad altre risorse. Rimuovi o modifica prima i collegamenti")]
    InUse,
    #[error("Una classe attiva deve usare un’aliquota attiva")]
    InactiveRate,
    #[error(
        "Questa aliquota è usata da classi attive. Disattivale o cambia prima la loro aliquota"
    )]
    ActiveClasses,
    #[error(
        "Questa è la classe predefinita. Scegli un’altra classe o rimuovi il valore predefinito prima di disattivarla"
    )]
    DefaultInUse,
    #[error("Scegli una classe predefinita attiva con aliquota attiva")]
    InvalidDefault,
    #[error("È stato raggiunto il limite di 100 voci per questa sezione")]
    Limit,
    #[error(
        "La modalità prezzi non può cambiare finché esistono prodotti. È necessaria una conversione esplicita dei prezzi"
    )]
    ProductPrices,
    #[error(
        "La valuta può cambiare solo con catalogo vuoto e senza importazioni prodotti in corso"
    )]
    CurrencyInUse,
    #[error("Una nazione è già assegnata a una zona")]
    CountryInUse,
    #[error("Una zona attiva richiede una classe e un’aliquota attive")]
    InactiveClass,
    #[error("La classe è usata da zone attive")]
    ActiveZones,
    #[error("Configurazione modificata. Ricarica la pagina prima di salvare")]
    Stale,
    #[error("Voce non trovata")]
    NotFound,
    #[error("Il calcolo supera l’importo massimo supportato. Riduci prezzo o quantità")]
    Overflow,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for TaxError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            if db.constraint() == Some("tax_zone_countries_pkey") {
                return Self::CountryInUse;
            }
            if db.is_unique_violation() {
                return Self::Duplicate;
            }
            match db.constraint() {
                Some("store_currency_catalog") => return Self::CurrencyInUse,
                Some("tax_zone_countries_pkey") => return Self::CountryInUse,
                Some("tax_zone_countries_required") => {
                    return Self::Invalid(vec![Field::Countries]);
                }
                Some("tax_zones_active_class") => return Self::InactiveClass,
                Some("tax_classes_active_zones" | "tax_rates_active_zones") => {
                    return Self::ActiveZones;
                }
                Some("tax_zone_rules_class_id_fkey" | "tax_zone_rules_rate_id_fkey") => {
                    return Self::Invalid(vec![Field::Class]);
                }
                Some("pricing_settings_product_prices") => return Self::ProductPrices,
                Some("tax_rates_active_reference") => return Self::ActiveClasses,
                Some("tax_classes_active_rate") => return Self::InactiveRate,
                Some("tax_classes_active_default") => return Self::DefaultInUse,
                Some(
                    "pricing_settings_active_default" | "pricing_settings_default_class_id_fkey",
                ) => return Self::InvalidDefault,
                Some("tax_classes_rate_id_fkey") if db.is_foreign_key_violation() => {
                    return Self::Invalid(vec![Field::Rate]);
                }
                Some("tax_rates_limit" | "tax_classes_limit" | "tax_zones_limit") => {
                    return Self::Limit;
                }
                _ => {}
            }
        }
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    }
}
pub type Result<T> = std::result::Result<T, TaxError>;
#[derive(Clone, Default)]
pub struct Values {
    pub name: String,
    pub description: String,
    pub percentage: String,
    pub rate_id: String,
    pub active: bool,
}
#[derive(Clone, FromRow)]
pub struct Entry {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub active: bool,
    pub basis_points: i32,
    pub rate_id: Option<i64>,
    pub rate_name: String,
    pub rate_active: bool,
}
impl Entry {
    pub fn percentage(&self) -> String {
        format!("{},{:02}", self.basis_points / 100, self.basis_points % 100)
    }
    pub fn values(&self) -> Values {
        Values {
            name: self.name.clone(),
            description: self.description.clone(),
            percentage: self.percentage(),
            rate_id: self.rate_id.map(|n| n.to_string()).unwrap_or_default(),
            active: self.active,
        }
    }
}
#[derive(Clone)]
pub struct Settings {
    pub currency: String,
    pub country: String,
    pub mode: PriceMode,
    pub default_class_id: Option<i64>,
}
#[derive(Clone)]
pub struct Preferences {
    pub currency: String,
    pub country: String,
    pub mode: String,
    pub default_class_id: String,
}
impl Settings {
    pub fn values(&self) -> Preferences {
        Preferences {
            currency: self.currency.clone(),
            country: self.country.clone(),
            mode: self.mode.key().into(),
            default_class_id: self
                .default_class_id
                .map(|n| n.to_string())
                .unwrap_or_default(),
        }
    }
}
#[derive(Clone)]
pub struct Preview {
    pub country: String,
    pub amount: String,
    pub quantity: String,
    pub class_id: String,
}
impl Default for Preview {
    fn default() -> Self {
        Self {
            country: "store".into(),
            amount: String::new(),
            quantity: "1".into(),
            class_id: String::new(),
        }
    }
}
pub struct Quote {
    pub currency: String,
    pub country: String,
    pub zone_name: Option<String>,
    pub totals: LineTotals,
    pub percentage: String,
    pub class_name: String,
    pub mode: PriceMode,
    pub quantity: u32,
}
struct Validated {
    name: String,
    description: String,
    rate: Option<Rate>,
    rate_id: Option<i64>,
}
fn validate(input: &Values, kind: Kind) -> Result<Validated> {
    let name = input.name.trim().to_owned();
    let description = input.description.trim().to_owned();
    let mut errors = vec![];
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        errors.push(Field::Name);
    }
    if description.chars().count() > 500
        || description
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        errors.push(Field::Description);
    }
    let (rate, rate_id) = match kind {
        Kind::Rate => {
            let rate = Rate::parse(&input.percentage).ok();
            if rate.is_none() {
                errors.push(Field::Percentage);
            }
            (rate, None)
        }
        Kind::Class => {
            let id = input.rate_id.parse::<i64>().ok().filter(|n| *n > 0);
            if id.is_none() {
                errors.push(Field::Rate);
            }
            (None, id)
        }
    };
    if !errors.is_empty() {
        return Err(TaxError::Invalid(errors));
    }
    Ok(Validated {
        name,
        description,
        rate,
        rate_id,
    })
}
#[derive(Clone)]
pub struct Taxes {
    pool: PgPool,
    identity: Identity,
}
impl Taxes {
    /// Trusted installation boundary: caller holds identity_state and commits the whole setup.
    /// Existing or partial fiscal configuration belongs to the merchant and is preserved.
    pub async fn initialize_defaults(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> std::result::Result<(), sqlx::Error> {
        sqlx::query("SELECT initialize_default_taxes()")
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Read before loading a form. Any subsequent fiscal write invalidates its revision.
    pub async fn revision(&self) -> Result<i64> {
        Self::read_revision(&self.pool).await
    }
    async fn read_revision<'e, E>(executor: E) -> Result<i64>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Ok(
            sqlx::query_scalar("SELECT revision FROM tax_configuration WHERE id=1")
                .fetch_one(executor)
                .await?,
        )
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn revision_in(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<i64> {
        Self::read_revision(&mut **tx).await
    }

    /// Uses the application's shared pool and identity service for transaction authorization.
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn list_counted(
        &self,
        session: &Session,
        kind: Kind,
        q: &str,
        active: Option<bool>,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Entry>> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        if q.chars().count() > 80
            || q.chars().any(char::is_control)
            || !(1..=10_000).contains(&page)
        {
            return Err(TaxError::Invalid(vec![]));
        }
        let sql = format!(
            "{} WHERE strpos(lower(r.name),lower($1))>0 AND ($2::boolean IS NULL OR r.active=$2)",
            kind.select()
        );
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(q.trim())
                .bind(active),
            "lower(r.name),r.id",
            page,
            PAGE_SIZE as u16,
        )
        .await?)
    }

    pub async fn list(
        &self,
        session: &Session,
        kind: Kind,
        q: &str,
        active: Option<bool>,
        page: i64,
    ) -> Result<Vec<Entry>> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        if q.chars().count() > 80 || q.chars().any(char::is_control) || !(1..=10000).contains(&page)
        {
            return Err(TaxError::Invalid(vec![]));
        }
        let sql = format!(
            "{} \
                WHERE strpos(lower(r.name),lower($1))>0 \
                AND ($2::boolean IS NULL \
                    OR r.active=$2) \
                ORDER BY lower(r.name),r.id \
                LIMIT $3 \
                OFFSET $4",
            kind.select()
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(q.trim())
            .bind(active)
            .bind((PAGE_SIZE + 1) as i64)
            .bind((page - 1) * PAGE_SIZE as i64)
            .fetch_all(&self.pool)
            .await?)
    }
    /// Bounded catalogs feed typed selectors; no template or per-row database queries.
    pub async fn choices(&self, session: &Session, kind: Kind) -> Result<Vec<Entry>> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        Self::read_choices(&self.pool, kind).await
    }
    async fn read_choices<'e, E>(executor: E, kind: Kind) -> Result<Vec<Entry>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let sql = format!("{} ORDER BY lower(r.name),r.id LIMIT $1", kind.select());
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(CONFIGURATION_LIMIT)
            .fetch_all(executor)
            .await?)
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn choices_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
        kind: Kind,
    ) -> Result<Vec<Entry>> {
        Identity::authorize_read_transaction(tx, session, Permission::TaxesRead).await?;
        Self::read_choices(&mut **tx, kind).await
    }
    pub async fn entry(&self, session: &Session, kind: Kind, id: i64) -> Result<Entry> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        let sql = format!("{} WHERE r.id=$1", kind.select());
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TaxError::NotFound)
    }
    /// Product editors may select fiscal references without managing fiscal configuration.
    pub async fn product_defaults(&self, session: &Session) -> Result<(Settings, Vec<Entry>)> {
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        let (mode, default_class_id, currency, country): (String, Option<i64>, String, String) =
            sqlx::query_as(
                "SELECT p.price_mode,p.default_class_id,s.currency,coalesce(p.country_code,'') \
                FROM pricing_settings p \
                CROSS \
                JOIN store_settings s \
                WHERE p.id=1",
            )
            .fetch_one(&self.pool)
            .await?;
        let sql = format!(
            "{} ORDER BY lower(r.name),r.id LIMIT 100",
            Kind::Class.select()
        );
        let entries = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        Ok((
            Settings {
                currency,
                country,
                mode: PriceMode::parse(&mode).map_err(|_| IdentityError::Unavailable)?,
                default_class_id,
            },
            entries,
        ))
    }
    pub async fn settings(&self, session: &Session) -> Result<Settings> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        Self::read_settings(&self.pool).await
    }
    async fn read_settings<'e, E>(executor: E) -> Result<Settings>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let (mode, default_class_id, currency, country): (String, Option<i64>, String, String) =
            sqlx::query_as(
                "SELECT p.price_mode,p.default_class_id,s.currency,coalesce(p.country_code,'') \
                FROM pricing_settings p \
                CROSS \
                JOIN store_settings s \
                WHERE p.id=1",
            )
            .fetch_optional(executor)
            .await?
            .ok_or(IdentityError::Unavailable)?;
        Ok(Settings {
            currency,
            country,
            mode: PriceMode::parse(&mode).map_err(|_| IdentityError::Unavailable)?,
            default_class_id,
        })
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn settings_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
    ) -> Result<Settings> {
        Identity::authorize_read_transaction(tx, session, Permission::TaxesRead).await?;
        Self::read_settings(&mut **tx).await
    }
    /// Identity then fiscal lock; the complete configuration commits or rolls back together.
    pub async fn save_preferences(
        &self,
        session: &Session,
        csrf: &str,
        input: &Preferences,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.save_preferences_in(&mut tx, session, csrf, input, revision)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Caller owns the commit; all fiscal constraints and permissions still apply.
    pub async fn save_preferences_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
        csrf: &str,
        input: &Preferences,
        revision: i64,
    ) -> Result<()> {
        let currency = input.currency.as_str();
        let country = input.country.as_str();
        session.check_csrf(csrf)?;
        session.require(Permission::TaxesWrite)?;
        let mut fields = vec![];
        let mode = PriceMode::parse(&input.mode).ok();
        if mode.is_none() {
            fields.push(Field::Mode);
        }
        let default = if input.default_class_id.is_empty() {
            None
        } else {
            let n = input
                .default_class_id
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0);
            if n.is_none() {
                fields.push(Field::DefaultClass);
            }
            n
        };
        if !talocart_platform::geography::currency_valid(currency) {
            fields.push(Field::Currency);
        }
        if !country.is_empty() && !talocart_platform::geography::country_valid(country) {
            fields.push(Field::Country);
        }
        if !fields.is_empty() {
            return Err(TaxError::Invalid(fields));
        }
        Identity::authorize_transaction(tx, session, csrf, Permission::TaxesWrite).await?;
        check_revision(tx, revision).await?;
        product_write_lock(tx).await?;
        let currency_changed = sqlx::query("UPDATE store_settings SET currency=$1 WHERE id=1")
            .bind(currency)
            .execute(&mut **tx)
            .await?;
        if currency_changed.rows_affected() != 1 {
            return Err(IdentityError::Unavailable.into());
        }
        sqlx::query("UPDATE pricing_settings SET country_code=nullif($1,'') WHERE id=1")
            .bind(country)
            .execute(&mut **tx)
            .await?;
        let changed =
            sqlx::query("UPDATE pricing_settings SET price_mode=$1,default_class_id=$2 WHERE id=1")
                .bind(mode.map(PriceMode::key))
                .bind(default)
                .execute(&mut **tx)
                .await?;
        if changed.rows_affected() != 1 {
            return Err(IdentityError::Unavailable.into());
        }
        Ok(())
    }
    pub async fn save(
        &self,
        session: &Session,
        csrf: &str,
        kind: Kind,
        id: Option<i64>,
        input: &Values,
        revision: i64,
    ) -> Result<i64> {
        session.check_csrf(csrf)?;
        session.require(Permission::TaxesWrite)?;
        let valid = validate(input, kind)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::TaxesWrite).await?;
        check_revision(&mut tx, revision).await?;
        let sql = match (kind, id) {
            (Kind::Rate, None) => {
                "INSERT INTO tax_rates(name,description,active,basis_points) VALUES($1,$2,$3,$4) RETURNING id"
            }
            (Kind::Rate, Some(_)) => {
                "UPDATE tax_rates SET name=$1,description=$2,active=$3,basis_points=$4 WHERE id=$5 RETURNING id"
            }
            (Kind::Class, None) => {
                "INSERT INTO tax_classes(name,description,active,rate_id) VALUES($1,$2,$3,$4) RETURNING id"
            }
            (Kind::Class, Some(_)) => {
                "UPDATE tax_classes SET name=$1,description=$2,active=$3,rate_id=$4 WHERE id=$5 RETURNING id"
            }
        };
        let mut query = sqlx::query_scalar::<_, i64>(sql)
            .bind(&valid.name)
            .bind(&valid.description)
            .bind(input.active);
        query = match kind {
            Kind::Rate => query.bind(valid.rate.map(Rate::basis_points)),
            Kind::Class => query.bind(valid.rate_id),
        };
        if let Some(id) = id {
            query = query.bind(id);
        }
        let saved = query
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(TaxError::NotFound)?;
        tx.commit().await?;
        Ok(saved)
    }
    pub async fn delete(
        &self,
        session: &Session,
        csrf: &str,
        kind: Kind,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::TaxesWrite).await?;
        check_revision(&mut tx, revision).await?;
        let sql = format!("DELETE FROM {} WHERE id=$1", kind.table());
        let result = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .execute(&mut *tx)
            .await;
        let result = match result {
            Err(e)
                if e.as_database_error()
                    .is_some_and(|e| e.is_foreign_key_violation()) =>
            {
                return Err(TaxError::InUse);
            }
            other => other?,
        };
        if result.rows_affected() != 1 {
            return Err(TaxError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn preview(&self, session: &Session, csrf: &str, input: &Preview) -> Result<Quote> {
        self.preview_country(
            session,
            csrf,
            input,
            if input.country == "store" {
                None
            } else {
                Some(&input.country)
            },
        )
        .await
    }
    /// An explicit destination overrides the catalog country; empty destination uses product class.
    pub async fn preview_country(
        &self,
        session: &Session,
        csrf: &str,
        input: &Preview,
        country: Option<&str>,
    ) -> Result<Quote> {
        session.check_csrf(csrf)?;
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        let mut fields = vec![];
        if country.is_some_and(|code| {
            !code.is_empty() && !talocart_platform::geography::country_valid(code)
        }) {
            fields.push(Field::Country);
        }
        let amount = Money::parse(&input.amount).ok();
        if amount.is_none() {
            fields.push(Field::Amount);
        }
        let quantity = input
            .quantity
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=1000000).contains(n));
        if quantity.is_none() {
            fields.push(Field::Quantity);
        }
        let class_id = input.class_id.parse::<i64>().ok().filter(|n| *n > 0);
        if class_id.is_none() {
            fields.push(Field::Class);
        }
        if !fields.is_empty() {
            return Err(TaxError::Invalid(fields));
        }
        // One SQL snapshot reads mode and current class/rate together. Client tax totals are never trusted.
        let row: Option<(String, String, i32, String, String, Option<String>)> = sqlx::query_as(
            "SELECT p.price_mode,t.tax_name,t.basis_points,ss.currency,coalesce($2,p.country_code,\
                ''),CASE \
            WHEN t.explicit_rule THEN t.zone_name END \
            FROM pricing_settings p \
            CROSS \
            JOIN store_settings ss \
            CROSS \
            JOIN LATERAL resolve_tax(coalesce($2,p.country_code)) t \
            WHERE p.id=1 \
            AND t.class_id=$1",
        )
        .bind(class_id)
        .bind(country)
        .fetch_optional(&self.pool)
        .await?;
        let (mode, class_name, basis_points, currency, country, zone_name) =
            row.ok_or(TaxError::Invalid(vec![Field::Class]))?;
        let mode = PriceMode::parse(&mode).map_err(|_| IdentityError::Unavailable)?;
        let rate = Rate::from_basis_points(basis_points).map_err(|_| IdentityError::Unavailable)?;
        let amount = amount.ok_or(TaxError::Invalid(vec![Field::Amount]))?;
        let quantity = quantity.ok_or(TaxError::Invalid(vec![Field::Quantity]))?;
        let totals = pricing::line(amount, quantity, rate, mode).map_err(|e| match e {
            PriceError::Overflow => TaxError::Overflow,
            PriceError::Invalid => TaxError::Invalid(vec![Field::Amount]),
        })?;
        Ok(Quote {
            currency,
            country,
            zone_name,
            totals,
            percentage: rate.display(),
            class_name,
            mode,
            quantity,
        })
    }
}
/// Product prices must use the mode shown when the form was opened; lock order is identity then fiscal.
pub async fn product_mode_matches(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    expected: &str,
) -> std::result::Result<bool, IdentityError> {
    product_write_lock(tx).await?;
    let mode: String = sqlx::query_scalar("SELECT price_mode FROM pricing_settings WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    Ok(mode == expected)
}
/// Acquire after the identity lock, before reading fiscal configuration or writing products.
pub async fn product_write_lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> std::result::Result<(), IdentityError> {
    talocart_platform::database::commercial_lock::exclusive(tx).await?;
    Ok(())
}
/// Identity lock precedes the fiscal lock. Check and write share one transaction.
pub(crate) async fn check_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    expected: i64,
) -> Result<()> {
    product_write_lock(tx).await?;
    let current: i64 = sqlx::query_scalar("SELECT revision FROM tax_configuration WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    if current != expected {
        return Err(TaxError::Stale);
    }
    Ok(())
}

impl From<IdentityError> for TaxError {
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
    fn tax_fields_validate_exact_percentages_and_references() {
        let mut input = Values {
            name: "  Demo  ".into(),
            description: "Note\nsemplici".into(),
            percentage: "7,50".into(),
            rate_id: "1".into(),
            active: true,
        };
        assert_eq!(
            validate(&input, Kind::Rate)
                .unwrap()
                .rate
                .unwrap()
                .basis_points(),
            750
        );
        assert_eq!(validate(&input, Kind::Class).unwrap().name, "Demo");
        input.name = "\0".into();
        input.description = "x".repeat(501);
        input.percentage = "100.01".into();
        assert!(matches!(validate(&input,Kind::Rate),Err(TaxError::Invalid(f))if f.len()==3));
        input.rate_id = "0".into();
        assert!(
            matches!(validate(&input,Kind::Class),Err(TaxError::Invalid(f))if f.contains(&Field::Rate))
        );
    }
}
