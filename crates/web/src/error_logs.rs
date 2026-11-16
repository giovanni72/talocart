use super::*;
use axum::extract::Query;
use serde::Deserialize;

#[derive(Default, Deserialize)]
pub struct Filters {
    #[serde(default)]
    date: String,
    before: Option<u64>,
}
#[derive(Template)]
#[template(path = "error-logs.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    journal: talocart_platform::error_logs::Page,
}
pub async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(filters): Query<Filters>,
) -> Result<Response, access::IdentityErrorResponse> {
    // Check access even when logging is unavailable on a standalone module.
    let Some(logs) = state.error_logs.as_ref() else {
        state.identity.authorize_owner(&session).await?;
        return Err(talocart_platform::identity::IdentityError::NotFound.into());
    };
    let journal = logs
        .read(&state.identity, &session, &filters.date, filters.before)
        .await?;
    let shell = AdminShell::load(&state, &session).await?;
    let mut response = render(Page {
        store_name: &shell.store_name,
        logo: shell.logo,
        csrf: shell.csrf,
        section: "error_logs",
        theme: shell.theme,
        nav: shell.nav,
        account: shell.account,
        journal,
    });
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}
