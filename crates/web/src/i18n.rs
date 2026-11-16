//! Plain-text presentation catalogues. The locale is scoped to the request future,
//! never to an OS thread or a mutable global. Spawned background jobs deliberately
//! do not inherit the HTTP locale; stored diagnostics retain their source wording.
use super::*;
use axum::{extract::Form, response::Redirect};
use cookie::{Cookie, SameSite, time::Duration as CookieDuration};
use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use serde::Deserialize;
use std::{future::Future, sync::OnceLock};
use talocart_platform::{identity::IdentityError, locale::Locale};

pub use errors::*;
mod errors;

tokio::task_local! { static LANGUAGE: Locale; }
const COOKIE: &str = "talocart_admin_language";
include!(concat!(env!("OUT_DIR"), "/catalogues.rs"));
struct Catalogue {
    code: &'static str,
    bundle: FluentBundle<FluentResource>,
}
static CATALOGUES: OnceLock<Result<Vec<Catalogue>, &'static str>> = OnceLock::new();
fn catalogues() -> Result<&'static [Catalogue], &'static str> {
    CATALOGUES
        .get_or_init(|| {
            RESOURCES
                .iter()
                .map(|(code, source)| {
                    let language: unic_langid::LanguageIdentifier =
                        code.parse().map_err(|_| "Invalid language identifier")?;
                    let resource = FluentResource::try_new((*source).into())
                        .map_err(|_| "Invalid Fluent catalogue")?;
                    let mut bundle = FluentBundle::new_concurrent(vec![language]);
                    bundle.set_use_isolating(false);
                    bundle
                        .add_resource(resource)
                        .map_err(|_| "Duplicate Fluent message")?;
                    Ok(Catalogue { code, bundle })
                })
                .collect()
        })
        .as_ref()
        .map(Vec::as_slice)
        .map_err(|e| *e)
}
/// Fail server startup when the shipped resources cannot be loaded.
pub fn validate_catalogues() -> Result<(), &'static str> {
    catalogues().map(|_| ())
}
pub async fn scope<T>(locale: Locale, future: impl Future<Output = T>) -> T {
    LANGUAGE.scope(locale, future).await
}
pub fn locale() -> &'static str {
    LANGUAGE
        .try_with(|locale| locale.code())
        .unwrap_or(Locale::DEFAULT_CODE)
}
fn format(code: &str, key: &str, args: Option<&FluentArgs<'_>>) -> Option<String> {
    let catalogue = catalogues().ok()?.iter().find(|c| c.code == code)?;
    let value = catalogue.bundle.get_message(key)?.value()?;
    let mut errors = Vec::new();
    let result = catalogue.bundle.format_pattern(value, args, &mut errors);
    if errors.is_empty() {
        Some(result.into_owned())
    } else {
        None
    }
}
pub(crate) fn with_args(key: &str, args: &FluentArgs<'_>) -> String {
    translated(key, Some(args))
}
fn translated(key: &str, args: Option<&FluentArgs<'_>>) -> String {
    format(locale(), key, args)
        .or_else(|| format(Locale::DEFAULT_CODE, key, args))
        .unwrap_or_else(|| {
            tracing::error!(message_id = key, "Missing or invalid translation");
            key.to_owned()
        })
}
pub(crate) fn selection_counts() -> std::ops::Range<i64> {
    0..13
}
pub fn decimal(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    value.replace(',', &text("format-decimal-separator"))
}
pub fn text(key: &str) -> String {
    translated(key, None)
}
pub fn count(key: &str, count: impl std::borrow::Borrow<i64>) -> String {
    let mut args = FluentArgs::new();
    args.set("count", *count.borrow());
    translated(key, Some(&args))
}
pub fn languages() -> &'static [(&'static str, &'static str)] {
    Locale::SUPPORTED
}
fn preference(headers: &HeaderMap, fallback: Locale) -> Locale {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let Ok(header) = header.to_str() else {
            return fallback;
        };
        for parsed in Cookie::split_parse(header) {
            let Ok(cookie) = parsed else {
                return fallback;
            };
            if cookie.name() == COOKIE {
                if found.is_some() {
                    return fallback;
                }
                found = Some(Locale::parse(cookie.value()).unwrap_or(fallback));
            }
        }
    }
    found.unwrap_or(fallback)
}
pub(crate) fn remember(
    response: &mut Response,
    code: &str,
    secure: bool,
) -> Result<(), IdentityError> {
    let locale = Locale::parse(code).ok_or(IdentityError::InvalidLocale)?;
    let cookie = Cookie::build((COOKIE, locale.code()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .max_age(CookieDuration::days(180))
        .build();
    let value =
        HeaderValue::from_str(&cookie.to_string()).map_err(|_| IdentityError::Unavailable)?;
    response.headers_mut().append(header::SET_COOKIE, value);
    Ok(())
}
pub(crate) fn language_header(response: &mut Response, code: &'static str) {
    // Media responses keep their existing private cache/ETag contract.
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("image/"))
    {
        return;
    }
    response
        .headers_mut()
        .insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(code));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
pub(crate) async fn language_context(
    State(state): State<AppState>,
    mut request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let document = super::error_pages::document_request(&request);
    let head = request.method() == axum::http::Method::HEAD;
    let path = request.uri().path();
    if path == "/setup" || path == "/admin" || path.starts_with("/admin/") {
        let fallback = if path == "/setup" {
            Locale::parse("en").unwrap_or_default()
        } else {
            Locale::default()
        };
        let mut locale = preference(request.headers(), fallback);
        if let Some(raw) = access::session_token(request.headers()) {
            match state.identity.session(&raw).await {
                Ok(Some(session)) => {
                    if session.administrator_id.is_some() {
                        locale = Locale::parse(&session.locale).unwrap_or_default();
                    }
                    request.extensions_mut().insert(session);
                }
                Ok(None) => {}
                Err(error) => {
                    let mut response = scope(locale, async {
                        super::error_pages::early(
                            &state,
                            access::IdentityErrorResponse::from(error).into_response(),
                            true,
                            document,
                            head,
                        )
                        .await
                    })
                    .await;
                    language_header(&mut response, locale.code());
                    return response;
                }
            }
        }
        let mut response = scope(locale, next.run(request)).await;
        if !response.headers().contains_key(header::CONTENT_LANGUAGE) {
            language_header(&mut response, locale.code());
        }
        response
    } else {
        super::store_language::context(&state, request, next).await
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LanguageForm {
    csrf: String,
    locale: String,
    return_to: String,
}
pub(crate) async fn choose_language(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LanguageForm>,
) -> Result<Response, access::IdentityErrorResponse> {
    let raw = access::session_token(&headers).ok_or(IdentityError::Forbidden)?;
    let session = state
        .identity
        .session(&raw)
        .await?
        .ok_or(IdentityError::Forbidden)?;
    session.check_csrf(&form.csrf)?;
    if !["/admin/login", "/admin/reset", "/setup"].contains(&form.return_to.as_str()) {
        return Err(IdentityError::InvalidLocale.into());
    }
    if session.administrator_id.is_some() {
        state
            .identity
            .update_locale(&session, &form.csrf, &form.locale)
            .await?;
    }
    let mut response = Redirect::to(&form.return_to).into_response();
    remember(&mut response, &form.locale, state.secure_cookies)?;
    Ok(response)
}

pub(crate) struct CodeChoice {
    pub code: &'static str,
    pub name: String,
    pub selected: bool,
}
pub(crate) fn code_choices(
    codes: &'static [&'static str],
    prefix: &str,
    selected: &str,
) -> Vec<CodeChoice> {
    let mut choices: Vec<_> = codes
        .iter()
        .map(|code| CodeChoice {
            code,
            name: i18n::text(&format!("{prefix}-{}", code.to_ascii_lowercase())),
            selected: *code == selected,
        })
        .collect();
    choices.sort_by(|a, b| a.name.cmp(&b.name));
    choices
}

#[cfg(test)]
#[path = "../../../scripts/locale-catalogues.rs"]
mod discovery;

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    #[test]
    fn languages_follow_complete_catalogue_files_including_english_only_and_new_languages() {
        let directory = std::env::temp_dir().join(format!(
            "talocart-locales-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(directory.join("en")).unwrap();
        std::fs::write(
            directory.join("en/admin.ftl"),
            "# language-name: English\nadmin-title = Title\n",
        )
        .unwrap();
        std::fs::write(directory.join("en/store.ftl"), "store-title = Store\n").unwrap();
        let languages = discovery::discover(&directory).unwrap();
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].0, "en");
        std::fs::create_dir(directory.join("fr")).unwrap();
        std::fs::write(
            directory.join("fr/admin.ftl"),
            "# language-name: Français\nadmin-title = Titre\n",
        )
        .unwrap();
        assert_eq!(
            discovery::discover(&directory).unwrap().len(),
            1,
            "Incomplete pair is unavailable"
        );
        std::fs::write(directory.join("fr/store.ftl"), "store-title = Boutique\n").unwrap();
        let languages = discovery::discover(&directory).unwrap();
        assert_eq!(languages.len(), 2);
        assert_eq!(languages[1].1, "Français");
        std::fs::write(directory.join("fr/store.ftl"), "other-key = Missing\n").unwrap();
        assert!(
            discovery::discover(&directory).is_err(),
            "Incomplete translations fail the build"
        );
        std::fs::remove_dir_all(directory.join("fr")).unwrap();
        std::fs::remove_file(directory.join("en/store.ftl")).unwrap();
        assert!(
            discovery::discover(&directory).is_err(),
            "A build needs at least one complete language"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    fn keys(source: &str) -> BTreeSet<&str> {
        source
            .lines()
            .filter_map(|line| {
                if line.starts_with('#') || line.starts_with(' ') {
                    return None;
                }
                line.split_once(" =").map(|(key, _)| key)
            })
            .collect()
    }
    #[test]
    fn shipped_catalogues_cover_templates_and_all_error_identifiers() {
        validate_catalogues().expect("Shipped Fluent resources are valid");
        let expected = keys(RESOURCES[0].1);
        assert!(expected.len() > 800);
        assert_eq!(Locale::SUPPORTED.len(), RESOURCES.len());
        for (code, source) in RESOURCES {
            assert!(Locale::parse(code).is_some());
            assert_eq!(keys(source), expected, "Catalogue completeness: {code}");
            for key in &expected {
                let mut args = FluentArgs::new();
                args.set("count", 2);
                args.set("id", "42");
                args.set("parent", "7");
                args.set("path", "synthetic/photo.jpg");
                args.set("label", "Example");
                args.set("name", "Example");
                args.set("query", "Example");
                for name in ["complete", "pending", "running", "failed"] {
                    args.set(name, "2");
                }
                assert!(
                    format(code, key, Some(&args)).is_some(),
                    "Valid message: {code}/{key}"
                );
            }
        }
        let mut sources = vec![include_str!("i18n/errors.rs").to_owned()];
        sources.extend(
            crate::test_templates::sources()
                .into_iter()
                .map(|(_, source)| source),
        );
        for theme in talocart_platform::themes::COMPILED {
            assert!(
                expected.contains(format!("theme-name-{}", theme.id()).as_str()),
                "Every compiled theme has a localized name"
            );
        }
        for source in sources {
            for prefix in ["text(\"", "count(\"", "Some(\""] {
                for tail in source.split(prefix).skip(1) {
                    let key = tail.split('"').next().expect("Message identifier");
                    assert!(
                        expected.contains(key),
                        "Referenced identifier exists: {key}"
                    );
                }
            }
        }
    }
    #[tokio::test]
    async fn request_scopes_plural_rules_and_cancellation_are_isolated() {
        assert_eq!(locale(), "it");
        let italian = scope(Locale::ITALIAN, async {
            tokio::task::yield_now().await;
            assert_eq!(locale(), "it");
            assert_eq!(count("admin-count-variants", 0), "0 varianti");
            assert_eq!(count("admin-count-variants", 1), "1 variante");
            count("admin-count-variants", 2)
        });
        let english = scope(Locale::ENGLISH, async {
            tokio::task::yield_now().await;
            assert_eq!(locale(), "en");
            assert_eq!(count("admin-count-variants", 0), "0 variants");
            assert_eq!(count("admin-count-variants", 1), "1 variant");
            assert_eq!(text("admin-save-profile"), "Save profile");

            assert_eq!(decimal("12,34"), "12.34");
            #[derive(Template)]
            #[template(source = "{{ message }}", ext = "html")]
            struct Diagnostic {
                message: String,
            }
            let escaped = Diagnostic {
                message: "unexpected <script>".to_owned(),
            }
            .render()
            .expect("Diagnostic renders");
            assert!(escaped.contains("&#60;script&#62;") && !escaped.contains("<script>"));
            count("admin-count-variants", 2)
        });
        let (it, en) = tokio::join!(italian, english);
        assert_eq!((it.as_str(), en.as_str()), ("2 varianti", "2 variants"));
        let cancelled = tokio::time::timeout(
            Duration::from_millis(1),
            scope(Locale::ENGLISH, std::future::pending::<()>()),
        )
        .await;
        assert!(cancelled.is_err());
        assert_eq!(locale(), "it");
        assert_eq!(text("admin-save-profile"), "Salva profilo");
        assert!(format("en", "admin-count-variants", None).is_none());
        assert_eq!(
            format("unknown", "admin-save-profile", None)
                .or_else(|| format("it", "admin-save-profile", None))
                .as_deref(),
            Some("Salva profilo")
        );
    }
    #[test]
    fn untrusted_preferences_and_secure_cookies() {
        for (raw, expected) in [
            ("", "it"),
            ("talocart_admin_language=en", "en"),
            ("talocart_admin_language=../../en", "it"),
            ("talocart_admin_language=EN", "it"),
            (
                "talocart_admin_language=en; talocart_admin_language=it",
                "it",
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::COOKIE,
                HeaderValue::from_str(raw).expect("Cookie header"),
            );
            assert_eq!(preference(&headers, Locale::default()).code(), expected);
        }
        for secure in [true, false] {
            let mut response = StatusCode::OK.into_response();
            remember(&mut response, "en", secure).expect("Supported locale");
            let cookie = response.headers()[header::SET_COOKIE]
                .to_str()
                .expect("Cookie header");
            assert!(
                cookie.contains("HttpOnly")
                    && cookie.contains("SameSite=Strict")
                    && cookie.contains("Max-Age=15552000")
            );
            assert_eq!(cookie.contains("Secure"), secure);
            assert!(remember(&mut response, "unknown", secure).is_err());
        }
    }
}
