mod admin;
mod preference;
mod related;
pub(crate) mod search;
mod storefront;
use super::*;
use axum::{
    extract::{Form, Multipart, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::{
    pricing::{LineTotals, PriceMode},
    products::{
        AdminFilters, AdminOrder, AdminPageSize, AdminSort, CATALOG_PAGE_SIZES, CatalogPageSize,
        Entry, Field, GalleryAction, GalleryImage, MAX_GALLERY_IMAGES, PRODUCT_PAGE_SIZES,
        ProductError, SortDirection, Values,
    },
};
type Result<T> = std::result::Result<T, ProductResponseError>;
pub struct ProductResponseError(ProductError);
impl From<ProductError> for ProductResponseError {
    fn from(error: ProductError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for ProductResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl From<talocart_commerce::CatalogError> for ProductResponseError {
    fn from(error: talocart_commerce::CatalogError) -> Self {
        match error {
            talocart_commerce::CatalogError::Technical(source_error) => source_error.into(),
            talocart_commerce::CatalogError::Access(source_error) => source_error.into(),
            talocart_commerce::CatalogError::Identity(source_error) => source_error.into(),
            talocart_commerce::CatalogError::NotFound => ProductError::NotFound.into(),
            talocart_commerce::CatalogError::Invalid(_) => ProductError::Invalid(vec![]).into(),
            _ => talocart_platform::identity::IdentityError::Unavailable.into(),
        }
    }
}
impl From<talocart_commerce::cart::CartError> for ProductResponseError {
    fn from(error: talocart_commerce::cart::CartError) -> Self {
        match error {
            talocart_commerce::cart::CartError::Technical(source_error) => source_error.into(),
            talocart_commerce::cart::CartError::Access(source_error) => source_error.into(),
            talocart_commerce::cart::CartError::Identity(source_error) => source_error.into(),
            talocart_commerce::cart::CartError::Forbidden => {
                talocart_platform::identity::IdentityError::Forbidden.into()
            }
            _ => ProductError::Invalid(vec![]).into(),
        }
    }
}
impl From<talocart_commerce::taxes::TaxError> for ProductResponseError {
    fn from(error: talocart_commerce::taxes::TaxError) -> Self {
        match error {
            talocart_commerce::taxes::TaxError::Technical(source_error) => source_error.into(),
            talocart_commerce::taxes::TaxError::Access(source_error) => source_error.into(),
            talocart_commerce::taxes::TaxError::Identity(source_error) => source_error.into(),
            _ => talocart_platform::identity::IdentityError::Unavailable.into(),
        }
    }
}
impl From<talocart_commerce::variants::VariantError> for ProductResponseError {
    fn from(error: talocart_commerce::variants::VariantError) -> Self {
        match error {
            talocart_commerce::variants::VariantError::Technical(error) => error.into(),
            talocart_commerce::variants::VariantError::Access(error) => error.into(),
            talocart_commerce::variants::VariantError::Identity(error) => error.into(),
            talocart_commerce::variants::VariantError::Product(error) => error.into(),
            _ => talocart_platform::identity::IdentityError::Unavailable.into(),
        }
    }
}
pub(crate) fn status(error: &ProductError) -> StatusCode {
    match error {
        ProductError::Content(error) => crate::content::status(error),
        ProductError::Invalid(_)
        | ProductError::Selection
        | ProductError::BulkConfirmation
        | ProductError::BulkAction
        | ProductError::Confirmation
        | ProductError::Image
        | ProductError::GalleryAction => StatusCode::UNPROCESSABLE_ENTITY,
        ProductError::GalleryFull
        | ProductError::StockChanged
        | ProductError::ModeChanged
        | ProductError::DuplicateModel
        | ProductError::DuplicateSlug
        | ProductError::Publication
        | ProductError::InUse => StatusCode::CONFLICT,
        ProductError::NotFound => StatusCode::NOT_FOUND,
        ProductError::Technical(source_error) => crate::access::technical_status(source_error),
        ProductError::Access(source_error) => crate::access::access_status(source_error),
        ProductError::Identity(source_error) => {
            access::IdentityErrorResponse::status_for(source_error)
        }
    }
}
impl IntoResponse for ProductResponseError {
    fn into_response(self) -> Response {
        if let ProductError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let ProductError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let mut response = (status(&self.0), i18n::product_error(&self.0)).into_response();
        if let ProductError::Identity(error) = &self.0 {
            access::IdentityErrorResponse::retry_for(error, &mut response);
        }
        response
    }
}
#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    id: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    price_op: String,
    #[serde(default)]
    stock: String,
    #[serde(default)]
    stock_op: String,
    #[serde(default)]
    modified_date: String,
    #[serde(default)]
    sort: String,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    per_page: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    brand: String,
    #[serde(default)]
    available: bool,
    #[serde(default = "first_page")]
    page: i64,
    deleted: Option<String>,
    bulk_deleted: Option<String>,
    bulk_status: Option<String>,
    bulk_category: Option<String>,
    bulk_applied: Option<String>,
}
fn first_page() -> i64 {
    1
}
impl ListQuery {
    fn admin_link(&self, order: AdminOrder, size: AdminPageSize) -> String {
        let mut link = format!(
            "/admin/catalog/products?q={}&status={}&type={}&model={}&category={}&brand={}&sort={}&dir={}&per_page={}",
            catalog::encoded(&self.q),
            catalog::encoded(&self.status),
            catalog::encoded(&self.kind),
            catalog::encoded(&self.model),
            catalog::encoded(&self.category),
            catalog::encoded(&self.brand),
            order.column.key(),
            order.direction.key(),
            size.value()
        );
        for (key, value) in [
            ("price", &self.price),
            ("price_op", &self.price_op),
            ("stock", &self.stock),
            ("stock_op", &self.stock_op),
            ("modified_date", &self.modified_date),
            ("id", &self.id),
        ] {
            if !value.is_empty() {
                link.push_str(&format!("&{key}={}", catalog::encoded(value)));
            }
        }
        link
    }
}
fn euro_currency() -> String {
    "EUR".into()
}

pub use admin::routes;
pub(crate) use admin::{configuration_result, missing_stock_revision};
use storefront::{ListingKind, listing_response};
pub use storefront::{
    public_brand_index, public_category_index, public_detail, public_index, public_price,
};

impl From<talocart_platform::technical::TechnicalError> for ProductResponseError {
    fn from(error: talocart_platform::technical::TechnicalError) -> Self {
        Self(ProductError::Technical(error))
    }
}

impl From<talocart_platform::technical::AccessError> for ProductResponseError {
    fn from(error: talocart_platform::technical::AccessError) -> Self {
        Self(ProductError::Access(error))
    }
}
