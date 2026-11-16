//! Manual storefront closure. The privileged updater owns a separate, stronger fence.
use super::*;
use axum::{extract::Form, response::Redirect};
use serde::Deserialize;

const PATH: &str = "/admin/settings/maintenance";
const PAGE: &str = include_str!("../../../assets/maintenance.html");
const POLICY: &str = include_str!("../../../assets/maintenance.csp");

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(index).post(save))
}

// Exact administrative boundary; lookalikes such as /administrator stay closed.
fn exempt(path: &str) -> bool {
    path == "/admin"
        || path.starts_with("/admin/")
        || path == "/setup"
        || path == "/maintenance/status"
        || ["/assets/", "/media/", "/branding/", "/health/"]
            .iter()
            .any(|p| path.starts_with(p))
        || path == "/favicon.svg"
}

pub(super) fn response(head: bool) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Html(if head { "" } else { PAGE }),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(POLICY),
    );
    headers.insert("x-talocart-maintenance", HeaderValue::from_static("manual"));
    response
}

pub async fn gate(
    State(state): State<AppState>,
    request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    if exempt(request.uri().path())
        || state
            .online
            .as_ref()
            .is_some_and(|online| request.uri().path() == online.webhook_path())
        || request
            .extensions()
            .get::<axum::extract::MatchedPath>()
            .is_none()
    {
        return next.run(request).await;
    }
    match state.storefront.maintenance_enabled().await {
        Ok(false) => next.run(request).await,
        Ok(true) => {
            let mut response = response(request.method() == axum::http::Method::HEAD);
            if request
                .headers()
                .get("hx-request")
                .is_some_and(|value| value == "true")
            {
                // HTMX does not swap error bodies; reopen the page with a native GET.
                response
                    .headers_mut()
                    .insert("hx-redirect", HeaderValue::from_static("/"));
            }
            response
        }
        Err(error) => access::IdentityErrorResponse::from(error).into_response(),
    }
}

pub async fn status(State(state): State<AppState>) -> Response {
    match state.storefront.maintenance_enabled().await {
        Ok(false) => StatusCode::NO_CONTENT.into_response(),
        Ok(true) => response(true),
        Err(error) => access::IdentityErrorResponse::from(error).into_response(),
    }
}

#[derive(Template)]
#[template(path = "maintenance-settings.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    enabled: bool,
    can_write: bool,
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(&session, Permission::SettingsRead)
        .await?;
    let shell = AdminShell::load(&state, &session).await?;
    let enabled = shell.theme.maintenance_enabled;
    Ok(render(Page {
        store_name: &shell.store_name,
        logo: shell.logo,
        csrf: shell.csrf,
        section: "maintenance_settings",
        theme: shell.theme,
        nav: shell.nav,
        account: shell.account,
        enabled,
        can_write: session.allows(Permission::SettingsWrite),
    }))
}

#[derive(Deserialize)]
struct FormData {
    csrf: String,
    enabled: bool,
}
async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<FormData>,
) -> Result<Response, access::IdentityErrorResponse> {
    state
        .storefront
        .set_maintenance(&session, &form.csrf, form.enabled)
        .await?;
    Ok(Redirect::to(PATH).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closure_exemptions_preserve_admin_health_assets_and_payment_notifications_only() {
        for path in [
            "/admin",
            "/admin/login",
            "/admin/settings/maintenance",
            "/health/ready",
            "/assets/ui.css",
            "/maintenance/status",
        ] {
            assert!(exempt(path), "{path}");
        }
        for path in [
            "/",
            "/en/catalog",
            "/cart",
            "/checkout",
            "/account",
            "/administrator",
            "/en/admin",
            "/payments/stripe/webhook",
            "/payments/stripe/webhook/extra",
        ] {
            assert!(!exempt(path), "{path}");
        }
    }
    #[test]
    fn maintenance_is_an_uncached_service_unavailable_document() {
        let response = response(false);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("sha256-")
        );
        assert!(PAGE.contains("/maintenance/status"));
    }
}
