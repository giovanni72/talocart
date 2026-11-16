use super::*;
use axum::{
    extract::{Form, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::{
    identity::IdentityError,
    storefront::{Settings, StorefrontError},
};
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/admin/settings/storefront", get(page).post(save))
        .route(
            "/admin/settings/storefront/languages",
            axum::routing::post(save_languages),
        )
}
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/robots.txt", get(robots))
        .route("/sitemap.xml", get(index))
        .route("/sitemap-catalog.xml", get(catalog_map))
        .route("/sitemap-products.xml", get(products_map))
}
#[derive(Template)]
#[template(path = "storefront-settings.html")]
struct Page<'a> {
    content: content::Editor,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    owner: bool,
    values: Settings,
    languages: LanguageValues,
    saved: bool,
    errors: Vec<&'static str>,
    error: String,
}
#[derive(Deserialize)]
struct Saved {
    saved: Option<String>,
}
async fn response(
    state: &AppState,
    session: &Session,
    values: Settings,
    saved: bool,
    error: Option<StorefrontError>,
    submitted_languages: Option<LanguageValues>,
    submitted_content: Option<&content::Submission>,
) -> Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let errors = match &error {
        Some(StorefrontError::Invalid(source_error)) => source_error.clone(),
        _ => vec![],
    };
    let theme = admin_shell.theme;
    let languages = submitted_languages.unwrap_or_else(|| LanguageValues {
        primary_locale: theme.primary_locale.clone(),
        active_locales: theme.active_locales.clone(),
    });
    let rows = state
        .storefront
        .translations(&state.identity, session)
        .await
        .map_err(content::identity_error)?;
    // GET values carry their read snapshot; historical POSTs have no language marker.
    let content_primary = if submitted_content.is_some() {
        &theme.primary_locale
    } else {
        &values.primary_locale
    };
    let content = content::Editor::new(
        content_primary,
        if submitted_content.is_some() {
            &theme.active_locales
        } else {
            &values.active_locales
        },
        talocart_platform::storefront::Storefront::CONTENT,
        rows,
        submitted_content,
        error.as_ref().and_then(|source_error| {
            if let StorefrontError::Content(source_error) = source_error {
                Some(source_error)
            } else {
                None
            }
        }),
        "theme-content",
    );
    let mut r = render(Page {
        content,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "settings",
        theme,
        languages,
        nav: admin_shell.nav,
        account: admin_shell.account,
        owner: session.allows(Permission::SettingsWrite),
        values,
        saved,
        errors,
        error: error
            .as_ref()
            .map(i18n::storefront_error)
            .unwrap_or_default(),
    });
    if error.is_some() {
        *r.status_mut() = if let Some(StorefrontError::Content(source_error)) = &error {
            content::status(source_error)
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
    }
    Ok(r)
}
async fn page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<Saved>,
) -> Result<Response, access::IdentityErrorResponse> {
    unified_settings::show(
        &state,
        &session,
        "storefront",
        q.saved.as_deref() == Some("1"),
    )
    .await
}
#[derive(Deserialize)]
struct Input {
    #[serde(flatten)]
    content: content::Submission,
    csrf: String,
    palette: String,
    typography: String,
    density: String,
    #[serde(default)]
    show_hero: bool,
    #[serde(default)]
    show_featured: bool,
    heading: String,
    introduction: String,
    seo_title: String,
    seo_description: String,
    public_origin: String,
    #[serde(default)]
    indexing_enabled: bool,
}
impl Input {
    fn settings(&self) -> Settings {
        Settings {
            palette: self.palette.clone(),
            typography: self.typography.clone(),
            density: self.density.clone(),
            show_hero: self.show_hero,
            show_featured: self.show_featured,
            heading: self.heading.clone(),
            introduction: self.introduction.clone(),
            seo_title: self.seo_title.clone(),
            seo_description: self.seo_description.clone(),
            public_origin: self.public_origin.clone(),
            indexing_enabled: self.indexing_enabled,
            ..Settings::default()
        }
    }
}

async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(input): Form<Input>,
) -> Result<Response, access::IdentityErrorResponse> {
    match state
        .storefront
        .save_with_content(
            &session,
            &input.csrf,
            input.settings(),
            content::draft(&input.content).as_ref(),
        )
        .await
    {
        Ok(()) => Ok(Redirect::to("/admin/settings/storefront?saved=1").into_response()),
        Err(StorefrontError::Technical(error)) => Err(error.into()),
        Err(StorefrontError::Access(error)) => Err(error.into()),
        Err(StorefrontError::Identity(error)) => Err(error.into()),
        Err(error) => {
            response(
                &state,
                &session,
                input.settings(),
                false,
                Some(error),
                None,
                Some(&input.content),
            )
            .await
        }
    }
}
struct LanguageValues {
    primary_locale: String,
    active_locales: Vec<String>,
}
impl LanguageValues {
    fn selected(&self, code: &str) -> bool {
        self.active_locales.iter().any(|selected| selected == code)
    }
}
async fn save_languages(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, access::IdentityErrorResponse> {
    let mut csrf = None;
    let mut primary = None;
    let mut active = Vec::new();
    for (key, value) in pairs {
        match key.as_str() {
            "csrf" if csrf.is_none() => csrf = Some(value),
            "primary_locale" if primary.is_none() => primary = Some(value),
            "locale" => active.push(value),
            _ => return Err(IdentityError::Forbidden.into()),
        }
    }
    let csrf = csrf.ok_or(IdentityError::Forbidden)?;
    let primary = primary.unwrap_or_default();
    match state
        .storefront
        .save_languages(&session, &csrf, &primary, &active)
        .await
    {
        Ok(()) => Ok(Redirect::to("/admin/settings/storefront?saved=1").into_response()),
        Err(StorefrontError::Technical(error)) => Err(error.into()),
        Err(StorefrontError::Access(error)) => Err(error.into()),
        Err(StorefrontError::Identity(error)) => Err(error.into()),
        Err(error) => {
            response(
                &state,
                &session,
                store_language::settings(&state).await?,
                false,
                Some(error),
                Some(LanguageValues {
                    primary_locale: primary,
                    active_locales: active,
                }),
                None,
            )
            .await
        }
    }
}

pub async fn featured(
    state: &AppState,
    settings: &Settings,
) -> Result<Vec<product_card::ProductCard>, IdentityError> {
    let localized_state = state.localized();
    let state = &localized_state;
    if !settings.show_featured {
        return Ok(vec![]);
    }
    let entries = state
        .products
        .public_featured()
        .await
        .map_err(identity_error)?;
    entries
        .into_iter()
        .map(|entry| product_card::ProductCard::try_from(entry).map_err(identity_error))
        .collect()
}
fn identity_error(error: talocart_commerce::products::ProductError) -> IdentityError {
    match error {
        talocart_commerce::products::ProductError::Technical(error) => error.into(),
        talocart_commerce::products::ProductError::Access(error) => error.into(),
        talocart_commerce::products::ProductError::Identity(error) => error,
        _ => IdentityError::Unavailable,
    }
}
async fn robots(State(state): State<AppState>) -> Result<Response, access::IdentityErrorResponse> {
    let s = store_language::settings(&state).await?;
    // Crawlers must be allowed to read noindex on the actual resources.
    let discovery = if s.indexing_enabled {
        format!(
            "Sitemap: {}\n",
            s.canonical(&state.public_origin, "/sitemap.xml")
        )
    } else {
        "# Indexing disabled via X-Robots-Tag on resource responses.\n".into()
    };
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("User-agent: *\nDisallow:\n{discovery}"),
    )
        .into_response())
}
#[derive(Template)]
#[template(path = "sitemap.xml", escape = "html")]
struct Sitemap {
    urls: Vec<String>,
    index: bool,
}
fn xml(urls: Vec<String>, index: bool) -> Response {
    let mut r = render(Sitemap { urls, index });
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    r
}
async fn index(State(state): State<AppState>) -> Result<Response, access::IdentityErrorResponse> {
    let s = store_language::settings(&state).await?;
    let count = state
        .products
        .sitemap_count()
        .await
        .map_err(identity_error)?;
    let pages = (count + 999) / 1000;
    // Protocol limit for a sitemap index; never silently truncate a large catalog.
    if pages >= 50_000 {
        return Err(IdentityError::Unavailable.into());
    }
    let mut urls = vec![s.canonical(&state.public_origin, "/sitemap-catalog.xml")];
    urls.extend((1..=pages).map(|p| {
        s.canonical(
            &state.public_origin,
            &format!("/sitemap-products.xml?page={p}"),
        )
    }));
    Ok(xml(urls, true))
}
async fn catalog_map(
    State(state): State<AppState>,
) -> Result<Response, access::IdentityErrorResponse> {
    let s = store_language::settings(&state).await?;
    let languages = s.languages().map_err(|_| IdentityError::Unavailable)?;
    let mut urls = Vec::new();
    let pages = super::pages::service(&state)
        .sitemap()
        .await
        .map_err(super::pages::identity_error)?;
    for locale in languages.active() {
        let prefix = if *locale == languages.primary() {
            String::new()
        } else {
            format!("/{}", locale.code())
        };
        urls.push(s.canonical(
            &state.public_origin,
            if prefix.is_empty() { "/" } else { &prefix },
        ));
        urls.push(s.canonical(&state.public_origin, &format!("{prefix}/catalog")));
        for slug in &pages {
            urls.push(s.canonical(&state.public_origin, &format!("{prefix}/pages/{slug}")));
        }
    }
    Ok(xml(urls, false))
}
#[derive(Deserialize)]
struct MapPage {
    #[serde(default = "first_map_page")]
    page: i64,
}
fn first_map_page() -> i64 {
    1
}
async fn products_map(
    State(state): State<AppState>,
    Query(query): Query<MapPage>,
) -> Result<Response, products::ProductResponseError> {
    let s = store_language::settings(&state).await?;
    let slugs = state.products.sitemap_products(query.page).await?;
    if slugs.is_empty() {
        return Err(talocart_commerce::products::ProductError::NotFound.into());
    }
    let languages = s.languages().map_err(|_| IdentityError::Unavailable)?;
    let mut urls = Vec::new();
    for slug in slugs {
        for locale in languages.active() {
            let prefix = if *locale == languages.primary() {
                String::new()
            } else {
                format!("/{}", locale.code())
            };
            urls.push(s.canonical(&state.public_origin, &format!("{prefix}/products/{slug}")));
        }
    }
    Ok(xml(urls, false))
}
