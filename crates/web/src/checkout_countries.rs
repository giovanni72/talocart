use super::fiscal_forms::FiscalRevision;
use super::*;
use axum::{
    extract::{Form, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_commerce::countries::{CheckoutCountries, CountryError};
type Result<T> = std::result::Result<T, CountryResponseError>;
struct CountryResponseError(CountryError);
impl From<CountryError> for CountryResponseError {
    fn from(error: CountryError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for CountryResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for CountryResponseError {
    fn into_response(self) -> Response {
        match self.0 {
            CountryError::Technical(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            CountryError::Access(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            CountryError::Identity(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            CountryError::Stale => {
                (StatusCode::CONFLICT, i18n::text("admin-fiscal-stale")).into_response()
            }
            CountryError::Invalid => (
                StatusCode::UNPROCESSABLE_ENTITY,
                i18n::text("admin-purchase-countries-invalid"),
            )
                .into_response(),
            CountryError::NotEnabled => (
                StatusCode::CONFLICT,
                i18n::text("admin-purchase-country-disabled"),
            )
                .into_response(),
        }
    }
}
const PATH: &str = "/admin/settings/countries";
pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(index).post(save))
}
fn service(state: &AppState) -> CheckoutCountries {
    CheckoutCountries::new(state.pool.clone(), state.identity.clone())
}
#[derive(Deserialize)]
struct SavedQuery {
    saved: Option<String>,
}
#[derive(Template)]
#[template(path = "checkout-countries.html")]
struct Page<'a> {
    revision: i64,
    stale: bool,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    countries: Vec<i18n::CodeChoice>,
    can_write: bool,
    saved: bool,
    error: bool,
    invalid_codes: String,
    enabled_count: usize,
}
async fn page(
    state: &AppState,
    session: &Session,
    selected: &[String],
    saved: bool,
    error: bool,
    revision: i64,
    stale: bool,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let mut countries = i18n::code_choices(talocart_platform::geography::COUNTRIES, "country", "");
    for c in &mut countries {
        c.selected = selected.iter().any(|code| code == c.code);
    }
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut response = render(Page {
        revision,
        stale,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "settings",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        countries,
        can_write: session.allows(Permission::SettingsWrite),
        saved,
        error,
        invalid_codes: selected
            .iter()
            .filter(|c| !talocart_platform::geography::country_valid(c))
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        enabled_count: selected.len(),
    });
    if stale {
        *response.status_mut() = StatusCode::CONFLICT;
    } else if error {
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    Ok(response)
}
async fn index(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<SavedQuery>,
) -> Result<Response> {
    let _ = revision;
    unified_settings::show(
        &state,
        &session,
        "countries",
        q.saved.as_deref() == Some("1"),
    )
    .await
    .map_err(|error| CountryResponseError(error.0.into()))
}
async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let mut csrf = None;
    let mut revision = None;
    let mut selected = vec![];
    let mut invalid = false;
    for (key, value) in fields {
        match key.as_str() {
            "csrf" => {
                if csrf.replace(value).is_some() {
                    invalid = true;
                }
            }
            "revision" => {
                if revision.replace(value).is_some() {
                    invalid = true;
                }
            }
            "country" => selected.push(value),
            _ => invalid = true,
        }
    }
    let csrf = csrf.unwrap_or_default();
    session.check_csrf(&csrf)?;
    let revision = revision
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or(CountryError::Stale)?;
    if invalid {
        return page(&state, &session, &selected, false, true, revision, false).await;
    }
    match service(&state)
        .save(&session, &csrf, &selected, revision)
        .await
    {
        Ok(()) => Ok(Redirect::to(&format!("{PATH}?saved=1")).into_response()),
        Err(CountryError::Invalid) => {
            page(&state, &session, &selected, false, true, revision, false).await
        }
        Err(CountryError::Stale) => {
            page(&state, &session, &selected, false, false, revision, true).await
        }
        Err(error) => Err(error.into()),
    }
}
