//! Order labels and colors. A label does not prove payment or trigger commercial effects.
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use talocart_platform::identity::{Identity, IdentityError, Permission, Session};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StatusError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid order status fields")]
    Invalid(Vec<&'static str>),
    #[error("Configuration changed")]
    Stale,
    #[error("Order status not found")]
    NotFound,
    #[error("Order status is assigned to a payment method")]
    InUse,
    #[error("An order status already has this name")]
    Duplicate,
    #[error("Order status capacity reached")]
    Capacity,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for StatusError {
    fn from(e: sqlx::Error) -> Self {
        if let Some(db) = e.as_database_error() {
            if db.is_unique_violation() {
                return Self::Duplicate;
            }
            if db.is_foreign_key_violation() {
                return Self::InUse;
            }
            if db.constraint() == Some("order_statuses_capacity") {
                return Self::Capacity;
            }
        }
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
        )
    }
}
pub type Result<T> = std::result::Result<T, StatusError>;
#[derive(Clone, Debug, Default)]
pub struct StatusInput {
    pub name_it: String,
    pub name_en: String,
    pub color: String,
    pub sort_order: String,
}
#[derive(Clone, Debug, FromRow)]
pub struct OrderStatus {
    #[sqlx(default)]
    pub role: String,
    pub id: i64,
    pub name_it: String,
    pub name_en: String,
    pub color: String,
    pub sort_order: i32,
}
impl OrderStatus {
    pub fn initial_allowed(&self) -> bool {
        matches!(self.role.as_str(), "" | "custom" | "preparing")
    }
    pub fn role_key(&self) -> &str {
        match self.role.as_str() {
            "preparing" => "status-role-preparing",
            "shipped" => "status-role-shipped",
            "delivered" => "status-role-delivered",
            "cancelled" => "status-role-cancelled",
            "refunded" => "status-role-refunded",
            _ => "status-role-custom",
        }
    }
    pub fn name(&self, locale: &str) -> &str {
        if locale == "it" {
            &self.name_it
        } else {
            &self.name_en
        }
    }
    pub fn input(&self) -> StatusInput {
        StatusInput {
            name_it: self.name_it.clone(),
            name_en: self.name_en.clone(),
            color: self.color.clone(),
            sort_order: self.sort_order.to_string(),
        }
    }
}
pub fn valid_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}
fn validate(i: &StatusInput) -> Result<(String, i32)> {
    let mut errors = Vec::new();
    for (key, value) in [("name_it", &i.name_it), ("name_en", &i.name_en)] {
        if value.trim().is_empty()
            || value.chars().count() > 120
            || value.chars().any(char::is_control)
        {
            errors.push(key);
        }
    }
    if !valid_color(&i.color) {
        errors.push("color");
    }
    let order = i
        .sort_order
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=9999).contains(n));
    if order.is_none() {
        errors.push("sort_order");
    }
    if !errors.is_empty() {
        return Err(StatusError::Invalid(errors));
    }
    Ok((i.color.to_ascii_lowercase(), order.unwrap_or_default()))
}
#[derive(Clone)]
pub struct OrderStatuses {
    pool: PgPool,
    identity: Identity,
}
impl OrderStatuses {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn list(
        &self,
        s: &Session,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<OrderStatus>> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        if !(1..=1000000).contains(&page) {
            return Err(StatusError::Invalid(vec![]));
        }
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(
                "SELECT s.*,coalesce(r.role,'custom') AS role \
                FROM order_statuses s \
                LEFT \
                JOIN order_status_roles r ON r.status_id=s.id",
            ),
            "sort_order,id",
            page,
            20,
        )
        .await?)
    }
    pub async fn editor(&self, s: &Session, id: Option<i64>) -> Result<(i64, Option<OrderStatus>)> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let revision = sqlx::query_scalar("SELECT revision FROM payment_configuration WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let status = if let Some(id) = id {
            Some(
                sqlx::query_as(
                    "SELECT s.*,coalesce(r.role,'custom') AS role \
                    FROM order_statuses s \
                    LEFT \
                    JOIN order_status_roles r ON r.status_id=s.id \
                    WHERE s.id=$1",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(StatusError::NotFound)?,
            )
        } else {
            None
        };
        tx.commit().await?;
        Ok((revision, status))
    }
    async fn guard(
        tx: &mut Transaction<'_, Postgres>,
        s: &Session,
        csrf: &str,
        revision: i64,
    ) -> Result<()> {
        Identity::authorize_transaction(tx, s, csrf, Permission::SettingsWrite).await?;
        // Same order as payment/currency writes; configuration revisions cover both editors.
        talocart_platform::database::commercial_lock::exclusive(tx).await?;
        if sqlx::query(
            "UPDATE payment_configuration SET revision=revision+1 WHERE id=1 AND revision=$1",
        )
        .bind(revision)
        .execute(&mut **tx)
        .await?
        .rows_affected()
            != 1
        {
            return Err(StatusError::Stale);
        }
        Ok(())
    }
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: Option<i64>,
        revision: i64,
        i: &StatusInput,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let (color, order) = validate(i)?;
        let mut tx = self.pool.begin().await?;
        Self::guard(&mut tx, s, csrf, revision).await?;
        let sql = if id.is_some() {
            "UPDATE order_statuses SET name_it=$1,name_en=$2,color=$3,sort_order=$4 WHERE id=$5 RETURNING id"
        } else {
            "INSERT INTO order_statuses(name_it,name_en,color,sort_order) VALUES($1,$2,$3,$4) RETURNING id"
        };
        let mut q = sqlx::query_scalar(sql)
            .bind(i.name_it.trim())
            .bind(i.name_en.trim())
            .bind(color)
            .bind(order);
        if let Some(id) = id {
            q = q.bind(id);
        }
        let saved = q
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StatusError::NotFound)?;
        tx.commit().await?;
        Ok(saved)
    }
    pub async fn delete(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> Result<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let mut tx = self.pool.begin().await?;
        Self::guard(&mut tx, s, csrf, revision).await?;
        if sqlx::query("DELETE FROM order_statuses WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(StatusError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Future checkout must lock the chosen status inside the same order transaction.
    pub async fn require(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<OrderStatus> {
        sqlx::query_as(
            "SELECT s.*,coalesce(r.role,'custom') AS role \
            FROM order_statuses s \
            LEFT \
            JOIN order_status_roles r ON r.status_id=s.id \
            WHERE s.id=$1 FOR SHARE OF s",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(StatusError::NotFound)
    }
}
impl From<IdentityError> for StatusError {
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
    fn colors_and_labels_reject_injection_and_validate_boundaries() {
        for good in ["#ffffff", "#000000", "#A1B2c3"] {
            assert!(valid_color(good));
        }
        for bad in [
            "red",
            "#fff",
            "#ffffff;display:none",
            "#gg0000",
            "#00000é",
            "#123456\n",
        ] {
            assert!(!valid_color(bad));
        }
        let mut i = StatusInput {
            name_it: " Pagato ".into(),
            name_en: "Paid".into(),
            color: "#ABCDEF".into(),
            sort_order: "9999".into(),
        };
        assert_eq!(validate(&i).unwrap(), ("#abcdef".into(), 9999));
        for bad in ["", "\n", "Paid\0"] {
            i.name_en = bad.into();
            assert!(validate(&i).is_err());
        }
        i.name_en = "x".repeat(121);
        assert!(validate(&i).is_err());
        i.name_en = "x".repeat(120);
        i.sort_order = "10000".into();
        assert!(validate(&i).is_err());
    }
}
