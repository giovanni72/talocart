//! Reusable options and variant identities. All mutations share identity/fiscal/media locks.
mod imports;
/// Values shown per administrative page of an option; also locates a value's page.
pub const VALUE_PAGE_SIZE: u16 = 50;
/// Longest accepted value search, matching the value name limit.
pub const VALUE_SEARCH_MAX: usize = 80;

/// Literal, case-insensitive match on the displayed name of `alias` or on its
/// translation in any of the `$locales` languages; an empty `$text` matches all.
/// `alias` and parameter positions are static owner SQL, never browser input.
fn value_filter(alias: &str, text: usize, locales: usize) -> String {
    format!(
        "(${text}='' \
                OR strpos(lower({alias}.name),lower(${text}))>0 \
                OR EXISTS(SELECT 1 \
                    FROM catalog_option_values_translations t \
                    WHERE t.option_id={alias}.option_id \
                    AND t.entity_id={alias}.id \
                    AND t.locale=ANY(${locales}::text[]) \
                    AND strpos(lower(t.name),lower(${text}))>0))"
    )
}
/// One-based page of value `$2` within the values of option `$1` that match the
/// search (`$4`, `$5`), using `$3` rows per page and the editor ordering.
fn value_page_query() -> String {
    format!(
        "SELECT 1+(SELECT count(*) \
                FROM [[option_values]] preceding \
                WHERE preceding.option_id=v.option_id \
                AND {} \
                AND (preceding.sort_order,lower(preceding.name),preceding.id)<(v.sort_order,lower(v.name),\
                    v.id))/$3 \
            FROM [[option_values]] v \
            WHERE v.option_id=$1 \
            AND v.id=$2 \
            AND {}",
        value_filter("preceding", 4, 5),
        value_filter("v", 4, 5)
    )
}

/// Validated search over option values: trimmed literal text plus the content
/// languages whose translations participate. The default matches every value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValueSearch {
    text: String,
    locales: Vec<String>,
}
impl ValueSearch {
    pub fn new(text: &str, locales: &[String]) -> Result<Self> {
        let text = text.trim();
        if text.chars().count() > VALUE_SEARCH_MAX || text.chars().any(char::is_control) {
            return Err(VariantError::Search);
        }
        Ok(Self {
            text: text.to_owned(),
            locales: locales.to_vec(),
        })
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

mod commands;
mod reads;
use crate::products::{self, ProductError};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use talocart_platform::content::{Draft, Translation};
use talocart_platform::{
    identity::{Identity, IdentityError, Permission, Session},
    media::{MediaError, MediaStore},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VariantError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error(transparent)]
    Content(talocart_platform::content::ContentError),
    #[error("Inserisci un nome da 1 a 80 caratteri senza caratteri di controllo")]
    Name,
    #[error("Questo nome è già utilizzato")]
    DuplicateName,
    #[error("Inserisci un ordinamento intero da 0 a 9999")]
    SortOrder,
    #[error("Questo SKU è già utilizzato da un’altra variante")]
    DuplicateSku,
    #[error("Questa combinazione esiste già nel prodotto")]
    DuplicateCombination,
    #[error("Scegli un valore per ogni opzione del prodotto, senza duplicati")]
    Combination,
    #[error("Scegli da una a tre opzioni diverse, oppure Nessuna per un prodotto semplice")]
    Options,
    #[error(
        "Elimina prima le varianti per cambiare le opzioni; opzioni e valori utilizzati non possono essere eliminati"
    )]
    InUse,
    #[error("Risorsa non trovata nel prodotto o nell’opzione indicata")]
    NotFound,
    #[error("Conferma l’eliminazione")]
    Confirmation,
    #[error("Filtri varianti non validi")]
    Filters,
    #[error("Inserisci una ricerca di al massimo 80 caratteri senza caratteri di controllo")]
    Search,
    #[error(transparent)]
    Product(#[from] ProductError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<talocart_platform::content::ContentError> for VariantError {
    fn from(error: talocart_platform::content::ContentError) -> Self {
        match error {
            talocart_platform::content::ContentError::Conflict => Self::DuplicateName,
            other => Self::Content(other),
        }
    }
}
impl From<sqlx::Error> for VariantError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            match db.constraint() {
                Some("order_stock_structure") => return Self::InUse,
                Some("catalog_options_name_unique" | "catalog_option_values_name_unique") => {
                    return Self::DuplicateName;
                }
                Some("catalog_variants_sku_unique") => return Self::DuplicateSku,
                Some("catalog_variants_product_id_value_ids_key") => {
                    return Self::DuplicateCombination;
                }
                Some("catalog_variants_combination") => return Self::Combination,
                Some(
                    "catalog_product_options_in_use"
                    | "catalog_option_values_owner"
                    | "catalog_variants_owner",
                ) => return Self::InUse,
                _ => {}
            }
            if db.is_foreign_key_violation() {
                return Self::InUse;
            }
        }
        Self::Technical(
            talocart_platform::technical::DatabaseFailure::capture(error, module_path!()).into(),
        )
    }
}
impl From<MediaError> for VariantError {
    fn from(e: MediaError) -> Self {
        Self::Product(e.into())
    }
}
pub type Result<T> = std::result::Result<T, VariantError>;
/// Combined administrative filters. Text matches literal, case-insensitive substrings
/// of the displayed combination label and SKU, within the requested product only.
#[derive(Default)]
pub struct AdminFilters<'a> {
    pub combination: &'a str,
    pub sku: &'a str,
    pub status: &'a str,
    pub page_size: products::AdminPageSize,
}
impl AdminFilters<'_> {
    fn validate(&self) -> Result<Option<bool>> {
        for text in [self.combination, self.sku] {
            if text.chars().count() > 120 || text.chars().any(char::is_control) {
                return Err(VariantError::Filters);
            }
        }
        match self.status {
            "" => Ok(None),
            "active" => Ok(Some(true)),
            "inactive" => Ok(Some(false)),
            _ => Err(VariantError::Filters),
        }
    }
}
#[derive(Clone, FromRow)]
pub struct OptionEntry {
    pub id: i64,
    pub name: String,
    pub sort_order: i32,
}
#[derive(Clone, FromRow)]
pub struct ValueEntry {
    pub id: i64,
    pub option_id: i64,
    pub name: String,
    pub sort_order: i32,
}
/// Shared, language-independent priority. Omission preserves an existing priority
/// (or uses zero on creation), including callers that only update translated names.
pub struct OptionInput<'a> {
    pub name: &'a str,
    pub sort_order: Option<&'a str>,
}
fn sort_order(input: Option<&str>) -> Result<Option<i32>> {
    input
        .map(|value| {
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(VariantError::SortOrder);
            }
            value
                .parse::<i32>()
                .ok()
                .filter(|n| (0..=9999).contains(n))
                .ok_or(VariantError::SortOrder)
        })
        .transpose()
}
#[derive(Clone)]
pub struct Axis {
    /// Persisted product position (1–3), kept stable in value_N form/URL fields.
    pub position: i16,
    pub id: i64,
    pub name: String,
    pub values: Vec<ValueEntry>,
}
pub struct Configuration {
    pub axes: Vec<Axis>,
    pub count: i64,
}
#[derive(Clone, Default)]
pub struct Values {
    pub sku: String,
    pub price: String,
    pub price_mode: String,
    pub stock: String,
    pub stock_revision: i64,
    pub active: bool,
    pub value_ids: Vec<i64>,
}
#[derive(Clone, FromRow)]
pub struct Entry {
    pub id: i64,
    pub product_id: i64,
    pub value_ids: Vec<i64>,
    pub sku: String,
    pub price_cents: i64,
    pub stock: i32,
    pub stock_revision: i64,
    pub active: bool,
    pub image_key: Option<String>,
    pub image_id: Option<i64>,
    pub image_width: Option<i32>,
    pub image_height: Option<i32>,
    pub label: String,
}
impl Entry {
    pub fn image_url(&self, profile: &str) -> String {
        match (self.image_id, &self.image_key) {
            (Some(id), Some(_)) => format!("/media/image/{id}/{profile}"),
            _ => String::new(),
        }
    }
    pub fn price(&self) -> String {
        format!("{},{:02}", self.price_cents / 100, self.price_cents % 100)
    }
    pub fn values(&self, mode: &str) -> Values {
        Values {
            sku: self.sku.clone(),
            price: self.price(),
            price_mode: mode.into(),
            stock: self.stock.to_string(),
            stock_revision: self.stock_revision,
            active: self.active,
            value_ids: self.value_ids.clone(),
        }
    }
}
const SELECT: &str = "SELECT v.*,m.id AS image_id,m.width AS image_width,m.height AS image_height,coalesce((SELECT string_agg(o.name||': '||x.name,\
                ' / ' \
                ORDER BY a.position) \
            FROM catalog_variant_values c \
            JOIN [[option_values]] x ON x.id=c.value_id \
            JOIN [[options]] o ON o.id=x.option_id \
            JOIN catalog_product_options a ON a.option_id=o.id \
            AND a.product_id=v.product_id \
            WHERE c.variant_id=v.id),'') AS label \
    FROM catalog_variants v \
    LEFT \
    JOIN media_objects m ON m.key=v.image_key";
/// Positive pages with a representable SQL offset; no fixed maximum page count.
fn page_offset(page: i64) -> Result<i64> {
    page_offset_sized(page, 20)
}
fn value_page_offset(page: i64) -> Result<i64> {
    page_offset_sized(page, VALUE_PAGE_SIZE)
}
fn page_offset_sized(page: i64, size: u16) -> Result<i64> {
    if page <= 0 {
        return Err(VariantError::Options);
    }
    page.checked_sub(1)
        .and_then(|n| n.checked_mul(i64::from(size)))
        .ok_or(VariantError::Options)
}
fn name(input: &str) -> Result<String> {
    let value = input.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        Err(VariantError::Name)
    } else {
        Ok(value.to_owned())
    }
}
fn validate(v: &Values) -> Result<(String, i64, i32, Vec<i64>)> {
    let (sku, price, stock) = products::validate_commercial_fields(&v.sku, &v.price, &v.stock)?;
    let mut ids = v.value_ids.clone();
    ids.sort_unstable();
    if ids.is_empty()
        || ids.len() > 3
        || ids.iter().any(|n| *n <= 0)
        || ids.windows(2).any(|w| w[0] == w[1])
    {
        return Err(VariantError::Combination);
    }
    Ok((sku, price, stock, ids))
}
#[derive(Clone)]
pub struct Variants {
    language: Option<talocart_platform::content::Language>,
    pool: PgPool,
    identity: Identity,
    media: MediaStore,
}
impl Variants {
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

impl From<IdentityError> for VariantError {
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
    fn option_priority_validates_bounds_and_omission() {
        assert_eq!(sort_order(None).unwrap(), None);
        for (input, expected) in [("0", 0), ("9999", 9999), (" 42 ", 42)] {
            assert_eq!(sort_order(Some(input)).unwrap(), Some(expected));
        }
        for input in ["", "-1", "10000", "1.5", "abc", "999999999999999999999"] {
            assert!(matches!(
                sort_order(Some(input)),
                Err(VariantError::SortOrder)
            ));
        }
    }

    #[test]
    fn administrative_variant_filters_are_bounded_and_validate_status() {
        assert_eq!(AdminFilters::default().validate().unwrap(), None);
        for (status, expected) in [("active", true), ("inactive", false)] {
            assert_eq!(
                AdminFilters {
                    status,
                    ..Default::default()
                }
                .validate()
                .unwrap(),
                Some(expected)
            );
        }
        let boundary = "è".repeat(120);
        assert!(
            AdminFilters {
                combination: &boundary,
                sku: " %_ ",
                status: "",
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        for invalid in ["x".repeat(121), "rosso\n".into(), "\0".into()] {
            for filters in [
                AdminFilters {
                    combination: &invalid,
                    ..Default::default()
                },
                AdminFilters {
                    sku: &invalid,
                    ..Default::default()
                },
            ] {
                assert!(matches!(filters.validate(), Err(VariantError::Filters)));
            }
        }
        assert!(matches!(
            AdminFilters {
                status: "published",
                ..Default::default()
            }
            .validate(),
            Err(VariantError::Filters)
        ));
    }
    #[test]
    fn pagination_offsets_are_checked_without_a_fixed_page_cap() {
        for size in products::ADMIN_PAGE_SIZES {
            let last = i64::MAX / i64::from(size) + 1;
            assert_eq!(page_offset_sized(2, size).unwrap(), i64::from(size));
            assert!(page_offset_sized(last, size).is_ok());
            assert!(page_offset_sized(last + 1, size).is_err());
        }
        assert_eq!(page_offset(1).unwrap(), 0);
        assert_eq!(page_offset(1_000_001).unwrap(), 20_000_000);
        assert_eq!(
            page_offset(i64::MAX / 20 + 1).unwrap(),
            (i64::MAX / 20) * 20
        );
        for page in [0, -1, i64::MIN, i64::MAX] {
            assert!(page_offset(page).is_err());
            assert!(value_page_offset(page).is_err());
        }
        assert_eq!(value_page_offset(3).unwrap(), 100);
    }
    #[test]
    fn value_search_is_trimmed_bounded_and_literal() {
        let locales = vec!["it".to_owned(), "en".to_owned()];
        let search = ValueSearch::new("  50% _Verde  ", &locales).unwrap();
        assert_eq!(search.text(), "50% _Verde");
        assert!(!search.is_empty());
        assert!(ValueSearch::new("   ", &locales).unwrap().is_empty());
        assert!(ValueSearch::new(&"è".repeat(VALUE_SEARCH_MAX), &locales).is_ok());
        assert!(matches!(
            ValueSearch::new(&"è".repeat(VALUE_SEARCH_MAX + 1), &locales),
            Err(VariantError::Search)
        ));
        assert!(matches!(
            ValueSearch::new("ver\u{0}de", &locales),
            Err(VariantError::Search)
        ));
        let filter = value_filter("v", 2, 3);
        assert!(filter.contains("strpos(lower(v.name),lower($2))>0"));
        assert!(filter.contains("t.locale=ANY($3::text[])"));
        assert!(!filter.contains("LIKE"));
    }
    #[test]
    fn option_and_variant_fields_are_bounded_and_exact() {
        assert_eq!(name("  Colore ").unwrap(), "Colore");
        for invalid in ["", "\n", &"x".repeat(81)] {
            assert!(name(invalid).is_err());
        }
        let mut v = Values {
            sku: " demo-1 ".into(),
            price: "0,05".into(),
            stock: "0".into(),
            value_ids: vec![2, 1],
            ..Values::default()
        };
        let valid = validate(&v).unwrap();
        assert_eq!(
            (valid.0, valid.1, valid.2, valid.3),
            ("DEMO-1".into(), 5, 0, vec![1, 2])
        );
        v.value_ids = vec![1, 1];
        assert!(matches!(validate(&v), Err(VariantError::Combination)));
        v.value_ids = vec![1];
        v.price = "0.001".into();
        assert!(validate(&v).is_err());
    }
}
