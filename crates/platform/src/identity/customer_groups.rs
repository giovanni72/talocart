//! Customer membership administration. Registration uses the database default atomically.
use super::*;
mod member_list;
pub use member_list::{MemberFilters, MemberOrder, MemberSort};

pub const MAX_GROUPS: i64 = 100;
pub const MAX_QUANTITY_TIERS: usize = 20;
#[derive(Clone, Default)]
pub struct QuantityTierInput {
    pub min_quantity: String,
    pub percent: String,
}
#[derive(Clone, Debug, FromRow)]
pub struct QuantityTier {
    pub min_quantity: i64,
    pub discount_basis_points: i32,
}
impl QuantityTier {
    fn input(&self) -> QuantityTierInput {
        QuantityTierInput {
            min_quantity: self.min_quantity.to_string(),
            percent: format!(
                "{}.{:02}",
                self.discount_basis_points / 100,
                self.discount_basis_points % 100
            ),
        }
    }
}
fn parse_percent(value: &str) -> Option<i32> {
    let mut parts = value.trim().split(['.', ',']);
    let whole = parts.next()?;
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let fraction = match parts.next() {
        None => 0,
        Some(v) if !v.is_empty() && v.len() <= 2 && v.bytes().all(|b| b.is_ascii_digit()) => {
            v.parse::<i32>().ok()? * if v.len() == 1 { 10 } else { 1 }
        }
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    whole
        .parse::<i32>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction)
        .filter(|v| (1..=10000).contains(v))
}
#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error("Invalid customer group fields")]
    Invalid(Vec<&'static str>),
    #[error("Customer group name already exists")]
    Duplicate,
    #[error("Customer group limit reached")]
    Capacity,
    #[error("Customer group or customer not found")]
    NotFound,
    #[error("Customer group configuration or customer changed")]
    Stale,
    #[error("Customer group is assigned or is the default")]
    InUse,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for GroupError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            if db.is_unique_violation() {
                return Self::Duplicate;
            }
            if db.is_foreign_key_violation() {
                return Self::InUse;
            }
            if db.constraint() == Some("customer_groups_capacity") {
                return Self::Capacity;
            }
        }
        Self::Technical(crate::technical::DatabaseFailure::capture(error, module_path!()).into())
    }
}
type GroupResult<T> = std::result::Result<T, GroupError>;
#[derive(Clone, Default)]
pub struct GroupInput {
    pub name_it: String,
    pub name_en: String,
    pub is_default: bool,
    pub quantity_discounts_enabled: bool,
    pub quantity_discounts_for_guests: bool,
    pub quantity_tiers: Vec<QuantityTierInput>,
}
impl GroupInput {
    pub fn validated_tiers(&self) -> GroupResult<Vec<QuantityTier>> {
        let invalid = || GroupError::Invalid(vec!["quantity_tiers"]);
        if self.quantity_tiers.len() > MAX_QUANTITY_TIERS
            || (self.quantity_discounts_enabled && self.quantity_tiers.is_empty())
        {
            return Err(invalid());
        }
        let mut tiers = self
            .quantity_tiers
            .iter()
            .map(|input| {
                let quantity = input.min_quantity.trim();
                if quantity.is_empty() || !quantity.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid());
                }
                Ok(QuantityTier {
                    min_quantity: quantity
                        .parse::<i64>()
                        .ok()
                        .filter(|v| (1..=99999900).contains(v))
                        .ok_or_else(invalid)?,
                    discount_basis_points: parse_percent(&input.percent).ok_or_else(invalid)?,
                })
            })
            .collect::<GroupResult<Vec<_>>>()?;
        tiers.sort_by_key(|t| t.min_quantity);
        if tiers
            .windows(2)
            .any(|t| t[0].min_quantity == t[1].min_quantity)
        {
            return Err(invalid());
        }
        Ok(tiers)
    }
    fn validate(&self) -> GroupResult<()> {
        let errors: Vec<_> = [("name_it", &self.name_it), ("name_en", &self.name_en)]
            .into_iter()
            .filter_map(|(field, value)| {
                (value.trim().is_empty()
                    || value.trim().chars().count() > 120
                    || value.chars().any(char::is_control))
                .then_some(field)
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(GroupError::Invalid(errors))
        }
    }
}
#[derive(Clone, FromRow)]
pub struct CustomerGroup {
    pub id: i64,
    pub name_it: String,
    pub name_en: String,
    pub is_default: bool,
    pub quantity_discounts_enabled: bool,
    pub quantity_discounts_for_guests: bool,
    #[sqlx(skip)]
    pub quantity_tiers: Vec<QuantityTier>,
}
impl CustomerGroup {
    pub fn name(&self, locale: &str) -> &str {
        if locale == "it" {
            &self.name_it
        } else {
            &self.name_en
        }
    }
    pub fn input(&self) -> GroupInput {
        GroupInput {
            name_it: self.name_it.clone(),
            name_en: self.name_en.clone(),
            is_default: self.is_default,
            quantity_discounts_enabled: self.quantity_discounts_enabled,
            quantity_discounts_for_guests: self.quantity_discounts_for_guests,
            quantity_tiers: self
                .quantity_tiers
                .iter()
                .map(QuantityTier::input)
                .collect(),
        }
    }
}
#[derive(FromRow)]
pub struct Member {
    pub id: i64,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub revision: i64,
    pub active: bool,
    pub customer_type: String,
    pub group_id: i64,
    pub name_it: String,
    pub name_en: String,
}
impl Member {
    pub fn group_name(&self, locale: &str) -> &str {
        if locale == "it" {
            &self.name_it
        } else {
            &self.name_en
        }
    }
}
#[derive(Clone)]
pub struct CustomerGroups {
    pool: PgPool,
    identity: Identity,
}
const GROUPS: &str = "SELECT g.*,g.id=c.default_group_id AS is_default FROM customer_groups g CROSS JOIN customer_group_configuration c WHERE c.id=1";
const MEMBERS: &str = "SELECT c.id,c.email,c.first_name,c.last_name,c.revision,c.active,c.customer_type,c.group_id,g.name_it,g.name_en FROM customers c JOIN customer_groups g ON g.id=c.group_id";
impl CustomerGroups {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn list(
        &self,
        s: &Session,
        page: i64,
    ) -> GroupResult<crate::pagination::Page<CustomerGroup>> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        valid_page(page)?;
        Ok(crate::pagination::read(&self.pool, sqlx::query(GROUPS), "id", page, 20).await?)
    }
    pub async fn editor(
        &self,
        s: &Session,
        id: Option<i64>,
    ) -> GroupResult<(i64, Option<CustomerGroup>)> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let revision =
            sqlx::query_scalar("SELECT revision FROM customer_group_configuration WHERE id=1")
                .fetch_one(&mut *tx)
                .await?;
        let mut group: Option<CustomerGroup> = if let Some(id) = id {
            Some(
                sqlx::query_as(sqlx::AssertSqlSafe(format!("{GROUPS} AND g.id=$1")))
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or(GroupError::NotFound)?,
            )
        } else {
            None
        };
        if let Some(group) = &mut group {
            group.quantity_tiers = sqlx::query_as("SELECT min_quantity,discount_basis_points FROM customer_group_quantity_tiers WHERE group_id=$1 ORDER BY min_quantity LIMIT 20")
                .bind(group.id).fetch_all(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok((revision, group))
    }
    async fn guard(
        tx: &mut Transaction<'_, Postgres>,
        s: &Session,
        csrf: &str,
        revision: i64,
    ) -> GroupResult<()> {
        Identity::authorize_transaction(tx, s, csrf, Permission::CustomersWrite).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(5426645964561798231)")
            .execute(&mut **tx)
            .await?;
        if sqlx::query("UPDATE customer_group_configuration SET revision=revision+1 WHERE id=1 AND revision=$1").bind(revision).execute(&mut **tx).await?.rows_affected() != 1 {
            return Err(GroupError::Stale);
        }
        Ok(())
    }
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: Option<i64>,
        revision: i64,
        input: &GroupInput,
    ) -> GroupResult<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::CustomersWrite)?;
        input.validate()?;
        let tiers = input.validated_tiers()?;
        let mut tx = self.pool.begin().await?;
        Self::guard(&mut tx, s, csrf, revision).await?;
        // The default cannot be unset: another group must replace it explicitly.
        let id: i64 = if let Some(id) = id {
            sqlx::query_scalar(
                "UPDATE customer_groups SET name_it=$1,name_en=$2 WHERE id=$3 RETURNING id",
            )
            .bind(input.name_it.trim())
            .bind(input.name_en.trim())
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(GroupError::NotFound)?
        } else {
            sqlx::query_scalar(
                "INSERT INTO customer_groups(name_it,name_en) VALUES($1,$2) RETURNING id",
            )
            .bind(input.name_it.trim())
            .bind(input.name_en.trim())
            .fetch_one(&mut *tx)
            .await?
        };
        sqlx::query("UPDATE customer_groups SET quantity_discounts_enabled=$1,quantity_discounts_for_guests=$3 WHERE id=$2")
            .bind(input.quantity_discounts_enabled)
            .bind(id)
            .bind(input.quantity_discounts_for_guests)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM customer_group_quantity_tiers WHERE group_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (position, tier) in tiers.iter().enumerate() {
            sqlx::query("INSERT INTO customer_group_quantity_tiers(group_id,position,min_quantity,discount_basis_points) VALUES($1,$2,$3,$4)")
                .bind(id).bind(i16::try_from(position).map_err(|_| GroupError::Invalid(vec!["quantity_tiers"]))?)
                .bind(tier.min_quantity).bind(tier.discount_basis_points).execute(&mut *tx).await?;
        }
        if input.is_default {
            sqlx::query("UPDATE customer_group_configuration SET default_group_id=$1 WHERE id=1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> GroupResult<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::CustomersWrite)?;
        let mut tx = self.pool.begin().await?;
        Self::guard(&mut tx, s, csrf, revision).await?;
        if sqlx::query("DELETE FROM customer_groups WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(GroupError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn members(
        &self,
        s: &Session,
        page: i64,
    ) -> GroupResult<crate::pagination::Page<Member>> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        valid_page(page)?;
        Ok(crate::pagination::read(&self.pool, sqlx::query(MEMBERS), "id DESC", page, 20).await?)
    }
    /// Bounded group choices for customer creation; the database caps this list.
    pub async fn choices(&self, s: &Session) -> GroupResult<Vec<CustomerGroup>> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{GROUPS} ORDER BY g.id LIMIT $1"
        )))
        .bind(MAX_GROUPS)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn member_editor(
        &self,
        s: &Session,
        id: i64,
    ) -> GroupResult<(Member, Vec<CustomerGroup>)> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let member = sqlx::query_as(sqlx::AssertSqlSafe(format!("{MEMBERS} WHERE c.id=$1")))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(GroupError::NotFound)?;
        let groups = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{GROUPS} ORDER BY g.id LIMIT $1"
        )))
        .bind(MAX_GROUPS)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((member, groups))
    }
    pub async fn assign(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        group_id: i64,
    ) -> GroupResult<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::CustomersWrite)?;
        if group_id <= 0 {
            return Err(GroupError::Invalid(vec!["group_id"]));
        }
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::CustomersWrite).await?;
        if sqlx::query_scalar::<_, i64>("SELECT id FROM customer_groups WHERE id=$1 FOR KEY SHARE")
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Err(GroupError::Invalid(vec!["group_id"]));
        }
        let current: i64 =
            sqlx::query_scalar("SELECT revision FROM customers WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(GroupError::NotFound)?;
        if current != revision {
            return Err(GroupError::Stale);
        }
        sqlx::query("UPDATE customers SET group_id=$1,revision=revision+1 WHERE id=$2")
            .bind(group_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
fn valid_page(page: i64) -> GroupResult<()> {
    if (1..=1_000_000).contains(&page) {
        Ok(())
    } else {
        Err(GroupError::Invalid(vec![]))
    }
}
/// An ordered schedule owned by identity, resolved for a customer or for guests.
#[derive(Clone, Debug)]
pub struct QuantitySchedule {
    pub id: i64,
    pub name_it: String,
    pub name_en: String,
    pub tiers: Vec<QuantityTier>,
}
impl QuantitySchedule {
    /// Thresholds are inclusive; the highest reached threshold wins.
    pub fn tier_for(&self, quantity: i64) -> Option<&QuantityTier> {
        self.tiers.iter().rev().find(|t| t.min_quantity <= quantity)
    }
}
/// Call after authorizing the cart in the same transaction. None means an
/// authenticated guest capability, not a fallback for an invalid customer.
/// One statement resolves current membership/default and its complete schedule.
pub async fn quantity_schedule_in(
    tx: &mut Transaction<'_, Postgres>,
    customer_id: Option<i64>,
) -> std::result::Result<Option<QuantitySchedule>, IdentityError> {
    let rows: Vec<(i64, String, String, i64, i32)> = sqlx::query_as("SELECT g.id,g.name_it,g.name_en,t.min_quantity,t.discount_basis_points FROM customer_groups g JOIN customer_group_quantity_tiers t ON t.group_id=g.id WHERE g.id=CASE WHEN $1::bigint IS NULL THEN (SELECT default_group_id FROM customer_group_configuration WHERE id=1) ELSE (SELECT group_id FROM customers WHERE id=$1) END AND g.quantity_discounts_enabled AND ($1::bigint IS NOT NULL OR g.quantity_discounts_for_guests) ORDER BY t.min_quantity LIMIT 20")
        .bind(customer_id).fetch_all(&mut **tx).await?;
    let Some((id, name_it, name_en, _, _)) = rows.first() else {
        return Ok(None);
    };
    Ok(Some(QuantitySchedule {
        id: *id,
        name_it: name_it.clone(),
        name_en: name_en.clone(),
        tiers: rows
            .into_iter()
            .map(
                |(_, _, _, min_quantity, discount_basis_points)| QuantityTier {
                    min_quantity,
                    discount_basis_points,
                },
            )
            .collect(),
    }))
}

impl From<IdentityError> for GroupError {
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
    fn quantity_tiers_validate_exact_percentages_thresholds_and_disabled_schedules() {
        let tier = |q: &str, p: &str| QuantityTierInput {
            min_quantity: q.into(),
            percent: p.into(),
        };
        let mut input = GroupInput {
            quantity_discounts_enabled: true,
            quantity_tiers: vec![tier("50", "10"), tier("10", "2,55")],
            ..GroupInput::default()
        };
        let valid = input.validated_tiers().unwrap();
        assert_eq!(valid[0].min_quantity, 10);
        assert_eq!(valid[0].discount_basis_points, 255);
        for bad in [
            "0",
            "-1",
            "100.01",
            "1.001",
            "NaN",
            "1e1",
            "",
            ".5",
            "99999999999999999999",
        ] {
            input.quantity_tiers = vec![tier("1", bad)];
            assert!(input.validated_tiers().is_err(), "{bad}");
        }
        for bad in ["0", "-1", "1.5", "+1", "99999901", "99999999999999999999"] {
            input.quantity_tiers = vec![tier(bad, "10")];
            assert!(input.validated_tiers().is_err(), "{bad}");
        }
        input.quantity_tiers = vec![tier("1", "0.01"), tier("01", "100")];
        assert!(input.validated_tiers().is_err());
        input.quantity_tiers.clear();
        assert!(input.validated_tiers().is_err());
        input.quantity_discounts_enabled = false;
        assert!(input.validated_tiers().unwrap().is_empty());
        input.quantity_tiers = vec![tier("1", "100")];
        assert_eq!(
            input.validated_tiers().unwrap()[0].discount_basis_points,
            10000
        );
        input.quantity_tiers = (1..=21).map(|q| tier(&q.to_string(), "1")).collect();
        assert!(input.validated_tiers().is_err());
    }
    #[test]
    fn group_names_validate_both_languages_and_limits() {
        let valid = GroupInput {
            name_it: " Rivenditori ".into(),
            name_en: "Resellers".into(),
            is_default: false,
            ..GroupInput::default()
        };
        assert!(valid.validate().is_ok());
        for input in [
            GroupInput {
                name_en: " ".into(),
                ..valid.clone()
            },
            GroupInput {
                name_it: "a".repeat(121),
                ..valid.clone()
            },
            GroupInput {
                name_it: "Name\n".into(),
                ..valid.clone()
            },
        ] {
            assert!(matches!(input.validate(), Err(GroupError::Invalid(_))));
        }
    }
}
