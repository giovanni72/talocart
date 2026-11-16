use super::*;
mod bulk;
mod columns;
mod images;
use images::{change_gallery, remove_image, upload, upload_gallery};
// Two 4,000-character descriptions plus metadata, including UTF-8 form encoding.
const FORM_BODY_LIMIT: usize = 128 * 1024;
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/catalog/products", get(index))
        .route("/admin/catalog/products/columns", post(columns::save))
        .route(
            "/admin/catalog/products/bulk",
            post(bulk::preview).layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .route(
            "/admin/catalog/products/bulk/apply",
            post(bulk::apply).layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .route(
            "/admin/catalog/products/new",
            get(new_page)
                .post(create)
                .layer(DefaultBodyLimit::max(FORM_BODY_LIMIT)),
        )
        .route(
            "/admin/catalog/products/{id}",
            get(edit_page)
                .post(update)
                .layer(DefaultBodyLimit::max(FORM_BODY_LIMIT)),
        )
        .route(
            "/admin/catalog/products/{id}/related",
            get(super::related::index)
                .post(super::related::change)
                .layer(DefaultBodyLimit::max(4096)),
        )
        .route("/admin/catalog/products/{id}/delete", post(delete))
        .route(
            "/admin/catalog/products/{id}/image",
            post(upload).layer(DefaultBodyLimit::max(
                talocart_platform::identity::BRANDING_UPLOAD_LIMIT + 16 * 1024,
            )),
        )
        .route(
            "/admin/catalog/products/{id}/gallery",
            post(upload_gallery).layer(DefaultBodyLimit::max(
                talocart_platform::identity::BRANDING_UPLOAD_LIMIT + 16 * 1024,
            )),
        )
        .route(
            "/admin/catalog/products/{id}/gallery/{image}",
            post(change_gallery),
        )
        .route(
            "/admin/catalog/products/{id}/image/remove",
            post(remove_image),
        )
}
struct Row {
    entry: Entry,
    description_text: String,
    image: String,
    preview: String,
    modified_date: String,
    modified_time: String,
    price: String,
}
struct SortHeader {
    key: &'static str,
    label: &'static str,
    href: String,
    sort: crate::table_sort::SortState,
    numeric: bool,
    centered: bool,
    hint: &'static str,
}
#[derive(Template)]
#[template(path = "product-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rows: Vec<Row>,
    can_write: bool,
    id_filter: &'a str,
    q: &'a str,
    filter: &'a str,
    kind: &'a str,
    model: &'a str,
    price_filter: &'a str,
    price_op: &'a str,
    stock_filter: &'a str,
    stock_op: &'a str,
    modified_date: &'a str,
    categories: Vec<Choice>,
    brands: Vec<Choice>,
    filtered: bool,
    headers: Vec<SortHeader>,
    columns: talocart_platform::identity::ProductColumns,
    sort: &'static str,
    direction: &'static str,
    pagination: super::pagination::Pagination,
    page_size: u16,
    page_sizes: [u16; 5],
    category: &'a str,
    brand: &'a str,
    deleted: bool,
    bulk_deleted: bool,
    bulk_status: &'static str,
    bulk_category: bool,
    bulk_applied: bool,
    return_to: String,
}
fn filter_id(value: &str, field: Field) -> Result<Option<i64>> {
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(Some)
        .ok_or_else(|| ProductError::Invalid(vec![field]).into())
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<ListQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let state = state.primary_content().await?;
    let order = AdminOrder::parse(&query.sort, &query.dir)?;
    let page_size = preference::resolve_admin(&query.per_page, &headers)?;
    let id = if query.id.is_empty() {
        None
    } else {
        if query.id.len() > 19 || !query.id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ProductError::Invalid(vec![]).into());
        }
        Some(
            query
                .id
                .parse::<i64>()
                .map_err(|_| ProductError::Invalid(vec![]))?,
        )
    };
    let category = filter_id(&query.category, Field::Category)?;
    let brand = filter_id(&query.brand, Field::Brand)?;
    let result = state
        .products
        .admin_filtered(
            &session,
            &AdminFilters {
                id,
                q: &query.q,
                status: &query.status,
                kind: &query.kind,
                model: &query.model,
                price: &query.price,
                price_op: &query.price_op,
                stock: &query.stock,
                stock_op: &query.stock_op,
                modified_date: &query.modified_date,
                category,
                brand,
                order,
                page_size,
            },
            query.page,
        )
        .await?;
    let (categories, brands) = state.catalog.product_choices(&session, brand).await?;
    for (selected, choices, field) in [
        (category, &categories, Field::Category),
        (brand, &brands, Field::Brand),
    ] {
        if selected.is_some_and(|id| !choices.iter().any(|choice| choice.id == id)) {
            return Err(ProductError::Invalid(vec![field]).into());
        }
    }
    let choices = |entries: Vec<talocart_commerce::Entry>, selected| {
        entries
            .into_iter()
            .map(|entry| Choice {
                id: entry.id,
                name: entry.name,
                active: entry.active,
                selected: Some(entry.id) == selected,
            })
            .collect()
    };
    let categories = choices(categories, category);
    let brands = choices(brands, brand);
    let rows = result
        .entries
        .into_iter()
        .map(|entry| {
            let (date, time) = entry
                .modified_at
                .split_once(' ')
                .unwrap_or((&entry.modified_at, ""));
            Row {
                description_text: talocart_platform::rich_text::plain(&entry.description),
                image: entry.image_url("thumbnail"),
                preview: entry.image_url("detail"),
                modified_date: date.to_owned(),
                modified_time: time.to_owned(),
                price: entry.values().price,
                entry,
            }
        })
        .collect();
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let headers = [
        (AdminSort::Id, "admin-product-id"),
        (AdminSort::Image, "admin-image-2"),
        (AdminSort::Type, "admin-type"),
        (AdminSort::Name, "admin-name"),
        (AdminSort::Model, "admin-model"),
        (AdminSort::Category, "admin-resource-category"),
        (AdminSort::Brand, "admin-brand-2"),
        (AdminSort::Price, "admin-price"),
        (AdminSort::Variants, "admin-variants-2"),
        (AdminSort::Stock, "admin-stock"),
        (AdminSort::Status, "admin-status"),
        (AdminSort::ModifiedAt, "admin-product-last-modified"),
    ]
    .into_iter()
    .map(|(column, label)| {
        let sort = crate::table_sort::SortState::new(
            column == order.column,
            order.direction == SortDirection::Desc,
            i18n::text(label),
        );
        let direction = if sort.next_descending {
            SortDirection::Desc
        } else {
            SortDirection::Asc
        };
        SortHeader {
            key: column.key(),
            label,
            href: query.admin_link(AdminOrder { column, direction }, page_size),
            sort,
            numeric: matches!(column, AdminSort::Price | AdminSort::Stock),
            centered: matches!(
                column,
                AdminSort::Id | AdminSort::Variants | AdminSort::Status
            ),
            hint: match column {
                AdminSort::Image => "admin-sort-by-main-image-presence",
                AdminSort::Price => "admin-product-price-or-minimum-active-variant-price",
                AdminSort::Stock => "admin-product-stock-or-total-active-variant-stock",
                AdminSort::Variants => "admin-total-number-of-variants-including-inactive-ones",
                _ => "admin-click-to-change-sorting",
            },
        }
    })
    .collect();
    let link = |page| format!("{}&page={page}", query.admin_link(order, page_size));
    let response = render(ListPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "products",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rows,
        can_write: session.allows(Permission::ProductsWrite),
        id_filter: &query.id,
        q: &query.q,
        filter: &query.status,
        kind: &query.kind,
        model: &query.model,
        price_filter: &query.price,
        price_op: &query.price_op,
        stock_filter: &query.stock,
        stock_op: &query.stock_op,
        modified_date: &query.modified_date,
        categories,
        brands,
        headers,
        columns: state.identity.product_columns(&session).await?,
        sort: order.column.key(),
        direction: order.direction.key(),
        filtered: [
            &query.id,
            &query.q,
            &query.status,
            &query.kind,
            &query.model,
            &query.category,
            &query.brand,
            &query.price,
            &query.stock,
            &query.modified_date,
        ]
        .iter()
        .any(|s| !s.is_empty()),
        pagination: super::pagination::Pagination::products(
            result.total,
            result.page,
            page_size.value(),
            &i18n::text("admin-product-pages"),
            link,
        ),
        page_size: page_size.value(),
        page_sizes: PRODUCT_PAGE_SIZES,
        category: &query.category,
        brand: &query.brand,
        deleted: query.deleted.as_deref() == Some("1"),
        bulk_deleted: query.bulk_deleted.as_deref() == Some("1"),
        bulk_category: query.bulk_category.as_deref() == Some("1"),
        bulk_applied: query.bulk_applied.as_deref() == Some("1"),
        bulk_status: match query.bulk_status.as_deref() {
            Some("published") => "published",
            Some("draft") => "draft",
            _ => "",
        },
        return_to: link(result.page),
    });
    preference::remember_admin(response, &query.per_page, page_size, state.secure_cookies)
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum ProductTab {
    #[default]
    General,
    Commerce,
    Organization,
    Image,
    Variants,
    Related,
    Attributes,
    Delete,
}
impl ProductTab {
    fn parse(key: &str) -> Self {
        match key {
            "commerce" => Self::Commerce,
            "organization" => Self::Organization,
            "image" => Self::Image,
            "variants" => Self::Variants,
            "related" => Self::Related,
            "attributes" => Self::Attributes,
            "delete" => Self::Delete,
            _ => Self::General,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Commerce => "commerce",
            Self::Organization => "organization",
            Self::Image => "image",
            Self::Variants => "variants",
            Self::Related => "related",
            Self::Attributes => "attributes",
            Self::Delete => "delete",
        }
    }
    fn error_tab(error: Option<&ProductError>, fallback: Self) -> Self {
        if matches!(error, Some(ProductError::Content(_))) {
            return Self::General;
        }
        if [
            Field::Name,
            Field::Code,
            Field::Slug,
            Field::Description,
            Field::SeoTitle,
            Field::SeoDescription,
        ]
        .into_iter()
        .any(|f| field(error, f))
        {
            Self::General
        } else if [
            Field::Price,
            Field::Stock,
            Field::MinimumQuantity,
            Field::Packs,
            Field::TaxClass,
        ]
        .into_iter()
        .any(|f| field(error, f))
            || matches!(
                error,
                Some(ProductError::ModeChanged | ProductError::StockChanged)
            )
        {
            Self::Commerce
        } else if [Field::Category, Field::Brand, Field::Status]
            .into_iter()
            .any(|f| field(error, f))
        {
            Self::Organization
        } else {
            fallback
        }
    }
}
#[derive(Default)]
struct FormFeedback<'a> {
    related_query: &'a str,
    related_saved: bool,
    submitted: Option<&'a content::Submission>,
    error: Option<&'a ProductError>,
    variant_error: Option<&'a talocart_commerce::variants::VariantError>,
    saved: bool,
    tab: ProductTab,
}
struct Choice {
    id: i64,
    name: String,
    active: bool,
    selected: bool,
}
#[derive(Template)]
#[template(path = "product-form.html")]
struct FormPage<'a> {
    related: Option<talocart_commerce::products::RelatedEditor>,
    related_query: &'a str,
    related_saved: bool,
    currency: String,
    content: content::Editor,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    id: i64,
    values: &'a Values,
    categories: Vec<Choice>,
    brands: Vec<Choice>,
    classes: Vec<Choice>,
    mode: PriceMode,
    image: String,
    gallery: Vec<GalleryImage>,
    gallery_full: bool,
    attributes: Vec<talocart_commerce::attributes::ProductValue>,
    quote: Option<LineTotals>,
    variant_commerce: Option<VariantCommerceView>,
    visible: bool,
    can_write: bool,
    axes: Vec<super::variants::AxisView>,
    option_slots: Vec<OptionSlot>,
    variant_count: i64,
    error: String,
    saved: bool,
    active_tab: ProductTab,
    name_error: bool,
    model_error: bool,
    slug_error: bool,
    description_error: bool,
    seo_title_error: bool,
    seo_description_error: bool,
    price_error: bool,
    stock_error: bool,
    minimum_quantity_error: bool,
    packs_error: bool,
    status_error: bool,
    category_error: bool,
    brand_error: bool,
    tax_error: bool,
}
fn field(error: Option<&ProductError>, field: Field) -> bool {
    match error {
        Some(ProductError::Invalid(fields)) => fields.contains(&field),
        Some(ProductError::DuplicateModel) => field == Field::Code,
        Some(ProductError::DuplicateSlug) => field == Field::Slug,
        Some(ProductError::Publication) => {
            matches!(field, Field::Category | Field::Brand | Field::TaxClass)
        }
        _ => false,
    }
}
struct VariantCommerceView {
    price: String,
    stock: i64,
}
impl VariantCommerceView {
    fn new(summary: talocart_commerce::products::VariantCommerceSummary, currency: &str) -> Self {
        let amount = |cents: i64| i18n::decimal(format!("{},{:02}", cents / 100, cents % 100));
        let price = match (summary.minimum_price, summary.maximum_price) {
            (Some(min), Some(max)) if min != max => format!(
                "{} – {} {}",
                amount(min),
                amount(max),
                talocart_platform::geography::currency_label(currency)
            ),
            (Some(price), _) => format!(
                "{} {}",
                amount(price),
                talocart_platform::geography::currency_label(currency)
            ),
            _ => i18n::text("admin-no-active-variants"),
        };
        Self {
            price,
            stock: summary.stock,
        }
    }
}
struct OptionSlot {
    field: String,
    label: String,
    choices: Vec<super::variants::Choice>,
}
pub(crate) async fn configuration_result(
    state: &AppState,
    session: &Session,
    id: i64,
    result: talocart_commerce::variants::Result<()>,
) -> Result<Response> {
    match result {
        Ok(()) => Ok(Redirect::to(&format!(
            "/admin/catalog/products/{id}?saved=1&tab=variants"
        ))
        .into_response()),
        Err(talocart_commerce::variants::VariantError::Technical(error)) => Err(error.into()),
        Err(talocart_commerce::variants::VariantError::Access(error)) => Err(error.into()),
        Err(talocart_commerce::variants::VariantError::Identity(error)) => Err(error.into()),
        Err(talocart_commerce::variants::VariantError::NotFound) => {
            Err(ProductError::NotFound.into())
        }
        Err(error) => {
            let entry = state.products.entry(session, id).await?;
            form_response(
                state,
                session,
                id,
                &entry.values(),
                None,
                FormFeedback {
                    variant_error: Some(&error),
                    tab: ProductTab::Variants,
                    ..Default::default()
                },
            )
            .await
        }
    }
}
async fn form_response(
    state: &AppState,
    session: &Session,
    id: i64,
    values: &Values,
    loaded_entry: Option<talocart_commerce::products::EditorEntry>,
    feedback: FormFeedback<'_>,
) -> Result<Response> {
    let primary_state = state.primary_content().await?;
    let state = &primary_state;
    let FormFeedback {
        related_query,
        related_saved,
        error,
        variant_error,
        submitted,
        saved,
        tab,
    } = feedback;
    let editor = if let Some(entry) = loaded_entry {
        Some(entry)
    } else if id > 0 {
        Some(state.products.editor_entry(session, id).await?)
    } else {
        None
    };
    let variant_count = editor
        .as_ref()
        .map_or(0, |entry| entry.product.variant_count);
    let configuration_axes = state.variants.configuration_axes(session, id).await?;
    let options = state.variants.options(session).await?;
    let option_slots = (0..3)
        .map(|i| OptionSlot {
            field: format!("option_{}", i + 1),
            label: i18n::count("admin-option-number", i as i64 + 1),
            choices: options
                .iter()
                .map(|o| super::variants::Choice {
                    id: o.id,
                    name: o.name.clone(),
                    selected: configuration_axes.get(i).is_some_and(|a| a.id == o.id),
                })
                .collect(),
        })
        .collect();
    let axes = super::variants::axis_views(configuration_axes, &[]);
    let (categories, brands) = state
        .catalog
        .product_choices(session, values.brand_id.parse().ok())
        .await?;
    let (settings, classes) = state.taxes.product_defaults(session).await?;
    let (entry, attributes, variant_commerce) = match editor {
        Some(editor) => (
            Some(editor.product),
            editor.attributes,
            editor
                .variant_commerce
                .map(|summary| VariantCommerceView::new(summary, &settings.currency)),
        ),
        None => (None, vec![], None),
    };
    let image = entry
        .as_ref()
        .map(|entry| entry.image_url("card"))
        .unwrap_or_default();
    let gallery = if id > 0 {
        state.products.gallery(session, id).await?
    } else {
        vec![]
    };
    let gallery_full = gallery.len() >= MAX_GALLERY_IMAGES;
    let quote = match &entry {
        Some(product) if product.basis_points.is_some() => Some(product.quote()?),
        _ => None,
    };
    let visible = match &entry {
        Some(product) => state.products.public_visible(&product.slug).await?,
        None => false,
    };
    let categories = categories
        .into_iter()
        .map(|entry| Choice {
            id: entry.id,
            name: entry.name,
            active: entry.active,
            selected: values.category_id == entry.id.to_string(),
        })
        .collect();
    let brands = brands
        .into_iter()
        .map(|entry| Choice {
            id: entry.id,
            name: entry.name,
            active: entry.active,
            selected: values.brand_id == entry.id.to_string(),
        })
        .collect();
    let classes = classes
        .into_iter()
        .map(|entry| Choice {
            id: entry.id,
            selected: values.tax_class_id == entry.id.to_string(),
            active: entry.active && entry.rate_active,
            name: format!("{} · {}%", entry.name, i18n::decimal(entry.percentage())),
        })
        .collect();
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let content = content::Editor::load(
        state,
        session,
        talocart_commerce::content::Resource::Product,
        id,
        submitted,
        error.and_then(|source_error| {
            if let ProductError::Content(source_error) = source_error {
                Some(source_error)
            } else {
                None
            }
        }),
        "product-content",
    )
    .await?;
    let mut related = if id > 0 {
        Some(
            state
                .products
                .related_editor(session, id, related_query)
                .await?,
        )
    } else {
        None
    };
    if let (Some(editor), Some(mode)) = (&mut related, &values.related_fallback) {
        editor.fallback.clone_from(mode);
    }
    let mut response = render(FormPage {
        related,
        related_query,
        related_saved,
        content,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "products",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        id,
        values,
        categories,
        brands,
        classes,
        mode: settings.mode,
        currency: settings.currency.clone(),
        image,
        gallery,
        gallery_full,
        attributes,
        quote,
        variant_commerce,
        visible,
        can_write: session.allows(Permission::ProductsWrite),
        axes,
        option_slots,
        variant_count,
        error: error
            .map(i18n::product_error)
            .or_else(|| variant_error.map(i18n::variant_error))
            .unwrap_or_default(),
        saved,
        active_tab: ProductTab::error_tab(error, tab),
        name_error: field(error, Field::Name),
        model_error: field(error, Field::Code),
        slug_error: field(error, Field::Slug),
        description_error: field(error, Field::Description),
        seo_title_error: field(error, Field::SeoTitle),
        seo_description_error: field(error, Field::SeoDescription),
        price_error: field(error, Field::Price),
        stock_error: field(error, Field::Stock),
        minimum_quantity_error: field(error, Field::MinimumQuantity),
        packs_error: field(error, Field::Packs),
        status_error: field(error, Field::Status),
        category_error: field(error, Field::Category) && !values.category_id.is_empty(),
        brand_error: field(error, Field::Brand),
        tax_error: field(error, Field::TaxClass),
    });
    if let Some(source_error) = error {
        *response.status_mut() = status(source_error);
    }
    if let Some(source_error) = variant_error {
        *response.status_mut() = super::variants::status(source_error);
    }
    Ok(response)
}
#[derive(Deserialize, Default)]
struct SavedQuery {
    #[serde(default)]
    related_q: String,
    related_saved: Option<String>,
    saved: Option<String>,
    #[serde(default)]
    tab: String,
}
async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    let (settings, _) = state.taxes.product_defaults(&session).await?;
    let values = Values {
        price_mode: settings.mode.key().into(),
        currency: settings.currency,
        tax_class_id: settings
            .default_class_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        ..Values::default()
    };
    form_response(&state, &session, 0, &values, None, FormFeedback::default()).await
}
async fn edit_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(query): Query<SavedQuery>,
) -> Result<Response> {
    let state = state.primary_content().await?;
    let entry = state.products.editor_entry(&session, id).await?;
    let values = entry.product.values();
    form_response(
        &state,
        &session,
        id,
        &values,
        Some(entry),
        FormFeedback {
            saved: query.saved.as_deref() == Some("1"),
            tab: ProductTab::parse(&query.tab),
            related_query: &query.related_q,
            related_saved: query.related_saved.as_deref() == Some("1"),
            ..Default::default()
        },
    )
    .await
}
#[derive(Deserialize)]
struct ProductForm {
    #[serde(flatten)]
    content: content::Submission,
    csrf: String,
    #[serde(default)]
    editor_tab: String,
    name: String,
    model: String,
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    description_format: String,
    #[serde(default)]
    seo_title: String,
    #[serde(default)]
    seo_description: String,
    #[serde(default)]
    price: String,
    price_mode: String,
    #[serde(default = "euro_currency")]
    currency: String,
    #[serde(default)]
    stock: String,
    #[serde(default)]
    manage_stock: bool,
    #[serde(default = "missing_stock_revision")]
    stock_revision: i64,
    #[serde(default = "default_quantity")]
    minimum_quantity: String,
    #[serde(default = "default_quantity")]
    packs: String,
    status: String,
    #[serde(default)]
    category_id: String,
    #[serde(default)]
    brand_id: String,
    #[serde(default)]
    tax_class_id: String,
    #[serde(default)]
    related_fallback: Option<String>,
}
pub(crate) fn missing_stock_revision() -> i64 {
    -1
}
fn default_quantity() -> String {
    "1".into()
}
async fn save_response(
    state: AppState,
    session: Session,
    id: Option<i64>,
    form: ProductForm,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let tab = ProductTab::parse(&form.editor_tab);
    let values = Values {
        name: form.name,
        model: form.model,
        slug: form.slug,
        description: talocart_platform::rich_text::submitted(
            &form.description,
            &form.description_format,
        ),
        seo_title: form.seo_title,
        seo_description: form.seo_description,
        price: form.price,
        price_mode: form.price_mode,
        currency: form.currency,
        manage_stock: form.manage_stock,
        stock: form.stock,
        stock_revision: form.stock_revision,
        minimum_quantity: form.minimum_quantity,
        packs: form.packs,
        status: form.status,
        category_id: form.category_id,
        brand_id: form.brand_id,
        tax_class_id: form.tax_class_id,
        related_fallback: form.related_fallback,
    };
    match state
        .products
        .save_with_content(
            &session,
            &form.csrf,
            id,
            &values,
            content::draft(&form.content).as_ref(),
        )
        .await
    {
        Ok(id) => Ok(Redirect::to(&format!(
            "/admin/catalog/products/{id}?saved=1&tab={}",
            tab.key()
        ))
        .into_response()),
        Err(ProductError::Technical(source_error)) => Err(source_error.into()),
        Err(ProductError::Access(source_error)) => Err(source_error.into()),
        Err(ProductError::Identity(source_error)) => Err(source_error.into()),
        Err(ProductError::NotFound) => Err(ProductError::NotFound.into()),
        Err(source_error) => {
            form_response(
                &state,
                &session,
                id.unwrap_or(0),
                &values,
                None,
                FormFeedback {
                    submitted: Some(&form.content),
                    error: Some(&source_error),
                    tab,
                    ..Default::default()
                },
            )
            .await
        }
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<ProductForm>,
) -> Result<Response> {
    save_response(state, session, None, form).await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<ProductForm>,
) -> Result<Response> {
    save_response(state, session, Some(id), form).await
}
#[derive(Deserialize)]
struct ActionForm {
    csrf: String,
    #[serde(default)]
    confirm: bool,
}
async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<ActionForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let entry = state.products.entry(&session, id).await?;
    let result = if form.confirm {
        state.products.delete(&session, &form.csrf, id).await
    } else {
        Err(ProductError::Confirmation)
    };
    match result {
        Ok(()) => Ok(Redirect::to("/admin/catalog/products?deleted=1").into_response()),
        Err(ProductError::Technical(source_error)) => Err(source_error.into()),
        Err(ProductError::Access(source_error)) => Err(source_error.into()),
        Err(ProductError::Identity(source_error)) => Err(source_error.into()),
        Err(source_error) => {
            form_response(
                &state,
                &session,
                id,
                &entry.values(),
                None,
                FormFeedback {
                    error: Some(&source_error),
                    tab: ProductTab::Delete,
                    ..Default::default()
                },
            )
            .await
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_tab_is_whitelisted_and_validation_opens_the_affected_section() {
        for key in [
            "general",
            "commerce",
            "organization",
            "image",
            "variants",
            "related",
            "attributes",
            "delete",
        ] {
            assert_eq!(ProductTab::parse(key).key(), key);
        }
        for key in ["", "unknown", "https://example.test", "<script>"] {
            assert_eq!(ProductTab::parse(key), ProductTab::General);
        }
        for (error, expected) in [
            (
                ProductError::Invalid(vec![Field::Price]),
                ProductTab::Commerce,
            ),
            (
                ProductError::Invalid(vec![Field::Status]),
                ProductTab::Organization,
            ),
            (ProductError::DuplicateModel, ProductTab::General),
            (ProductError::ModeChanged, ProductTab::Commerce),
            (ProductError::Image, ProductTab::Image),
            (ProductError::Confirmation, ProductTab::Delete),
        ] {
            let fallback = match error {
                ProductError::Image => ProductTab::Image,
                ProductError::Confirmation => ProductTab::Delete,
                _ => ProductTab::Variants,
            };
            assert_eq!(ProductTab::error_tab(Some(&error), fallback), expected);
        }
        assert_eq!(
            ProductTab::error_tab(None, ProductTab::Organization),
            ProductTab::Organization
        );
    }
}
