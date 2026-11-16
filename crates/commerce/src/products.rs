//! Simple products own persistence, publication and stock display; checkout is a later module.
mod imports;
mod validation;
use validation::validate;
pub use validation::{check_import_values, validate_commercial_fields};
mod commands;
mod reads;
mod related;
pub use related::{RelatedChoice, RelatedEditor};
use talocart_platform::content::{Draft, Translation};
mod bulk;
mod gallery;
mod listing;
use crate::pricing::{self, LineTotals, Money, PriceMode, Rate};
pub use bulk::{BulkAction, BulkActions, ProductSelection};
pub use gallery::{GalleryAction, GalleryImage, MAX_GALLERY_IMAGES};
pub use listing::{CardEntry, PublicProductPage};
use sqlx::{FromRow, PgPool};
use talocart_platform::{
    identity::{Identity, IdentityError, Permission, Session},
    media::{MediaError, MediaStore},
};
use thiserror::Error;

pub const PAGE_SIZE: usize = 21;
pub const CATALOG_PAGE_SIZES: [u16; 4] = [21, 42, 63, 84];
/// Bounded catalog sizes keep full rows in the desktop three-column grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogPageSize(u16);
impl Default for CatalogPageSize {
    fn default() -> Self {
        Self(21)
    }
}
impl CatalogPageSize {
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty() {
            return Ok(Self::default());
        }
        value
            .parse::<u16>()
            .ok()
            .filter(|size| CATALOG_PAGE_SIZES.contains(size))
            .map(Self)
            .ok_or(ProductError::Invalid(vec![]))
    }
    pub fn value(self) -> u16 {
        self.0
    }
}
pub use talocart_platform::pagination::{ADMIN_PAGE_SIZES, AdminPageSize, PRODUCT_PAGE_SIZES};
/// Counted product page from one read-only database snapshot.
pub struct ProductPage {
    pub entries: Vec<Entry>,
    pub total: i64,
    pub page: i64,
    pub pages: i64,
    pub first: i64,
    pub last: i64,
}
pub const MAX_PRICE: i64 = 999_999_999_999;
pub const MAX_QUANTITY: i32 = 999_999;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QuantityError {
    #[error("Quantità minima non valida")]
    InvalidMinimum,
    #[error("Packs non valido")]
    InvalidPacks,
    #[error("La quantità è inferiore al minimo acquistabile")]
    BelowMinimum,
    #[error("La quantità deve essere un multiplo di packs")]
    NotPackMultiple,
}

/// Quantities and stock count pieces; prices remain per piece. A product's rules also
/// apply to each selected variant. Private fields guarantee positive, bounded divisors.
#[derive(Clone, Copy, Debug)]
pub struct PurchaseQuantities {
    minimum: i32,
    packs: i32,
}
impl PurchaseQuantities {
    pub fn new(minimum: i32, packs: i32) -> std::result::Result<Self, QuantityError> {
        if !(1..=MAX_QUANTITY).contains(&minimum) {
            return Err(QuantityError::InvalidMinimum);
        }
        if !(1..=MAX_QUANTITY).contains(&packs) {
            return Err(QuantityError::InvalidPacks);
        }
        Ok(Self { minimum, packs })
    }
    /// The sum and product are below 2 * MAX_QUANTITY, so i32 arithmetic cannot overflow.
    pub fn first_quantity(self) -> i32 {
        ((self.minimum + self.packs - 1) / self.packs) * self.packs
    }
    /// Validate the final line quantity, including repeated additions, before buying.
    pub fn validate(self, quantity: i32) -> std::result::Result<(), QuantityError> {
        if quantity < self.minimum {
            return Err(QuantityError::BelowMinimum);
        }
        if quantity % self.packs != 0 {
            return Err(QuantityError::NotPackMultiple);
        }
        Ok(())
    }
    pub fn available(self, stock: i32) -> bool {
        stock >= self.first_quantity()
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdminSort {
    Id,
    Image,
    Type,
    #[default]
    Name,
    Model,
    Category,
    Brand,
    Price,
    Variants,
    Stock,
    Status,
    ModifiedAt,
}
impl AdminSort {
    pub fn key(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Image => "image",
            Self::Type => "type",
            Self::Name => "name",
            Self::Model => "model",
            Self::Category => "category",
            Self::Brand => "brand",
            Self::Price => "price",
            Self::Variants => "variants",
            Self::Stock => "stock",
            Self::Status => "status",
            Self::ModifiedAt => "modified_at",
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Image => "(image_id IS NOT NULL)",
            Self::Type => "has_variants",
            Self::Name => "lower(name)",
            Self::Model => "lower(model)",
            Self::Category => "lower(category_name)",
            Self::Brand => "lower(brand_name)",
            Self::Price => {
                "CASE \
                    WHEN has_variants THEN (SELECT min(v.price_cents) \
                        FROM catalog_variants v \
                        WHERE v.product_id=admin_products.id \
                        AND v.active) \
                    ELSE price_cents END"
            }
            Self::Variants => "CASE WHEN has_variants THEN variant_count END",
            Self::Stock => {
                "CASE \
                    WHEN has_variants THEN (SELECT sum(v.stock) \
                        FROM catalog_variants v \
                        WHERE v.product_id=admin_products.id \
                        AND v.active) \
                    ELSE stock END"
            }
            Self::Status => "status",
            Self::ModifiedAt => "modified_instant",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}
impl SortDirection {
    pub fn key(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
    pub fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}
/// Only these typed choices can enter ORDER BY. Missing values sort last in both directions;
/// ties use name then ID ascending, keeping pagination deterministic on an unchanged catalog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdminOrder {
    pub column: AdminSort,
    pub direction: SortDirection,
}
impl AdminOrder {
    pub fn parse(column: &str, direction: &str) -> Result<Self> {
        let column = match column {
            "id" => AdminSort::Id,
            "image" => AdminSort::Image,
            "type" => AdminSort::Type,
            "" | "name" => AdminSort::Name,
            "model" => AdminSort::Model,
            "category" => AdminSort::Category,
            "brand" => AdminSort::Brand,
            "price" => AdminSort::Price,
            "variants" => AdminSort::Variants,
            "stock" => AdminSort::Stock,
            "status" => AdminSort::Status,
            "modified_at" => AdminSort::ModifiedAt,
            _ => return Err(ProductError::Invalid(vec![])),
        };
        let direction = match direction {
            "" | "asc" => SortDirection::Asc,
            "desc" => SortDirection::Desc,
            _ => return Err(ProductError::Invalid(vec![])),
        };
        Ok(Self { column, direction })
    }
}
/// Administrative column filters are combined with AND; text matches are literal substrings.
#[derive(Default)]
pub struct AdminFilters<'a> {
    pub id: Option<i64>,
    pub q: &'a str,
    pub status: &'a str,
    pub kind: &'a str,
    pub model: &'a str,
    pub price: &'a str,
    pub price_op: &'a str,
    pub stock: &'a str,
    pub stock_op: &'a str,
    pub modified_date: &'a str,
    pub category: Option<i64>,
    pub brand: Option<i64>,
    pub order: AdminOrder,
    pub page_size: AdminPageSize,
}
impl AdminFilters<'_> {
    fn validate(&self, page: i64) -> Result<()> {
        if self.id.is_some_and(|id| id <= 0) {
            return Err(ProductError::Invalid(vec![]));
        }
        for text in [self.q, self.model] {
            validate_query(text, page)?;
        }
        for (id, field) in [(self.category, Field::Category), (self.brand, Field::Brand)] {
            if id.is_some_and(|id| id <= 0) {
                return Err(ProductError::Invalid(vec![field]));
            }
        }
        if !matches!(self.status, "" | "draft" | "published")
            || !matches!(self.kind, "" | "simple" | "variants")
        {
            return Err(ProductError::Invalid(vec![]));
        }
        self.numeric()?;
        if !self.modified_date.is_empty()
            && !talocart_platform::regional::valid_date(self.modified_date)
        {
            return Err(ProductError::Invalid(vec![]));
        }
        Ok(())
    }
    fn numeric(&self) -> Result<(Option<i64>, Option<i64>)> {
        if !matches!(self.price_op, "" | "eq" | "gt" | "lt")
            || !matches!(self.stock_op, "" | "eq" | "gt" | "lt")
        {
            return Err(ProductError::Invalid(vec![]));
        }
        let price = if self.price.is_empty() {
            None
        } else {
            if self.price.len() > 24 {
                return Err(ProductError::Invalid(vec![Field::Price]));
            }
            Some(
                Money::parse(self.price)
                    .map_err(|_| ProductError::Invalid(vec![Field::Price]))?
                    .cents(),
            )
        };
        let stock = if self.stock.is_empty() {
            None
        } else {
            if self.stock.len() > 19 || !self.stock.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ProductError::Invalid(vec![Field::Stock]));
            }
            Some(
                self.stock
                    .parse::<i64>()
                    .map_err(|_| ProductError::Invalid(vec![Field::Stock]))?,
            )
        };
        Ok((price, stock))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Code,
    Slug,
    Description,
    SeoTitle,
    SeoDescription,
    Price,
    Stock,
    MinimumQuantity,
    Packs,
    Status,
    Category,
    Brand,
    TaxClass,
}
#[derive(Debug, Error)]
pub enum ProductError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error(transparent)]
    Content(#[from] talocart_platform::content::ContentError),
    #[error("Controlla i campi evidenziati")]
    Invalid(Vec<Field>),
    #[error("Questo modello è già utilizzato")]
    DuplicateModel,
    #[error("Questo slug è già utilizzato")]
    DuplicateSlug,
    #[error(
        "Per pubblicare scegli una classe fiscale con aliquota attiva. Categoria e marchio, se assegnati, devono essere attivi"
    )]
    Publication,
    #[error(
        "La modalità prezzi è cambiata mentre compilavi il prodotto. Ricontrolla l’importo secondo la modalità ora visualizzata prima di salvare"
    )]
    ModeChanged,
    #[error("Stock changed; reload before saving")]
    StockChanged,
    #[error("Prodotto non trovato")]
    NotFound,
    #[error("Il prodotto è collegato ad altre risorse e non può essere eliminato")]
    InUse,
    #[error("Seleziona da 1 a {limit} prodotti con ID validi e senza duplicati", limit = ProductSelection::MAX_LEN)]
    Selection,
    #[error("Conferma l’azione su tutti i prodotti selezionati")]
    BulkConfirmation,
    #[error("Scegli azioni valide: Elimina si esegue da sola e Pubblica e Bozza sono alternative")]
    BulkAction,
    #[error("Conferma l’eliminazione del prodotto")]
    Confirmation,
    #[error("Carica un’immagine PNG, JPEG o WebP statico valida, fino a 10 MiB")]
    Image,
    #[error("La galleria può contenere al massimo 12 immagini aggiuntive")]
    GalleryFull,
    #[error("Azione sulla galleria non valida")]
    GalleryAction,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for ProductError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            match db.constraint() {
                Some("order_lines_product_id_fkey") => return Self::InUse,
                Some("catalog_product_images_limit") => return Self::GalleryFull,
                Some("catalog_products_model_unique") => return Self::DuplicateModel,
                Some("catalog_products_slug_key") => return Self::DuplicateSlug,
                Some("catalog_products_publication" | "catalog_products_check") => {
                    return Self::Publication;
                }
                Some("catalog_products_category_id_fkey") => {
                    return Self::Invalid(vec![Field::Category]);
                }
                Some("catalog_products_minimum_quantity_check") => {
                    return Self::Invalid(vec![Field::MinimumQuantity]);
                }
                Some("catalog_products_packs_check") => return Self::Invalid(vec![Field::Packs]),
                Some("catalog_products_brand_id_fkey") => return Self::Invalid(vec![Field::Brand]),
                Some("catalog_products_tax_class_id_fkey") => {
                    return Self::Invalid(vec![Field::TaxClass]);
                }
                _ => {}
            }
        }
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    }
}
impl From<MediaError> for ProductError {
    fn from(error: MediaError) -> Self {
        match error {
            MediaError::Invalid => Self::Image,
            other => Self::Identity(other.into()),
        }
    }
}
pub type Result<T> = std::result::Result<T, ProductError>;
#[derive(Clone)]
pub struct Values {
    /// None preserves existing related settings for imports and older callers.
    pub related_fallback: Option<String>,
    pub name: String,
    pub model: String,
    pub slug: String,
    pub description: String,
    pub seo_title: String,
    pub seo_description: String,
    pub price: String,
    pub price_mode: String,
    pub currency: String,
    pub manage_stock: bool,
    pub stock: String,
    pub stock_revision: i64,
    pub minimum_quantity: String,
    pub packs: String,
    pub status: String,
    pub category_id: String,
    pub brand_id: String,
    pub tax_class_id: String,
}
impl Default for Values {
    fn default() -> Self {
        Self {
            name: String::new(),
            model: String::new(),
            slug: String::new(),
            description: String::new(),
            seo_title: String::new(),
            seo_description: String::new(),
            price: "0,00".into(),
            price_mode: "inclusive".into(),
            currency: "EUR".into(),
            manage_stock: true,
            stock: "0".into(),
            stock_revision: 0,
            minimum_quantity: "1".into(),
            packs: "1".into(),
            status: "draft".into(),
            category_id: String::new(),
            brand_id: String::new(),
            tax_class_id: String::new(),
            related_fallback: None,
        }
    }
}
#[derive(Clone, FromRow)]
pub struct Entry {
    pub id: i64,
    pub name: String,
    pub model: String,
    pub slug: String,
    pub description: String,
    pub seo_title: String,
    pub seo_description: String,
    pub price_cents: i64,
    pub manage_stock: bool,
    pub stock: i32,
    #[sqlx(default)]
    pub stock_revision: i64,
    pub minimum_quantity: i32,
    pub packs: i32,
    pub has_variants: bool,
    /// Administrative count, including inactive variants; omitted by public projections.
    #[sqlx(default)]
    pub variant_count: i64,
    pub status: String,
    pub category_id: Option<i64>,
    pub brand_id: Option<i64>,
    pub tax_class_id: Option<i64>,
    pub image_key: Option<String>,
    pub image_id: Option<i64>,
    pub image_width: Option<i32>,
    pub image_height: Option<i32>,
    pub category_name: Option<String>,
    pub brand_name: Option<String>,
    /// Public card projection; omitted from administrative queries.
    #[sqlx(default)]
    pub brand_logo_id: Option<i64>,
    #[sqlx(default)]
    pub brand_logo_edge: Option<i32>,
    /// Public detail projection; visible products only reference active brands.
    #[sqlx(default)]
    pub brand_slug: Option<String>,
    pub tax_name: Option<String>,
    pub basis_points: Option<i32>,
    pub price_mode: String,
    pub currency: String,
    /// Administrative product/translation audit date in the configured regional format.
    #[sqlx(default)]
    pub modified_at: String,
}
impl Entry {
    pub fn values(&self) -> Values {
        Values {
            related_fallback: None,
            name: self.name.clone(),
            model: self.model.clone(),
            slug: self.slug.clone(),
            description: self.description.clone(),
            seo_title: self.seo_title.clone(),
            seo_description: self.seo_description.clone(),
            price: format!("{},{:02}", self.price_cents / 100, self.price_cents % 100),
            price_mode: self.price_mode.clone(),
            currency: self.currency.clone(),
            manage_stock: self.manage_stock,
            stock: self.stock.to_string(),
            stock_revision: self.stock_revision,
            minimum_quantity: self.minimum_quantity.to_string(),
            packs: self.packs.to_string(),
            status: self.status.clone(),
            category_id: self
                .category_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            brand_id: self.brand_id.map(|id| id.to_string()).unwrap_or_default(),
            tax_class_id: self
                .tax_class_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
        }
    }
    pub fn purchase_quantities(&self) -> std::result::Result<PurchaseQuantities, QuantityError> {
        PurchaseQuantities::new(self.minimum_quantity, self.packs)
    }
    pub fn available(&self) -> bool {
        self.purchase_quantities().is_ok_and(|rules| {
            rules.available(if self.manage_stock {
                self.stock
            } else {
                MAX_QUANTITY
            })
        })
    }
    pub fn quote(&self) -> Result<LineTotals> {
        quote(self.price_cents, self.basis_points, &self.price_mode)
    }

    pub fn image_url(&self, profile: &str) -> String {
        match (self.image_id, &self.image_key) {
            (Some(id), Some(_)) => format!("/media/image/{id}/{profile}"),
            _ => String::new(),
        }
    }
}
fn quote(price_cents: i64, basis_points: Option<i32>, price_mode: &str) -> Result<LineTotals> {
    let mode = PriceMode::parse(price_mode).map_err(|_| IdentityError::Unavailable)?;
    let rate = Rate::from_basis_points(basis_points.ok_or(IdentityError::Unavailable)?)
        .map_err(|_| IdentityError::Unavailable)?;
    let money = Money::from_cents(price_cents).map_err(|_| IdentityError::Unavailable)?;
    pricing::line(money, 1, rate, mode).map_err(|_| IdentityError::Unavailable.into())
}
const SELECT: &str = "SELECT p.*,EXISTS(SELECT 1 \
        FROM catalog_product_options a \
        WHERE a.product_id=p.id) AS has_variants,(SELECT count(*) \
        FROM catalog_variants v \
        WHERE v.product_id=p.id) AS variant_count,m.id AS image_id,m.width AS image_width,\
    m.height AS image_height,c.name AS category_name,b.name AS brand_name,coalesce(e.tax_name,\
        t.name) AS tax_name,coalesce(e.basis_points,r.basis_points) AS basis_points,s.price_mode,\
    coalesce(ss.currency,'EUR') AS currency \
    FROM [[products]] p \
    JOIN pricing_settings s ON s.id=1 \
    LEFT \
    JOIN store_settings ss ON ss.id=1 \
    LEFT \
    JOIN catalog_effective_tax e ON e.class_id=p.tax_class_id \
    LEFT \
    JOIN [[categories]] c ON c.id=p.category_id \
    LEFT \
    JOIN [[brands]] b ON b.id=p.brand_id \
    LEFT \
    JOIN tax_classes t ON t.id=p.tax_class_id \
    LEFT \
    JOIN tax_rates r ON r.id=t.rate_id \
    LEFT \
    JOIN media_objects m ON m.key=p.image_key";
// The database bounds the tree to 1000 nodes / 16 levels and forbids cycles.
// Traverse inactive ancestors too: the visible-products projection owns direct
// publication visibility. IDs never duplicate rows or change backend assignments.
// $2 is the category binding in both listings and relevant-brand queries.
const PUBLIC_CATEGORY_SCOPE: &str = "WITH RECURSIVE category_scope(id) AS (SELECT id \
        FROM catalog_categories \
        WHERE slug=$2 \
        AND active \
        UNION ALL \
        SELECT c.id \
        FROM catalog_categories c \
        JOIN category_scope parent ON c.parent_id=parent.id)";
const PUBLIC_FILTER: &str = "(strpos(lower(p.name),lower($1))>0 \
        OR strpos(lower(p.model),lower($1))>0 \
        OR EXISTS(SELECT 1 \
            FROM catalog_variants v \
            WHERE v.product_id=p.id \
            AND v.active \
            AND strpos(lower(v.sku),lower($1))>0)) \
    AND ($2='' \
        OR p.category_id IN (SELECT id \
            FROM category_scope)) \
    AND ($3='' \
        OR EXISTS(SELECT 1 \
            FROM [[brands]] b \
            WHERE b.id=p.brand_id \
            AND b.slug=$3 \
            AND b.active)) \
    AND (NOT $4 \
        OR CASE WHEN source.manage_stock THEN p.stock ELSE 999999 END >= ((source.minimum_quantity + source.packs - 1) / source.packs) * source.packs)";
const PUBLIC_SELECT: &str = "SELECT p.*,source.manage_stock,source.seo_title,source.seo_description,source.minimum_quantity,source.packs,\
    m.id AS image_id,m.width AS image_width,m.height AS image_height,bm.id AS brand_logo_id,GREATEST(bm.width,bm.height) AS brand_logo_edge,b.slug AS brand_slug \
    FROM [[visible_products]] p \
    JOIN [[products]] source ON source.id=p.id \
    LEFT \
    JOIN media_objects m ON m.key=p.image_key \
    LEFT \
    JOIN [[brands]] b ON b.id=p.brand_id \
    LEFT \
    JOIN media_objects bm ON bm.key=b.logo_key \
    AND bm.public";
const PUBLIC_CARD_SELECT: &str = "SELECT p.id,p.name,p.slug,p.price_cents,p.stock,source.manage_stock,source.minimum_quantity,source.packs,\
    p.has_variants,m.id AS image_id,m.width AS image_width,m.height AS image_height,p.category_name,\
    p.brand_name,bm.id AS brand_logo_id,GREATEST(bm.width,bm.height) AS brand_logo_edge,p.basis_points,p.price_mode,p.currency \
    FROM [[visible_products]] p \
    JOIN catalog_products source ON source.id=p.id \
    LEFT \
    JOIN media_objects m ON m.key=p.image_key \
    LEFT \
    JOIN catalog_brands b ON b.id=p.brand_id \
    LEFT \
    JOIN media_objects bm ON bm.key=b.logo_key \
    AND bm.public";

/// Authorized product editor projection. Its attributes share the product's content language.
pub struct EditorEntry {
    pub product: Entry,
    pub attributes: Vec<crate::attributes::ProductValue>,
    pub variant_commerce: Option<VariantCommerceSummary>,
}

/// Informational totals for active variants; never used as purchasable stock.
#[derive(FromRow)]
pub struct VariantCommerceSummary {
    pub minimum_price: Option<i64>,
    pub maximum_price: Option<i64>,
    pub stock: i64,
}

#[derive(Clone)]
pub struct Products {
    language: Option<talocart_platform::content::Language>,
    pool: PgPool,
    identity: Identity,
    media: MediaStore,
}
impl Products {
    pub fn localized(&self, language: talocart_platform::content::Language) -> Self {
        let mut result = self.clone();
        result.language = Some(language);
        result
    }

    pub fn new(pool: PgPool, identity: Identity, media: MediaStore) -> Self {
        Self {
            language: None,
            pool,
            identity,
            media,
        }
    }
}

fn validate_query(q: &str, page: i64) -> Result<()> {
    if q.chars().count() > 120 || q.chars().any(char::is_control) || !(1..=1000000).contains(&page)
    {
        Err(ProductError::Invalid(vec![]))
    } else {
        Ok(())
    }
}
/// Current purchasable data for bounded cart selections. Catalog owns visibility,
/// variant membership, localized text and fiscal resolution; no stock is reserved here.
#[derive(sqlx::FromRow, Clone)]
pub struct CartMerchandise {
    pub image_id: Option<i64>,
    pub product_id: i64,
    pub variant_id: i64,
    pub name: String,
    pub slug: String,
    pub code: String,
    pub label: String,
    pub unit_cents: i64,
    pub manage_stock: bool,
    pub stock: i32,
    pub minimum_quantity: i32,
    pub packs: i32,
    pub basis_points: i32,
    pub price_mode: String,
    pub currency: String,
}

impl From<IdentityError> for ProductError {
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
    fn catalog_page_size_defaults_to_twenty_one_and_accepts_only_full_rows() {
        assert_eq!(CatalogPageSize::parse("").unwrap().value(), 21);
        assert_eq!(usize::from(CatalogPageSize::default().value()), PAGE_SIZE);
        for size in CATALOG_PAGE_SIZES {
            assert_eq!(
                CatalogPageSize::parse(&size.to_string()).unwrap().value(),
                size
            );
            assert_eq!(size % 3, 0);
        }
        for invalid in [
            "0",
            "-1",
            "20",
            "22",
            "85",
            "100",
            "65536",
            "all",
            "21;SELECT 1",
        ] {
            assert!(CatalogPageSize::parse(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn admin_page_size_is_bounded_and_defaults_to_twenty() {
        assert_eq!(AdminPageSize::parse("").unwrap().value(), 20);
        for size in ADMIN_PAGE_SIZES {
            assert_eq!(
                AdminPageSize::parse(&size.to_string()).unwrap().value(),
                size
            );
        }
        for invalid in ["0", "-1", "11", "101", "65536", "abc", "20;SELECT 1"] {
            assert!(AdminPageSize::parse(invalid).is_err());
            assert!(AdminPageSize::parse_products(invalid).is_err());
        }
        assert_eq!(AdminPageSize::parse_products("").unwrap().value(), 20);
        for size in PRODUCT_PAGE_SIZES {
            assert_eq!(
                AdminPageSize::parse_products(&size.to_string())
                    .unwrap()
                    .value(),
                size
            );
        }
        assert!(AdminPageSize::parse("500").is_err());
        for invalid in ["499", "501"] {
            assert!(AdminPageSize::parse_products(invalid).is_err());
        }
    }
    #[test]
    fn purchase_quantities_use_absolute_pack_multiples_above_the_minimum() {
        for (minimum, packs, first) in [
            (1, 1, 1),
            (5, 1, 5),
            (1, 6, 6),
            (10, 6, 12),
            (12, 6, 12),
            (999999, 999998, 1999996),
        ] {
            let rules = PurchaseQuantities::new(minimum, packs).unwrap();
            assert_eq!(rules.first_quantity(), first);
            assert!(rules.validate(first).is_ok());
            assert!(rules.validate(first + packs).is_ok());
            assert!(!rules.available(first - 1));
            assert!(rules.available(first));
            assert_eq!(rules.validate(0), Err(QuantityError::BelowMinimum));
            assert_eq!(rules.validate(-1), Err(QuantityError::BelowMinimum));
            if packs > 1 {
                assert_eq!(
                    rules.validate(first + 1),
                    Err(QuantityError::NotPackMultiple)
                );
            }
        }
        let rules = PurchaseQuantities::new(10, 6).unwrap();
        assert_eq!(rules.validate(6), Err(QuantityError::BelowMinimum));
        assert_eq!(rules.validate(10), Err(QuantityError::NotPackMultiple));
        assert_eq!(
            rules.validate(16),
            Err(QuantityError::NotPackMultiple),
            "Do not add pack steps to the minimum"
        );
        for bad in [i32::MIN, -1, 0, 1000000, i32::MAX] {
            assert_eq!(
                PurchaseQuantities::new(bad, 1).unwrap_err(),
                QuantityError::InvalidMinimum
            );
            assert_eq!(
                PurchaseQuantities::new(1, bad).unwrap_err(),
                QuantityError::InvalidPacks
            );
        }
    }
    #[test]
    fn product_quantity_fields_default_to_one_and_reject_non_positive_integers() {
        let mut values = Values {
            name: "Quantity".into(),
            model: "QUANTITY".into(),
            slug: "quantity".into(),
            ..Values::default()
        };
        let valid = validate(&values).unwrap();
        assert_eq!((valid.minimum_quantity, valid.packs), (1, 1));
        for bad in [
            "",
            "0",
            "-1",
            "1.5",
            "1,5",
            "1e2",
            " 1 ",
            "1000000",
            "2147483648",
            "<script>",
        ] {
            values.minimum_quantity = bad.into();
            values.packs = bad.into();
            assert!(
                matches!(validate(&values), Err(ProductError::Invalid(fields)) if fields == [Field::MinimumQuantity, Field::Packs])
            );
        }
        values.minimum_quantity = "999999".into();
        values.packs = "999998".into();
        assert!(validate(&values).is_ok());
    }
    #[test]
    fn admin_sort_accepts_only_known_columns_and_directions() {
        assert_eq!(AdminOrder::parse("", "").unwrap(), AdminOrder::default());
        for key in [
            "id",
            "image",
            "type",
            "name",
            "model",
            "category",
            "brand",
            "price",
            "variants",
            "stock",
            "status",
            "modified_at",
        ] {
            for direction in ["asc", "desc"] {
                let order = AdminOrder::parse(key, direction).unwrap();
                assert_eq!(order.column.key(), key);
                assert_eq!(order.direction.key(), direction);
                assert_eq!(order.direction.toggled().toggled(), order.direction);
            }
        }
        for key in [
            "actions",
            "price_cents",
            "Name",
            "name;DROP TABLE catalog_products",
            "name DESC",
            "name\0",
        ] {
            assert!(AdminOrder::parse(key, "asc").is_err());
        }
        for direction in ["ascending", "ASC", "desc NULLS FIRST", "asc;SELECT 1"] {
            assert!(AdminOrder::parse("name", direction).is_err());
        }
    }
    #[test]
    fn numeric_and_modified_filters_reject_invalid_contracts() {
        for (price, stock, date) in [
            ("0", "0", "2028-02-29"),
            ("12,30", "9223372036854775807", "9999-12-31"),
            ("12.30", "15", "0001-01-01"),
        ] {
            for op in ["", "eq", "gt", "lt"] {
                let f = AdminFilters {
                    price,
                    stock,
                    modified_date: date,
                    price_op: op,
                    stock_op: op,
                    ..Default::default()
                };
                assert!(f.validate(1).is_ok());
            }
        }
        for bad in ["-1", "1e2", "1.001", "abc", "92233720368547758.08"] {
            assert!(
                AdminFilters {
                    price: bad,
                    ..Default::default()
                }
                .validate(1)
                .is_err()
            );
        }
        for bad in ["-1", "+1", "1.0", "1e2", " 1 ", "9223372036854775808"] {
            assert!(
                AdminFilters {
                    stock: bad,
                    ..Default::default()
                }
                .validate(1)
                .is_err()
            );
        }
        for bad in [
            "2027-02-29",
            "2026-04-31",
            "0000-01-01",
            "2026-1-01",
            "2026-01-01;SELECT 1",
            "🚀2026-01",
        ] {
            assert!(
                AdminFilters {
                    modified_date: bad,
                    ..Default::default()
                }
                .validate(1)
                .is_err()
            );
        }
        for bad in ["gte", "eq;SELECT 1", "ASC"] {
            assert!(
                AdminFilters {
                    price_op: bad,
                    ..Default::default()
                }
                .validate(1)
                .is_err()
            );
            assert!(
                AdminFilters {
                    stock_op: bad,
                    ..Default::default()
                }
                .validate(1)
                .is_err()
            );
        }
        assert_eq!(
            AdminFilters {
                price: "12,30",
                stock: "0",
                ..Default::default()
            }
            .numeric()
            .unwrap(),
            (Some(1230), Some(0))
        );
    }
    #[test]
    fn product_id_filter_accepts_only_positive_ids() {
        for id in [1, i64::MAX] {
            assert!(
                AdminFilters {
                    id: Some(id),
                    ..Default::default()
                }
                .validate(1)
                .is_ok()
            );
        }
        for id in [0, -1, i64::MIN] {
            assert!(
                AdminFilters {
                    id: Some(id),
                    ..Default::default()
                }
                .validate(1)
                .is_err()
            );
        }
    }
    #[test]
    fn admin_column_filters_validate_limits_and_allowed_choices() {
        assert!(AdminFilters::default().validate(1).is_ok());
        assert!(
            AdminFilters {
                kind: "variants",
                status: "draft",
                model: "%_",
                category: Some(1),
                ..Default::default()
            }
            .validate(1_000_000)
            .is_ok()
        );
        for text in ["\n", "\0", &"a".repeat(121)] {
            for filters in [
                AdminFilters {
                    q: text,
                    ..Default::default()
                },
                AdminFilters {
                    model: text,
                    ..Default::default()
                },
            ] {
                assert!(filters.validate(1).is_err());
            }
        }
        assert!(
            AdminFilters {
                kind: "other",
                ..Default::default()
            }
            .validate(1)
            .is_err()
        );
        assert!(
            AdminFilters {
                status: "active",
                ..Default::default()
            }
            .validate(1)
            .is_err()
        );
        for id in [0, -1, i64::MIN] {
            for filters in [
                AdminFilters {
                    category: Some(id),
                    ..Default::default()
                },
                AdminFilters {
                    brand: Some(id),
                    ..Default::default()
                },
            ] {
                assert!(filters.validate(1).is_err());
            }
        }
        for page in [0, 1_000_001, i64::MAX] {
            assert!(AdminFilters::default().validate(page).is_err());
        }
    }
    #[test]
    fn source_compatibility_preserves_slashes_and_plus_in_commercial_skus() {
        assert_eq!(
            validate_commercial_fields("CMC-MABZ02-WH+-2-3", "5.90", "0")
                .unwrap()
                .0,
            "CMC-MABZ02-WH+-2-3"
        );
        assert_eq!(
            validate_commercial_fields(" CMC-MABZ52-WH/NA-12/18 ", "5.90", "0")
                .unwrap()
                .0,
            "CMC-MABZ52-WH/NA-12/18"
        );
    }
    #[test]
    fn commercial_fields_normalize_sku_and_report_all_invalid_fields() {
        assert_eq!(
            validate_commercial_fields(" model-1._ ", " 12,34 ", "0").unwrap(),
            ("MODEL-1._".into(), 1234, 0)
        );
        assert_eq!(
            validate_commercial_fields(&"A".repeat(64), "9999999999.99", "999999").unwrap(),
            ("A".repeat(64), MAX_PRICE, 999999)
        );
        for model in ["", "-MODEL", "MODEL:1", "à", &"A".repeat(65)] {
            assert!(
                matches!(validate_commercial_fields(model, "0", "0"), Err(ProductError::Invalid(fields)) if fields == [Field::Code])
            );
        }
        for price in ["-0.01", "0.001", "10000000000", "92233720368547758.08"] {
            assert!(
                matches!(validate_commercial_fields("MODEL", price, "0"), Err(ProductError::Invalid(fields)) if fields == [Field::Price])
            );
        }
        for stock in ["-1", "1000000", "2147483648", "1.0", ""] {
            assert!(
                matches!(validate_commercial_fields("MODEL", "0", stock), Err(ProductError::Invalid(fields)) if fields == [Field::Stock])
            );
        }
        assert!(
            matches!(validate_commercial_fields("?", "bad", "bad"), Err(ProductError::Invalid(fields)) if fields == [Field::Code, Field::Price, Field::Stock])
        );
    }
    #[test]
    fn published_product_allows_no_category_but_requires_tax_class() {
        let mut input = Values {
            name: "Without category".into(),
            model: "NO-CATEGORY".into(),
            slug: "without-category".into(),
            status: "published".into(),
            tax_class_id: "2".into(),
            ..Values::default()
        };
        assert!(
            validate(&input).is_ok(),
            "Category is optional when publishing"
        );
        input.tax_class_id.clear();
        assert!(
            matches!(validate(&input),Err(ProductError::Invalid(fields)) if fields == [Field::TaxClass])
        );
        input.tax_class_id = "2".into();
        input.category_id = "-1".into();
        assert!(
            matches!(validate(&input),Err(ProductError::Invalid(fields)) if fields == [Field::Category])
        );
    }
    #[test]
    fn simple_product_validation_is_exact_and_requires_publication_references() {
        let mut v = Values {
            name: "  Demo  ".into(),
            model: " demo-1 ".into(),
            slug: "demo-1".into(),
            price: "12,34".into(),
            ..Values::default()
        };
        let valid = validate(&v).unwrap();
        assert_eq!(valid.name, "Demo");
        assert_eq!(valid.model, "DEMO-1");
        assert_eq!(valid.price, 1234);
        v.status = "published".into();
        assert!(matches!(validate(&v),Err(ProductError::Invalid(f))if f == [Field::TaxClass]));
        v.category_id = "1".into();
        v.tax_class_id = "2".into();
        assert!(validate(&v).is_ok());
        for model in ["", "-invalid", "é", "a b", "/a"] {
            v.model = model.into();
            assert!(
                matches!(validate(&v),Err(ProductError::Invalid(f))if f.contains(&Field::Code))
            );
        }
        v.model = "DEMO-1".into();
        v.price = "9999999999.99".into();
        assert!(validate(&v).is_ok());
        v.name = "\0".into();
        v.price = "10000000000.00".into();
        v.stock = "-1".into();
        v.slug = "Bad--slug".into();
        v.description = "a".repeat(4001);
        v.status = "bad".into();
        v.brand_id = "0".into();
        assert!(matches!(validate(&v),Err(ProductError::Invalid(f))if f.len()==7));
    }
}
