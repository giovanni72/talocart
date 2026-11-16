//! Variant HTTP composition; option editing lives in its private module.
mod options;
use super::*;
use axum::{
    extract::{Form, Multipart, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::{
    products::{ADMIN_PAGE_SIZES, AdminPageSize, Field, ProductError},
    variants::{self, Values, VariantError},
};
use talocart_platform::identity::IdentityError;
type Result<T> = std::result::Result<T, ErrorResponse>;
pub struct ErrorResponse(VariantError);
impl From<VariantError> for ErrorResponse {
    fn from(error: VariantError) -> Self {
        Self(error)
    }
}
impl From<IdentityError> for ErrorResponse {
    fn from(error: IdentityError) -> Self {
        Self(error.into())
    }
}
impl From<ProductError> for ErrorResponse {
    fn from(error: ProductError) -> Self {
        Self(error.into())
    }
}
pub(crate) fn status(source_error: &VariantError) -> StatusCode {
    match source_error {
        VariantError::Content(error) => crate::content::status(error),
        VariantError::Technical(source_error) => crate::access::technical_status(source_error),
        VariantError::Access(source_error) => crate::access::access_status(source_error),
        VariantError::Identity(source_error) => {
            access::IdentityErrorResponse::status_for(source_error)
        }
        VariantError::Product(source_error) => super::products::status(source_error),
        VariantError::NotFound => StatusCode::NOT_FOUND,
        VariantError::Name
        | VariantError::Combination
        | VariantError::Options
        | VariantError::Confirmation
        | VariantError::Filters
        | VariantError::Search
        | VariantError::SortOrder => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::CONFLICT,
    }
}
impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        if let VariantError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let VariantError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let mut r = (status(&self.0), i18n::variant_error(&self.0)).into_response();
        if let VariantError::Identity(error) = &self.0 {
            access::IdentityErrorResponse::retry_for(error, &mut r);
        }
        r
    }
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(options::routes())
        .route("/admin/catalog/products/{product}/options", post(configure))
        .route("/admin/catalog/products/{product}/variants", get(index))
        .route(
            "/admin/catalog/products/{product}/variants/new",
            get(new_variant).post(create_variant),
        )
        .route(
            "/admin/catalog/products/{product}/variants/{id}",
            get(edit_variant).post(update_variant),
        )
        .route(
            "/admin/catalog/products/{product}/variants/{id}/delete",
            post(delete_variant),
        )
        .route(
            "/admin/catalog/products/{product}/variants/{id}/image",
            post(upload).layer(DefaultBodyLimit::max(
                talocart_platform::identity::BRANDING_UPLOAD_LIMIT + 16 * 1024,
            )),
        )
        .route(
            "/admin/catalog/products/{product}/variants/{id}/image/remove",
            post(remove_image),
        )
}
#[derive(Deserialize)]
struct Page {
    #[serde(default = "first_page")]
    page: i64,
    #[serde(default)]
    saved: String,
    #[serde(default)]
    deleted: String,
    #[serde(default)]
    q: String,
}
fn first_page() -> i64 {
    1
}
#[derive(Deserialize)]
struct Action {
    csrf: String,
    #[serde(default)]
    confirm: bool,
}
#[derive(Deserialize)]
struct ConfigForm {
    csrf: String,
    #[serde(default)]
    option_1: String,
    #[serde(default)]
    option_2: String,
    #[serde(default)]
    option_3: String,
}
async fn configure(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(product): Path<i64>,
    Form(f): Form<ConfigForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let raw = [f.option_1, f.option_2, f.option_3];
    let mut options = vec![];
    let mut empty = false;
    for value in raw {
        if value.is_empty() {
            empty = true;
        } else if empty {
            return Err(VariantError::Options.into());
        } else {
            options.push(value.parse().map_err(|_| VariantError::Options)?);
        }
    }
    let result = state
        .variants
        .configure(&session, &f.csrf, product, &options)
        .await;
    Ok(
        match super::products::configuration_result(&state, &session, product, result).await {
            Ok(r) => r,
            Err(error) => error.into_response(),
        },
    )
}
#[derive(Template)]
#[template(path = "variant-list.html")]
struct VariantList<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    product: talocart_commerce::products::Entry,
    rows: Vec<variants::Entry>,
    can_write: bool,
    configured: bool,
    pagination: super::pagination::Pagination,
    deleted: bool,
    combination: &'a str,
    sku: &'a str,
    status: &'a str,
    filtered: bool,
    page_sizes: &'static [u16],
    page_size: u16,
}
#[derive(Deserialize)]
struct VariantQuery {
    #[serde(default = "first_page")]
    page: i64,
    #[serde(default)]
    combination: String,
    #[serde(default)]
    sku: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    per_page: String,
    #[serde(default)]
    deleted: String,
}
impl VariantQuery {
    fn link(&self, product: i64, page: i64, size: AdminPageSize) -> String {
        format!(
            "/admin/catalog/products/{product}/variants?combination={}&sku={}&status={}&per_page={}&page={page}",
            catalog::encoded(&self.combination),
            catalog::encoded(&self.sku),
            catalog::encoded(&self.status),
            size.value()
        )
    }
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<VariantQuery>,
) -> Result<Response> {
    let product = state.products.entry(&session, id).await?;
    let conf = state.variants.configuration(&session, id).await?;
    let page_size = AdminPageSize::parse(&q.per_page).map_err(|_| ProductError::Invalid(vec![]))?;
    let filters = variants::AdminFilters {
        combination: &q.combination,
        sku: &q.sku,
        status: &q.status,
        page_size,
    };
    let result = state
        .variants
        .list_filtered_counted(&session, id, &filters, q.page)
        .await?;
    let rows = result.entries;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let store = admin_shell.store_name;
    let link = |n| q.link(id, n, page_size);
    Ok(render(VariantList {
        store_name: &store,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "products",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        product,
        rows,
        configured: !conf.axes.is_empty(),
        can_write: session.allows(Permission::ProductsWrite),
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            page_size.value(),
            &i18n::text("admin-variants-2"),
            &i18n::text("admin-variant-pages"),
            link,
        ),
        deleted: q.deleted == "1",
        combination: &q.combination,
        sku: &q.sku,
        status: &q.status,
        filtered: !q.combination.trim().is_empty()
            || !q.sku.trim().is_empty()
            || !q.status.is_empty(),
        page_sizes: &ADMIN_PAGE_SIZES,
        page_size: page_size.value(),
    }))
}
pub(crate) struct Choice {
    pub id: i64,
    pub name: String,
    pub selected: bool,
}
pub(crate) struct AxisView {
    pub name: String,
    pub field: String,
    pub choices: Vec<Choice>,
}
pub(crate) fn axis_views(axes: Vec<variants::Axis>, chosen: &[i64]) -> Vec<AxisView> {
    axes.into_iter()
        .map(|a| AxisView {
            name: a.name,
            field: format!("value_{}", a.position),
            choices: a
                .values
                .into_iter()
                .map(|v| Choice {
                    id: v.id,
                    name: v.name,
                    selected: chosen.contains(&v.id),
                })
                .collect(),
        })
        .collect()
}
#[derive(Template)]
#[template(path = "variant-form.html")]
struct VariantPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    product: talocart_commerce::products::Entry,
    id: i64,
    values: &'a Values,
    axes: Vec<AxisView>,
    image: String,
    can_write: bool,
    saved: bool,
    error: String,
    sku_error: bool,
    price_error: bool,
    stock_error: bool,
    combination_error: bool,
}
fn invalid(error: Option<&VariantError>, field: Field) -> bool {
    match error {
        Some(VariantError::Product(ProductError::Invalid(fields))) => fields.contains(&field),
        Some(VariantError::DuplicateSku) => field == Field::Code,
        _ => false,
    }
}
async fn form_page(
    state: &AppState,
    session: &Session,
    product: i64,
    id: i64,
    values: &Values,
    error: Option<&VariantError>,
    saved: bool,
) -> Result<Response> {
    let p = state.products.entry(session, product).await?;
    let configuration = state.variants.configuration(session, product).await?;
    if configuration.axes.is_empty() {
        return Err(VariantError::Options.into());
    }
    let image = if id > 0 {
        state
            .variants
            .entry(session, product, id)
            .await?
            .image_url("card")
    } else {
        String::new()
    };
    let admin_shell = AdminShell::load(state, session).await?;
    let store = admin_shell.store_name;
    let mut r = render(VariantPage {
        store_name: &store,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "products",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        product: p,
        id,
        values,
        axes: axis_views(configuration.axes, &values.value_ids),
        image,
        can_write: session.allows(Permission::ProductsWrite),
        saved,
        error: error.map(i18n::variant_error).unwrap_or_default(),
        sku_error: invalid(error, Field::Code),
        price_error: invalid(error, Field::Price),
        stock_error: invalid(error, Field::Stock),
        combination_error: matches!(
            error,
            Some(VariantError::Combination | VariantError::DuplicateCombination)
        ),
    });
    if let Some(source_error) = error {
        *r.status_mut() = status(source_error);
    }
    Ok(r)
}
async fn new_variant(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(product): Path<i64>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    let p = state.products.entry(&session, product).await?;
    let v = Values {
        price: p.values().price,
        price_mode: p.price_mode,
        stock: "0".into(),
        active: true,
        ..Values::default()
    };
    form_page(&state, &session, product, 0, &v, None, false).await
}
async fn edit_variant(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((product, id)): Path<(i64, i64)>,
    Query(q): Query<Page>,
) -> Result<Response> {
    let p = state.products.entry(&session, product).await?;
    let v = state
        .variants
        .entry(&session, product, id)
        .await?
        .values(&p.price_mode);
    form_page(&state, &session, product, id, &v, None, q.saved == "1").await
}
#[derive(Deserialize)]
struct VariantForm {
    csrf: String,
    sku: String,
    price: String,
    price_mode: String,
    stock: String,
    #[serde(default = "super::products::missing_stock_revision")]
    stock_revision: i64,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    value_1: String,
    #[serde(default)]
    value_2: String,
    #[serde(default)]
    value_3: String,
}
async fn save_variant(
    state: AppState,
    session: Session,
    product: i64,
    id: Option<i64>,
    f: VariantForm,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let mut values = Values {
        sku: f.sku,
        price: f.price,
        price_mode: f.price_mode,
        stock: f.stock,
        stock_revision: f.stock_revision,
        active: f.active,
        value_ids: vec![],
    };
    let mut malformed = false;
    for raw in [f.value_1, f.value_2, f.value_3] {
        if !raw.is_empty() {
            match raw.parse::<i64>() {
                Ok(id) => values.value_ids.push(id),
                Err(_) => malformed = true,
            }
        }
    }
    let result = if malformed {
        Err(VariantError::Combination)
    } else {
        state
            .variants
            .save(&session, &f.csrf, product, id, &values)
            .await
    };
    match result {
        Ok(id) => Ok(Redirect::to(&format!(
            "/admin/catalog/products/{product}/variants/{id}?saved=1"
        ))
        .into_response()),
        Err(VariantError::Technical(error)) => Err(error.into()),
        Err(VariantError::Access(error)) => Err(error.into()),
        Err(VariantError::Identity(error)) => Err(error.into()),
        Err(VariantError::NotFound) => Err(VariantError::NotFound.into()),
        Err(error) => {
            form_page(
                &state,
                &session,
                product,
                id.unwrap_or(0),
                &values,
                Some(&error),
                false,
            )
            .await
        }
    }
}
async fn create_variant(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(product): Path<i64>,
    Form(f): Form<VariantForm>,
) -> Result<Response> {
    save_variant(state, session, product, None, f).await
}
async fn update_variant(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((product, id)): Path<(i64, i64)>,
    Form(f): Form<VariantForm>,
) -> Result<Response> {
    save_variant(state, session, product, Some(id), f).await
}
async fn delete_variant(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((product, id)): Path<(i64, i64)>,
    Form(f): Form<Action>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    if !f.confirm {
        let p = state.products.entry(&session, product).await?;
        let v = state
            .variants
            .entry(&session, product, id)
            .await?
            .values(&p.price_mode);
        return form_page(
            &state,
            &session,
            product,
            id,
            &v,
            Some(&VariantError::Confirmation),
            false,
        )
        .await;
    }
    state
        .variants
        .delete(&session, &f.csrf, product, id)
        .await?;
    Ok(Redirect::to(&format!(
        "/admin/catalog/products/{product}/variants?deleted=1"
    ))
    .into_response())
}
async fn image_result(
    state: &AppState,
    session: &Session,
    product: i64,
    id: i64,
    result: variants::Result<()>,
) -> Result<Response> {
    match result {
        Ok(()) => Ok(Redirect::to(&format!(
            "/admin/catalog/products/{product}/variants/{id}?saved=1"
        ))
        .into_response()),
        Err(VariantError::Technical(error)) => Err(error.into()),
        Err(VariantError::Access(error)) => Err(error.into()),
        Err(VariantError::Identity(error)) => Err(error.into()),
        Err(VariantError::NotFound) => Err(VariantError::NotFound.into()),
        Err(error) => {
            let p = state.products.entry(session, product).await?;
            let v = state
                .variants
                .entry(session, product, id)
                .await?
                .values(&p.price_mode);
            form_page(state, session, product, id, &v, Some(&error), false).await
        }
    }
}
async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((product, id)): Path<(i64, i64)>,
    multipart: Multipart,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    let result = match branding::read_upload(&session, multipart).await {
        Ok((raw, name)) => {
            state
                .variants
                .image(&session, &session.csrf, product, id, Some(raw), &name)
                .await
        }
        Err(IdentityError::InvalidBranding) => Err(ProductError::Image.into()),
        Err(error) => Err(error.into()),
    };
    image_result(&state, &session, product, id, result).await
}
async fn remove_image(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((product, id)): Path<(i64, i64)>,
    Form(f): Form<Action>,
) -> Result<Response> {
    let result = state
        .variants
        .image(&session, &f.csrf, product, id, None, "")
        .await;
    image_result(&state, &session, product, id, result).await
}

impl From<talocart_platform::technical::TechnicalError> for ErrorResponse {
    fn from(error: talocart_platform::technical::TechnicalError) -> Self {
        Self(VariantError::Technical(error))
    }
}

impl From<talocart_platform::technical::AccessError> for ErrorResponse {
    fn from(error: talocart_platform::technical::AccessError) -> Self {
        Self(VariantError::Access(error))
    }
}
