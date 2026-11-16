//! Native single settings form, progressively enhanced with local tabs.
use super::*;
use axum::{
    extract::{Form, OriginalUri, Query},
    response::Redirect,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use talocart_commerce::{
    settings::{self, Error},
    taxes::Preferences,
};
use talocart_platform::{
    identity::{IdentityError, SettingsInput},
    media::library::MediaLibrary,
    regional,
    storefront::Settings,
};
type Result<T> = std::result::Result<T, access::IdentityErrorResponse>;

#[derive(Clone, Default)]
pub(super) struct Fields {
    values: BTreeMap<String, String>,
    countries: Vec<String>,
    locales: Vec<String>,
}
impl Fields {
    fn get(&self, key: &str) -> String {
        self.values.get(key).cloned().unwrap_or_default()
    }
    fn set(&mut self, key: &str, value: impl ToString) {
        self.values.insert(key.into(), value.to_string());
    }
    fn parse(pairs: Vec<(String, String)>) -> std::result::Result<Self, IdentityError> {
        let mut fields = Self::default();
        for (k, v) in pairs {
            match k.as_str() {
                "purchase_country" => fields.countries.push(v),
                "locale" => fields.locales.push(v),
                _ if KEYS.contains(&k.as_str()) || content_key(&k) => {
                    if fields.values.insert(k, v).is_some() {
                        return Err(IdentityError::Invalid);
                    }
                }
                _ => return Err(IdentityError::Invalid),
            }
        }
        Ok(fields)
    }
    fn baseline(&self) -> String {
        let mut pairs: Vec<_> = self
            .values
            .iter()
            .filter(|(k, _)| TRACKED.contains(&k.as_str()) || content_key(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        pairs.extend(
            self.countries
                .iter()
                .map(|v| ("purchase_country".into(), v.clone())),
        );
        pairs.extend(self.locales.iter().map(|v| ("locale".into(), v.clone())));
        pairs.sort();
        pairs
            .iter()
            .map(|(k, v)| format!("{}={}", catalog::encoded(k), catalog::encoded(v)))
            .collect::<Vec<_>>()
            .join("&")
    }
    fn general(&self) -> SettingsInput {
        SettingsInput {
            name: self.get("name"),
            legal_name: self.get("legal_name"),
            contact_email: self.get("contact_email"),
            phone: self.get("phone"),
            address_line1: self.get("address_line1"),
            address_line2: self.get("address_line2"),
            postal_code: self.get("postal_code"),
            city: self.get("city"),
            province: self.get("province"),
            country: self.get("country"),
        }
    }
    fn storefront(&self) -> Settings {
        Settings {
            theme_id: self.get("theme_id"),
            palette: self.get("palette"),
            typography: self.get("typography"),
            density: self.get("density"),
            show_hero: self.get("show_hero") == "true",
            show_featured: self.get("show_featured") == "true",
            indexing_enabled: self.get("indexing_enabled") == "true",
            maintenance_enabled: self.get("maintenance_enabled") == "true",
            heading: self.get("heading"),
            introduction: self.get("introduction"),
            seo_title: self.get("seo_title"),
            seo_description: self.get("seo_description"),
            public_origin: self.get("public_origin"),
            ..Settings::default()
        }
    }
    fn preferences(&self) -> Preferences {
        Preferences {
            currency: self.get("currency"),
            country: self.get("tax_country"),
            mode: self.get("mode"),
            default_class_id: self.get("default_class_id"),
        }
    }
    fn input(&self, fallback: talocart_platform::config::PackageInstall) -> settings::Input {
        settings::Input {
            module_install: self.values.get("module_install_mode").map(|mode| {
                settings::ModuleInstall {
                    mode: mode.clone(),
                    fallback,
                    password: self.get("module_install_password"),
                }
            }),
            general: self.general(),
            storefront: self.storefront(),
            content: content::draft(&self.values),
            primary: self.get("primary_locale"),
            locales: self.locales.clone(),
            time_zone: self.get("time_zone"),
            date_format: self.get("date_format"),
            regional_revision: self.get("regional_revision").parse().unwrap_or(-1),
            fiscal_revision: self.get("fiscal_revision").parse().unwrap_or(-1),
            revision: self.get("settings_revision"),
            preferences: self.preferences(),
            countries: self.countries.clone(),
            logo: self.get("logo"),
            favicon: self.get("favicon"),
        }
    }
}
const TAX_FIELDS: &[&str] = &["currency", "tax_country", "mode", "default_class_id"];
const TRACKED: &[&str] = &[
    "name",
    "legal_name",
    "contact_email",
    "phone",
    "address_line1",
    "address_line2",
    "postal_code",
    "city",
    "province",
    "country",
    "time_zone",
    "date_format",
    "theme_id",
    "palette",
    "typography",
    "density",
    "show_hero",
    "show_featured",
    "indexing_enabled",
    "maintenance_enabled",
    "module_install_mode",
    "heading",
    "introduction",
    "seo_title",
    "seo_description",
    "public_origin",
    "primary_locale",
    "currency",
    "tax_country",
    "mode",
    "default_class_id",
    "logo",
    "favicon",
];
const KEYS: &[&str] = &[
    "module_install_password",
    "csrf",
    "settings_revision",
    "regional_revision",
    "fiscal_revision",
    "tab",
    "action",
    "media_query",
    "media_page",
    "media_target",
    "media_editor",
    "picker_image",
    "media_folder",
    "media_folders_page",
    "media_upload_failed",
    "media_format",
    "media_usage",
    "media_sort",
    "media_selected",
    "media_saved",
    "name",
    "legal_name",
    "contact_email",
    "phone",
    "address_line1",
    "address_line2",
    "postal_code",
    "city",
    "province",
    "country",
    "time_zone",
    "date_format",
    "theme_id",
    "palette",
    "typography",
    "density",
    "show_hero",
    "show_featured",
    "indexing_enabled",
    "maintenance_enabled",
    "module_install_mode",
    "heading",
    "introduction",
    "seo_title",
    "seo_description",
    "public_origin",
    "primary_locale",
    "currency",
    "tax_country",
    "mode",
    "default_class_id",
    "logo",
    "favicon",
];
fn content_key(key: &str) -> bool {
    matches!(key, "content_primary_locale" | "content_active_locales")
        || ["it", "en"].iter().any(|l| {
            ["name", "description", "seo_title", "seo_description"]
                .iter()
                .any(|f| key == format!("content_{l}_{f}"))
        })
}
#[derive(Deserialize)]
pub struct Saved {
    saved: Option<String>,
}
#[derive(Template)]
#[template(path = "settings-editor.html")]
struct Page<'a> {
    store_name: String,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    owner: bool,
    module_owner: bool,
    can_save: bool,
    tax_write: bool,
    saved: bool,
    error: String,
    error_panel: &'static str,
    baseline: String,
    fields: &'a Fields,
    tab: String,
    sections: Vec<administration::SettingsSection<'a>>,
    regional: regional::Settings,
    regional_preview: String,
    time_zones: Vec<String>,
    values: Settings,
    content: content::Editor,
    errors: Vec<&'static str>,
    languages: Languages,
    preferences: Preferences,
    default_known: bool,
    currencies: Vec<i18n::CodeChoice>,
    tax_countries: Vec<i18n::CodeChoice>,
    countries: Vec<i18n::CodeChoice>,
    defaults: Vec<TaxChoice>,
    images: Vec<Image>,
}
struct TaxChoice {
    id: i64,
    name: String,
    active: bool,
    selected: bool,
}
struct Languages {
    primary_locale: String,
    active_locales: Vec<String>,
}
impl Languages {
    fn selected(&self, code: &str) -> bool {
        self.active_locales.iter().any(|c| c == code)
    }
}
struct Image {
    key: &'static str,
    selected: String,
    preview: String,
    name: String,
}
fn panel(error: &Error) -> &'static str {
    match error {
        Error::General(_) | Error::Regional(_) => "general",
        Error::Storefront(_) => "storefront",
        Error::Taxes(_) => "taxes",
        Error::Countries(_) => "countries",
        Error::Branding(..) => "appearance",
        _ => "general",
    }
}
fn failure(error: &Error) -> (StatusCode, String) {
    match error {
        Error::Technical(source_error) => (
            access::technical_status(source_error),
            i18n::technical_error(source_error),
        ),
        Error::Access(source_error) => (
            access::access_status(source_error),
            i18n::access_error(source_error),
        ),
        Error::Conflict => (StatusCode::CONFLICT, i18n::text("settings-conflict")),
        Error::Identity(source_error)
        | Error::General(source_error)
        | Error::Branding(_, source_error) => (
            access::IdentityErrorResponse::status_for(source_error),
            i18n::identity_error(source_error),
        ),

        Error::Regional(regional::RegionalError::Technical(source_error)) => (
            access::technical_status(source_error),
            i18n::technical_error(source_error),
        ),
        Error::Regional(regional::RegionalError::Access(source_error)) => (
            access::access_status(source_error),
            i18n::access_error(source_error),
        ),
        Error::Storefront(talocart_platform::storefront::StorefrontError::Technical(
            source_error,
        )) => (
            access::technical_status(source_error),
            i18n::technical_error(source_error),
        ),
        Error::Storefront(talocart_platform::storefront::StorefrontError::Access(source_error)) => {
            (
                access::access_status(source_error),
                i18n::access_error(source_error),
            )
        }
        Error::Countries(talocart_commerce::countries::CountryError::Technical(source_error)) => (
            access::technical_status(source_error),
            i18n::technical_error(source_error),
        ),
        Error::Countries(talocart_commerce::countries::CountryError::Access(source_error)) => (
            access::access_status(source_error),
            i18n::access_error(source_error),
        ),
        Error::Regional(regional::RegionalError::Identity(source_error)) => (
            access::IdentityErrorResponse::status_for(source_error),
            i18n::identity_error(source_error),
        ),
        Error::Regional(regional::RegionalError::Conflict) => {
            (StatusCode::CONFLICT, i18n::text("regional-conflict"))
        }
        Error::Regional(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            i18n::text("regional-invalid"),
        ),
        Error::Storefront(source_error) => (
            if let talocart_platform::storefront::StorefrontError::Content(source_error) =
                source_error
            {
                content::status(source_error)
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            },
            i18n::storefront_error(source_error),
        ),
        Error::Taxes(source_error) => (taxes::status(source_error), i18n::tax_error(source_error)),
        Error::Countries(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            i18n::text("admin-purchase-countries-invalid"),
        ),
    }
}

pub async fn page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    OriginalUri(uri): OriginalUri,
    Query(q): Query<Saved>,
) -> Result<Response> {
    let tab = uri
        .path()
        .rsplit('/')
        .next()
        .filter(|v| ["appearance", "storefront", "taxes", "countries"].contains(v))
        .unwrap_or("general");
    response(
        &state,
        &session,
        None,
        None,
        tab,
        q.saved.as_deref() == Some("1"),
    )
    .await
}

pub(super) async fn response(
    state: &AppState,
    session: &Session,
    submitted: Option<Fields>,
    error: Option<Error>,
    tab: &str,
    saved: bool,
) -> Result<Response> {
    let settings::Snapshot {
        store,
        theme,
        regional,
        revision,
        fiscal_revision,
        countries: selected_countries,
        preferences,
        defaults,
        translations: rows,
        branding,
        logo,
        regional_preview,
        time_zones,
    } = settings::snapshot(&state.pool, &state.identity, session)
        .await
        .map_err(access::IdentityErrorResponse::from)?;
    let mut stored = Fields::default();
    if theme.maintenance_enabled {
        stored.set("maintenance_enabled", "true");
    }
    if session.is_owner() {
        stored.set(
            "module_install_mode",
            if state.panel_install().await? {
                "panel"
            } else {
                "cli"
            },
        );
    }
    for f in talocart_platform::identity::SettingsField::ALL {
        stored.set(f.key(), store.values.value(f));
    }
    for (k, v) in [
        ("theme_id", &theme.theme_id),
        ("palette", &theme.palette),
        ("typography", &theme.typography),
        ("density", &theme.density),
        ("heading", &theme.heading),
        ("introduction", &theme.introduction),
        ("seo_title", &theme.seo_title),
        ("seo_description", &theme.seo_description),
        ("public_origin", &theme.public_origin),
        ("primary_locale", &theme.primary_locale),
        ("time_zone", &regional.time_zone),
        ("date_format", &regional.date_format),
    ] {
        stored.set(k, v);
    }
    if theme.show_hero {
        stored.set("show_hero", "true");
    }
    if theme.show_featured {
        stored.set("show_featured", "true");
    }
    if theme.indexing_enabled {
        stored.set("indexing_enabled", "true");
    }
    stored.set("settings_revision", revision);
    stored.set("regional_revision", regional.revision);
    stored.set("fiscal_revision", fiscal_revision);
    stored.set("logo", "");
    stored.set("favicon", "");
    stored.locales = theme.active_locales.clone();
    stored.countries = selected_countries;
    if let Some(prefs) = preferences {
        for (k, v) in [
            ("currency", prefs.currency),
            ("tax_country", prefs.country),
            ("mode", prefs.mode),
            ("default_class_id", prefs.default_class_id),
        ] {
            stored.set(k, v);
        }
    }
    let editor = content::Editor::new(
        &theme.primary_locale,
        &theme.active_locales,
        talocart_platform::storefront::Storefront::CONTENT,
        rows.clone(),
        None,
        None,
        "theme-content",
    );
    stored.set("content_primary_locale", &editor.primary);
    stored.set("content_active_locales", &editor.active);
    for language in editor.secondary {
        for f in language.fields {
            stored.set(&f.name, f.value);
        }
    }
    let mut baseline_fields = stored.clone();
    if !session.allows(Permission::TaxesWrite) {
        for k in ["currency", "tax_country", "mode", "default_class_id"] {
            baseline_fields.values.remove(k);
        }
    }
    let baseline = baseline_fields.baseline();
    let mut submitted = submitted;
    // Read-only fields are not successful native controls. Reload their display
    // values after a tax-only submission; never treat crafted values as writable.
    if !session.allows(Permission::SettingsWrite)
        && let Some(fields) = &mut submitted
    {
        for (key, value) in &stored.values {
            if (TRACKED.contains(&key.as_str()) && !TAX_FIELDS.contains(&key.as_str()))
                || content_key(key)
            {
                fields.set(key, value);
            }
        }
        fields.locales.clone_from(&stored.locales);
        fields.countries.clone_from(&stored.countries);
    }
    if let Some(fields) = &mut submitted {
        fields.values.remove("module_install_password");
    }
    let fields = submitted.as_ref().unwrap_or(&stored);
    let general = fields.general();
    let general_error = if let Some(Error::General(source_error)) = &error {
        Some(source_error)
    } else {
        None
    };
    let mut errors = if let Some(Error::Storefront(
        talocart_platform::storefront::StorefrontError::Invalid(source_error),
    )) = &error
    {
        source_error.clone()
    } else {
        vec![]
    };
    match &error {
        Some(Error::Regional(_)) => errors.extend(["time_zone", "date_format"]),
        Some(Error::Countries(_)) => errors.push("purchase_country"),
        Some(Error::Taxes(talocart_commerce::taxes::TaxError::Invalid(fields))) => {
            for field in fields {
                errors.push(match field {
                    talocart_commerce::taxes::Field::Currency => "currency",
                    talocart_commerce::taxes::Field::Country => "tax_country",
                    talocart_commerce::taxes::Field::DefaultClass => "default_class_id",
                    _ => "mode",
                });
            }
        }
        Some(Error::Taxes(talocart_commerce::taxes::TaxError::CurrencyInUse)) => {
            errors.push("currency")
        }
        Some(Error::Taxes(talocart_commerce::taxes::TaxError::InvalidDefault)) => {
            errors.push("default_class_id")
        }
        Some(Error::Taxes(talocart_commerce::taxes::TaxError::ProductPrices)) => {
            errors.push("mode")
        }
        Some(Error::Branding(key, _)) => errors.push(key),
        _ => {}
    }
    let content_error = if let Some(Error::Storefront(
        talocart_platform::storefront::StorefrontError::Content(source_error),
    )) = &error
    {
        Some(source_error)
    } else {
        None
    };
    let content = content::Editor::new(
        &theme.primary_locale,
        &theme.active_locales,
        talocart_platform::storefront::Storefront::CONTENT,
        rows,
        Some(&fields.values),
        content_error,
        "theme-content",
    );
    let mut countries = i18n::code_choices(talocart_platform::geography::COUNTRIES, "country", "");
    for c in &mut countries {
        c.selected = fields.countries.iter().any(|session| session == c.code);
    }
    let media_library = MediaLibrary::new(
        state.pool.clone(),
        state.identity.clone(),
        state.media.clone(),
    );
    let mut images = vec![];
    for key in ["logo", "favicon"] {
        let selected = fields.get(key);
        let mut preview = format!("/branding/{key}?variant=card");
        let present = if key == "logo" {
            branding.has_logo
        } else {
            branding.has_favicon
        };
        let mut name = i18n::text(if present {
            "settings-image-current"
        } else {
            "admin-default-image"
        });
        if selected == "remove" {
            preview = "/favicon.svg".into();
            name = i18n::text("settings-image-removed");
        } else if let Ok(id) = selected.parse::<i64>() {
            // An untrusted ID never reveals a private/trashed image in the draft.
            match media_library.asset(session, id, false).await {
                Ok(a) if !a.trashed && a.owner_id.is_none() => {
                    preview = a.preview();
                    name = a.name;
                }
                _ => {
                    preview = "/favicon.svg".into();
                    name = i18n::text("settings-image-invalid");
                }
            }
        }
        images.push(Image {
            key,
            selected,
            preview,
            name,
        });
    }
    let (status, message) = error
        .as_ref()
        .map(failure)
        .unwrap_or((StatusCode::OK, String::new()));
    let active = if let Some(source_error) = &error {
        panel(source_error)
    } else {
        tab
    };
    let default_selection = fields.get("default_class_id");
    let default_id = default_selection.parse::<i64>().ok();
    let default_known =
        default_selection.is_empty() || defaults.iter().any(|entry| Some(entry.id) == default_id);
    let admin_shell =
        AdminShell::from_parts(state, session, store.values.name, logo, theme.clone());
    let response = render(Page {
        store_name: admin_shell.store_name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "settings",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        owner: session.allows(Permission::SettingsWrite),
        module_owner: session.is_owner(),
        can_save: session.allows(Permission::SettingsWrite)
            || session.allows(Permission::TaxesWrite),
        tax_write: session.allows(Permission::TaxesWrite),
        saved,
        error: message,
        error_panel: if let Some(source_error) = &error {
            panel(source_error)
        } else {
            ""
        },
        baseline,
        fields,
        tab: active.into(),
        sections: administration::settings_sections(&general, general_error),
        regional: regional::Settings {
            time_zone: fields.get("time_zone"),
            date_format: fields.get("date_format"),
            revision: fields.get("regional_revision").parse().unwrap_or(-1),
        },
        regional_preview,
        time_zones,
        values: fields.storefront(),
        content,
        errors,
        languages: Languages {
            primary_locale: fields.get("primary_locale"),
            active_locales: fields.locales.clone(),
        },
        preferences: fields.preferences(),
        default_known,
        currencies: i18n::code_choices(
            talocart_platform::geography::CURRENCIES,
            "currency",
            &fields.get("currency"),
        ),
        tax_countries: i18n::code_choices(
            talocart_platform::geography::COUNTRIES,
            "country",
            &fields.get("tax_country"),
        ),
        countries,
        defaults: defaults
            .into_iter()
            .map(|entry| TaxChoice {
                selected: default_id == Some(entry.id),
                id: entry.id,
                name: entry.name,
                active: entry.active && entry.rate_active,
            })
            .collect(),
        images,
    });
    Ok((status, response).into_response())
}

pub async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let mut fields = Fields::parse(pairs)?;
    state
        .identity
        .authorize(&session, Permission::SettingsRead)
        .await?;
    let permission = if session.allows(Permission::SettingsWrite) {
        Permission::SettingsWrite
    } else {
        Permission::TaxesWrite
    };
    state.identity.authorize(&session, permission).await?;
    session.check_csrf(&fields.get("csrf"))?;
    let action = fields.get("action");
    if !matches!(action.as_str(), "" | "save") {
        state
            .identity
            .authorize(&session, Permission::SettingsWrite)
            .await?;
    }
    if action.starts_with("browse_")
        || action.starts_with("choose_")
        || action.starts_with("media_folder")
        || matches!(
            action.as_str(),
            "media_search" | "media_next" | "media_previous"
        )
    {
        state
            .identity
            .authorize(&session, Permission::MediaRead)
            .await?;
    }
    let tab = fields.get("tab");
    let navigated = super::media_picker::navigate(&action, |k, v| fields.set(k, v))?;
    match action.as_str() {
        _ if navigated => {}
        "browse_logo" | "browse_favicon" => {
            fields.set("media_target", action.trim_start_matches("browse_"));
            fields.set("media_page", 1);
        }
        "media_search" => fields.set("media_page", 1),
        "media_next" => fields.set(
            "media_page",
            fields
                .get("media_page")
                .parse::<i64>()
                .unwrap_or(1)
                .saturating_add(1),
        ),
        "media_previous" => fields.set(
            "media_page",
            fields
                .get("media_page")
                .parse::<i64>()
                .unwrap_or(1)
                .saturating_sub(1)
                .max(1),
        ),
        "media_close" => fields.set("media_target", ""),
        "remove_logo" | "remove_favicon" => {
            fields.set(action.trim_start_matches("remove_"), "remove");
        }
        "restore_logo" | "restore_favicon" => {
            fields.set(action.trim_start_matches("restore_"), "");
        }
        _ if action.starts_with("choose_") => {
            let target = fields.get("media_target");
            if !["logo", "favicon"].contains(&target.as_str()) {
                return Err(IdentityError::InvalidBranding.into());
            }
            let id = action
                .trim_start_matches("choose_")
                .parse::<i64>()
                .map_err(|_| IdentityError::InvalidBranding)?;
            let lib = MediaLibrary::new(
                state.pool.clone(),
                state.identity.clone(),
                state.media.clone(),
            );
            let asset = lib
                .asset(&session, id, false)
                .await
                .map_err(|_| IdentityError::InvalidBranding)?;
            if asset.trashed || !asset.is_image() || asset.owner_id.is_some() {
                return Err(IdentityError::InvalidBranding.into());
            }
            fields.set(&target, id);
            fields.set("media_target", "");
        }
        "" | "save" => {
            match settings::save(
                &state.pool,
                &state.identity,
                &session,
                &fields.get("csrf"),
                fields.input(state.package_install),
            )
            .await
            {
                Ok(()) => {
                    return Ok(Redirect::to(&format!("{}?saved=1", tab_path(&tab))).into_response());
                }
                Err(error) => {
                    return response(&state, &session, Some(fields), Some(error), &tab, false)
                        .await;
                }
            }
        }
        _ => return Err(IdentityError::Invalid.into()),
    }
    let active = if action.contains("logo")
        || action.contains("favicon")
        || action.starts_with("media_")
        || action.starts_with("choose_")
    {
        "appearance"
    } else {
        safe_tab(&tab)
    };
    fields.values.remove("module_install_password");
    if ["logo", "favicon"].contains(&fields.get("media_target").as_str()) {
        let mut pairs = fields.values.into_iter().collect::<Vec<_>>();
        pairs.extend(
            fields
                .countries
                .into_iter()
                .map(|value| ("purchase_country".into(), value)),
        );
        pairs.extend(
            fields
                .locales
                .into_iter()
                .map(|value| ("locale".into(), value)),
        );
        return super::media::draft_page(
            &state,
            &session,
            super::media_picker::Draft::settings(pairs),
        )
        .await
        .map_err(|_| IdentityError::InvalidBranding.into());
    }
    response(&state, &session, Some(fields), None, active, false).await
}
fn safe_tab(tab: &str) -> &str {
    if ["general", "appearance", "storefront", "taxes", "countries"].contains(&tab) {
        tab
    } else {
        "general"
    }
}

pub(super) async fn show(
    state: &AppState,
    session: &Session,
    tab: &str,
    saved: bool,
) -> Result<Response> {
    response(state, session, None, None, tab, saved).await
}

fn tab_path(tab: &str) -> &'static str {
    match safe_tab(tab) {
        "appearance" => "/admin/settings/appearance",
        "storefront" => "/admin/settings/storefront",
        "taxes" => "/admin/settings/taxes",
        "countries" => "/admin/settings/countries",
        _ => "/admin/settings",
    }
}

impl From<settings::ReadError> for access::IdentityErrorResponse {
    fn from(error: settings::ReadError) -> Self {
        match error {
            settings::ReadError::Technical(source_error) => source_error.into(),
            settings::ReadError::Access(source_error) => source_error.into(),
            settings::ReadError::Identity(source_error) => source_error.into(),
        }
    }
}
