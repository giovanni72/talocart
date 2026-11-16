//! Fiscal boundary for shipping/payment charges. The caller owns the transaction.
use crate::pricing::{self, LineTotals, Money, PriceError, PriceMode, Rate};
use sqlx::{FromRow, Postgres, Transaction};
use talocart_platform::{geography, identity::IdentityError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChargeError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Charge tax class or destination unavailable")]
    Unavailable,
    #[error(transparent)]
    Price(#[from] PriceError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for ChargeError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
#[derive(Clone, FromRow)]
pub struct ClassChoice {
    pub id: i64,
    pub name: String,
    pub available: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxSnapshot {
    pub class_id: i64,
    pub class_name: String,
    pub basis_points: i32,
    pub zone_name: Option<String>,
}
/// Monetary breakdown and provenance to copy into the future order snapshot.
#[derive(Debug, PartialEq, Eq)]
pub struct ChargeQuote {
    pub entered: Money,
    pub totals: LineTotals,
    pub tax: Option<TaxSnapshot>,
    pub currency: String,
    pub price_mode: PriceMode,
    pub destination: String,
}
/// Acquire before method row locks, including for zero costs. All fiscal/currency
/// writers use the exclusive guard, keeping mode, rate and method coherent to commit.
pub async fn lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), ChargeError> {
    talocart_platform::database::commercial_lock::shared(tx).await?;
    Ok(())
}
pub(crate) async fn choices(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<ClassChoice>, ChargeError> {
    Ok(sqlx::query_as(
        "SELECT c.id,c.name,(c.active \
            AND r.active) AS available \
        FROM tax_classes c \
        JOIN tax_rates r ON r.id=c.rate_id \
        ORDER BY c.name,c.id \
        LIMIT 100",
    )
    .fetch_all(&mut **tx)
    .await?)
}
pub(crate) async fn validate_class(
    tx: &mut Transaction<'_, Postgres>,
    id: Option<i64>,
) -> Result<(), ChargeError> {
    if let Some(id) = id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 \
                FROM tax_classes c \
                JOIN tax_rates r ON r.id=c.rate_id \
                WHERE c.id=$1 \
                AND c.active \
                AND r.active)",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        if !exists {
            return Err(ChargeError::Unavailable);
        }
    }
    Ok(())
}
/// Blank is allowed only without a configured positive charge. An explicit zero
/// rate is represented by an actual class, never by a missing selection.
pub(crate) fn parse_class(value: &str, required: bool) -> Result<Option<i64>, ChargeError> {
    if value.is_empty() && !required {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(Some)
        .ok_or(ChargeError::Unavailable)
}
pub async fn quote(
    tx: &mut Transaction<'_, Postgres>,
    class_id: Option<i64>,
    entered: Money,
    destination: &str,
) -> Result<ChargeQuote, ChargeError> {
    if !geography::country_valid(destination) {
        return Err(ChargeError::Unavailable);
    }
    lock(tx).await?;
    let (currency, mode): (String, String) = sqlx::query_as(
        "SELECT s.currency,p.price_mode \
        FROM store_settings s \
        CROSS \
        JOIN pricing_settings p \
        WHERE s.id=1 \
        AND p.id=1",
    )
    .fetch_one(&mut **tx)
    .await?;
    let mode = PriceMode::parse(&mode)?;
    let tax = if let Some(id) = class_id {
        let row: Option<(i64, String, i32, Option<String>)> = sqlx::query_as(
            "SELECT class_id,tax_name,basis_points,CASE \
            WHEN explicit_rule THEN zone_name END \
            FROM resolve_tax($1) \
            WHERE class_id=$2",
        )
        .bind(destination)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        let (class_id, class_name, basis_points, zone_name) =
            row.ok_or(ChargeError::Unavailable)?;
        Some(TaxSnapshot {
            class_id,
            class_name,
            basis_points,
            zone_name,
        })
    } else {
        if entered.cents() != 0 {
            return Err(ChargeError::Unavailable);
        }
        None
    };
    let rate = Rate::from_basis_points(tax.as_ref().map_or(0, |t| t.basis_points))?;
    Ok(ChargeQuote {
        entered,
        totals: pricing::line(entered, 1, rate, mode)?,
        tax,
        currency,
        price_mode: mode,
        destination: destination.into(),
    })
}

impl From<IdentityError> for ChargeError {
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
