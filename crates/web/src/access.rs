use super::*;
use axum::{extract::Form, response::Redirect};
use cookie::{Cookie, SameSite, time::Duration as CookieDuration};
use serde::Deserialize;
use talocart_platform::identity::{
    ANONYMOUS_SESSION_SECONDS, AUTHENTICATED_SESSION_SECONDS, IdentityError, RATE_WINDOW_SECONDS,
    valid_token,
};

impl IdentityErrorResponse {
    pub(crate) fn status(&self) -> StatusCode {
        Self::status_for(&self.0)
    }
    pub fn status_for(error: &IdentityError) -> StatusCode {
        match error {
            IdentityError::Invalid
            | IdentityError::InvalidLocale
            | IdentityError::InvalidSettings(_)
            | IdentityError::RoleLimit
            | IdentityError::CurrentPassword
            | IdentityError::InvalidAvatar
            | IdentityError::InvalidBranding
            | IdentityError::PasswordMismatch => StatusCode::UNPROCESSABLE_ENTITY,
            IdentityError::Unauthorized => StatusCode::UNAUTHORIZED,
            IdentityError::Forbidden => StatusCode::FORBIDDEN,
            IdentityError::Installed
            | IdentityError::LastOwner
            | IdentityError::DuplicateEmail
            | IdentityError::ProtectedRole
            | IdentityError::RoleInUse
            | IdentityError::DuplicateRole => StatusCode::CONFLICT,
            IdentityError::NotFound | IdentityError::RoleNotFound => StatusCode::NOT_FOUND,
            IdentityError::Limited => StatusCode::TOO_MANY_REQUESTS,
            IdentityError::Busy
            | IdentityError::Unavailable
            | IdentityError::Storage
            | IdentityError::Technical(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
    fn retry_header(&self, response: &mut Response) {
        Self::retry_for(&self.0, response);
    }
    pub(crate) fn retry_for(error: &IdentityError, response: &mut Response) {
        let seconds = match error {
            IdentityError::Limited => RATE_WINDOW_SECONDS,
            IdentityError::Busy
            | IdentityError::Technical(talocart_platform::technical::TechnicalError::Busy) => 1,
            _ => return,
        };
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    }
}
impl IntoResponse for IdentityErrorResponse {
    fn into_response(self) -> Response {
        let mut response = (self.status(), i18n::identity_error(&self.0)).into_response();
        self.retry_header(&mut response);
        response
    }
}
#[derive(Debug)]
pub struct IdentityErrorResponse(pub(super) IdentityError);
impl From<IdentityError> for IdentityErrorResponse {
    fn from(e: IdentityError) -> Self {
        Self(e)
    }
}
type Result<T> = std::result::Result<T, IdentityErrorResponse>;
const COOKIE: &str = "talocart_session";
pub(crate) fn session_token(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for parsed in Cookie::split_parse(value.to_str().ok()?) {
            let cookie = parsed.ok()?;
            if cookie.name() == COOKIE {
                if found.is_some() || !valid_token(cookie.value()) {
                    return None;
                }
                found = Some(cookie.value().to_owned());
            }
        }
    }
    found
}
pub(crate) fn set_cookie(
    response: &mut Response,
    raw: &str,
    secure: bool,
    seconds: i64,
) -> Result<()> {
    let cookie = Cookie::build((COOKIE, raw.to_owned()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .max_age(CookieDuration::seconds(seconds))
        .build();
    let value =
        HeaderValue::from_str(&cookie.to_string()).map_err(|_| IdentityError::Unavailable)?;
    response.headers_mut().append(header::SET_COOKIE, value);
    Ok(())
}
async fn existing(state: &AppState, headers: &HeaderMap) -> Result<Session> {
    let raw = session_token(headers).ok_or(IdentityError::Forbidden)?;
    state
        .identity
        .session(&raw)
        .await?
        .ok_or(IdentityError::Forbidden.into())
}
pub async fn require_admin(request: axum::http::Request<Body>, next: Next) -> Response {
    // language_context already validated the session and loaded the current
    // personal preference in one query; actions still recheck authorisation.
    if request
        .extensions()
        .get::<Session>()
        .is_some_and(|s| s.administrator_id.is_some())
    {
        return next.run(request).await;
    }
    let mut response = Redirect::to("/admin/login").into_response();
    if request
        .headers()
        .get("hx-request")
        .is_some_and(|v| v == "true")
    {
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        response
            .headers_mut()
            .insert("hx-redirect", HeaderValue::from_static("/admin/login"));
    }
    response
}

#[derive(Template)]
#[template(path = "access.html")]
struct AccessPage<'a> {
    theme: talocart_platform::storefront::Settings,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    kind: &'a str,
    csrf: &'a str,
    error: &'a str,
    install_demo: bool,
    languages: SetupLanguages,
    language_errors: Vec<&'static str>,
}
#[derive(Clone)]
struct SetupLanguages {
    primary_locale: String,
    active_locales: Vec<String>,
}
impl Default for SetupLanguages {
    fn default() -> Self {
        let languages = talocart_platform::storefront::Languages::default();
        Self {
            primary_locale: languages.primary().code().into(),
            active_locales: languages
                .active()
                .iter()
                .map(|locale| locale.code().into())
                .collect(),
        }
    }
}
impl SetupLanguages {
    fn selected(&self, code: &str) -> bool {
        self.active_locales.iter().any(|value| value == code)
    }
}
async fn page(
    state: &AppState,
    headers: &HeaderMap,
    kind: &str,
    error: &str,
    status: StatusCode,
) -> Result<Response> {
    page_with_demo(state, headers, kind, error, status, false).await
}
async fn page_with_demo(
    state: &AppState,
    headers: &HeaderMap,
    kind: &str,
    error: &str,
    status: StatusCode,
    install_demo: bool,
) -> Result<Response> {
    page_with_choices(
        state,
        headers,
        kind,
        error,
        status,
        install_demo,
        SetupLanguages::default(),
        Vec::new(),
    )
    .await
}
#[allow(clippy::too_many_arguments)] // Shared access rendering also preserves installer-specific choices.
async fn page_with_choices(
    state: &AppState,
    headers: &HeaderMap,
    kind: &str,
    error: &str,
    status: StatusCode,
    install_demo: bool,
    languages: SetupLanguages,
    language_errors: Vec<&'static str>,
) -> Result<Response> {
    let existing = match session_token(headers) {
        Some(raw) => state.identity.session(&raw).await?,
        None => None,
    };
    let (new, session) = match existing {
        Some(session) => (None, session),
        None => {
            let (raw, session) = state.identity.anonymous().await?;
            (Some(raw), session)
        }
    };
    // Installation identifies the platform before a store has been created.
    // Login and recovery retain the merchant's configured branding.
    let name = if kind == "setup" {
        "Talocart".to_owned()
    } else {
        current_name(state).await?
    };
    let mut response = render(AccessPage {
        theme: state.storefront.settings().await?,
        store_name: &name,
        logo: current_logo(state).await?,
        kind,
        csrf: &session.csrf,
        error,
        install_demo,
        languages,
        language_errors,
    });
    *response.status_mut() = status;
    if let Some(raw) = new {
        set_cookie(
            &mut response,
            &raw,
            state.secure_cookies,
            ANONYMOUS_SESSION_SECONDS,
        )?;
    }
    Ok(response)
}
pub async fn setup_page(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    if state.identity.installed().await? {
        return Ok(Redirect::to("/admin/login").into_response());
    }
    page(&state, &headers, "setup", "", StatusCode::OK).await
}
pub async fn login_page(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    if !state.identity.installed().await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    page(&state, &headers, "login", "", StatusCode::OK).await
}
pub async fn reset_page(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    page(&state, &headers, "reset", "", StatusCode::OK).await
}
#[derive(Deserialize)]
pub struct AccessForm {
    csrf: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    email: String,
    password: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    install_demo: bool,
}
async fn failure(
    state: &AppState,
    headers: &HeaderMap,
    kind: &str,
    error: IdentityError,
) -> Result<Response> {
    let error = IdentityErrorResponse(error);
    let mut response = page(
        state,
        headers,
        kind,
        &i18n::identity_error(&error.0),
        error.status(),
    )
    .await?;
    error.retry_header(&mut response);
    Ok(response)
}
/// Form pairs preserve repeated checkboxes and reject duplicate scalar fields.
fn setup_input(
    fields: Vec<(String, String)>,
) -> std::result::Result<(AccessForm, SetupLanguages), IdentityError> {
    let mut scalars = std::collections::BTreeMap::new();
    let mut active_locales = Vec::new();
    for (key, value) in fields {
        if key == "locale" {
            active_locales.push(value);
        } else if ![
            "csrf",
            "email",
            "password",
            "name",
            "display_name",
            "install_demo",
            "primary_locale",
        ]
        .contains(&key.as_str())
            || scalars.insert(key, value).is_some()
        {
            return Err(IdentityError::Forbidden);
        }
    }
    let install_demo = match scalars.remove("install_demo").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        _ => return Err(IdentityError::Invalid),
    };
    let languages = SetupLanguages {
        primary_locale: scalars.remove("primary_locale").unwrap_or_default(),
        active_locales,
    };
    let form = AccessForm {
        csrf: scalars.remove("csrf").unwrap_or_default(),
        token: String::new(),
        email: scalars.remove("email").unwrap_or_default(),
        password: scalars.remove("password").unwrap_or_default(),
        name: scalars.remove("name").unwrap_or_default(),
        display_name: scalars.remove("display_name").unwrap_or_default(),
        install_demo,
    };
    Ok((form, languages))
}
pub async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let (form, choices) = setup_input(fields)?;
    let session = existing(&state, &headers).await?;
    session.check_csrf(&form.csrf)?;
    if state.identity.installed().await? {
        return Err(IdentityError::Installed.into());
    }
    let languages = match talocart_platform::storefront::Languages::new(
        &choices.primary_locale,
        &choices.active_locales,
    ) {
        Ok(languages) => languages,
        Err(talocart_platform::storefront::StorefrontError::Invalid(errors)) => {
            return page_with_choices(
                &state,
                &headers,
                "setup",
                &i18n::text("admin-check-the-highlighted-fields"),
                StatusCode::UNPROCESSABLE_ENTITY,
                form.install_demo,
                choices,
                errors,
            )
            .await;
        }
        Err(talocart_platform::storefront::StorefrontError::Content(_)) => {
            return Err(IdentityError::Unavailable.into());
        }
        Err(talocart_platform::storefront::StorefrontError::Technical(error)) => {
            return Err(error.into());
        }
        Err(talocart_platform::storefront::StorefrontError::Access(error)) => {
            return Err(error.into());
        }
        Err(talocart_platform::storefront::StorefrontError::Identity(error)) => {
            return Err(error.into());
        }
    };
    let prepared = match state
        .identity
        .prepare_bootstrap(&form.email, form.password, &form.name, &form.display_name)
        .await
    {
        Ok(value) => value.with_languages(languages.clone()),
        Err(error) => {
            return page_with_choices(
                &state,
                &headers,
                "setup",
                &i18n::identity_error(&error),
                IdentityErrorResponse::status_for(&error),
                form.install_demo,
                choices,
                Vec::new(),
            )
            .await;
        }
    };
    let installed = talocart_commerce::installation::install(
        &state.pool,
        prepared,
        form.install_demo.then_some(&state.demo),
    )
    .await
    .map_err(|error| match error {
        // The presentation contract of setup remains the existing localized identity response.
        talocart_commerce::installation::Error::Technical(cause) => {
            talocart_commerce::installation::Error::Identity(cause.into())
        }
        error => error,
    });
    match installed {
        Ok(()) => {
            let mut response = setup_redirect(&state)?;
            i18n::remember(
                &mut response,
                languages.primary().code(),
                state.secure_cookies,
            )?;
            Ok(response)
        }
        Err(talocart_commerce::installation::Error::Identity(error)) => {
            let mut response = page_with_choices(
                &state,
                &headers,
                "setup",
                &i18n::identity_error(&error),
                IdentityErrorResponse::status_for(&error),
                form.install_demo,
                choices,
                Vec::new(),
            )
            .await?;
            IdentityErrorResponse::retry_for(&error, &mut response);
            Ok(response)
        }
        Err(error) => {
            tracing::warn!(error = %error, "Installation with demo data did not complete");
            page_with_choices(&state, &headers, "setup", &i18n::text("admin-installation-incomplete-no-data-was-saved-check-the-demo-package-or-retr"), StatusCode::SERVICE_UNAVAILABLE, form.install_demo, choices, Vec::new()).await
        }
    }
}
fn setup_redirect(state: &AppState) -> Result<Response> {
    let mut r = Redirect::to("/admin/login").into_response();
    set_cookie(&mut r, "", state.secure_cookies, 0)?;
    Ok(r)
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AccessForm>,
) -> Result<Response> {
    let session = existing(&state, &headers).await?;
    match state
        .identity
        .login(&session, &form.csrf, &form.email, form.password)
        .await
    {
        Ok(raw) => {
            let session = state
                .identity
                .session(&raw)
                .await?
                .ok_or(IdentityError::Unauthorized)?;
            let mut r = Redirect::to(landing_path(&session)).into_response();
            set_cookie(
                &mut r,
                &raw,
                state.secure_cookies,
                AUTHENTICATED_SESSION_SECONDS,
            )?;
            i18n::remember(&mut r, &session.locale, state.secure_cookies)?;
            Ok(r)
        }
        Err(e) => failure(&state, &headers, "login", e).await,
    }
}
pub async fn reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AccessForm>,
) -> Result<Response> {
    let session = existing(&state, &headers).await?;
    match state
        .identity
        .reset(&session, &form.csrf, &form.token, form.password)
        .await
    {
        Ok(()) => {
            let mut r = Redirect::to("/admin/login").into_response();
            set_cookie(&mut r, "", state.secure_cookies, 0)?;
            Ok(r)
        }
        Err(e) => failure(&state, &headers, "reset", e).await,
    }
}
#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}
pub async fn logout(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<CsrfForm>,
) -> Result<Response> {
    state.identity.logout(&session, &form.csrf).await?;
    let mut response = Redirect::to("/admin/login").into_response();
    set_cookie(&mut response, "", state.secure_cookies, 0)?;
    Ok(response)
}
pub(crate) fn technical_status(_: &talocart_platform::technical::TechnicalError) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}
pub(crate) fn access_status(error: &talocart_platform::technical::AccessError) -> StatusCode {
    match error {
        talocart_platform::technical::AccessError::Forbidden => StatusCode::FORBIDDEN,
        talocart_platform::technical::AccessError::Unauthorized => StatusCode::UNAUTHORIZED,
    }
}
impl From<talocart_platform::technical::TechnicalError> for IdentityErrorResponse {
    fn from(error: talocart_platform::technical::TechnicalError) -> Self {
        Self(error.into())
    }
}
impl From<talocart_platform::technical::AccessError> for IdentityErrorResponse {
    fn from(error: talocart_platform::technical::AccessError) -> Self {
        Self(error.into())
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn direct_service_causes_preserve_http_status_retry_and_localization() {
        use talocart_platform::technical::{AccessError, TechnicalError};
        for (locale, expected) in [
            (talocart_platform::locale::Locale::ENGLISH, "busy"),
            (talocart_platform::locale::Locale::ITALIAN, "occupato"),
        ] {
            crate::i18n::scope(locale, async {
                let responses = [
                    crate::products::ProductResponseError::from(TechnicalError::Busy)
                        .into_response(),
                    crate::catalog::CatalogResponseError::from(TechnicalError::Busy)
                        .into_response(),
                    crate::taxes::TaxResponseError::from(TechnicalError::Busy).into_response(),
                    crate::variants::ErrorResponse::from(TechnicalError::Busy).into_response(),
                ];
                for response in responses {
                    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                    assert_eq!(response.headers()[header::RETRY_AFTER], "1");
                    let body = axum::body::to_bytes(response.into_body(), 4096)
                        .await
                        .unwrap();
                    assert!(String::from_utf8(body.to_vec()).unwrap().contains(expected));
                }
                assert_eq!(
                    crate::products::ProductResponseError::from(AccessError::Forbidden)
                        .into_response()
                        .status(),
                    StatusCode::FORBIDDEN
                );
                assert_eq!(
                    crate::products::ProductResponseError::from(AccessError::Unauthorized)
                        .into_response()
                        .status(),
                    StatusCode::UNAUTHORIZED
                );
            })
            .await;
        }
    }

    use super::*;
    #[test]
    fn cookie_has_security_attributes_and_rejects_duplicates() {
        for secure in [false, true] {
            let mut response = StatusCode::OK.into_response();
            set_cookie(
                &mut response,
                &"a".repeat(64),
                secure,
                AUTHENTICATED_SESSION_SECONDS,
            )
            .unwrap();
            let value = response.headers()[header::SET_COOKIE].to_str().unwrap();
            assert!(value.contains("HttpOnly"));
            assert!(value.contains("SameSite=Strict"));
            assert!(value.contains("Path=/"));
            assert_eq!(value.contains("Secure"), secure);
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "talocart_session={}; talocart_session={}",
                "a".repeat(64),
                "b".repeat(64)
            ))
            .unwrap(),
        );
        assert!(session_token(&headers).is_none());
    }

    #[test]
    fn capacity_and_abuse_have_different_status_and_retry_delay() {
        for (error, status, retry) in [
            (IdentityError::Busy, StatusCode::SERVICE_UNAVAILABLE, 1),
            (
                IdentityError::Limited,
                StatusCode::TOO_MANY_REQUESTS,
                RATE_WINDOW_SECONDS,
            ),
        ] {
            let response = IdentityErrorResponse(error).into_response();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::RETRY_AFTER], retry.to_string());
        }
        let response = IdentityErrorResponse(IdentityError::Unavailable).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
    }
}
