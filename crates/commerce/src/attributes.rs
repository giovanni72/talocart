//! Descriptive product specifications owned by the base catalog.
//! All writes share the catalog lock; row revisions reject stale forms.
use crate::content::Resource;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use talocart_platform::{
    content::{ContentError, Draft, Language, Translation},
    identity::{Identity, IdentityError, Permission, Session},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AttributeError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid attribute fields")]
    Invalid,
    #[error("Attribute not found")]
    NotFound,
    #[error("Attribute changed or already assigned")]
    Conflict,
    #[error("Attribute is in use")]
    InUse,
    #[error("Attribute limit reached")]
    Limit,
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for AttributeError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            if db.is_unique_violation() {
                return Self::Conflict;
            }
            if db.is_foreign_key_violation() {
                return Self::InUse;
            }
            if db.constraint() == Some("catalog_attribute_limit") {
                return Self::Limit;
            }
            if db.is_check_violation() {
                return Self::Invalid;
            }
        }
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    }
}
pub type Result<T> = std::result::Result<T, AttributeError>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Group,
    Attribute,
}
impl Kind {
    pub fn resource(self) -> Resource {
        match self {
            Self::Group => Resource::AttributeGroup,
            Self::Attribute => Resource::Attribute,
        }
    }
    pub fn source(self) -> &'static str {
        match self {
            Self::Group => "[[attribute_groups]]",
            Self::Attribute => "[[attributes]]",
        }
    }
}
#[derive(Default, Clone)]
pub struct Values {
    pub name: String,
    pub sort_order: String,
    pub group_id: String,
    pub attribute_id: String,
    pub revision: String,
}
#[derive(FromRow, Clone)]
pub struct Entry {
    pub id: i64,
    pub name: String,
    pub sort_order: i32,
    pub revision: i64,
    pub group_id: Option<i64>,
}
#[derive(FromRow, Clone)]
pub struct ProductValue {
    pub id: i64,
    pub attribute_id: i64,
    pub name: String,
    pub attribute_name: String,
    pub group_name: String,
    pub group_id: i64,
    pub sort_order: i32,
    pub revision: i64,
}
#[derive(Clone)]
pub struct Attributes {
    pool: PgPool,
    identity: Identity,
    language: Option<Language>,
}
impl Attributes {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self {
            pool,
            identity,
            language: None,
        }
    }
    pub fn localized(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }
    async fn write<'a>(&'a self, s: &Session, csrf: &str) -> Result<Transaction<'a, Postgres>> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::ProductsWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        Ok(tx)
    }
    pub async fn entries(&self, s: &Session, kind: Kind) -> Result<Vec<Entry>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        let group = if kind == Kind::Group {
            "NULL::bigint"
        } else {
            "group_id"
        };
        let limit = if kind == Kind::Group { 100 } else { 500 };
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            format!(
                "SELECT id,name,sort_order,revision,{group} AS group_id \
                FROM {} \
                ORDER BY sort_order,lower(name),id \
                LIMIT {limit}",
                kind.source()
            ),
            self.language,
        )))
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn entry(&self, s: &Session, kind: Kind, id: i64) -> Result<Entry> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        let group = if kind == Kind::Group {
            "NULL::bigint"
        } else {
            "group_id"
        };
        sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            format!(
                "SELECT id,name,sort_order,revision,{group} AS group_id FROM {} WHERE id=$1",
                kind.source()
            ),
            self.language,
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AttributeError::NotFound)
    }
    /// Only unassigned choices; the product must exist and the caller must have live read permission.
    pub async fn available_entries(&self, s: &Session, product: i64) -> Result<Vec<Entry>> {
        self.authorize_product(s, product).await?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT a.id,a.name,a.sort_order,a.revision,a.group_id \
                FROM [[attributes]] a \
                WHERE NOT EXISTS(SELECT 1 \
                    FROM catalog_product_attributes v \
                    WHERE v.product_id=$1 \
                    AND v.attribute_id=a.id) \
                ORDER BY a.sort_order,lower(a.name),a.id \
                LIMIT 500",
            self.language,
        )))
        .bind(product)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Product ownership is part of the lookup, before any translation editor is loaded.
    pub async fn product_value(&self, s: &Session, product: i64, id: i64) -> Result<ProductValue> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        self.values(product, false, Some(id))
            .await?
            .into_iter()
            .next()
            .ok_or(AttributeError::NotFound)
    }

    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        kind: Kind,
        id: Option<i64>,
        values: &Values,
        draft: Option<&Draft>,
    ) -> Result<i64> {
        let mut tx = self.write(s, csrf).await?;
        let order = order(&values.sort_order)?;
        let definition = kind.resource().definition();
        let translations = definition
            .prepare(
                &mut tx,
                draft,
                id.unwrap_or(0),
                Translation {
                    name: values.name.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let name = values.name.trim();
        let group = if kind == Kind::Attribute {
            let group = positive_id(&values.group_id)?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM catalog_attribute_groups WHERE id=$1)",
            )
            .bind(group)
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                return Err(AttributeError::Invalid);
            }
            Some(group)
        } else {
            None
        };
        let table = definition.table;
        let new_id: i64 = if let Some(id) = id {
            let revision = revision(&values.revision)?;
            let sql = if kind == Kind::Attribute {
                format!(
                    "UPDATE {table} SET name=$1,sort_order=$2,group_id=$5 WHERE id=$3 AND revision=$4 RETURNING id"
                )
            } else {
                format!(
                    "UPDATE {table} SET name=$1,sort_order=$2 WHERE id=$3 AND revision=$4 RETURNING id"
                )
            };
            let mut q = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(name)
                .bind(order)
                .bind(id)
                .bind(revision);
            if let Some(group) = group {
                q = q.bind(group);
            }
            q.fetch_optional(&mut *tx)
                .await?
                .ok_or(AttributeError::Conflict)?
        } else {
            let sql = if kind == Kind::Attribute {
                format!(
                    "INSERT INTO {table}(name,sort_order,group_id) VALUES($1,$2,$3) RETURNING id"
                )
            } else {
                format!("INSERT INTO {table}(name,sort_order) VALUES($1,$2) RETURNING id")
            };
            let mut q = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(name)
                .bind(order);
            if let Some(group) = group {
                q = q.bind(group);
            }
            q.fetch_one(&mut *tx).await?
        };
        definition.persist(&mut tx, new_id, &translations).await?;
        tx.commit().await?;
        Ok(new_id)
    }
    pub async fn delete(
        &self,
        s: &Session,
        csrf: &str,
        kind: Kind,
        id: i64,
        expected: i64,
    ) -> Result<()> {
        let mut tx = self.write(s, csrf).await?;
        let sql = format!(
            "DELETE FROM {} WHERE id=$1 AND revision=$2",
            kind.resource().definition().table
        );
        if sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(expected)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(AttributeError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
    async fn authorize_product(&self, s: &Session, product: i64) -> Result<()> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_products WHERE id=$1)")
                .bind(product)
                .fetch_one(&self.pool)
                .await?;
        if !exists {
            return Err(AttributeError::NotFound);
        }
        Ok(())
    }
    pub async fn product_values(&self, s: &Session, product: i64) -> Result<Vec<ProductValue>> {
        self.authorize_product(s, product).await?;
        Ok(self.values(product, false, None).await?)
    }
    /// Internal composition only: Products::editor_entry has authorized and loaded this product.
    /// Never expose this helper directly through an HTTP handler.
    pub(crate) async fn editor_values(
        &self,
        product: i64,
    ) -> std::result::Result<Vec<ProductValue>, sqlx::Error> {
        self.values(product, false, None).await
    }
    /// One bounded query for all groups, labels and values; shared by full and HTMX pages.
    /// Visibility is checked in the same SQL snapshot, including direct category/brand status.
    pub async fn public_values(&self, product: i64) -> Result<Vec<ProductValue>> {
        Ok(self.values(product, true, None).await?)
    }
    async fn values(
        &self,
        product: i64,
        public: bool,
        id: Option<i64>,
    ) -> std::result::Result<Vec<ProductValue>, sqlx::Error> {
        let visible = if public {
            "AND EXISTS(SELECT 1 FROM catalog_visible_products p WHERE p.id=v.product_id)"
        } else if id.is_some() {
            "AND v.id=$2"
        } else {
            ""
        };
        let sql = crate::content::query(
            format!(
                "SELECT v.id,v.attribute_id,v.name,a.name AS attribute_name,g.name AS group_name,g.id AS group_id,\
                    v.sort_order,v.revision \
                    FROM [[product_attributes]] v \
                    JOIN [[attributes]] a ON a.id=v.attribute_id \
                    JOIN [[attribute_groups]] g ON g.id=a.group_id \
                    WHERE v.product_id=$1 {visible} \
                    ORDER BY g.sort_order,lower(g.name),g.id,v.sort_order,a.sort_order,lower(a.name),\
                    a.id \
                    LIMIT 100"
            ),
            self.language,
        );
        let mut query = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(product);
        if let Some(id) = id {
            query = query.bind(id);
        }
        query.fetch_all(&self.pool).await
    }
    pub async fn save_product(
        &self,
        s: &Session,
        csrf: &str,
        product: i64,
        id: Option<i64>,
        values: &Values,
        draft: Option<&Draft>,
    ) -> Result<i64> {
        let mut tx = self.write(s, csrf).await?;
        let order = order(&values.sort_order)?;
        let definition = Resource::ProductAttribute.definition();
        let translations = definition
            .prepare(
                &mut tx,
                draft,
                id.unwrap_or(0),
                Translation {
                    name: values.name.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_products WHERE id=$1)")
                .bind(product)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(AttributeError::NotFound);
        }
        let result = if let Some(id) = id {
            sqlx::query_scalar(
                "UPDATE catalog_product_attributes \
                SET name=$1,sort_order=$2 \
                WHERE id=$3 \
                AND product_id=$4 \
                AND revision=$5 \
                RETURNING id",
            )
            .bind(values.name.trim())
            .bind(order)
            .bind(id)
            .bind(product)
            .bind(revision(&values.revision)?)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AttributeError::Conflict)?
        } else {
            let attribute = positive_id(&values.attribute_id)?;
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_attributes WHERE id=$1)")
                    .bind(attribute)
                    .fetch_one(&mut *tx)
                    .await?;
            if !exists {
                return Err(AttributeError::Invalid);
            }
            sqlx::query_scalar(
                "INSERT INTO catalog_product_attributes(product_id,attribute_id,name,sort_order) \
                VALUES($1,$2,$3,$4) \
                RETURNING id",
            )
            .bind(product)
            .bind(attribute)
            .bind(values.name.trim())
            .bind(order)
            .fetch_one(&mut *tx)
            .await?
        };
        definition.persist(&mut tx, result, &translations).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn remove_product(
        &self,
        s: &Session,
        csrf: &str,
        product: i64,
        id: i64,
        expected: i64,
    ) -> Result<()> {
        let mut tx = self.write(s, csrf).await?;
        if sqlx::query(
            "DELETE FROM catalog_product_attributes WHERE id=$1 AND product_id=$2 AND revision=$3",
        )
        .bind(id)
        .bind(product)
        .bind(expected)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 1
        {
            return Err(AttributeError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}
pub fn order(value: &str) -> Result<i32> {
    value
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=9999).contains(n))
        .ok_or(AttributeError::Invalid)
}
pub fn revision(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .ok_or(AttributeError::Invalid)
}
fn positive_id(value: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or(AttributeError::Invalid)
}
impl From<IdentityError> for AttributeError {
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
    fn orders_and_revisions_reject_overflow_negative_and_untrusted_input() {
        for input in [
            "-1",
            "10000",
            "99999999999999999999999",
            "1.5",
            "<script>",
            "",
        ] {
            assert!(order(input).is_err());
        }
        assert_eq!(order("0").unwrap(), 0);
        assert_eq!(order("9999").unwrap(), 9999);
        for input in ["-1", "", "9223372036854775808"] {
            assert!(revision(input).is_err());
        }
        assert_eq!(revision("0").unwrap(), 0);
        for input in ["-1", "0", "", "9223372036854775808"] {
            assert!(positive_id(input).is_err());
        }
    }
}
