//! Taxonomy owner: queries, authorization, validation and category tree.
mod imports;
use crate::content;
pub use imports::{find_import_brand, lock_import_tree};
mod category_tree;
pub use category_tree::CategoryTreeEntry;
use sqlx::{FromRow, PgPool};
use talocart_platform::content::{Draft, Translation};
use talocart_platform::{
    identity::{Identity, IdentityError, Permission, Session},
    media::{MediaError, MediaStore},
};
use thiserror::Error;

pub const PAGE_SIZE: usize = 20;
pub const CATEGORY_LIMIT: i64 = 1000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Category,
    Brand,
}
impl Kind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Category => "/admin/catalog/categories",
            Self::Brand => "/admin/catalog/brands",
        }
    }
    pub fn read(self) -> Permission {
        match self {
            Self::Category => Permission::CategoriesRead,
            Self::Brand => Permission::BrandsRead,
        }
    }
    pub fn write(self) -> Permission {
        match self {
            Self::Category => Permission::CategoriesWrite,
            Self::Brand => Permission::BrandsWrite,
        }
    }
    // Identifiers and SQL fragments are selected exclusively from these static values.
    fn content_source(self) -> &'static str {
        match self {
            Self::Category => "[[categories]]",
            Self::Brand => "[[brands]]",
        }
    }
    fn table(self) -> &'static str {
        match self {
            Self::Category => "catalog_categories",
            Self::Brand => "catalog_brands",
        }
    }
    fn columns(self) -> &'static str {
        match self {
            Self::Category => {
                "id,name,slug,description,sort_order,active,parent_id,NULL::text AS logo_key"
            }
            Self::Brand => {
                "id,name,slug,description,sort_order,active,NULL::bigint AS parent_id,logo_key"
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Slug,
    Description,
    Parent,
    Order,
}
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error(transparent)]
    Content(talocart_platform::content::ContentError),
    #[error("Controlla i campi evidenziati")]
    Invalid(Vec<Field>),
    #[error("Questo slug è già utilizzato")]
    DuplicateSlug,
    #[error("Esiste già un marchio con questo nome")]
    DuplicateName,
    #[error("Categoria superiore non valida: evita cicli e gerarchie oltre 16 livelli")]
    Hierarchy,
    #[error("È stato raggiunto il limite di 1000 categorie")]
    Limit,
    #[error("Voce non trovata")]
    NotFound,
    #[error("La voce è utilizzata. Sposta prima le sottocategorie o le risorse collegate")]
    InUse,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<talocart_platform::content::ContentError> for CatalogError {
    fn from(error: talocart_platform::content::ContentError) -> Self {
        match error {
            talocart_platform::content::ContentError::Conflict => Self::DuplicateName,
            other => Self::Content(other),
        }
    }
}
impl From<sqlx::Error> for CatalogError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            if db.is_unique_violation() {
                return if db.constraint() == Some("catalog_brands_name_unique") {
                    Self::DuplicateName
                } else {
                    Self::DuplicateSlug
                };
            }
            match db.constraint() {
                Some(
                    "catalog_categories_hierarchy"
                    | "catalog_categories_check"
                    | "catalog_categories_parent_id_fkey",
                ) => return Self::Hierarchy,
                Some("catalog_categories_limit") => return Self::Limit,
                _ => {}
            }
        }
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    }
}
impl From<MediaError> for CatalogError {
    fn from(error: MediaError) -> Self {
        Self::Identity(error.into())
    }
}
pub type Result<T> = std::result::Result<T, CatalogError>;
#[derive(Clone, Default)]
pub struct Values {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub parent_id: Option<i64>,
    pub sort_order: String,
    pub active: bool,
}
#[derive(FromRow, Clone)]
pub struct Entry {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub parent_id: Option<i64>,
    pub sort_order: i32,
    pub active: bool,
    pub logo_key: Option<String>,
}
/// Public navigation exposes taxonomy only, including empty categories.
#[derive(FromRow)]
pub struct NavigationCategory {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i64>,
}
/// Public category identity, independent of whether it currently contains products.
#[derive(FromRow)]
pub struct PublicCategory {
    pub name: String,
    pub slug: String,
    pub description: String,
}
pub const PUBLIC_BRAND_PAGE_SIZE: usize = 100;
/// Public taxonomy links include only public logo metadata, without per-brand reads.
#[derive(FromRow)]
pub struct NavigationBrand {
    pub name: String,
    pub slug: String,
    pub logo_id: Option<i64>,
}
const PUBLIC_BRAND_LINKS_SELECT: &str = "SELECT b.name,b.slug,m.id AS logo_id \
    FROM [[brands]] b \
    LEFT \
    JOIN media_objects m ON m.key=b.logo_key \
    AND m.public \
    WHERE b.active";
#[derive(FromRow)]
pub struct PublicBrand {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub logo_key: Option<String>,
}
/// Complete product selector, including inactive categories and their descendants.
#[derive(FromRow)]
pub struct ProductCategory {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub depth: i32,
    pub active: bool,
}
impl Entry {
    pub fn values(&self) -> Values {
        Values {
            name: self.name.clone(),
            slug: self.slug.clone(),
            description: self.description.clone(),
            parent_id: self.parent_id,
            sort_order: self.sort_order.to_string(),
            active: self.active,
        }
    }
}
struct Validated {
    values: Values,
    order: i32,
}
/// Shared ASCII slug contract for stored catalog resources and public filters.
pub(crate) fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}
/// Validate source values using the catalog contract. Callers still authorize and
/// validate references under the appropriate transaction locks before writing.
pub fn check_import_values(input: &Values, kind: Kind) -> Result<()> {
    validate(input, kind).map(|_| ())
}
fn validate(input: &Values, kind: Kind) -> Result<Validated> {
    let mut values = input.clone();
    values.name = values.name.trim().to_owned();
    values.slug = values.slug.trim().to_owned();
    values.description = values.description.trim().to_owned();
    let mut fields = Vec::new();
    if values.name.is_empty()
        || values.name.chars().count() > 120
        || values.name.chars().any(char::is_control)
    {
        fields.push(Field::Name);
    }
    if !valid_slug(&values.slug) {
        fields.push(Field::Slug);
    }
    if values.description.chars().count() > 4000
        || values
            .description
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        fields.push(Field::Description);
    }
    if values.parent_id.is_some_and(|id| id <= 0)
        || (kind == Kind::Brand && values.parent_id.is_some())
    {
        fields.push(Field::Parent);
    }
    let order = values
        .sort_order
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=999999).contains(n));
    if order.is_none() {
        fields.push(Field::Order);
    }
    if !fields.is_empty() {
        return Err(CatalogError::Invalid(fields));
    }
    Ok(Validated {
        values,
        order: order.unwrap_or_default(),
    })
}
#[derive(Clone)]
pub struct Catalog {
    language: Option<talocart_platform::content::Language>,
    pool: PgPool,
    identity: Identity,
    media: MediaStore,
}
impl Catalog {
    pub fn localized(&self, language: talocart_platform::content::Language) -> Self {
        let mut result = self.clone();
        result.language = Some(language);
        result
    }

    /// Counted active brands, including empty ones, for the complete directory.
    pub async fn public_brands_counted(
        &self,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<NavigationBrand>> {
        if !(1..=1_000_000).contains(&page) {
            return Err(CatalogError::Invalid(vec![]));
        }
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(content::query(
                PUBLIC_BRAND_LINKS_SELECT,
                self.language,
            ))),
            "lower(b.name),b.id",
            page,
            PUBLIC_BRAND_PAGE_SIZE as u16,
        )
        .await?)
    }

    /// First page and sentinel for the menu; logo lookup shares the bounded query.
    pub async fn public_brand_navigation(&self) -> Result<Vec<NavigationBrand>> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(content::query(
            format!("{PUBLIC_BRAND_LINKS_SELECT} ORDER BY lower(b.name),b.id LIMIT $1"),
            self.language,
        )))
        .bind((PUBLIC_BRAND_PAGE_SIZE + 1) as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Sentinel API retained for callers that do not need totals.
    pub async fn public_brand_page(&self, page: i64) -> Result<Vec<(String, String)>> {
        if !(1..=1_000_000).contains(&page) {
            return Err(CatalogError::Invalid(vec![]));
        }
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(content::query("SELECT name,slug FROM [[brands]] WHERE active ORDER BY lower(name),id LIMIT $1 OFFSET $2", self.language)))
            .bind((PUBLIC_BRAND_PAGE_SIZE + 1) as i64)
            .bind((page - 1) * PUBLIC_BRAND_PAGE_SIZE as i64)
            .fetch_all(&self.pool).await?)
    }
    pub async fn public_brand(&self, slug: &str) -> Result<PublicBrand> {
        if !valid_slug(slug) {
            return Err(CatalogError::Invalid(vec![Field::Slug]));
        }
        sqlx::query_as(sqlx::AssertSqlSafe(content::query(
            "SELECT name,slug,description,logo_key FROM [[brands]] WHERE slug=$1 AND active",
            self.language,
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(CatalogError::NotFound)
    }
    /// One lookup through the unique slug index. Product visibility uses the direct
    /// category's state; an inactive ancestor only hides the navigation branch.
    pub async fn public_category(&self, slug: &str) -> Result<PublicCategory> {
        if !valid_slug(slug) {
            return Err(CatalogError::Invalid(vec![Field::Slug]));
        }
        sqlx::query_as(sqlx::AssertSqlSafe(content::query(
            "SELECT name,slug,description FROM [[categories]] WHERE slug=$1 AND active",
            self.language,
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(CatalogError::NotFound)
    }
    /// One bounded preorder traversal, in sibling sort order/name/ID, authorized for products.
    pub async fn product_category_tree(&self, session: &Session) -> Result<Vec<ProductCategory>> {
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(content::query("WITH RECURSIVE ranked AS (SELECT id,name,parent_id,active,row_number() OVER(PARTITION BY parent_id \
                    ORDER BY sort_order,lower(name),id) AS ordinal \
                FROM [[categories]]), tree AS (SELECT id,name,parent_id,active,name AS path,0 AS depth,\
                ARRAY[ordinal] AS ordering \
                FROM ranked \
                WHERE parent_id IS NULL \
                UNION ALL \
                SELECT c.id,c.name,c.parent_id,c.active,t.path || ' / ' || c.name,t.depth+1,t.ordering || c.ordinal \
                FROM ranked c \
                JOIN tree t ON c.parent_id=t.id \
                WHERE t.depth<15) \
            SELECT id,name,path,depth,active \
            FROM tree \
            ORDER BY ordering \
            LIMIT $1", self.language)))
            .bind(CATEGORY_LIMIT).fetch_all(&self.pool).await?)
    }
    /// One bounded query. Only active roots and their active descendants are included;
    /// an inactive ancestor hides its branch. Siblings follow the admin sort order,
    /// then case-insensitive name and ID. The schema limits the tree to 1000 nodes/16 levels.
    pub async fn public_navigation(
        &self,
    ) -> std::result::Result<Vec<NavigationCategory>, IdentityError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(content::query(
            "WITH RECURSIVE tree AS (SELECT id,name,slug,parent_id,sort_order,1 AS depth \
                    FROM [[categories]] \
                    WHERE parent_id IS NULL \
                    AND active \
                    UNION ALL \
                    SELECT c.id,c.name,c.slug,c.parent_id,c.sort_order,t.depth+1 \
                    FROM [[categories]] c \
                    JOIN tree t ON c.parent_id=t.id \
                    WHERE c.active \
                    AND t.depth<16) \
                SELECT id,name,slug,parent_id \
                FROM tree \
                ORDER BY sort_order,lower(name),id \
                LIMIT $1",
            self.language,
        )))
        .bind(CATEGORY_LIMIT)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Identity and catalog must use the same application pool; media is the shared store.
    pub fn new(pool: PgPool, identity: Identity, media: MediaStore) -> Self {
        Self {
            language: None,
            pool,
            identity,
            media,
        }
    }
    pub async fn list_counted(
        &self,
        session: &Session,
        kind: Kind,
        q: &str,
        active: Option<bool>,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Entry>> {
        self.identity.authorize(session, kind.read()).await?;
        if q.chars().count() > 120
            || q.chars().any(char::is_control)
            || !(1..=1_000_000).contains(&page)
        {
            return Err(CatalogError::Invalid(vec![]));
        }
        let sql = format!(
            "SELECT {} \
                FROM {} \
                WHERE (strpos(lower(name),lower($1))>0 \
                    OR strpos(slug,lower($1))>0) \
                AND ($2::boolean IS NULL \
                    OR active=$2)",
            kind.columns(),
            kind.content_source()
        );
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(crate::content::query(
                sql,
                self.language,
            )))
            .bind(q.trim())
            .bind(active),
            "sort_order,lower(name),id",
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
        self.identity.authorize(session, kind.read()).await?;
        if q.chars().count() > 120
            || q.chars().any(char::is_control)
            || !(1..=1000000).contains(&page)
        {
            return Err(CatalogError::Invalid(vec![]));
        }
        let sql = format!(
            "SELECT {} \
                FROM {} \
                WHERE (strpos(lower(name),lower($1))>0 \
                    OR strpos(slug,lower($1))>0) \
                AND ($2::boolean IS NULL \
                    OR active=$2) \
                ORDER BY sort_order,lower(name),id \
                LIMIT $3 \
                OFFSET $4",
            kind.columns(),
            kind.content_source()
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(q.trim())
        .bind(active)
        .bind((PAGE_SIZE + 1) as i64)
        .bind((page - 1) * PAGE_SIZE as i64)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn entry(&self, session: &Session, kind: Kind, id: i64) -> Result<Entry> {
        self.identity.authorize(session, kind.read()).await?;
        let sql = format!(
            "SELECT {} FROM {} WHERE id=$1",
            kind.columns(),
            kind.content_source()
        );
        sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(CatalogError::NotFound)
    }
    /// Bounded selector; descendants/self are excluded. SQL trigger validates every submitted ID.
    pub async fn parents(&self, session: &Session, id: i64) -> Result<Vec<Entry>> {
        self.identity
            .authorize(session, Permission::CategoriesRead)
            .await?;
        self.category_choices(id).await
    }
    async fn category_choices(&self, id: i64) -> Result<Vec<Entry>> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "WITH RECURSIVE excluded AS (SELECT id \
                FROM [[categories]] \
                WHERE id=$1 \
                UNION ALL \
                SELECT c.id \
                FROM [[categories]] c \
                JOIN excluded e ON c.parent_id=e.id), tree AS (SELECT id,name AS path \
                FROM [[categories]] \
                WHERE parent_id IS NULL \
                UNION ALL \
                SELECT c.id,t.path || ' / ' || c.name \
                FROM [[categories]] c \
                JOIN tree t ON c.parent_id=t.id) \
            SELECT c.id,t.path AS name,c.slug,c.description,c.sort_order,c.active,c.parent_id,\
            NULL::text AS logo_key \
            FROM [[categories]] c \
            JOIN tree t ON t.id=c.id \
            WHERE c.id NOT IN (SELECT id \
                FROM excluded) \
            ORDER BY lower(t.path),c.id \
            LIMIT $2",
            self.language,
        )))
        .bind(id)
        .bind(CATEGORY_LIMIT)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Bounded selectors authorized for the product use case, not taxonomy administration.
    pub async fn product_choices(
        &self,
        session: &Session,
        selected_brand: Option<i64>,
    ) -> Result<(Vec<Entry>, Vec<Entry>)> {
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        let categories = self.category_choices(0).await?;
        let brands = sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,name,slug,description,sort_order,active,NULL::bigint AS parent_id,logo_key \
                FROM [[brands]] \
                WHERE id=$1 \
                OR id IN (SELECT id \
                    FROM [[brands]] \
                    ORDER BY lower(name),id \
                    LIMIT 1000) \
                ORDER BY lower(name),id \
                LIMIT 1001",
            self.language,
        )))
        .bind(selected_brand)
        .fetch_all(&self.pool)
        .await?;
        Ok((categories, brands))
    }
    pub async fn save(
        &self,
        session: &Session,
        csrf: &str,
        kind: Kind,
        id: Option<i64>,
        input: &Values,
    ) -> Result<i64> {
        self.save_with_content(session, csrf, kind, id, input, None)
            .await
    }
    pub async fn save_with_content(
        &self,
        session: &Session,
        csrf: &str,
        kind: Kind,
        id: Option<i64>,
        input: &Values,
        content: Option<&Draft>,
    ) -> Result<i64> {
        session.check_csrf(csrf)?;
        session.require(kind.write())?;
        let valid = validate(input, kind)?;
        let v = &valid.values;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, kind.write()).await?;
        let definition = match kind {
            Kind::Category => content::Resource::Category,
            Kind::Brand => content::Resource::Brand,
        }
        .definition();
        let translations = definition
            .prepare(
                &mut tx,
                content,
                id.unwrap_or(0),
                Translation {
                    name: v.name.clone(),
                    description: v.description.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let sql = match (kind, id) {
            (Kind::Category, None) => {
                "INSERT INTO catalog_categories(name,slug,description,sort_order,active,parent_id) \
                    VALUES($1,$2,$3,$4,$5,$6) \
                    RETURNING id"
            }
            (Kind::Category, Some(_)) => {
                "UPDATE catalog_categories \
                    SET name=$1,slug=$2,description=$3,sort_order=$4,active=$5,parent_id=$6 \
                    WHERE id=$7 \
                    RETURNING id"
            }
            (Kind::Brand, None) => {
                "INSERT INTO catalog_brands(name,slug,description,sort_order,active) \
                    VALUES($1,$2,$3,$4,$5) \
                    RETURNING id"
            }
            (Kind::Brand, Some(_)) => {
                "UPDATE catalog_brands \
                    SET name=$1,slug=$2,description=$3,sort_order=$4,active=$5 \
                    WHERE id=$6 \
                    RETURNING id"
            }
        };
        let mut query = sqlx::query_scalar::<_, i64>(sql)
            .bind(&v.name)
            .bind(&v.slug)
            .bind(&v.description)
            .bind(valid.order)
            .bind(v.active);
        if kind == Kind::Category {
            query = query.bind(v.parent_id);
        }
        if let Some(id) = id {
            query = query.bind(id);
        }
        let saved = query
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(CatalogError::NotFound)?;
        definition.persist(&mut tx, saved, &translations).await?;
        tx.commit().await?;
        Ok(saved)
    }
    pub async fn delete(&self, session: &Session, csrf: &str, kind: Kind, id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, kind.write()).await?;
        let sql = format!("DELETE FROM {} WHERE id=$1", kind.table());
        let result = sqlx::query(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(id)
        .execute(&mut *tx)
        .await;
        let deleted = match result {
            Err(e)
                if e.as_database_error()
                    .is_some_and(|e| e.is_foreign_key_violation()) =>
            {
                return Err(CatalogError::InUse);
            }
            other => other?,
        };
        if deleted.rows_affected() != 1 {
            return Err(CatalogError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn select_brand_logo(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        media_id: i64,
    ) -> Result<()> {
        self.identity.authorize(s, Permission::BrandsWrite).await?;
        self.identity.authorize(s, Permission::MediaRead).await?;
        s.check_csrf(csrf)?;
        let profile = talocart_platform::media::AttachmentProfile::Logo;
        let prepared = self
            .media
            .prepare_attachment(&self.pool, media_id, profile, None)
            .await?;
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::BrandsWrite).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::MediaRead).await?;
        let source = MediaStore::selected_key(&mut tx, media_id, None).await?;
        let key = self
            .media
            .publish_attachment(&mut tx, &source, profile, prepared)
            .await?;
        let result = sqlx::query("UPDATE catalog_brands SET logo_key=$1 WHERE id=$2")
            .bind(key)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() != 1 {
            return Err(CatalogError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn brand_logo(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        raw: Option<Vec<u8>>,
        filename: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.identity
            .authorize(session, Permission::BrandsWrite)
            .await?;
        let prepared = match raw {
            Some(raw) => Some(self.media.prepare_logo(raw).await?),
            None => None,
        };
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::BrandsWrite).await?;
        let exists: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT EXISTS(SELECT 1 FROM [[brands]] WHERE id=$1)",
            self.language,
        )))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(CatalogError::NotFound);
        }
        let stored = match prepared {
            Some(p) => Some(self.media.put_brand_prepared(p, filename).await?),
            None => None,
        };
        if let Some(stored) = &stored {
            stored.register(&mut tx, true).await?;
        }
        sqlx::query("UPDATE catalog_brands SET logo_key=$1 WHERE id=$2")
            .bind(stored.as_ref().map(|s| &s.key))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn logo_metadata(
        &self,
        key: &str,
    ) -> Result<Option<talocart_platform::media::ImageMetadata>> {
        Ok(self.media.metadata(&self.pool, key).await?)
    }
}
/// The product service has already authorized and locked its mutation. Hold the category
/// against updates/deletion until commit so its publication state cannot change mid-assignment.
pub(crate) async fn product_category_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
) -> std::result::Result<bool, IdentityError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT id FROM catalog_categories WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .is_some(),
    )
}
impl From<IdentityError> for CatalogError {
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
    fn slug_contract_covers_ascii_segments_and_length_boundaries() {
        for value in ["a", "0", "a-1-b", &"a".repeat(160)] {
            assert!(valid_slug(value), "{value}");
        }
        for value in [
            "",
            "-a",
            "a-",
            "a--b",
            "A",
            "è",
            "a_b",
            "a/b",
            "a.b",
            "a b",
            "a\n",
            &"a".repeat(161),
        ] {
            assert!(!valid_slug(value), "{value}");
        }
    }
    fn valid() -> Values {
        Values {
            name: "Abbigliamento".into(),
            slug: "abbigliamento".into(),
            description: "Testo\nsemplice".into(),
            sort_order: "0".into(),
            active: true,
            ..Values::default()
        }
    }
    #[test]
    fn taxonomy_validation_and_normalization() {
        let mut v = valid();
        v.name = "  Abbigliamento  ".into();
        assert_eq!(
            validate(&v, Kind::Category).unwrap().values.name,
            "Abbigliamento"
        );
        for slug in [
            "",
            "Abiti",
            "abiti--uomo",
            "abiti/../",
            "-abiti",
            "abiti_",
            "à",
        ] {
            v = valid();
            v.slug = slug.into();
            assert!(
                matches!(validate(&v,Kind::Category),Err(CatalogError::Invalid(f)) if f.contains(&Field::Slug))
            );
        }
        v = valid();
        v.name = "\0".into();
        v.sort_order = "2147483648".into();
        v.description = "a".repeat(4001);
        v.parent_id = Some(-1);
        assert!(matches!(validate(&v,Kind::Category),Err(CatalogError::Invalid(f)) if f.len()==4));
        v = valid();
        v.parent_id = Some(1);
        assert!(validate(&v, Kind::Brand).is_err());
        v = valid();
        v.sort_order = "999999".into();
        assert!(validate(&v, Kind::Category).is_ok());
        v.sort_order = "1000000".into();
        assert!(validate(&v, Kind::Category).is_err());
    }
}
