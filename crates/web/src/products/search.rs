use super::*;
use talocart_commerce::products::Products;

/// A search term is text, not a slug. Encode one segment and prevent the browser
/// from normalizing a literal dot segment away. Commerce already trims whitespace.
fn search_path(term: &str) -> String {
    let encoded = catalog::encoded(term.trim());
    let suffix = if matches!(encoded.as_str(), "." | "..") {
        "%20"
    } else {
        ""
    };
    crate::store_language::path(format!("/search/{encoded}{suffix}"))
}
impl ListingKind {
    pub(super) fn path(&self, query: &ListQuery) -> String {
        crate::store_language::path(match self {
            Self::Catalog => "/catalog".into(),
            Self::Category(category) => format!("/catalog/{}", catalog::encoded(&category.slug)),
            Self::Brand(brand) => format!("/brands/{}", catalog::encoded(&brand.slug)),
            Self::Search => search_path(&query.q),
        })
    }
    pub(super) fn url(
        &self,
        query: &ListQuery,
        size: CatalogPageSize,
        page: Option<i64>,
    ) -> String {
        let mut params = vec![];
        if !matches!(self, Self::Category(_)) && !query.category.is_empty() {
            params.push(format!("category={}", catalog::encoded(&query.category)));
        }
        if !matches!(self, Self::Brand(_)) && !query.brand.is_empty() {
            params.push(format!("brand={}", catalog::encoded(&query.brand)));
        }
        if query.available {
            params.push("available=true".into());
        }
        if let Some(page) = page {
            params.push(format!("page={page}"));
        }
        if page.is_some() || !query.per_page.is_empty() || size != CatalogPageSize::default() {
            params.push(format!("per_page={}", size.value()));
        }
        let path = self.path(query);
        if params.is_empty() {
            path
        } else {
            format!("{path}?{}", params.join("&"))
        }
    }
}

/// Native GET form entry point. No JavaScript is needed to put the keyword in the path.
pub async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(mut query): Query<ListQuery>,
) -> Result<Response> {
    let state = state.localized();
    Products::validate_public_query(&query.q, &query.category, &query.brand, query.page)?;
    let size = preference::resolve(&query.per_page, &headers)?;
    query.q = query.q.trim().into();
    query.page = 1;
    let kind = if !query.q.is_empty() {
        ListingKind::Search
    } else if !query.category.is_empty() {
        ListingKind::Category(state.catalog.public_category(&query.category).await?)
    } else if !query.brand.is_empty() {
        ListingKind::Brand(state.catalog.public_brand(&query.brand).await?)
    } else {
        ListingKind::Catalog
    };
    preference::remember(
        Redirect::to(&kind.url(&query, size, None)).into_response(),
        &query.per_page,
        size,
        state.secure_cookies,
    )
}
pub async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(term): Path<String>,
    Query(mut query): Query<ListQuery>,
) -> Result<Response> {
    let state = state.localized();
    Products::validate_public_query(&term, &query.category, &query.brand, query.page)?;
    query.q = term.trim().into();
    if query.q.is_empty() {
        return Err(ProductError::Invalid(vec![]).into());
    }
    listing_response(&state, &headers, &query, ListingKind::Search).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_paths_preserve_text_and_resist_path_normalization() {
        assert_eq!(
            search_path("Caffè & tè / 50% +"),
            "/search/Caff%C3%A8%20%26%20t%C3%A8%20%2F%2050%25%20%2B"
        );
        assert_eq!(search_path(" .. "), "/search/..%20");
        assert_eq!(search_path("."), "/search/.%20");
        assert_eq!(search_path("%2F"), "/search/%252F");
        assert_eq!(search_path(" a?b#c "), "/search/a%3Fb%23c");
    }
}
