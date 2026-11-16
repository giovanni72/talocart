//! Public language comes from the URL and one request snapshot of store settings.
//! Neutral assets and administration do not participate in public URL routing.
use axum::{
    body::Body,
    http::{HeaderValue, Method, Request, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use talocart_platform::{
    identity::IdentityError,
    locale::Locale,
    storefront::{Languages, Settings},
    themes::Theme,
};

struct Context {
    uri: Uri,
    settings: Settings,
    languages: Languages,
    theme: Theme,
}
tokio::task_local! { static PUBLIC_CONTEXT: Context; }

fn split_language(path: &str) -> (Option<Locale>, &str) {
    for (code, _) in Locale::SUPPORTED {
        if let Some(tail) = path.strip_prefix(&format!("/{code}")) {
            if tail.is_empty() || tail == "/" {
                return (Locale::parse(code), "/");
            }
            if tail.starts_with('/') {
                return (Locale::parse(code), tail);
            }
        }
    }
    (None, path)
}
fn public_path(path: &str) -> bool {
    super::customers::public_path(path)
        || matches!(
            path,
            "/" | "/status"
                | "/catalog"
                | "/brands"
                | "/search"
                | "/cart"
                | "/cart/add"
                | "/cart/merge"
                | "/cart/coupon"
                | "/checkout"
                | "/checkout/methods"
        )
        || ["/catalog/", "/brands/", "/products/", "/pages/"]
            .iter()
            .any(|prefix| {
                path.strip_prefix(prefix)
                    .is_some_and(|slug| !slug.is_empty() && !slug.contains('/'))
            })
        || path.strip_prefix("/checkout/orders/").is_some_and(|r| {
            let reference = ["/pay", "/payment", "/session", "/payment-status"]
                .iter()
                .find_map(|suffix| r.strip_suffix(suffix))
                .unwrap_or(r);
            !reference.is_empty() && !reference.contains('/')
        })
        || path
            .strip_prefix("/cart/items/")
            .is_some_and(|id| id.parse::<i64>().is_ok())
        || path
            .strip_prefix("/search/")
            .is_some_and(|term| !term.is_empty())
}
fn policy() -> Languages {
    PUBLIC_CONTEXT
        .try_with(|context| context.languages.clone())
        .unwrap_or_default()
}
fn locale(code: &str, languages: &Languages) -> Locale {
    Locale::parse(code)
        .filter(|locale| languages.contains(*locale))
        .unwrap_or_else(|| languages.primary())
}
fn language_prefix(locale: Locale, languages: &Languages) -> String {
    if locale == languages.primary() {
        String::new()
    } else {
        format!("/{}", locale.code())
    }
}
fn localized(value: &str, code: &str, languages: &Languages) -> String {
    let (path, query) = value
        .split_once('?')
        .map_or((value, None), |(path, query)| (path, Some(query)));
    let (_, path) = split_language(path);
    let prefix = language_prefix(locale(code, languages), languages);
    let mut result = if path == "/" && !prefix.is_empty() {
        prefix
    } else {
        format!("{prefix}{path}")
    };
    if let Some(query) = query {
        result.push('?');
        result.push_str(query);
    }
    result
}
pub(crate) fn routes() -> axum::Router<super::AppState> {
    Locale::SUPPORTED
        .iter()
        .fold(axum::Router::new(), |routes, (code, _)| {
            routes.merge(super::routing::public_routes(&format!("/{code}")))
        })
}
/// Public page handlers reuse the same settings snapshot as URL resolution.
pub(crate) fn snapshot_settings() -> Option<Settings> {
    PUBLIC_CONTEXT
        .try_with(|context| context.settings.clone())
        .ok()
}
pub(crate) async fn settings(state: &super::AppState) -> Result<Settings, IdentityError> {
    if let Some(settings) = snapshot_settings() {
        Ok(settings)
    } else {
        state.storefront.settings().await
    }
}
fn signature_for(languages: &Languages, theme: &str) -> String {
    format!(
        "{};{};theme={}",
        languages.primary().code(),
        languages
            .active()
            .iter()
            .map(|locale| locale.code())
            .collect::<Vec<_>>()
            .join(","),
        theme
    )
}
pub(crate) fn theme_id() -> &'static str {
    PUBLIC_CONTEXT
        .try_with(|context| context.theme)
        .unwrap_or(Theme::Default)
        .id()
}
pub(crate) fn signature() -> String {
    signature_for(&policy(), theme_id())
}
pub(crate) fn content_language() -> talocart_platform::content::Language {
    talocart_platform::content::Language::new(
        Locale::parse(super::i18n::locale()).unwrap_or_default(),
        policy().primary(),
    )
}
pub(crate) fn primary() -> &'static str {
    policy().primary().code()
}
pub(crate) fn prefix(code: &str) -> String {
    let languages = policy();
    language_prefix(locale(code, &languages), &languages)
}
pub(crate) fn languages() -> Vec<(&'static str, &'static str, String)> {
    let languages = policy();
    Locale::SUPPORTED
        .iter()
        .filter_map(|(code, name)| {
            Locale::parse(code)
                .filter(|locale| languages.contains(*locale))
                .map(|locale| (*code, *name, language_prefix(locale, &languages)))
        })
        .collect()
}
pub(crate) fn product_path(slug: impl AsRef<str>) -> String {
    path(format!("/products/{}", slug.as_ref()))
}
pub(crate) fn status_path(value: &str) -> String {
    if value.starts_with("/admin/") {
        value.into()
    } else {
        path(value)
    }
}
fn banner_destination(value: &str, code: &str, languages: &Languages) -> String {
    let (without_fragment, fragment) = value
        .split_once('#')
        .map_or((value, None), |(p, f)| (p, Some(f)));
    let path = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(p, _)| p);
    let (_, base) = split_language(path);
    if !public_path(base) {
        return value.to_owned();
    }
    let mut result = localized(without_fragment, code, languages);
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }
    result
}
pub(crate) fn banner_path(value: &str) -> String {
    banner_destination(value, super::i18n::locale(), &policy())
}

pub(crate) fn path(value: impl AsRef<str>) -> String {
    localized(value.as_ref(), super::i18n::locale(), &policy())
}
pub(crate) fn request_target() -> String {
    PUBLIC_CONTEXT
        .try_with(|c| {
            c.uri
                .path_and_query()
                .map_or("/account", |p| p.as_str())
                .to_owned()
        })
        .unwrap_or_else(|_| "/account".into())
}
pub(crate) fn language_link(code: &str) -> String {
    PUBLIC_CONTEXT
        .try_with(|context| {
            let (_, base) = split_language(context.uri.path());
            // Native mutation errors render at their POST URL. Language links
            // must navigate to the readable resource, never replay a mutation.
            let target = if (base == "/cart/add" || base == "/cart/merge" || base == "/cart/coupon")
                || base.starts_with("/cart/items/")
            {
                "/cart"
            } else if base.starts_with("/checkout/orders/")
                && (base.ends_with("/pay") || base.ends_with("/session"))
            {
                base.strip_suffix("/pay")
                    .or_else(|| base.strip_suffix("/session"))
                    .unwrap_or(base)
            } else {
                context.uri.path_and_query().map_or("/", |p| p.as_str())
            };
            localized(target, code, &context.languages)
        })
        .unwrap_or_else(|_| localized("/", code, &Languages::default()))
}
fn decorate(response: &mut Response, locale: Locale) {
    response.headers_mut().insert(
        header::CONTENT_LANGUAGE,
        HeaderValue::from_static(locale.code()),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
pub(crate) async fn context(
    state: &super::AppState,
    request: Request<Body>,
    next: Next,
) -> Response {
    let document = super::error_pages::document_request(&request);
    let head = request.method() == Method::HEAD;
    let uri = request.uri().clone();
    let (specified, base) = split_language(uri.path());
    let index_candidate = super::indexing::candidate(&request, base);
    let known = request
        .extensions()
        .get::<super::routing::PublicRoute>()
        .is_some();
    let neutral = ["/assets", "/media", "/branding", "/health"]
        .iter()
        .any(|prefix| uri.path() == *prefix || uri.path().starts_with(&format!("{prefix}/")))
        || matches!(
            uri.path(),
            "/robots.txt"
                | "/sitemap.xml"
                | "/sitemap-catalog.xml"
                | "/sitemap-products.xml"
                | "/favicon.svg"
        );
    if neutral && specified.is_none() && !index_candidate {
        return next.run(request).await;
    }
    let snapshot = match state.storefront.settings().await {
        Ok(settings) => match (settings.languages(), Theme::parse(&settings.theme_id)) {
            (Ok(languages), Ok(theme)) => Ok((settings, languages, theme)),
            _ => Err(IdentityError::Unavailable),
        },
        Err(error) => Err(error),
    };
    let (settings, languages, theme) = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            if !known {
                return next.run(request).await;
            }
            let locale = specified.unwrap_or_default();
            let mut response = super::i18n::scope(locale, async {
                super::error_pages::early(
                    state,
                    super::access::IdentityErrorResponse::from(error).into_response(),
                    false,
                    document,
                    head,
                )
                .await
            })
            .await;
            decorate(&mut response, locale);
            return response;
        }
    };
    // A fresh primary-only installation has never published /{primary} URLs.
    // Do not manufacture aliases: absent prefixes are ordinary localized 404s.
    if known
        && specified.is_some_and(|locale| {
            let secondary = locale != languages.primary() && languages.contains(locale);
            let published = settings
                .published_prefixes
                .iter()
                .any(|code| code == locale.code());
            !secondary && !published
        })
    {
        let primary = languages.primary();
        let mut response = PUBLIC_CONTEXT
            .scope(
                Context {
                    uri,
                    settings,
                    languages,
                    theme,
                },
                super::i18n::scope(primary, async {
                    super::error_pages::public_not_found(state, document, head).await
                }),
            )
            .await;
        decorate(&mut response, primary);
        return response;
    }
    let current = specified
        .filter(|locale| languages.contains(*locale))
        .unwrap_or_else(|| languages.primary());
    let canonical_path = localized(uri.path(), current.code(), &languages);
    let partial = request
        .headers()
        .get("hx-request")
        .is_some_and(|value| value == "true");
    let stale = partial
        && request
            .headers()
            .get("x-talocart-store-policy")
            .is_some_and(|value| {
                value.as_bytes() != signature_for(&languages, &settings.theme_id).as_bytes()
            });
    // Unknown/neutral paths never redirect. HTMX needs a 200 + HX-Redirect:
    // browsers follow 301 before HTMX can process the original response headers.
    if known
        && matches!(*request.method(), Method::GET | Method::HEAD)
        && (canonical_path != uri.path() || stale)
    {
        // A preview endpoint is a fragment even on normal GET. After a policy
        // change, navigate to the full cart rather than rendering that fragment
        // as a document. Other readable URLs retain their filters/query.
        let target = localized(
            if stale && base == "/cart" {
                "/cart"
            } else {
                uri.path_and_query().map_or("/", |p| p.as_str())
            },
            current.code(),
            &languages,
        );
        let mut response = if partial {
            match HeaderValue::from_str(&target) {
                Ok(value) => {
                    let mut response = Response::new(Body::empty());
                    response.headers_mut().insert("hx-redirect", value);
                    response
                }
                Err(_) => super::access::IdentityErrorResponse::from(IdentityError::Unavailable)
                    .into_response(),
            }
        } else {
            let mut response = axum::response::Redirect::permanent(&target).into_response();
            *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
            response
        };
        decorate(&mut response, current);
        return response;
    }
    let mut settings = settings;
    if known
        && let Err(error) = state
            .storefront
            .localize(
                &mut settings,
                talocart_platform::content::Language::new(current, languages.primary()),
            )
            .await
    {
        let mut response = super::i18n::scope(current, async {
            super::error_pages::early(
                state,
                super::access::IdentityErrorResponse::from(error).into_response(),
                false,
                document,
                head,
            )
            .await
        })
        .await;
        decorate(&mut response, current);
        return response;
    }
    let indexing_enabled = settings.indexing_enabled;
    let mut response = PUBLIC_CONTEXT
        .scope(
            Context {
                uri,
                settings,
                languages,
                theme,
            },
            super::i18n::scope(
                current,
                super::customers::context(state, request, next, known),
            ),
        )
        .await;
    if !neutral {
        decorate(&mut response, current);
    }
    super::indexing::mark(&mut response, indexing_enabled, index_candidate);
    response
}
pub(crate) fn alternate(canonical: &str, code: &str) -> String {
    let Some((scheme, remainder)) = canonical.split_once("://") else {
        return String::new();
    };
    let Some((authority, rest)) = remainder.split_once('/') else {
        return String::new();
    };
    format!(
        "{scheme}://{authority}{}",
        localized(&format!("/{rest}"), code, &policy())
    )
}
#[cfg(test)]
mod tests {
    #[test]
    fn banner_destinations_preserve_neutral_paths_and_fragments() {
        let dual = Languages::new("en", &["en".into(), "it".into()]).unwrap();
        for path in [
            "/media/image/1/large",
            "/admin/login",
            "/assets/file.pdf",
            "https://example.test/x",
            "/custom",
        ] {
            assert_eq!(banner_destination(path, "it", &dual), path);
        }
        assert_eq!(
            banner_destination("/catalog?q=a%26b#offers", "it", &dual),
            "/it/catalog?q=a%26b#offers"
        );
        assert_eq!(banner_destination("/en#offers", "it", &dual), "/it#offers");
    }

    use super::*;
    #[tokio::test]
    async fn primary_secondary_and_single_language_paths_preserve_encoded_state() {
        let dual = Languages::new("en", &["it".into(), "en".into()]).unwrap();
        let single = Languages::new("en", &["en".into()]).unwrap();
        assert_eq!(
            localized("/en/search/a%2Fb?brand=a%26b&page=2", "en", &dual),
            "/search/a%2Fb?brand=a%26b&page=2"
        );
        assert_eq!(localized("/catalog", "it", &dual), "/it/catalog");
        assert_eq!(localized("/en", "en", &single), "/");
        assert_eq!(
            localized("/it/products/test?photo=2", "it", &single),
            "/products/test?photo=2"
        );
        assert_eq!(localized("/enough", "it", &Languages::default()), "/enough");
        for path in [
            "/admin",
            "/media/image/1/card",
            "/health/ready",
            "//external.test",
            "/products/one/two",
        ] {
            assert!(!public_path(path));
        }
        let a = PUBLIC_CONTEXT.scope(
            Context {
                uri: "/en/search/a%2Fb?page=2".parse().unwrap(),
                settings: Settings {
                    theme_id: "essential".into(),
                    ..Settings::default()
                },
                languages: Languages::default(),
                theme: Theme::Essential,
            },
            super::super::i18n::scope(Locale::ENGLISH, async {
                tokio::task::yield_now().await;
                assert_eq!(path("/catalog"), "/en/catalog");
                assert_eq!(theme_id(), "essential");
                assert_eq!(language_link("it"), "/search/a%2Fb?page=2");
            }),
        );
        let b = PUBLIC_CONTEXT.scope(
            Context {
                uri: "/search/a%2Fb?page=2".parse().unwrap(),
                settings: Settings::default(),
                languages: single,
                theme: Theme::Default,
            },
            super::super::i18n::scope(Locale::ENGLISH, async {
                tokio::task::yield_now().await;
                assert_eq!(path("/catalog"), "/catalog");
                assert_eq!(theme_id(), "default");
                assert_eq!(languages().len(), 1);
                assert_eq!(
                    alternate("https://shop.test/catalog", "en"),
                    "https://shop.test/catalog"
                );
            }),
        );
        tokio::join!(a, b);
        assert_eq!(path("/catalog"), "/catalog");
        assert_eq!(theme_id(), "default");
    }
}
