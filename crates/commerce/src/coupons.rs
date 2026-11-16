//! Server-owned promotion configuration, integer allocation and permanent order uses.
use crate::{
    cart::{CartError, Carts, Guest, Quote},
    pricing::{self, Money, PriceError, PriceMode, Rate},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use talocart_platform::{
    content::Language,
    identity::{Identity, IdentityError, Permission, Session},
};
#[derive(Debug, thiserror::Error)]
pub enum CouponError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid coupon fields")]
    Invalid(Vec<&'static str>),
    #[error("Coupon changed or not found")]
    Stale,
    #[error("Coupon code already exists")]
    Duplicate,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for CouponError {
    fn from(e: sqlx::Error) -> Self {
        if e.as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Self::Duplicate
        } else {
            Self::Technical(
                talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
            )
        }
    }
}
type Result<T> = std::result::Result<T, CouponError>;
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub currency: String,
    pub code: String,
    pub kind: String,
    pub value: String,
    pub expires_on: String,
    pub usage_limit: String,
    pub active: bool,
}
#[derive(sqlx::FromRow)]
pub struct Coupon {
    pub id: i64,
    pub code: String,
    pub kind: String,
    pub value: i64,
    pub currency: String,
    pub active: bool,
    pub expires_on: String,
    pub usage_limit: Option<i64>,
    pub revision: i64,
    pub uses: i64,
}
impl Coupon {
    pub fn input(&self) -> Input {
        Input {
            currency: self.currency.clone(),
            code: self.code.clone(),
            kind: self.kind.clone(),
            value: format!("{}.{:02}", self.value / 100, self.value % 100),
            expires_on: self.expires_on.clone(),
            usage_limit: self.usage_limit.map(|v| v.to_string()).unwrap_or_default(),
            active: self.active,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Snapshot {
    pub id: i64,
    pub code: String,
    pub revision: i64,
    pub kind: String,
    pub value: i64,
    pub original_cents: i64,
    pub discount_cents: i64,
    pub lines: Vec<LineDiscount>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct LineDiscount {
    pub position: usize,
    pub original_cents: i64,
    pub discount_cents: i64,
}
impl Snapshot {
    pub fn line(&self, position: &usize) -> Option<&LineDiscount> {
        self.lines.get(*position)
    }
}
pub fn normalize(code: &str) -> Option<String> {
    let c = code.trim().to_ascii_uppercase();
    if (1..=40).contains(&c.len())
        && c.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && c.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Some(c)
    } else {
        None
    }
}
fn date_valid(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    let y = s[..4].parse::<u32>().unwrap_or_default();
    let m = s[5..7].parse::<u32>().unwrap_or_default();
    let d = s[8..].parse::<u32>().unwrap_or_default();
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    (2000..=9999).contains(&y) && d > 0 && d <= max
}
fn validate(i: &Input) -> Result<(String, i64, Option<i64>)> {
    let mut errors = vec![];
    let code = normalize(&i.code);
    if code.is_none() {
        errors.push("code")
    }
    let value = Money::parse(&i.value)
        .ok()
        .map(|m| m.cents())
        .filter(|v| *v > 0 && *v <= 999999999999 && (i.kind != "percent" || *v <= 10000));
    if value.is_none() {
        errors.push("value")
    }
    if !matches!(i.kind.as_str(), "fixed" | "percent") {
        errors.push("kind")
    }
    if !date_valid(&i.expires_on) {
        errors.push("expires_on")
    }
    let limit = if i.usage_limit.trim().is_empty() {
        None
    } else {
        let n = i
            .usage_limit
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|v| (1..=1000000000).contains(v));
        if n.is_none() {
            errors.push("usage_limit")
        }
        n
    };
    if !errors.is_empty() {
        return Err(CouponError::Invalid(errors));
    }
    Ok((code.unwrap_or_default(), value.unwrap_or_default(), limit))
}
pub struct Coupons {
    pool: PgPool,
    identity: Identity,
}
impl Coupons {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn currency(&self, s: &Session) -> Result<String> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        Ok(
            sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn list(
        &self,
        s: &Session,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Coupon>> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        if !(1..=1000000).contains(&page) {
            return Err(CouponError::Invalid(vec!["page"]));
        }
        Ok(talocart_platform::pagination::read(&self.pool,sqlx::query("SELECT c.id,c.code,c.kind,c.value,c.currency,c.active,coalesce(to_char(c.expires_on,\
                    'YYYY-MM-DD'),'') AS expires_on,c.usage_limit,c.revision,(SELECT count(*) \
                FROM coupon_uses u \
                WHERE u.coupon_id=c.id) AS uses \
            FROM coupons c"),"id DESC",page,20).await?)
    }
    pub async fn get(&self, s: &Session, id: i64) -> Result<Coupon> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        sqlx::query_as(
            "SELECT c.id,c.code,c.kind,c.value,c.currency,c.active,coalesce(to_char(c.expires_on,\
                    'YYYY-MM-DD'),'') AS expires_on,c.usage_limit,c.revision,(SELECT count(*) \
                FROM coupon_uses u \
                WHERE u.coupon_id=c.id) AS uses \
            FROM coupons c \
            WHERE c.id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(CouponError::Stale)
    }
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        i: &Input,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let (code, value, limit) = validate(i)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        talocart_platform::database::commercial_lock::exclusive(&mut tx).await?;
        let currency: String = sqlx::query_scalar("SELECT currency FROM store_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        if i.currency != currency {
            return Err(CouponError::Stale);
        }
        let sql = if id == 0 {
            "INSERT INTO coupons(code,kind,value,currency,active,expires_on,usage_limit) \
                SELECT $1,$2,$3,$4,$5,nullif($6,'')::date,$7 \
                WHERE $8=0 \
                AND $9=0 \
                RETURNING id"
        } else {
            "UPDATE coupons \
                SET code=$1,kind=$2,value=$3,currency=$4,active=$5,expires_on=nullif($6,'')::date,\
                usage_limit=$7,revision=revision+1 \
                WHERE id=$8 \
                AND revision=$9 \
                RETURNING id"
        };
        let saved = sqlx::query_scalar(sql)
            .bind(code)
            .bind(&i.kind)
            .bind(value)
            .bind(currency)
            .bind(i.active)
            .bind(&i.expires_on)
            .bind(limit)
            .bind(id)
            .bind(revision)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(CouponError::Stale)?;
        tx.commit().await?;
        Ok(saved)
    }
}
/// Largest-remainder allocation. Input order breaks ties; every cent is assigned once.
pub fn allocate(gross: &[i64], discount: i64) -> std::result::Result<Vec<i64>, PriceError> {
    if gross.iter().any(|v| *v < 0) || discount < 0 {
        return Err(PriceError::Invalid);
    }
    let total = gross
        .iter()
        .try_fold(0i64, |a, b| a.checked_add(*b).ok_or(PriceError::Overflow))?;
    if discount > total {
        return Err(PriceError::Invalid);
    }
    if total == 0 {
        return Ok(vec![0; gross.len()]);
    }
    let mut result = Vec::with_capacity(gross.len());
    let mut remainder = Vec::with_capacity(gross.len());
    for (index, g) in gross.iter().enumerate() {
        let n = i128::from(*g) * i128::from(discount);
        result.push((n / i128::from(total)) as i64);
        remainder.push((n % i128::from(total), index));
    }
    let remaining = discount - result.iter().sum::<i64>();
    remainder.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, index) in remainder
        .into_iter()
        .take(usize::try_from(remaining).map_err(|_| PriceError::Overflow)?)
    {
        result[index] += 1
    }
    Ok(result)
}
impl Carts {
    pub async fn set_coupon(
        &self,
        g: &Guest,
        csrf: &str,
        revision: i64,
        code: &str,
        language: Language,
    ) -> crate::cart::Result<()> {
        let mut tx = self.pool.begin().await?;
        crate::taxes::charges::lock(&mut tx)
            .await
            .map_err(|_| CartError::Unavailable)?;
        self.lock(&mut tx, g, csrf, revision).await?;
        let id: Option<i64> = if code.trim().is_empty() {
            None
        } else {
            let code = normalize(code).ok_or(CartError::Coupon)?;
            Some(
                sqlx::query_scalar("SELECT id FROM coupons WHERE code=$1")
                    .bind(code)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or(CartError::Coupon)?,
            )
        };
        sqlx::query("UPDATE guest_carts SET coupon_id=$2 WHERE id=$1")
            .bind(g.id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if id.is_some() {
            let q = Self::quote_in(&mut tx, g, language, None).await?;
            if q.coupon_invalid {
                return Err(CartError::Coupon);
            }
        }
        Self::changed(&mut tx, g.id).await?;
        tx.commit().await?;
        Ok(())
    }
}
pub(crate) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    g: &Guest,
    q: &mut Quote,
) -> crate::cart::Result<()> {
    let id: Option<i64> = sqlx::query_scalar("SELECT coupon_id FROM guest_carts WHERE id=$1")
        .bind(g.id)
        .fetch_one(&mut **tx)
        .await?;
    let Some(id) = id else { return Ok(()) };
    apply_id(tx, id, q).await
}

pub(crate) async fn apply_id(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    q: &mut Quote,
) -> crate::cart::Result<()> {
    let row: Option<(String, String, i64, i64, bool)> = sqlx::query_as(
        "SELECT c.code,c.kind,c.value,c.revision,c.active \
            AND (c.kind='percent' \
                OR c.currency=s.currency) \
            AND (c.expires_on IS NULL \
                OR c.expires_on >= (statement_timestamp() AT TIME ZONE r.time_zone)::date) \
            AND (c.usage_limit IS NULL \
                OR c.usage_limit>(SELECT count(*) \
                    FROM coupon_uses u \
                    WHERE u.coupon_id=c.id)) \
            FROM coupons c \
            CROSS \
            JOIN store_settings s \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE c.id=$1 \
            AND s.id=1 \
            AND r.singleton",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((code, kind, value, revision, valid)) = row else {
        return Err(CartError::Coupon);
    };
    q.coupon_code = code.clone();
    if !valid {
        q.coupon_invalid = true;
        q.totals = None;
        return Ok(());
    }
    apply_snapshot(
        q,
        Snapshot {
            id,
            code,
            kind,
            value,
            revision,
            original_cents: 0,
            discount_cents: 0,
            lines: Vec::new(),
        },
    )
}

/// Reallocate the already redeemed commercial terms when revising an order.
/// It neither redeems again nor changes the original usage ledger.
pub(crate) fn apply_snapshot(q: &mut Quote, snapshot: Snapshot) -> crate::cart::Result<()> {
    let Snapshot {
        id,
        code,
        kind,
        value,
        revision,
        ..
    } = snapshot;
    let Some(total) = q.totals else { return Ok(()) };
    let gross = total.gross().cents();
    let discount = if kind == "fixed" {
        value.min(gross)
    } else {
        i64::try_from((i128::from(gross) * i128::from(value) + 5000) / 10000)
            .map_err(|_| CartError::Invalid)?
    };
    let amounts: Vec<i64> = q
        .lines
        .iter()
        .map(|l| l.totals.map_or(0, |t| t.gross().cents()))
        .collect();
    let allocated = allocate(&amounts, discount).map_err(|_| CartError::Invalid)?;
    let mut lines = vec![];
    let mut totals = vec![];
    for (position, (line, discount)) in q.lines.iter_mut().zip(allocated).enumerate() {
        let original = amounts[position];
        let item = line.current.as_ref().ok_or(CartError::Unavailable)?;
        let amount = if discount == 0 {
            line.totals.ok_or(CartError::Unavailable)?
        } else {
            pricing::line(
                Money::from_cents(original - discount).map_err(|_| CartError::Invalid)?,
                1,
                Rate::from_basis_points(item.basis_points).map_err(|_| CartError::Invalid)?,
                PriceMode::Inclusive,
            )
            .map_err(|_| CartError::Invalid)?
        };
        line.totals = Some(amount);
        totals.push(amount);
        lines.push(LineDiscount {
            position,
            original_cents: original,
            discount_cents: discount,
        });
    }
    q.totals = Some(pricing::total(&totals).map_err(|_| CartError::Invalid)?);
    q.coupon = Some(Snapshot {
        id,
        code,
        kind,
        value,
        revision,
        original_cents: gross,
        discount_cents: discount,
        lines,
    });
    Ok(())
}
impl From<IdentityError> for CouponError {
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
    fn allocation_is_exact_bounded_and_stable() {
        assert_eq!(allocate(&[1, 1, 1], 2).unwrap(), vec![1, 1, 0]);
        assert_eq!(allocate(&[0, 100, 300], 100).unwrap(), vec![0, 25, 75]);
        for a in 0..20 {
            for b in 0..20 {
                for d in 0..=a + b {
                    let v = allocate(&[a, b], d).unwrap();
                    assert_eq!(v.iter().sum::<i64>(), d);
                    assert!(v[0] <= a && v[1] <= b);
                }
            }
        }
        assert!(allocate(&[i64::MAX, 1], 1).is_err());
        assert!(allocate(&[1], 2).is_err());
        assert!(allocate(&[-1], 0).is_err());
    }
    #[test]
    fn inputs_are_bounded() {
        assert_eq!(normalize(" sale-10 ").as_deref(), Some("SALE-10"));
        for s in ["", "é", "A B", "<x>"] {
            assert!(normalize(s).is_none())
        }
        for s in ["2028-02-29", "2026-12-31", ""] {
            assert!(date_valid(s))
        }
        for s in [
            "2026-02-29",
            "2026-13-01",
            "2026-01-00",
            "2000-1-1",
            "infinity",
        ] {
            assert!(!date_valid(s))
        }
    }
}
