//! Transaction-scoped catalog intake for trusted background integrations.
//! The caller owns commit/rollback and source receipts; owner methods own commercial SQL.
use sqlx::{Postgres, Transaction};
use talocart_platform::technical::{AccessError, DatabaseFailure, TechnicalError};
use talocart_platform::{
    content::Translation,
    identity::{Identity, IdentityError, Permission},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    Brand,
    Product,
    Category,
    Variant,
    VariantLabel,
    Combination,
    Image,
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid imported catalog data: {0:?}")]
    Invalid(Issue),
    #[error("Imported catalog data conflicts with local records")]
    Conflict,
    #[error(transparent)]
    Identity(IdentityError),
    #[error(transparent)]
    Access(#[from] AccessError),
    #[error(transparent)]
    Technical(#[from] TechnicalError),
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
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Self::Conflict
        } else {
            Self::Technical(DatabaseFailure::capture(error, module_path!()).into())
        }
    }
}
impl From<talocart_platform::content::ContentError> for Error {
    fn from(error: talocart_platform::content::ContentError) -> Self {
        match error {
            talocart_platform::content::ContentError::Conflict => Self::Conflict,
            talocart_platform::content::ContentError::Technical(error) => Self::from(error),
            talocart_platform::content::ContentError::Access(error) => Self::from(error),
            talocart_platform::content::ContentError::Identity(error) => Self::from(error),
            _ => Self::Technical(TechnicalError::Unavailable),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Holds an exclusive borrow of the caller transaction. Identity's mutation lock
/// keeps the active actor/permission stable until the caller commits or rolls back.
/// Source lease and receipt checks remain the integration's responsibility.
/// On any error the caller rolls back its transaction or item savepoint before
/// continuing. Media publishers acquire publish_lock before authorizing a writer.
pub struct Writer<'a, 'c> {
    pub(crate) tx: &'a mut Transaction<'c, Postgres>,
    permission: Permission,
}
impl<'a, 'c> Writer<'a, 'c> {
    pub async fn authorize_in(
        tx: &'a mut Transaction<'c, Postgres>,
        actor: i64,
        permission: Permission,
    ) -> Result<Self> {
        if !matches!(
            permission,
            Permission::BrandsWrite | Permission::CategoriesWrite | Permission::ProductsWrite
        ) {
            return Err(IdentityError::Forbidden.into());
        }
        Identity::authorize_background(tx, actor, permission).await?;
        crate::taxes::product_write_lock(tx).await?;
        Ok(Self { tx, permission })
    }
    pub(crate) fn require(&self, permission: Permission) -> Result<()> {
        if self.permission == permission {
            Ok(())
        } else {
            Err(IdentityError::Forbidden.into())
        }
    }
}

pub struct Brand {
    pub name: String,
    pub slug: String,
    pub translations: Vec<Translation>,
}
pub struct Category {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub parent_id: Option<i64>,
    pub sort_order: i32,
    pub active: bool,
    pub translations: Vec<Translation>,
}
pub struct Product {
    pub name: String,
    pub model: String,
    pub slug: String,
    pub description: String,
    pub price_cents: i64,
    pub stock: i32,
    pub minimum: i32,
    pub packs: i32,
    pub brand_id: i64,
    pub category_id: Option<i64>,
    pub tax_class_id: Option<i64>,
    pub translations: Vec<Translation>,
    pub axes: Vec<Axis>,
    pub variants: Vec<Variant>,
}
pub struct Axis {
    pub italian: String,
    pub english: String,
}
pub struct Value {
    pub label: String,
    pub locale: String,
    pub translations: Vec<Translation>,
}
pub struct Variant {
    pub values: Vec<Value>,
    pub sku: String,
    pub price_cents: i64,
    pub stock: i32,
    pub active: bool,
}
pub struct Images {
    pub primary: Option<String>,
    pub gallery: Vec<String>,
    pub variants: Vec<(String, String)>,
}

pub use crate::catalog::{find_import_brand, lock_import_tree};
pub(crate) fn price(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}
pub(crate) async fn persist(
    tx: &mut Transaction<'_, Postgres>,
    resource: crate::content::Resource,
    id: i64,
    rows: &[Translation],
) -> Result<()> {
    let primary = rows.first().ok_or(Error::Invalid(Issue::Product))?;
    let definition = resource.definition();
    let rows = definition
        .validate(rows, &primary.locale)
        .map_err(|_| Error::Invalid(Issue::Product))?;
    definition.persist(tx, id, &rows).await?;
    Ok(())
}
