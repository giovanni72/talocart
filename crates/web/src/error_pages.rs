//! Decorate complete browser error documents while preserving transport contracts.
use super::*;
use axum::http::Method;

struct ErrorContent {
    code: u16,
    title: String,
    message: String,
    back: String,
    back_label: String,
}
#[derive(Template)]
#[template(path = "default/templates/error.html")]
struct PublicError {
    store_name: String,
    logo: Option<talocart_platform::media::ImageMetadata>,
    category_menu: navigation::CategoryMenu,
    theme: talocart_platform::storefront::Settings,
    search_query: &'static str,
    search_action: &'static str,
    search_filters: navigation::SearchFilters,
    error: ErrorContent,
}
#[derive(Template)]
#[template(path = "admin-error.html")]
struct AdminError<'a> {
    store_name: String,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    section: &'static str,
    csrf: &'a str,
    error: ErrorContent,
}

#[derive(Template)]
#[template(path = "access-error.html")]
struct AccessError {
    store_name: String,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    error: ErrorContent,
}

pub(super) fn document_request(request: &axum::http::Request<Body>) -> bool {
    let path = request.uri().path();
    if request
        .headers()
        .get("hx-request")
        .is_some_and(|v| v == "true")
        || [
            "/assets",
            "/media",
            "/branding",
            "/health",
            "/webhooks",
            "/webhook",
        ]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
        || path.ends_with(".xml")
        || path == "/robots.txt"
        || path.starts_with("/payments/")
    {
        return false;
    }
    // Fetch/XHR clients and APIs keep their original response representation.
    if request
        .headers()
        .get("sec-fetch-dest")
        .is_some_and(|v| v != "document" && v != "iframe")
    {
        return false;
    }
    request.headers().get(header::ACCEPT).is_none_or(|v| {
        v.to_str()
            .is_ok_and(|v| v.contains("text/html") || v == "*/*")
    })
}

pub(super) async fn context(
    State(state): State<AppState>,
    request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let document = document_request(&request);
    let head = request.method() == Method::HEAD;
    let admin = request.uri().path() == "/admin" || request.uri().path().starts_with("/admin/");
    let session = request
        .extensions()
        .get::<Session>()
        .cloned()
        .filter(|s| s.administrator_id.is_some());
    let response = next.run(request).await;
    if !document {
        return response;
    }
    decorate(&state, response, admin, session, head).await
}

pub(super) async fn public_not_found(state: &AppState, document: bool, head: bool) -> Response {
    early(state, not_found().await, false, document, head).await
}

/// Language/session failures can precede the inner response middleware.
pub(super) async fn early(
    state: &AppState,
    response: Response,
    admin: bool,
    document: bool,
    head: bool,
) -> Response {
    if document {
        decorate(state, response, admin, None, head).await
    } else {
        response
    }
}

async fn decorate(
    state: &AppState,
    response: Response,
    admin: bool,
    session: Option<Session>,
    head: bool,
) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error())
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_none_or(|v| v.to_str().is_ok_and(|v| v.starts_with("text/plain")))
        || response.headers().contains_key("hx-redirect")
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let message = if status.is_server_error() {
        // Internal/provider diagnostics never become public HTML.
        i18n::text("error-service-help")
    } else {
        match axum::body::to_bytes(body, 16 * 1024).await {
            Ok(bytes) => String::from_utf8(bytes.to_vec())
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| i18n::text("error-request-help")),
            Err(_) => i18n::text("error-request-help"),
        }
    };
    let title = i18n::text(match status {
        StatusCode::NOT_FOUND => "admin-page-not-found",
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => "error-access-title",
        _ if status.is_server_error() => "error-service-title",
        _ => "error-request-title",
    });
    let snapshot = store_language::snapshot_settings();
    let configured = snapshot.is_some();
    let mut theme = snapshot.unwrap_or_default();
    let mut name = if status.is_server_error() {
        i18n::text("error-store-label")
    } else {
        state.store_name.to_string()
    };
    let mut logo = None;
    let mut menu = navigation::CategoryMenu::default();
    if !status.is_server_error() && (configured || admin && session.is_some()) {
        if admin && let Ok(settings) = state.storefront.settings().await {
            theme = settings;
        }
        if let Ok((store_name, store_logo)) =
            tokio::try_join!(current_name(state), current_logo(state))
        {
            name = store_name;
            logo = store_logo;
        }
        if !admin
            && let Ok(categories) =
                navigation::CategoryMenu::load(state, "", false, "", false).await
        {
            menu = categories;
        }
    }
    let error = ErrorContent {
        code: status.as_u16(),
        title,
        message,
        back: if admin {
            if session.is_some() {
                "/admin".into()
            } else {
                "/admin/login".into()
            }
        } else {
            store_language::path("/catalog")
        },
        back_label: i18n::text(if admin {
            if session.is_some() {
                "error-back-admin"
            } else {
                "admin-back-to-sign-in"
            }
        } else {
            "error-back-catalog"
        }),
    };
    let rendered = if admin && let Some(session) = session {
        let shell = AdminShell::from_parts(state, &session, name, logo, theme);
        render(AdminError {
            store_name: shell.store_name,
            logo: shell.logo,
            theme: shell.theme,
            nav: shell.nav,
            account: shell.account,
            section: "",
            csrf: &session.csrf,
            error,
        })
    } else if admin {
        render(AccessError {
            store_name: name,
            logo,
            theme,
            error,
        })
    } else {
        render(PublicError {
            store_name: name,
            logo,
            theme,
            category_menu: menu,
            search_query: "",
            search_action: "/search",
            search_filters: navigation::SearchFilters::default(),
            error,
        })
    };
    if rendered.status().is_server_error() {
        return rendered;
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    parts
        .headers
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Response::from_parts(
        parts,
        if head {
            Body::empty()
        } else {
            rendered.into_body()
        },
    )
}

pub(super) async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        i18n::text("admin-this-page-does-not-exist"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_documents_are_distinct_from_fragments_static_files_and_apis() {
        for path in ["/products/missing", "/admin/unknown", "/pages/missing"] {
            let request = axum::http::Request::builder()
                .uri(path)
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .unwrap();
            assert!(document_request(&request));
        }
        for (path, key, value) in [
            ("/assets/missing.js", "accept", "text/html"),
            ("/health/ready", "accept", "text/html"),
            ("/payments/stripe/webhook", "accept", "text/html"),
            ("/products/missing", "hx-request", "true"),
            ("/products/missing", "accept", "application/json"),
            ("/products/missing", "sec-fetch-dest", "empty"),
        ] {
            let request = axum::http::Request::builder()
                .uri(path)
                .header(key, value)
                .body(Body::empty())
                .unwrap();
            assert!(!document_request(&request));
        }
    }
    #[tokio::test]
    async fn unavailable_documents_hide_diagnostics_keep_retry_headers_and_head_empty() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://synthetic:synthetic@127.0.0.1:1/synthetic")
            .unwrap();
        let state = AppState::configured(pool, "Unavailable".into(), Default::default());
        for locale in [
            talocart_platform::locale::Locale::parse("en").unwrap(),
            talocart_platform::locale::Locale::parse("it").unwrap(),
        ] {
            i18n::scope(locale, async {
                for (admin, head) in [(false, false), (true, false), (false, true)] {
                    let response = (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "private provider diagnostic <script>canary</script>",
                    )
                        .into_response();
                    let mut response = response;
                    response
                        .headers_mut()
                        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
                    let response = decorate(&state, response, admin, None, head).await;
                    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                    assert_eq!(response.headers()[header::RETRY_AFTER], "60");
                    assert!(
                        response.headers()[header::CONTENT_TYPE]
                            .to_str()
                            .unwrap()
                            .starts_with("text/html")
                    );
                    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .unwrap();
                    let body = String::from_utf8(bytes.to_vec()).unwrap();
                    if head {
                        assert!(body.is_empty());
                    } else {
                        assert!(body.contains(if admin {
                            "class=\"access-shell\""
                        } else {
                            "class=\"site-header\""
                        }));
                        assert!(!body.contains("private provider") && !body.contains("canary"));
                    }
                }
            })
            .await;
        }
    }
}
