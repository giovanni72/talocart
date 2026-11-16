//! Field policy editor uses native forms, finite identifiers and shared controls.
use super::*;
use axum::{extract::Form, response::Redirect};
use talocart_platform::customer_fields::{self, Error, Policy};
use talocart_platform::identity::IdentityError;
const PATH: &str = "/admin/settings/customer-fields";
pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(index).post(save)).route(
        "/admin/settings/customer-registration",
        axum::routing::post(save_registration),
    )
}
struct Row {
    name: &'static str,
    label: String,
    columns: Vec<Column>,
    locked: bool,
    heading: String,
    hint: String,
}
struct Column {
    kind: &'static str,
    label: String,
    options: Vec<Choice>,
    modified: bool,
}
struct Choice {
    key: &'static str,
    label: String,
    selected: bool,
}
struct Section {
    key: &'static str,
    title: String,
    rows: Vec<Row>,
}
#[derive(Template)]
#[template(path = "customer-field-settings.html")]
struct Page<'a> {
    registration: talocart_platform::identity::customer_registration::RegistrationSettings,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    revision: i64,
    can_write: bool,
    sections: Vec<Section>,
    error: String,
    saved: bool,
    stale: bool,
}
async fn page(
    state: &AppState,
    session: &Session,
    policy: Policy,
    status: StatusCode,
    error: &str,
    saved: bool,
) -> std::result::Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let sections = ["customer", "billing", "shipping"]
        .map(|key| Section {
            key,
            title: i18n::text(&format!("cf-scope-{key}")),
            rows: customer_fields::FIELDS
                .iter()
                .filter(|f| f.scope == key)
                .map(|f| Row {
                    name: f.name,
                    label: i18n::text(f.label),
                    columns: ["private", "business"]
                        .map(|kind| Column {
                            kind,
                            modified: policy.overridden(f.name, kind),
                            label: i18n::text(if kind == "private" {
                                "co-private"
                            } else {
                                "co-business"
                            }),
                            options: ["hidden", "optional", "required"]
                                .map(|key| Choice {
                                    key,
                                    label: i18n::text(&format!("cf-mode-{key}")),
                                    selected: policy.mode(f.name, kind).key() == key,
                                })
                                .into_iter()
                                .collect(),
                        })
                        .into_iter()
                        .collect(),
                    locked: f.locked,
                    heading: match f.name {
                        "billing_first_name" => i18n::text("cf-heading-holder"),
                        "billing_country" => i18n::text("cf-heading-address"),
                        "vat_id" => i18n::text("cf-heading-fiscal"),
                        _ => String::new(),
                    },
                    hint: if matches!(f.name, "tax_id" | "recipient_code" | "pec") {
                        i18n::text("cf-hint-italy")
                    } else if f.name.ends_with("_postal") || f.name.ends_with("_region") {
                        i18n::text(if f.scope == "billing" {
                            "cf-hint-billing-country"
                        } else {
                            "cf-hint-country"
                        })
                    } else if f.scope == "billing" {
                        i18n::text("cf-hint-invoice")
                    } else {
                        String::new()
                    },
                })
                .collect(),
        })
        .into_iter()
        .collect();
    let mut r = render(Page {
        registration:
            talocart_platform::identity::customer_registration::RegistrationSettings::load(
                &state.pool,
            )
            .await?,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "customer_fields",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        revision: policy.revision,
        can_write: session.allows(Permission::SettingsWrite),
        sections,
        error: error.into(),
        saved,
        stale: status == StatusCode::CONFLICT,
    });
    *r.status_mut() = status;
    Ok(r)
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationInput {
    csrf: String,
    revision: i64,
    activation: String,
    allow_guest_checkout: bool,
}
async fn save_registration(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<RegistrationInput>,
) -> std::result::Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let require_email_confirmation = match f.activation.as_str() {
        "immediate" => false,
        "email" => true,
        _ => return Err(IdentityError::Invalid.into()),
    };
    let settings = talocart_platform::identity::customer_registration::RegistrationSettings {
        revision: f.revision,
        require_email_confirmation,
        allow_guest_checkout: f.allow_guest_checkout,
    };
    match settings.save(&state.pool, &session, &f.csrf).await {
        Ok(()) => {
            Ok(Redirect::to(&format!("{PATH}?saved=1#customer-registration")).into_response())
        }
        Err(talocart_platform::identity::CustomerError::Stale) => {
            page(
                &state,
                &session,
                customer_fields::load(&state.pool).await?,
                StatusCode::CONFLICT,
                &i18n::text("cf-conflict"),
                false,
            )
            .await
        }
        Err(talocart_platform::identity::CustomerError::Technical(
            talocart_platform::technical::TechnicalError::Unavailable,
        )) => {
            page(
                &state,
                &session,
                customer_fields::load(&state.pool).await?,
                StatusCode::UNPROCESSABLE_ENTITY,
                &i18n::text("registration-mail-unavailable"),
                false,
            )
            .await
        }
        Err(talocart_platform::identity::CustomerError::Technical(error)) => Err(error.into()),
        Err(talocart_platform::identity::CustomerError::Access(error)) => Err(error.into()),
        Err(talocart_platform::identity::CustomerError::Identity(error)) => Err(error.into()),
        Err(_) => Err(IdentityError::Unavailable.into()),
    }
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    axum::extract::Query(query): axum::extract::Query<std::collections::BTreeMap<String, String>>,
) -> std::result::Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(&session, Permission::SettingsRead)
        .await?;
    let policy = customer_fields::load(&state.pool).await?;
    page(
        &state,
        &session,
        policy,
        StatusCode::OK,
        "",
        query.get("saved").is_some_and(|session| session == "1"),
    )
    .await
}
async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> std::result::Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let mut csrf = None;
    let mut revision = None;
    let mut choices = Vec::new();
    let mut reset = None;
    for (k, v) in pairs {
        match k.as_str() {
            "action" if matches!(v.as_str(), "save" | "save-bottom") => {}
            "reset"
                if reset.is_none() && ["customer", "billing", "shipping"].contains(&v.as_str()) =>
            {
                reset = Some(v)
            }
            "csrf" if csrf.is_none() => csrf = Some(v),
            "revision" if revision.is_none() => {
                revision = Some(v.parse::<i64>().map_err(|_| IdentityError::Invalid)?)
            }
            _ if k.starts_with("private.") || k.starts_with("business.") => choices.push((k, v)),
            _ => return Err(IdentityError::Invalid.into()),
        }
    }
    let csrf = csrf.ok_or(IdentityError::Invalid)?;
    session.check_csrf(&csrf)?;
    let revision = revision.ok_or(IdentityError::Invalid)?;
    if let Some(scope) = reset {
        // Prepare defaults in the form; saving remains explicit and revision checked.
        Policy::from_choices(&choices).map_err(|_| IdentityError::Invalid)?;
        let defaults = Policy::default();
        for (key, value) in &mut choices {
            if let Some((kind, name)) = key.split_once('.')
                && customer_fields::field(name).is_some_and(|f| f.scope == scope)
            {
                *value = defaults.mode(name, kind).key().into();
            }
        }
        let mut policy = Policy::from_choices(&choices).map_err(|_| IdentityError::Invalid)?;
        policy.revision = revision;
        return page(&state, &session, policy, StatusCode::OK, "", false).await;
    }
    match customer_fields::save(&state.pool, &session, &csrf, revision, &choices).await {
        Ok(()) => Ok(Redirect::to(&format!("{PATH}?saved=1")).into_response()),
        Err(Error::Technical(error)) => Err(error.into()),
        Err(Error::Access(error)) => Err(error.into()),
        Err(Error::Identity(error)) => Err(error.into()),
        Err(error) => {
            let stale = matches!(error, Error::Conflict);
            let mut policy = Policy::from_choices(&choices).unwrap_or_default();
            policy.revision = revision;
            page(
                &state,
                &session,
                policy,
                if stale {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::UNPROCESSABLE_ENTITY
                },
                &i18n::text(if stale { "cf-conflict" } else { "cf-invalid" }),
                false,
            )
            .await
        }
    }
}
