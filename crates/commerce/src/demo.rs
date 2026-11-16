//! The bundled development catalog. Trusted CLI/setup callers own authorization.
//! All database rows, media references and the package receipt share one transaction.
use serde::Deserialize;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeMap;
use talocart_platform::media::{MediaError, MediaStore, PreparedImage};
use thiserror::Error;

pub const PACKAGE: &str = "catalog-demo";
pub const VERSION: i32 = 5;
const MANIFEST: &str = include_str!("../../../seeders/catalog-demo/catalog.toml");
const ASSETS: [(&str, &[u8]); 6] = [
    (
        "mug",
        include_bytes!("../../../seeders/catalog-demo/assets/mug.png"),
    ),
    (
        "tote",
        include_bytes!("../../../seeders/catalog-demo/assets/tote.png"),
    ),
    (
        "notebook",
        include_bytes!("../../../seeders/catalog-demo/assets/notebook.png"),
    ),
    (
        "shirt-red",
        include_bytes!("../../../seeders/catalog-demo/assets/shirt-red.png"),
    ),
    (
        "shirt-blue",
        include_bytes!("../../../seeders/catalog-demo/assets/shirt-blue.png"),
    ),
    (
        "shirt-sage",
        include_bytes!("../../../seeders/catalog-demo/assets/shirt-sage.png"),
    ),
];
#[derive(Debug, Error)]
pub enum SeedError {
    #[error(transparent)]
    Identity(#[from] talocart_platform::identity::IdentityError),
    #[error("Complete store installation first")]
    NotInstalled,
    #[error("The demo package conflicts with existing data; no data was changed")]
    Collision,
    #[error("Category, tax rate or tax class capacity exceeded")]
    Capacity,
    #[error("Invalid demo package or version differs from the installed one")]
    Dataset,
    #[error("Demo installation failed; no database changes were applied")]
    Database,
    #[error(transparent)]
    Media(#[from] MediaError),
}
impl From<sqlx::Error> for SeedError {
    fn from(e: sqlx::Error) -> Self {
        if let Some(db) = e.as_database_error() {
            if db.is_unique_violation() {
                return Self::Collision;
            }
            if db.constraint().is_some_and(|c| c.ends_with("_limit")) {
                return Self::Capacity;
            }
        }
        let _ = talocart_platform::technical::DatabaseFailure::capture(e, module_path!());
        Self::Database
    }
}
type Result<T> = std::result::Result<T, SeedError>;
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Preview,
    Installed,
    AlreadyInstalled,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub outcome: Outcome,
    pub price_mode: String,
    pub categories: usize,
    pub brands: usize,
    pub rates: usize,
    pub classes: usize,
    pub options: usize,
    pub values: usize,
    pub products: usize,
    pub variants: usize,
    pub images: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dataset {
    package: String,
    version: i32,
    categories: Vec<Category>,
    brands: Vec<Brand>,
    rates: Vec<Rate>,
    classes: Vec<Class>,
    options: Vec<OptionSeed>,
    products: Vec<Product>,
    variants: Vec<Variant>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Category {
    key: String,
    name: String,
    slug: String,
    parent: Option<String>,
    active: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Brand {
    key: String,
    name: String,
    slug: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rate {
    key: String,
    name: String,
    basis_points: i32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Class {
    key: String,
    name: String,
    rate: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionSeed {
    key: String,
    name: String,
    values: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Value {
    key: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Product {
    key: String,
    name: String,
    model: String,
    slug: String,
    description: String,
    price: String,
    stock: i32,
    status: String,
    category: String,
    brand: Option<String>,
    tax_class: String,
    image: Option<String>,
    #[serde(default)]
    options: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    product: String,
    values: Vec<String>,
    sku: String,
    price: String,
    stock: i32,
    active: bool,
    image: Option<String>,
}
impl Dataset {
    fn parse() -> Result<Self> {
        let d: Self = toml::from_str(MANIFEST).map_err(|_| SeedError::Dataset)?;
        if d.package != PACKAGE || d.version != VERSION {
            return Err(SeedError::Dataset);
        }
        Ok(d)
    }
    fn report(&self, outcome: Outcome, price_mode: String) -> Report {
        Report {
            outcome,
            price_mode,
            categories: self.categories.len(),
            brands: self.brands.len(),
            rates: self.rates.len(),
            classes: self.classes.len(),
            options: self.options.len(),
            values: self.options.iter().map(|o| o.values.len()).sum(),
            products: self.products.len(),
            variants: self.variants.len(),
            images: ASSETS.len(),
        }
    }
}
#[derive(Default)]
struct Ids(BTreeMap<String, i64>);
impl Ids {
    fn add(&mut self, key: &str, id: i64) -> Result<()> {
        if self.0.insert(key.into(), id).is_some() {
            return Err(SeedError::Dataset);
        }
        Ok(())
    }
    fn get(&self, key: &str) -> Result<i64> {
        self.0.get(key).copied().ok_or(SeedError::Dataset)
    }
    fn optional(&self, key: Option<&str>) -> Result<Option<i64>> {
        key.map(|k| self.get(k)).transpose()
    }
}
/// Prepared outside transactions: decoding/resizing runs through MediaStore's bounded CPU pool.
pub struct PreparedDemo {
    dataset: Dataset,
    images: Vec<(String, PreparedImage)>,
}
#[derive(Clone)]
pub struct DemoSeeder {
    pool: PgPool,
    media: MediaStore,
}
impl DemoSeeder {
    pub fn new(pool: PgPool, media: MediaStore) -> Self {
        Self { pool, media }
    }
    pub async fn prepare(&self) -> Result<PreparedDemo> {
        let dataset = Dataset::parse()?;
        let mut images = vec![];
        for (key, png) in ASSETS {
            images.push((key.into(), self.media.prepare_product(png.to_vec()).await?));
        }
        Ok(PreparedDemo { dataset, images })
    }
    /// Read-only preview: no sequence increments, catalog writes, receipt or filesystem access.
    pub async fn preview(&self) -> Result<Report> {
        let d = Dataset::parse()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
        let (mode, existing) = check(&mut tx, &d).await?;
        tx.rollback().await?;
        Ok(d.report(
            if existing {
                Outcome::AlreadyInstalled
            } else {
                Outcome::Preview
            },
            mode,
        ))
    }
    /// Trusted server operator only. HTTP uses this package exclusively after valid bootstrap.
    pub async fn install(&self) -> Result<Report> {
        let preview = self.preview().await?;
        if preview.outcome == Outcome::AlreadyInstalled {
            return Ok(preview);
        }
        let prepared = self.prepare().await?;
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        // The common identity row coordinates with setup and every browser mutation.
        sqlx::query("SELECT id FROM identity_state WHERE id=1 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        let report = self.install_in_transaction(&mut tx, prepared).await?;
        tx.commit().await?;
        Ok(report)
    }
    /// Caller holds publish_lock and identity_state, and commits/rolls back all owners together.
    /// Setup may set installed inside this same transaction before calling this method.
    pub async fn install_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        prepared: PreparedDemo,
    ) -> Result<Report> {
        sqlx::query("SELECT pg_advisory_xact_lock(5426645964561798228)")
            .execute(&mut **tx)
            .await?;
        talocart_platform::database::commercial_lock::exclusive(tx).await?;
        let d = prepared.dataset;
        let (mode, existing) = check(tx, &d).await?;
        if existing {
            return Ok(d.report(Outcome::AlreadyInstalled, mode));
        }
        let mut images = BTreeMap::new();
        for (key, image) in prepared.images {
            let stored = self
                .media
                .put_prepared(image, &format!("demo-{key}.png"))
                .await?;
            stored.register(tx, true).await?;
            images.insert(key, stored.key);
        }
        let (
            mut categories,
            mut brands,
            mut rates,
            mut classes,
            mut options,
            mut values,
            mut products,
        ) = (
            Ids::default(),
            Ids::default(),
            Ids::default(),
            Ids::default(),
            Ids::default(),
            Ids::default(),
            Ids::default(),
        );
        for c in &d.categories {
            let id = sqlx::query_scalar(
                "INSERT INTO catalog_categories(name,slug,description,parent_id,active) \
                VALUES($1,$2,$3,$4,true) \
                RETURNING id",
            )
            .bind(&c.name)
            .bind(&c.slug)
            .bind("Dati demo sintetici di sviluppo")
            .bind(categories.optional(c.parent.as_deref())?)
            .fetch_one(&mut **tx)
            .await?;
            crate::content::Resource::Category
                .seed_primary(tx, id)
                .await?;
            categories.add(&c.key, id)?;
        }
        for b in &d.brands {
            let id = sqlx::query_scalar(
                "INSERT INTO catalog_brands(name,slug,description) VALUES($1,$2,$3) RETURNING id",
            )
            .bind(&b.name)
            .bind(&b.slug)
            .bind("Marchio demo sintetico")
            .fetch_one(&mut **tx)
            .await?;
            crate::content::Resource::Brand.seed_primary(tx, id).await?;
            brands.add(&b.key, id)?;
        }
        for r in &d.rates {
            let id=sqlx::query_scalar("INSERT INTO tax_rates(name,description,basis_points) VALUES($1,$2,$3) RETURNING id").bind(&r.name).bind("Aliquota sintetica per test; non configura obblighi fiscali").bind(r.basis_points).fetch_one(&mut **tx).await?;
            rates.add(&r.key, id)?;
        }
        for c in &d.classes {
            let id = sqlx::query_scalar(
                "INSERT INTO tax_classes(name,description,rate_id) VALUES($1,$2,$3) RETURNING id",
            )
            .bind(&c.name)
            .bind("Classe demo sintetica")
            .bind(rates.get(&c.rate)?)
            .fetch_one(&mut **tx)
            .await?;
            classes.add(&c.key, id)?;
        }
        for o in &d.options {
            let id =
                sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES($1) RETURNING id")
                    .bind(&o.name)
                    .fetch_one(&mut **tx)
                    .await?;
            crate::content::Resource::Option
                .seed_primary(tx, id)
                .await?;
            options.add(&o.key, id)?;
            for v in &o.values {
                let id = sqlx::query_scalar(
                    "INSERT INTO catalog_option_values(option_id,name) VALUES($1,$2) RETURNING id",
                )
                .bind(id)
                .bind(&v.name)
                .fetch_one(&mut **tx)
                .await?;
                crate::content::Resource::Value.seed_primary(tx, id).await?;
                values.add(&format!("{}.{}", o.key, v.key), id)?;
            }
        }
        for p in &d.products {
            let id = insert_product(
                tx,
                p,
                categories.get(&p.category)?,
                brands.optional(p.brand.as_deref())?,
                classes.get(&p.tax_class)?,
                image_key(&images, p.image.as_deref())?,
            )
            .await?;
            products.add(&p.key, id)?;
            for (n, o) in p.options.iter().enumerate() {
                sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,$2,$3)").bind(id).bind((n+1)as i16).bind(options.get(o)?).execute(&mut **tx).await?;
            }
        }
        for v in &d.variants {
            let (sku, price, stock) = fields(&v.sku, &v.price, v.stock)?;
            let ids = v
                .values
                .iter()
                .map(|k| values.get(k))
                .collect::<Result<Vec<_>>>()?;
            sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock,active,image_key) \
                VALUES($1,$2,$3,$4,$5,$6,$7)").bind(products.get(&v.product)?).bind(ids).bind(sku).bind(price).bind(stock).bind(v.active).bind(image_key(&images,v.image.as_deref())?).execute(&mut **tx).await?;
        }
        // Publication references are validated while active, then the inactive-resource case is set.
        for c in &d.categories {
            if !c.active {
                sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1")
                    .bind(categories.get(&c.key)?)
                    .execute(&mut **tx)
                    .await?;
            }
        }
        sqlx::query(
            "INSERT INTO catalog_demo_packages(package,version,price_mode) VALUES($1,$2,$3)",
        )
        .bind(&d.package)
        .bind(d.version)
        .bind(&mode)
        .execute(&mut **tx)
        .await?;
        Ok(d.report(Outcome::Installed, mode))
    }
}
fn image_key(images: &BTreeMap<String, String>, key: Option<&str>) -> Result<Option<String>> {
    key.map(|k| images.get(k).cloned().ok_or(SeedError::Dataset))
        .transpose()
}
fn fields(sku: &str, price: &str, stock: i32) -> Result<(String, i64, i32)> {
    crate::products::validate_commercial_fields(sku, price, &stock.to_string())
        .map_err(|_| SeedError::Dataset)
}
async fn insert_product(
    tx: &mut Transaction<'_, Postgres>,
    p: &Product,
    category: i64,
    brand: Option<i64>,
    class: i64,
    image: Option<String>,
) -> Result<i64> {
    let (model, price, stock) = fields(&p.model, &p.price, p.stock)?;
    let id = sqlx::query_scalar(
        "INSERT INTO catalog_products(name,model,slug,description,price_cents,stock,status,\
                category_id,brand_id,tax_class_id,image_key) \
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) \
            RETURNING id",
    )
    .bind(&p.name)
    .bind(model)
    .bind(&p.slug)
    .bind(&p.description)
    .bind(price)
    .bind(stock)
    .bind(&p.status)
    .bind(category)
    .bind(brand)
    .bind(class)
    .bind(image)
    .fetch_one(&mut **tx)
    .await?;
    crate::content::Resource::Product
        .seed_primary(tx, id)
        .await?;
    Ok(id)
}
async fn check(tx: &mut Transaction<'_, Postgres>, d: &Dataset) -> Result<(String, bool)> {
    let installed: bool = sqlx::query_scalar("SELECT installed FROM identity_state WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    if !installed {
        return Err(SeedError::NotInstalled);
    }
    let mode: String = sqlx::query_scalar("SELECT price_mode FROM pricing_settings WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    if let Some(version) =
        sqlx::query_scalar::<_, i32>("SELECT version FROM catalog_demo_packages WHERE package=$1")
            .bind(&d.package)
            .fetch_optional(&mut **tx)
            .await?
    {
        // A matching receipt preserves edits and deletions; installation is not a repair.
        if version != d.version {
            return Err(SeedError::Dataset);
        }
        return Ok((mode, true));
    }
    let slugs: Vec<&str> = d.categories.iter().map(|r| r.slug.as_str()).collect();
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM catalog_categories WHERE slug=ANY($1))",
    )
    .bind(slugs)
    .fetch_one(&mut **tx)
    .await?
    {
        return Err(SeedError::Collision);
    }
    let names: Vec<String> = d.brands.iter().map(|r| r.name.to_lowercase()).collect();
    let slugs: Vec<&str> = d.brands.iter().map(|r| r.slug.as_str()).collect();
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 \
                FROM catalog_brands b \
                LEFT \
                JOIN catalog_brands_translations t ON t.entity_id=b.id \
                AND t.locale=(SELECT primary_locale \
                    FROM storefront_settings \
                    WHERE id=1) \
                WHERE lower(t.name)=ANY($1) \
                OR b.slug=ANY($2))",
    )
    .bind(names)
    .bind(slugs)
    .fetch_one(&mut **tx)
    .await?
    {
        return Err(SeedError::Collision);
    }
    for (table, names) in [
        (
            "tax_rates",
            d.rates
                .iter()
                .map(|r| r.name.to_lowercase())
                .collect::<Vec<_>>(),
        ),
        (
            "tax_classes",
            d.classes.iter().map(|r| r.name.to_lowercase()).collect(),
        ),
        (
            "catalog_options",
            d.options.iter().map(|r| r.name.to_lowercase()).collect(),
        ),
    ] {
        let query = sqlx::AssertSqlSafe(if table == "catalog_options" {
            "SELECT EXISTS(SELECT 1 \
                    FROM catalog_options_translations t \
                    JOIN storefront_settings s ON s.primary_locale=t.locale \
                    WHERE s.id=1 \
                    AND lower(t.name)=ANY($1))"
                .to_owned()
        } else {
            format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE lower(name)=ANY($1))")
        });
        if sqlx::query_scalar::<_, bool>(query)
            .bind(names)
            .fetch_one(&mut **tx)
            .await?
        {
            return Err(SeedError::Collision);
        }
    }
    let models: Vec<&str> = d.products.iter().map(|r| r.model.as_str()).collect();
    let skus: Vec<&str> = d.variants.iter().map(|r| r.sku.as_str()).collect();
    let slugs: Vec<&str> = d.products.iter().map(|r| r.slug.as_str()).collect();
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 \
            FROM catalog_products \
            WHERE model=ANY($1) \
            OR slug=ANY($2)) \
        OR EXISTS(SELECT 1 \
            FROM catalog_variants \
            WHERE sku=ANY($3))",
    )
    .bind(models)
    .bind(slugs)
    .bind(skus)
    .fetch_one(&mut **tx)
    .await?
    {
        return Err(SeedError::Collision);
    }
    // Every identifier comes from this fixed whitelist; all values remain bound parameters.
    for (table, extra, limit) in [
        ("catalog_categories", d.categories.len(), 1000),
        ("tax_rates", d.rates.len(), 100),
        ("tax_classes", d.classes.len(), 100),
    ] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&mut **tx)
                .await?;
        if count + extra as i64 > limit {
            return Err(SeedError::Capacity);
        }
    }
    Ok((mode, false))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_public_slugs_have_no_demo_marker() {
        let d = Dataset::parse().expect("Manifest demo valido");
        for slug in d
            .categories
            .iter()
            .map(|r| r.slug.as_str())
            .chain(d.brands.iter().map(|r| r.slug.as_str()))
            .chain(d.products.iter().map(|r| r.slug.as_str()))
        {
            assert!(
                !slug.contains("demo"),
                "Public slug still contains Demo: {slug}"
            );
        }
    }
    #[test]
    fn bundled_manifest_prices_and_images_are_synthetic_and_bounded() {
        let d = Dataset::parse().expect("Manifest demo valido");
        let names = d
            .categories
            .iter()
            .map(|row| row.name.as_str())
            .chain(d.brands.iter().map(|row| row.name.as_str()))
            .chain(d.rates.iter().map(|row| row.name.as_str()))
            .chain(d.classes.iter().map(|row| row.name.as_str()))
            .chain(d.options.iter().map(|row| row.name.as_str()))
            .chain(
                d.options
                    .iter()
                    .flat_map(|row| row.values.iter().map(|value| value.name.as_str())),
            )
            .chain(d.products.iter().map(|row| row.name.as_str()));
        for name in names {
            assert!(
                !name.to_lowercase().contains("demo"),
                "Public seed name contains Demo: {name}"
            );
        }
        let r = d.report(Outcome::Preview, "inclusive".into());
        assert_eq!(
            (
                r.categories,
                r.brands,
                r.rates,
                r.classes,
                r.options,
                r.values,
                r.products,
                r.variants,
                r.images
            ),
            (4, 2, 3, 3, 2, 6, 30, 4, 6)
        );
        let mut keys = std::collections::BTreeSet::new();
        let mut skus = std::collections::BTreeSet::new();
        let mut slugs = std::collections::BTreeSet::new();
        for p in &d.products {
            assert!(keys.insert(&p.key) && skus.insert(&p.model) && slugs.insert(&p.slug));
            assert!(d.categories.iter().any(|c| c.key == p.category));
            assert!(d.classes.iter().any(|c| c.key == p.tax_class));
            assert!(
                p.brand
                    .as_ref()
                    .is_none_or(|b| d.brands.iter().any(|r| &r.key == b))
            );
            assert!(
                p.image
                    .as_ref()
                    .is_none_or(|i| ASSETS.iter().any(|(key, _)| key == i))
            );
            assert!(!p.slug.contains("demo"));
            assert!(
                !p.model.to_ascii_lowercase().contains("demo"),
                "Demo product model must be clean: {}",
                p.model
            );
            fields(&p.model, &p.price, p.stock).expect("Prezzo e stock demo validi");
            assert!(matches!(p.status.as_str(), "draft" | "published"));
        }
        let mut skus = std::collections::BTreeSet::new();
        for v in &d.variants {
            assert!(
                !v.sku.to_ascii_lowercase().contains("demo"),
                "Demo variant SKU must be clean: {}",
                v.sku
            );
            assert!(skus.insert(&v.sku), "Variant SKUs must be unique");
            fields(&v.sku, &v.price, v.stock).expect("Variante demo valida");
            assert!(!v.values.is_empty() && v.values.len() <= 3);
        }
        assert!(
            ASSETS
                .iter()
                .all(|(_, png)| png.starts_with(b"\x89PNG\r\n\x1a\n"))
        );
        assert!(fields("DEMO-X", "1.001", 1).is_err());
    }
}
