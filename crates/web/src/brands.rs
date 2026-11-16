use super::*;
use axum::extract::Query;
use serde::Deserialize;
use talocart_commerce::PUBLIC_BRAND_PAGE_SIZE;

#[derive(Deserialize)]
pub struct DirectoryQuery {
    #[serde(default = "first_page")]
    page: i64,
}
fn first_page() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "default/templates/brand-directory.html")]
struct Directory<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    canonical: String,
    rows: Vec<navigation::BrandLink>,
    pagination: super::pagination::Pagination,
}
pub async fn index(
    State(state): State<AppState>,
    Query(query): Query<DirectoryQuery>,
) -> Result<Response, catalog::CatalogResponseError> {
    let state = state.localized();
    let result = state.catalog.public_brands_counted(query.page).await?;
    let brands = result.entries;
    let name = current_name(&state).await?;
    let theme = crate::store_language::settings(&state).await?;
    let path = if result.page == 1 {
        "/brands".into()
    } else {
        format!("/brands?page={}", result.page)
    };
    Ok(render(Directory {
        store_name: &name,
        logo: current_logo(&state).await?,
        search_query: "",
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        canonical: theme.canonical(&state.public_origin, &store_language::path(&path)),
        theme,
        category_menu: navigation::CategoryMenu::load(&state, "", false, "", true).await?,
        rows: brands
            .into_iter()
            .map(|brand| navigation::BrandLink::new(brand, ""))
            .collect(),
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            PUBLIC_BRAND_PAGE_SIZE as u16,
            &i18n::text("admin-brands-2"),
            &i18n::text("admin-brand-pages"),
            |page| store_language::path(format!("/brands?page={page}")),
        ),
    }))
}
