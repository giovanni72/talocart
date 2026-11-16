//! Shared cache, tracing, timeout and security-header policy.
use super::*;

/// Exclude private responses from storage and indexing without changing other headers.
pub(crate) fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) fn trace_path(path: &str) -> &str {
    if path.starts_with("/search/") {
        "/search/{term}"
    } else if talocart_platform::locale::Locale::SUPPORTED
        .iter()
        .any(|(code, _)| path.starts_with(&format!("/{code}/search/")))
    {
        "/{locale}/search/{term}"
    } else {
        path
    }
}

/// Prepare immutable validators when composing the router, including for HEAD/304.
pub(super) fn asset<S>(
    content_type: &'static str,
    bytes: &'static [u8],
) -> axum::routing::MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    use sha2::{Digest, Sha256};
    // Content-derived validators work across rebuilds without stale unversioned URLs.
    let etag = format!("\"asset-{:x}\"", Sha256::digest(bytes));
    let validator = HeaderValue::from_str(&etag).ok();
    get(move |headers: HeaderMap| {
        let mut response = if images::matches_etag(&headers, &etag) {
            StatusCode::NOT_MODIFIED.into_response()
        } else {
            bytes.into_response()
        };
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0, must-revalidate"),
        );
        if let Some(value) = &validator {
            response.headers_mut().insert(header::ETAG, value.clone());
        }
        async move { response }
    })
}

pub(super) async fn security_headers(request: axum::http::Request<Body>, next: Next) -> Response {
    let confirmation = request.uri().path().ends_with("/account/confirm");
    let mut response = next.run(request).await;
    let indexing_allowed = response
        .extensions_mut()
        .remove::<indexing::Allowed>()
        .is_some();
    let headers = response.headers_mut();
    headers.entry(header::CONTENT_SECURITY_POLICY).or_insert(HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static(if confirmation {
            "no-referrer"
        } else {
            "same-origin"
        }),
    );
    if indexing_allowed {
        // A 304 merges headers into a stored response: omission would retain an
        // earlier noindex. Preserve any explicit exclusion from the handler.
        headers
            .entry("x-robots-tag")
            .or_insert(HeaderValue::from_static("index, follow"));
    } else {
        headers.insert(
            "x-robots-tag",
            HeaderValue::from_static("noindex, nofollow"),
        );
    }
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.append(header::VARY, HeaderValue::from_static("HX-Request"));
    response
}

pub(super) async fn request_timeout(request: axum::http::Request<Body>, next: Next) -> Response {
    let package = request.uri().path().starts_with("/admin/modules/")
        || request.uri().path().starts_with("/admin/extensions/");
    let smtp_check = request.method() == axum::http::Method::POST
        && request.uri().path() == "/admin/settings/email/check";
    // SMTP has its own deadline (at most 20s); leave room for authorization,
    // the saved configuration snapshot and the redirect that reports its outcome.
    let seconds = if package {
        180
    } else if smtp_check {
        30
    } else {
        10
    };
    match tokio::time::timeout(Duration::from_secs(seconds), next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn only_package_routes_and_smtp_check_can_outlast_ordinary_request_timeout() {
        let app = Router::new()
            .fallback(|| async {
                tokio::time::sleep(Duration::from_secs(11)).await;
                StatusCode::OK
            })
            .layer(axum::middleware::from_fn(request_timeout));
        let cases = [
            ("GET", "/admin/extensions/fixture/jobs", StatusCode::OK),
            ("GET", "/admin/catalog/imports", StatusCode::REQUEST_TIMEOUT),
            ("POST", "/admin/settings/email/check", StatusCode::OK),
            (
                "GET",
                "/admin/settings/email/check",
                StatusCode::REQUEST_TIMEOUT,
            ),
            (
                "POST",
                "/admin/settings/email/check/extra",
                StatusCode::REQUEST_TIMEOUT,
            ),
            (
                "POST",
                "/admin/settings/email/test",
                StatusCode::REQUEST_TIMEOUT,
            ),
        ];
        futures_util::future::join_all(cases.into_iter().map(|(method, path, expected)| {
            let app = app.clone();
            async move {
                let request = axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap();
                assert_eq!(
                    app.oneshot(request).await.unwrap().status(),
                    expected,
                    "{method} {path}"
                );
            }
        }))
        .await;
    }
}
