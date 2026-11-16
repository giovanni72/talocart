use super::*;
struct StoreList<'a> {
    search_query: &'a str,
    search_action: String,
    search_filters: navigation::SearchFilters,
    filter_action: String,
    theme: talocart_platform::storefront::Settings,
    meta_title: String,
    meta_description: String,
    canonical: String,
    brands: Vec<(String, String)>,
    categories: Vec<(String, String)>,
    category_present: bool,
    fixed_brand: bool,
    show_category_filter: bool,
    has_filters: bool,
    brand: &'a str,
    brand_present: bool,
    filter_title: String,
    available: bool,
    category_menu: navigation::CategoryMenu,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    rows: Vec<super::product_card::ProductCard>,
    q: &'a str,
    category: &'a str,
    pagination: super::pagination::Pagination,
    per_page: u16,
    page_sizes: [u16; 4],
    reset: String,
    reset_label: String,
}
#[derive(Template)]
#[template(path = "default/templates/product-store.html")]
struct CatalogPage<'a> {
    list: StoreList<'a>,
}
#[derive(Template)]
#[template(path = "default/templates/category-store.html")]
struct CategoryPage<'a> {
    list: StoreList<'a>,
    category_info: talocart_commerce::PublicCategory,
}
#[derive(Template)]
#[template(path = "default/templates/brand-store.html")]
struct BrandPage<'a> {
    list: StoreList<'a>,
    brand_info: talocart_commerce::PublicBrand,
    brand_logo: Option<talocart_platform::media::ImageMetadata>,
    brand_logo_url: String,
}
#[derive(Template)]
#[template(path = "default/templates/search-store.html")]
struct SearchPage<'a> {
    list: StoreList<'a>,
}
pub(super) enum ListingKind {
    Catalog,
    Category(talocart_commerce::PublicCategory),
    Brand(talocart_commerce::PublicBrand),
    Search,
}
pub async fn public_index(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let state = state.localized();
    if !query.q.is_empty() || !query.category.is_empty() {
        return Err(ProductError::Invalid(vec![]).into());
    }
    listing_response(&state, &headers, &query, ListingKind::Catalog).await
}
pub async fn public_category_index(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Query(mut query): Query<ListQuery>,
) -> Result<Response> {
    let state = state.localized();
    if !query.q.is_empty() {
        return Err(ProductError::Invalid(vec![]).into());
    }
    let category = state.catalog.public_category(&slug).await?;
    query.category = slug;
    listing_response(&state, &headers, &query, ListingKind::Category(category)).await
}
pub async fn public_brand_index(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Query(mut query): Query<ListQuery>,
) -> Result<Response> {
    let state = state.localized();
    if !query.q.is_empty() {
        return Err(ProductError::Invalid(vec![]).into());
    }
    let brand = state.catalog.public_brand(&slug).await?;
    query.brand = slug;
    listing_response(&state, &headers, &query, ListingKind::Brand(brand)).await
}
pub(super) async fn listing_response(
    state: &AppState,
    headers: &HeaderMap,
    query: &ListQuery,
    kind: ListingKind,
) -> Result<Response> {
    let localized_state = state.localized();
    let state = &localized_state;
    let base_path = kind.path(query);
    let fixed_brand = matches!(kind, ListingKind::Brand(_));
    let is_search = matches!(kind, ListingKind::Search);
    let page_size = preference::resolve(&query.per_page, headers)?;
    let result = state
        .products
        .public_page(
            &query.q,
            &query.category,
            &query.brand,
            query.available,
            query.page,
            page_size,
        )
        .await?;
    let rows = result
        .entries
        .into_iter()
        .map(super::product_card::ProductCard::try_from)
        .collect::<talocart_commerce::products::Result<Vec<_>>>()?;
    let name = current_name(state).await?;
    let link = |page| kind.url(query, page_size, Some(page));
    let theme = crate::store_language::settings(state).await?;
    let canonical = theme.canonical(&state.public_origin, &base_path);
    let brands = if fixed_brand {
        vec![]
    } else if matches!(kind, ListingKind::Category(_)) {
        state
            .products
            .public_category_brands(&query.category, &query.brand)
            .await?
    } else {
        state.products.public_brands(&query.brand).await?
    };
    let categories = if fixed_brand {
        state.products.public_brand_categories(&query.brand).await?
    } else if is_search {
        state.products.public_search_categories().await?
    } else {
        vec![]
    };
    let mut list = StoreList {
        search_query: &query.q,
        search_action: "/search".into(),
        search_filters: navigation::SearchFilters {
            category: query.category.clone(),
            brand: query.brand.clone(),
            available: query.available,
            per_page: page_size.value(),
        },
        filter_action: base_path.clone(),
        category_present: categories
            .iter()
            .any(|category| category.1 == query.category),
        categories,
        fixed_brand,
        show_category_filter: fixed_brand || is_search,
        has_filters: query.available
            || (!fixed_brand && !query.brand.is_empty())
            || ((fixed_brand || is_search) && !query.category.is_empty()),
        meta_title: format!("{} · {}", i18n::text("store-catalog"), name),
        meta_description: theme.description(),
        canonical,
        theme,
        brand_present: brands.iter().any(|b| b.1 == query.brand),
        brands,
        filter_title: i18n::text("admin-catalogue-filters"),
        brand: &query.brand,
        available: query.available,
        category_menu: navigation::CategoryMenu::load(
            state,
            &query.category,
            matches!(kind, ListingKind::Catalog),
            if fixed_brand { &query.brand } else { "" },
            fixed_brand,
        )
        .await?,
        store_name: &name,
        logo: current_logo(state).await?,
        rows,
        q: &query.q,
        category: &query.category,
        pagination: super::pagination::Pagination::products(
            result.total,
            result.page,
            page_size.value(),
            &i18n::text("admin-catalogue-pages"),
            link,
        ),
        per_page: page_size.value(),
        page_sizes: CATALOG_PAGE_SIZES,
        reset: if page_size == CatalogPageSize::default() {
            base_path.clone()
        } else {
            format!("{base_path}?per_page={}", page_size.value())
        },
        reset_label: i18n::text("admin-show-all-products"),
    };
    let response = match kind {
        ListingKind::Brand(brand_info) => {
            list.meta_title = format!("{} · {}", brand_info.name, name);
            if !brand_info.description.is_empty() {
                list.meta_description.clone_from(&brand_info.description);
            }
            list.reset = base_path;
            if page_size != CatalogPageSize::default() {
                list.reset
                    .push_str(&format!("?per_page={}", page_size.value()));
            }
            list.reset_label = i18n::text("admin-remove-filters");
            list.filter_title = i18n::text("admin-brand-filters");
            let brand_logo = if let Some(key) = &brand_info.logo_key {
                state.catalog.logo_metadata(key).await?
            } else {
                None
            };
            let brand_logo_url = brand_logo
                .as_ref()
                .map(|image| {
                    let key = image
                        .key
                        .split('/')
                        .map(catalog::encoded)
                        .collect::<Vec<_>>()
                        .join("/");
                    format!("/media/public/{}/{key}/card", image.id)
                })
                .unwrap_or_default();
            render(BrandPage {
                list,
                brand_info,
                brand_logo,
                brand_logo_url,
            })
        }
        ListingKind::Category(category_info) => {
            list.meta_title = format!("{} · {}", category_info.name, name);
            if !category_info.description.is_empty() {
                list.meta_description.clone_from(&category_info.description);
            }
            list.reset_label = i18n::text("admin-remove-filters");
            list.filter_title = i18n::text("admin-category-filters");
            render(CategoryPage {
                list,
                category_info,
            })
        }
        ListingKind::Search => {
            list.meta_title = format!("{}: {} · {}", i18n::text("store-search"), query.q, name);
            list.meta_description =
                format!("{} {}.", i18n::text("store-search-results-meta"), query.q);
            list.filter_title = i18n::text("admin-search-filters");
            list.reset_label = i18n::text("admin-remove-filters");
            render(SearchPage { list })
        }
        ListingKind::Catalog => render(CatalogPage { list }),
    };
    preference::remember(response, &query.per_page, page_size, state.secure_cookies)
}

struct SpecificationGroup {
    id: i64,
    name: String,
    rows: Vec<talocart_commerce::attributes::ProductValue>,
}
fn specification_groups(
    rows: Vec<talocart_commerce::attributes::ProductValue>,
) -> Vec<SpecificationGroup> {
    let mut groups: Vec<SpecificationGroup> = vec![];
    for row in rows {
        if let Some(group) = groups.last_mut().filter(|group| group.id == row.group_id) {
            group.rows.push(row);
        } else {
            groups.push(SpecificationGroup {
                id: row.group_id,
                name: row.group_name.clone(),
                rows: vec![row],
            });
        }
    }
    groups
}
#[derive(Template)]
#[template(path = "default/templates/product-detail.html")]
struct StoreDetail<'a> {
    related: Vec<super::product_card::ProductCard>,
    specifications: Vec<SpecificationGroup>,
    variant_id: i64,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    theme: talocart_platform::storefront::Settings,
    meta_title: String,
    meta_description: String,
    canonical: String,
    category_menu: navigation::CategoryMenu,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    entry: Entry,
    brand_href: String,
    brand_logo: String,
    brand_initial: String,
    description_html: String,
    image: String,
    image_srcset: String,
    photos: Vec<PhotoView>,
    totals: LineTotals,
    percentage: String,
    axes: Vec<super::variants::AxisView>,
    selected: bool,
    selected_sku: String,
    selection_message: String,
    first_quantity: i32,
    quantity: i32,
    price_action: String,
    quantity_pricing: bool,
}
#[derive(Deserialize, Default)]
pub struct SelectionQuery {
    #[serde(default)]
    quantity: String,
    #[serde(default)]
    value_1: String,
    #[serde(default)]
    value_2: String,
    #[serde(default)]
    value_3: String,
    #[serde(default)]
    photo: String,
}
pub async fn public_detail(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(query): Query<SelectionQuery>,
    method: axum::http::Method,
    customer: crate::customers::OptionalCustomer,
    cart_context: crate::cart::CartContext,
) -> Result<Response> {
    let state = state.localized();
    let mut entry = state.products.public_entry(&slug).await?;
    let mut image = entry.image_url("detail");
    let mut dimensions = (entry.image_width, entry.image_height);
    let photo = query.photo;
    let raw = [query.value_1, query.value_2, query.value_3];
    let query_values = raw.clone();
    let any = raw.iter().any(|r| !r.is_empty());
    let mut chosen = vec![];
    if any {
        for r in &raw {
            chosen.push(if r.is_empty() {
                0
            } else {
                r.parse::<i64>()
                    .map_err(|_| ProductError::Invalid(vec![]))?
            });
        }
    }
    let (axes, selection) = state.variants.public_selection(entry.id, &chosen).await?;
    let selected = selection.is_some();
    let selected_sku = selection
        .as_ref()
        .map(|v| v.sku.clone())
        .unwrap_or_default();
    let variant_id = selection.as_ref().map_or(0, |v| v.id);
    if let Some(v) = selection {
        entry.price_cents = v.price_cents;
        entry.stock = v.stock;
        let variant_image = v.image_url("detail");
        if !variant_image.is_empty() {
            dimensions = (v.image_width, v.image_height);
            image = variant_image;
        }
    }
    let gallery = state.products.public_gallery(entry.id).await?;
    let (chosen_image, image_srcset, photos) = photo_views(
        &entry,
        &image,
        dimensions,
        gallery,
        &photo,
        &query_values,
        &query.quantity,
    )?;
    image = chosen_image;
    let first_quantity = entry
        .purchase_quantities()
        .map_err(|_| talocart_platform::identity::IdentityError::Unavailable)?
        .first_quantity();
    let quantity = if query.quantity.is_empty() {
        first_quantity
    } else {
        query
            .quantity
            .parse::<i32>()
            .ok()
            .filter(|n| (1..=999999).contains(n))
            .ok_or(ProductError::Invalid(vec![]))?
    };
    if method == axum::http::Method::GET {
        super::cart::establish(&state, &cart_context, customer.0.as_ref()).await?;
    }
    let mut quantity_pricing = false;
    if (!entry.has_variants || selected)
        && let Ok(cart) = cart_context.guest()
        && let Some(unit) = talocart_commerce::cart::Carts::new(state.pool.clone())
            .preview_product_unit(
                &cart,
                (entry.id, variant_id),
                quantity,
                store_language::content_language(),
            )
            .await?
    {
        entry.price_cents = unit.cents();
        quantity_pricing = true;
    }
    let price_action = store_language::path(format!(
        "/products/{}?value_1={}&value_2={}&value_3={}&photo={}",
        entry.slug,
        catalog::encoded(&query_values[0]),
        catalog::encoded(&query_values[1]),
        catalog::encoded(&query_values[2]),
        catalog::encoded(&photo)
    ));
    let totals = entry.quote()?;
    let selection_message = if !entry.has_variants || selected {
        String::new()
    } else if chosen.iter().filter(|n| **n > 0).count() < axes.len() {
        i18n::text("store-choose-all-options")
    } else {
        i18n::text("store-combination-unavailable")
    };
    let axes = super::variants::axis_views(axes, &chosen);
    let percentage = entry
        .basis_points
        .map(|n| format!("{},{:02}", n / 100, n % 100))
        .unwrap_or_default();
    let name = current_name(&state).await?;
    let categories = state.catalog.public_navigation().await?;
    let selected_category = categories
        .iter()
        .find(|category| Some(category.id) == entry.category_id)
        .map(|category| category.slug.clone())
        .unwrap_or_default();
    let theme = crate::store_language::settings(&state).await?;
    let canonical = theme.canonical(
        &state.public_origin,
        &crate::store_language::path(format!("/products/{}", entry.slug)),
    );
    let meta_title = if entry.seo_title.is_empty() {
        format!("{} · {}", entry.name, name)
    } else {
        entry.seo_title.clone()
    };
    let meta_description = if entry.seo_description.is_empty() {
        talocart_platform::rich_text::plain(&entry.description)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(320)
            .collect()
    } else {
        entry.seo_description.clone()
    };
    let category_menu = navigation::CategoryMenu::new(categories, &selected_category, false)
        .load_store_links(&state, "", false)
        .await?;
    let logo = current_logo(&state).await?;
    let brand_href = entry
        .brand_slug
        .as_deref()
        .map(|slug| store_language::path(format!("/brands/{}", catalog::encoded(slug))))
        .unwrap_or_default();
    let brand_logo = entry
        .brand_logo_id
        .map(|id| format!("/media/image/{id}/thumbnail"))
        .unwrap_or_default();
    let brand_initial = entry
        .brand_name
        .as_deref()
        .and_then(|name| name.chars().find(|c| c.is_alphanumeric()))
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let specifications = state
        .attributes()
        .public_values(entry.id)
        .await
        .map_err(|error| match error {
            talocart_commerce::attributes::AttributeError::Technical(source_error) => {
                ProductError::Technical(source_error)
            }
            talocart_commerce::attributes::AttributeError::Access(source_error) => {
                ProductError::Access(source_error)
            }
            talocart_commerce::attributes::AttributeError::Identity(source_error) => {
                ProductError::Identity(source_error)
            }
            _ => ProductError::Identity(talocart_platform::identity::IdentityError::Unavailable),
        })?;
    let related = state
        .products
        .public_related(entry.id)
        .await?
        .into_iter()
        .map(super::product_card::ProductCard::try_from)
        .collect::<talocart_commerce::products::Result<Vec<_>>>()?;
    Ok(render(StoreDetail {
        related,
        specifications: specification_groups(specifications),
        variant_id,
        search_query: "",
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        theme,
        canonical,
        meta_title,
        meta_description,
        category_menu,
        store_name: &name,
        logo,
        description_html: talocart_platform::rich_text::render(&entry.description),
        entry,
        brand_href,
        brand_logo,
        brand_initial,
        image_srcset,
        image,
        photos,
        totals,
        percentage,
        axes,
        selected,
        selected_sku,
        selection_message,
        first_quantity,
        quantity,
        price_action,
        quantity_pricing,
    }))
}

#[derive(Deserialize)]
pub struct PricePreview {
    csrf: String,
    quantity: String,
}
/// POST avoids putting the cart CSRF in a GET URL; this preview never mutates it.
pub async fn public_price(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(mut query): Query<SelectionQuery>,
    customer: crate::customers::OptionalCustomer,
    cart_context: crate::cart::CartContext,
    Form(input): Form<PricePreview>,
) -> Result<Response> {
    let guest = cart_context.guest()?;
    guest.check_csrf(&input.csrf)?;
    query.quantity = input.quantity;
    public_detail(
        State(state),
        Path(slug),
        Query(query),
        axum::http::Method::POST,
        customer,
        cart_context,
    )
    .await
}

struct PhotoView {
    id: String,
    key: String,
    image: String,
    srcset: String,
    thumbnail: String,
    href: String,
    label: String,
    selected: bool,
}
fn photo_views(
    entry: &Entry,
    default: &str,
    dimensions: (Option<i32>, Option<i32>),
    gallery: Vec<GalleryImage>,
    requested: &str,
    values: &[String; 3],
    quantity: &str,
) -> talocart_commerce::products::Result<(String, String, Vec<PhotoView>)> {
    let mut photos = Vec::new();
    let mut add =
        |key: String, image: String, label: String, dimensions: (Option<i32>, Option<i32>)| {
            let image_id = image
                .strip_prefix("/media/image/")
                .and_then(|path| path.split('/').next())
                .and_then(|id| id.parse().ok());
            let thumbnail = format!(
                "{}/thumbnail",
                image.rsplit_once('/').map_or(image.as_str(), |p| p.0)
            );
            let mut href = format!(
                "/products/{}?value_1={}&value_2={}&value_3={}&photo={}",
                entry.slug,
                catalog::encoded(&values[0]),
                catalog::encoded(&values[1]),
                catalog::encoded(&values[2]),
                key,
            );
            if !quantity.is_empty() {
                href.push_str(&format!("&quantity={}", catalog::encoded(quantity)));
            }
            photos.push(PhotoView {
                id: format!(
                    "product-photo-{}",
                    if key.is_empty() { "default" } else { &key }
                ),
                key,
                image,
                srcset: talocart_platform::media::responsive_srcset(
                    image_id,
                    dimensions.0,
                    dimensions.1,
                ),
                thumbnail,
                href: crate::store_language::path(href),
                label,
                selected: false,
            });
        };
    if !default.is_empty() {
        add(
            String::new(),
            default.to_owned(),
            i18n::text("store-selected-photo"),
            dimensions,
        );
    }
    let primary = entry.image_url("detail");
    if !primary.is_empty() && primary != default {
        add(
            "product".into(),
            primary,
            i18n::text("store-main-photo"),
            (entry.image_width, entry.image_height),
        );
    }
    for (index, image) in gallery.into_iter().enumerate() {
        add(
            image.id.to_string(),
            image.image_url("detail"),
            i18n::count("store-additional-photo", (index + 1) as i64),
            (Some(image.image_width), Some(image.image_height)),
        );
    }
    let selected = if requested.is_empty() {
        photos.first_mut()
    } else {
        Some(
            photos
                .iter_mut()
                .find(|photo| photo.key == requested)
                .ok_or(ProductError::NotFound)?,
        )
    };
    let (image, srcset) = selected
        .map(|photo| {
            photo.selected = true;
            (photo.image.clone(), photo.srcset.clone())
        })
        .unwrap_or_default();
    Ok((image, srcset, photos))
}
