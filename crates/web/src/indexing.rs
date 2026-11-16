//! Explicit public-resource allowlist; HTTP remains noindex unless marked here.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Allowed;

pub(super) fn candidate(request: &axum::http::Request<Body>, path: &str) -> bool {
    if !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) || request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .is_none()
        || request
            .headers()
            .get("hx-request")
            .is_some_and(|v| v == "true")
    {
        return false;
    }
    // Embedded assets keep a conservative policy without consulting the DB.
    // noindex does not prevent crawlers from loading CSS/JS to render a page.
    let resource = ["/media/public/", "/media/image/", "/branding/"]
        .iter()
        .any(|prefix| path.starts_with(prefix));
    // Sitemap shard pagination is structural. Other page query variants (including
    // listing pagination, filters and product selections) retain their canonical
    // clean URL and remain excluded from the index.
    if request.uri().query().is_some() && path != "/sitemap-products.xml" && !resource {
        return false;
    }
    matches!(
        path,
        "/" | "/catalog"
            | "/brands"
            | "/robots.txt"
            | "/sitemap.xml"
            | "/sitemap-catalog.xml"
            | "/sitemap-products.xml"
    ) || ["/catalog/", "/brands/", "/products/", "/pages/"]
        .iter()
        .any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|slug| !slug.is_empty() && !slug.contains('/'))
        })
        || resource
}

pub(super) fn mark(response: &mut Response, enabled: bool, candidate: bool) {
    if !candidate {
        return;
    }
    if enabled
        && matches!(response.status(), StatusCode::OK | StatusCode::NOT_MODIFIED)
        && !response.headers().contains_key("hx-redirect")
    {
        response.extensions_mut().insert(Allowed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_fragments_and_disabled_publication_never_receive_permission() {
        for enabled in [false, true] {
            for candidate in [false, true] {
                for status in [
                    StatusCode::OK,
                    StatusCode::NOT_MODIFIED,
                    StatusCode::SEE_OTHER,
                    StatusCode::NOT_FOUND,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    StatusCode::SERVICE_UNAVAILABLE,
                ] {
                    let mut response = status.into_response();
                    mark(&mut response, enabled, candidate);
                    assert_eq!(
                        response.extensions().get::<Allowed>().is_some(),
                        enabled
                            && candidate
                            && matches!(status, StatusCode::OK | StatusCode::NOT_MODIFIED)
                    );
                }
            }
        }
        let mut redirect = StatusCode::OK.into_response();
        redirect
            .headers_mut()
            .insert("hx-redirect", HeaderValue::from_static("/"));
        mark(&mut redirect, true, true);
        assert!(redirect.extensions().get::<Allowed>().is_none());
    }
}
