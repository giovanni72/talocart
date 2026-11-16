//! Shipping configuration and destination-based fiscal quotes; no shipment booking.
use crate::{
    pricing::{Money, PriceError, PriceMode, Rate},
    taxes::charges::{self, ChargeError, ChargeQuote, ClassChoice},
};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use talocart_platform::identity::{Identity, IdentityError, Permission, Session};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShippingError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid shipping fields")]
    Invalid(Vec<&'static str>),
    #[error("Configuration changed")]
    Stale,
    #[error("Shipping method not found")]
    NotFound,
    #[error("Shipping method unavailable")]
    Unavailable,
    #[error(transparent)]
    Price(#[from] PriceError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for ShippingError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
impl From<ChargeError> for ShippingError {
    fn from(e: ChargeError) -> Self {
        match e {
            ChargeError::Unavailable => Self::Unavailable,
            ChargeError::Price(e) => Self::Price(e),
            ChargeError::Technical(e) => Self::Technical(e),
            ChargeError::Access(e) => Self::Access(e),
            ChargeError::Identity(e) => Self::Identity(e),
        }
    }
}
pub type Result<T> = std::result::Result<T, ShippingError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum MethodCode {
    NationalExpress,
    FreeShipping,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum FeeKind {
    Fixed,
    Percent,
}
impl FeeKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Percent => "percent",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub name_it: String,
    pub name_en: String,
    pub instructions_it: String,
    pub instructions_en: String,
    pub active: bool,
    pub sort_order: String,
    pub delivery_days: String,
    pub fee_kind: String,
    pub fee_value: String,
    pub free_from: String,
    pub currency: String,
    pub price_mode: String,
    pub countries: Vec<String>,
    pub tax_class_id: String,
}
#[derive(Clone, Debug, FromRow)]
pub struct Method {
    pub id: i64,
    pub code: MethodCode,
    pub name_it: String,
    pub name_en: String,
    pub instructions_it: String,
    pub instructions_en: String,
    pub active: bool,
    pub sort_order: i32,
    pub delivery_days: Option<i32>,
    pub fee_kind: FeeKind,
    pub fee_value: i64,
    pub free_from_cents: i64,
    pub tax_class_id: Option<i64>,
    #[sqlx(default)]
    pub countries: Vec<String>,
}
impl Method {
    pub fn input(&self, currency: String, mode: PriceMode) -> Input {
        Input {
            name_it: self.name_it.clone(),
            name_en: self.name_en.clone(),
            instructions_it: self.instructions_it.clone(),
            instructions_en: self.instructions_en.clone(),
            active: self.active,
            sort_order: self.sort_order.to_string(),
            delivery_days: self
                .delivery_days
                .map(|n| n.to_string())
                .unwrap_or_default(),
            fee_kind: self.fee_kind.key().into(),
            fee_value: decimal(self.fee_value),
            free_from: decimal(self.free_from_cents),
            currency,
            price_mode: mode.key().into(),
            countries: self.countries.clone(),
            tax_class_id: self
                .tax_class_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
        }
    }
    /// Base: server-calculated goods total after discounts, including goods taxes,
    /// before shipping/payment fees. Threshold is inclusive; zero disables it.
    /// Destination must be assigned to the method. Percentage rounds half-up once.
    /// Private tariff arithmetic; require_method adds the destination tax breakdown.
    fn tariff(&self, base: Money, destination: &str) -> Result<Tariff> {
        if !self.active || !self.countries.iter().any(|c| c == destination) {
            return Err(ShippingError::Unavailable);
        }
        let free = self.code == MethodCode::FreeShipping
            || (self.free_from_cents > 0 && base.cents() >= self.free_from_cents);
        let cost = if free {
            0
        } else {
            match self.fee_kind {
                FeeKind::Fixed => self.fee_value,
                FeeKind::Percent => i64::try_from(
                    (i128::from(base.cents()) * i128::from(self.fee_value) + 5000) / 10000,
                )
                .map_err(|_| PriceError::Overflow)?,
            }
        };
        let total = base.cents().checked_add(cost).ok_or(PriceError::Overflow)?;
        Ok(Tariff {
            cost: Money::from_cents(cost)?,
            total: Money::from_cents(total)?,
            free,
        })
    }
}
fn decimal(value: i64) -> String {
    format!("{}.{:02}", value / 100, value % 100)
}
#[derive(Debug, PartialEq, Eq)]
struct Tariff {
    pub cost: Money,
    pub total: Money,
    pub free: bool,
}
/// Consistent administrator view of configuration and purchasable destinations.
pub struct Snapshot {
    pub revision: i64,
    pub currency: String,
    pub methods: Vec<Method>,
    pub enabled_countries: Vec<String>,
    pub tax_classes: Vec<ClassChoice>,
    pub price_mode: PriceMode,
}
/// A complete fiscal shipping quote; cost is the gross amount added to base.
#[derive(Debug, PartialEq, Eq)]
pub struct Quote {
    pub cost: Money,
    pub total: Money,
    pub free: bool,
    pub breakdown: ChargeQuote,
}
#[derive(Clone)]
pub struct Shipping {
    pool: PgPool,
    identity: Identity,
}
impl Shipping {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn snapshot(&self, s: &Session) -> Result<Snapshot> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let revision = sqlx::query_scalar("SELECT revision FROM shipping_configuration WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let currency = sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let methods = sqlx::query_as(
            "SELECT m.*,ARRAY(SELECT country_code \
                    FROM shipping_method_countries \
                    WHERE method_id=m.id \
                    ORDER BY country_code) AS countries \
                FROM shipping_methods m \
                ORDER BY sort_order,id \
                LIMIT 2",
        )
        .fetch_all(&mut *tx)
        .await?;
        let countries = sqlx::query_scalar(
            "SELECT country_code FROM checkout_countries ORDER BY country_code LIMIT 249",
        )
        .fetch_all(&mut *tx)
        .await?;
        let tax_classes = charges::choices(&mut tx).await?;
        let mode: String = sqlx::query_scalar("SELECT price_mode FROM pricing_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Snapshot {
            revision,
            currency,
            methods,
            enabled_countries: countries,
            tax_classes,
            price_mode: PriceMode::parse(&mode)?,
        })
    }
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &Input,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        self.identity
            .authorize(s, Permission::SettingsWrite)
            .await?;
        let code: MethodCode = sqlx::query_scalar("SELECT code FROM shipping_methods WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(ShippingError::NotFound)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        // Lock before currency reads and row locks, like payment/currency writes.
        talocart_platform::database::commercial_lock::exclusive(&mut tx).await?;
        let currency: String = sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let mode: String = sqlx::query_scalar("SELECT price_mode FROM pricing_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        if currency != input.currency || mode != input.price_mode {
            return Err(ShippingError::Stale);
        }
        let m = validate(input, code)?;
        sqlx::query("SELECT pg_advisory_xact_lock_shared(5426645964561798230)")
            .execute(&mut *tx)
            .await?;
        let enabled: Vec<String> = sqlx::query_scalar(
            "SELECT country_code FROM checkout_countries WHERE country_code=ANY($1::text[])",
        )
        .bind(&input.countries)
        .fetch_all(&mut *tx)
        .await?;
        if enabled.len() != input.countries.len() {
            return Err(ShippingError::Invalid(vec!["countries"]));
        }
        if sqlx::query(
            "UPDATE shipping_configuration SET revision=revision+1 WHERE id=1 AND revision=$1",
        )
        .bind(revision)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 1
        {
            return Err(ShippingError::Stale);
        }
        charges::validate_class(&mut tx, m.tax_class_id)
            .await
            .map_err(|e| match e {
                ChargeError::Unavailable => ShippingError::Invalid(vec!["tax_class_id"]),
                other => other.into(),
            })?;
        let saved = sqlx::query_scalar(
            "UPDATE shipping_methods \
                SET name_it=$1,name_en=$2,instructions_it=$3,instructions_en=$4,active=$5,sort_order=$6,\
                fee_kind=$7,fee_value=$8,free_from_cents=$9,delivery_days=$12,tax_class_id=$13 \
                WHERE id=$10 \
                AND code=$11 \
                RETURNING id",
        )
        .bind(&m.name_it)
        .bind(&m.name_en)
        .bind(&m.instructions_it)
        .bind(&m.instructions_en)
        .bind(m.active)
        .bind(m.sort_order)
        .bind(m.fee_kind)
        .bind(m.fee_value)
        .bind(m.free_from_cents)
        .bind(id)
        .bind(code)
        .bind(m.delivery_days)
        .bind(m.tax_class_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ShippingError::NotFound)?;
        // Preserve retained rows. Skip statements with no changes: statement-level
        // triggers invalidate revisions even when no rows are affected.
        let current: Vec<String> = sqlx::query_scalar(
            "SELECT country_code FROM shipping_method_countries WHERE method_id=$1",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let removed: Vec<&str> = current
            .iter()
            .filter(|c| !input.countries.contains(c))
            .map(String::as_str)
            .collect();
        let added: Vec<&str> = input
            .countries
            .iter()
            .filter(|c| !current.contains(c))
            .map(String::as_str)
            .collect();
        if !removed.is_empty() {
            sqlx::query("DELETE FROM shipping_method_countries WHERE method_id=$1 AND country_code=ANY($2::text[])")
                .bind(id).bind(&removed).execute(&mut *tx).await?;
        }
        if !added.is_empty() {
            sqlx::query("INSERT INTO shipping_method_countries(method_id,country_code) SELECT $1,unnest($2::text[])")
                .bind(id).bind(&added).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(saved)
    }
    /// Re-read and hold the configuration until the future order commits. Also
    /// validates CheckoutCountries in the same transaction. Base must come from
    /// server-side line totals; no browser-provided amount is authoritative.
    pub async fn require_method(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        base: Money,
        destination: &str,
    ) -> Result<(Method, Quote)> {
        charges::lock(tx).await?;
        crate::countries::CheckoutCountries::require_destination(tx, destination)
            .await
            .map_err(|e| match e {
                crate::countries::CountryError::Technical(e) => e.into(),
                crate::countries::CountryError::Access(e) => e.into(),
                crate::countries::CountryError::Identity(e) => ShippingError::Identity(e),
                _ => ShippingError::Unavailable,
            })?;
        let mut m: Method = sqlx::query_as("SELECT * FROM shipping_methods WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ShippingError::NotFound)?;
        m.countries = sqlx::query_scalar(
            "SELECT country_code \
            FROM shipping_method_countries \
            WHERE method_id=$1 \
            ORDER BY country_code \
            LIMIT 249",
        )
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
        let tariff = m.tariff(base, destination)?;
        if m.fee_value > 0 && m.tax_class_id.is_none() {
            return Err(ShippingError::Unavailable);
        }
        let breakdown = charges::quote(tx, m.tax_class_id, tariff.cost, destination).await?;
        let cost = breakdown.totals.gross();
        let added_tax = if breakdown.price_mode == PriceMode::Exclusive {
            breakdown.totals.tax().cents()
        } else {
            0
        };
        let total = Money::from_cents(
            tariff
                .total
                .cents()
                .checked_add(added_tax)
                .ok_or(PriceError::Overflow)?,
        )?;
        Ok((
            m,
            Quote {
                cost,
                total,
                free: tariff.free,
                breakdown,
            },
        ))
    }
}
fn validate(i: &Input, code: MethodCode) -> Result<Method> {
    let mut errors = Vec::new();
    for (field, value, max, required) in [
        ("name_it", i.name_it.as_str(), 120, true),
        ("name_en", &i.name_en, 120, true),
        ("instructions_it", &i.instructions_it, 4000, false),
        ("instructions_en", &i.instructions_en, 4000, false),
    ] {
        if (required && value.trim().is_empty())
            || value.chars().count() > max
            || value.contains('\0')
        {
            errors.push(field);
        }
    }
    let order = i
        .sort_order
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=9999).contains(n));
    if order.is_none() {
        errors.push("sort_order");
    }
    let kind = match i.fee_kind.as_str() {
        "fixed" => Some(FeeKind::Fixed),
        "percent" => Some(FeeKind::Percent),
        _ => None,
    };
    if kind.is_none() {
        errors.push("fee_kind");
    }
    let fee = match kind {
        Some(FeeKind::Fixed) => Money::parse(&i.fee_value).ok().map(Money::cents),
        Some(FeeKind::Percent) => Rate::parse(&i.fee_value)
            .ok()
            .map(|r| i64::from(r.basis_points())),
        None => None,
    };
    if fee.is_none() {
        errors.push("fee_value");
    }
    let free = if i.free_from.trim().is_empty() {
        Some(0)
    } else {
        Money::parse(&i.free_from).ok().map(Money::cents)
    };
    if free.is_none() {
        errors.push("free_from");
    }
    if PriceMode::parse(&i.price_mode).is_err() {
        errors.push("price_mode");
    }
    if !talocart_platform::geography::currency_valid(&i.currency) {
        errors.push("currency");
    }
    let days = if i.delivery_days.trim().is_empty() {
        None
    } else {
        match i
            .delivery_days
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|n| (0..=365).contains(n))
        {
            Some(n) => Some(n),
            None => {
                errors.push("delivery_days");
                None
            }
        }
    };
    let unique: std::collections::BTreeSet<_> = i.countries.iter().collect();
    if i.countries.len() > 249
        || unique.len() != i.countries.len()
        || i.countries
            .iter()
            .any(|c| !talocart_platform::geography::country_valid(c))
        || (i.active && i.countries.is_empty())
    {
        errors.push("countries");
    }
    if code == MethodCode::FreeShipping
        && (kind != Some(FeeKind::Fixed) || fee != Some(0) || free != Some(0))
    {
        errors.push("fee_value");
    }
    let tax_class_id = charges::parse_class(&i.tax_class_id, fee.is_some_and(|v| v > 0));
    if tax_class_id.is_err() || (code == MethodCode::FreeShipping && !i.tax_class_id.is_empty()) {
        errors.push("tax_class_id");
    }
    if !errors.is_empty() {
        return Err(ShippingError::Invalid(errors));
    }
    Ok(Method {
        id: 0,
        code,
        name_it: i.name_it.trim().into(),
        name_en: i.name_en.trim().into(),
        instructions_it: i.instructions_it.clone(),
        instructions_en: i.instructions_en.clone(),
        active: i.active,
        sort_order: order.unwrap_or_default(),
        delivery_days: days,
        fee_kind: kind.unwrap_or(FeeKind::Fixed),
        fee_value: fee.unwrap_or_default(),
        free_from_cents: free.unwrap_or_default(),
        countries: i.countries.clone(),
        tax_class_id: tax_class_id.unwrap_or_default(),
    })
}
impl From<IdentityError> for ShippingError {
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
    fn input() -> Input {
        Input {
            name_it: "Corriere espresso nazionale".into(),
            name_en: "National express courier".into(),
            active: true,
            sort_order: "0".into(),
            fee_kind: "fixed".into(),
            tax_class_id: "1".into(),
            fee_value: "5.90".into(),
            free_from: "100".into(),
            currency: "EUR".into(),
            price_mode: "inclusive".into(),
            countries: vec!["IT".into()],
            ..Default::default()
        }
    }
    #[test]
    fn free_shipping_is_inclusive_and_zero_or_blank_never_enables_it() {
        let mut i = input();
        let m = validate(&i, MethodCode::NationalExpress).unwrap();
        for (base, cost, free) in [(9999, 590, false), (10000, 0, true), (10001, 0, true)] {
            let q = m.tariff(Money::from_cents(base).unwrap(), "IT").unwrap();
            assert_eq!(
                (q.cost.cents(), q.free, q.total.cents()),
                (cost, free, base + cost)
            );
        }
        for threshold in ["", "0", "0,00"] {
            i.free_from = threshold.into();
            let m = validate(&i, MethodCode::NationalExpress).unwrap();
            for base in [0, 10000, 1000000] {
                let q = m.tariff(Money::from_cents(base).unwrap(), "IT").unwrap();
                assert_eq!((q.cost.cents(), q.free), (590, false));
            }
        }
    }
    #[test]
    fn percentage_rounding_threshold_destination_and_disabled() {
        let mut i = input();
        i.fee_kind = "percent".into();
        i.fee_value = "2,50".into();
        let mut m = validate(&i, MethodCode::NationalExpress).unwrap();
        for (base, cost) in [(0, 0), (19, 0), (20, 1), (9999, 250), (10000, 0)] {
            assert_eq!(
                m.tariff(Money::from_cents(base).unwrap(), "IT")
                    .unwrap()
                    .cost
                    .cents(),
                cost
            );
        }
        for destination in ["FR", "", "it"] {
            assert!(matches!(
                m.tariff(Money::from_cents(10000).unwrap(), destination),
                Err(ShippingError::Unavailable)
            ));
        }
        m.active = false;
        assert!(matches!(
            m.tariff(Money::from_cents(10000).unwrap(), "IT"),
            Err(ShippingError::Unavailable)
        ));
    }
    #[test]
    fn standalone_free_shipping_is_always_zero_and_rejects_costs() {
        let mut i = input();
        i.fee_value = "0".into();
        i.tax_class_id.clear();
        i.free_from.clear();
        let m = validate(&i, MethodCode::FreeShipping).unwrap();
        for base in [0, 9999, 10000, i64::MAX] {
            let q = m.tariff(Money::from_cents(base).unwrap(), "IT").unwrap();
            assert_eq!((q.cost.cents(), q.total.cents(), q.free), (0, base, true));
        }
        i.fee_value = "1".into();
        assert!(validate(&i, MethodCode::FreeShipping).is_err());
        i.fee_value = "0".into();
        i.free_from = "100".into();
        assert!(validate(&i, MethodCode::FreeShipping).is_err());
        i.free_from.clear();
        i.fee_kind = "percent".into();
        assert!(validate(&i, MethodCode::FreeShipping).is_err());
    }
    #[test]
    fn validation_and_overflow() {
        for field in ["fee_value", "free_from"] {
            for bad in ["-1", "1.001", "NaN", "92233720368547758.08"] {
                let mut i = input();
                if field == "fee_value" {
                    i.fee_value = bad.into();
                } else {
                    i.free_from = bad.into();
                }
                assert!(
                    matches!(validate(&i,MethodCode::NationalExpress),Err(ShippingError::Invalid(f)) if f.contains(&field))
                );
            }
        }
        let mut i = input();
        i.fee_kind = "percent".into();
        i.fee_value = "100.01".into();
        assert!(validate(&i, MethodCode::NationalExpress).is_err());
        i.fee_value = "100".into();
        i.free_from.clear();
        let m = validate(&i, MethodCode::NationalExpress).unwrap();
        assert!(matches!(
            m.tariff(Money::from_cents(i64::MAX).unwrap(), "IT"),
            Err(ShippingError::Price(PriceError::Overflow))
        ));
        i = input();
        i.free_from.clear();
        let m = validate(&i, MethodCode::NationalExpress).unwrap();
        assert!(
            m.tariff(Money::from_cents(i64::MAX).unwrap(), "IT")
                .is_err()
        );
        i = input();
        i.free_from = "100".into();
        i.fee_value = "92233720368547758.07".into();
        let m = validate(&i, MethodCode::NationalExpress).unwrap();
        assert_eq!(
            m.tariff(Money::from_cents(i64::MAX).unwrap(), "IT")
                .unwrap()
                .cost
                .cents(),
            0
        );
        for (name, value) in [
            ("name_it", ""),
            ("name_en", " "),
            ("sort_order", "10000"),
            ("fee_kind", "none"),
            ("currency", "ZZZ"),
            ("instructions_it", "\0"),
        ] {
            let mut i = input();
            match name {
                "name_it" => i.name_it = value.into(),
                "name_en" => i.name_en = value.into(),
                "sort_order" => i.sort_order = value.into(),
                "fee_kind" => i.fee_kind = value.into(),
                "currency" => i.currency = value.into(),
                _ => i.instructions_it = value.into(),
            };
            assert!(
                matches!(validate(&i,MethodCode::NationalExpress),Err(ShippingError::Invalid(f)) if f.contains(&name))
            );
        }
    }
}
