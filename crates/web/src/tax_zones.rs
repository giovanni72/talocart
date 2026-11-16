use super::fiscal_forms::FiscalRevision;
use super::taxes::TaxResponseError;
use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::taxes::{Field, Kind, TaxError, Zone, ZoneValues};
type Result<T> = std::result::Result<T, TaxResponseError>;
const PATH: &str = "/admin/settings/taxes/zones";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route(
            "/admin/settings/taxes/zones/new",
            get(new_page).post(create),
        )
        .route("/admin/settings/taxes/zones/{id}", get(edit).post(update))
        .route("/admin/settings/taxes/zones/{id}/delete", post(delete))
}
#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: String,
    #[serde(default = "first_page")]
    page: i64,
}
fn first_page() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "tax-zone-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    entries: Vec<Zone>,
    can_write: bool,
    q: String,
    filter: String,
    pagination: super::pagination::Pagination,
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let active = match q.status.as_str() {
        "" => None,
        "active" => Some(true),
        "inactive" => Some(false),
        _ => return Err(TaxError::Invalid(vec![]).into()),
    };
    let result = state.taxes.zones(&session, &q.q, active, q.page).await?;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(ListPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "tax_zones",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        entries: result.entries,
        can_write: session.allows(Permission::TaxesWrite),
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            20,
            &i18n::text("admin-tax-zones"),
            &i18n::text("admin-zone-pages"),
            |page| {
                format!(
                    "{PATH}?q={}&status={}&page={page}",
                    super::catalog::encoded(&q.q),
                    q.status
                )
            },
        ),
        q: q.q,
        filter: q.status,
    }))
}
struct RateChoice {
    id: i64,
    name: String,
    selected: bool,
}
struct ClassChoice {
    id: i64,
    name: String,
    rates: Vec<RateChoice>,
    active: bool,
}
#[derive(Template)]
#[template(path = "tax-zone-form.html")]
struct FormPage<'a> {
    revision: i64,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    id: i64,
    values: &'a ZoneValues,
    classes: Vec<ClassChoice>,
    countries: Vec<i18n::CodeChoice>,
    can_write: bool,
    error: String,
    saved: bool,
    name_error: bool,
    description_error: bool,
    class_error: bool,
    countries_error: bool,
}
async fn form_response(
    state: &AppState,
    session: &Session,
    id: i64,
    values: &ZoneValues,
    error: Option<&TaxError>,
    saved: bool,
    revision: i64,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::TaxesRead)
        .await?;
    let rates = state.taxes.choices(session, Kind::Rate).await?;
    let classes = state
        .taxes
        .choices(session, Kind::Class)
        .await?
        .into_iter()
        .map(|c| ClassChoice {
            id: c.id,
            rates: rates
                .iter()
                .map(|r| RateChoice {
                    id: r.id,
                    name: format!("{} · {}%", r.name, i18n::decimal(r.percentage())),
                    selected: values.rules.get(&c.id) == Some(&r.id),
                })
                .collect(),
            name: c.name,
            active: c.active && c.rate_active,
        })
        .collect();
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut countries = i18n::code_choices(talocart_platform::geography::COUNTRIES, "country", "");
    for choice in &mut countries {
        choice.selected = values
            .countries
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|c| c.eq_ignore_ascii_case(choice.code));
    }
    let invalid =
        |field| matches!(error,Some(TaxError::Invalid(fields)) if fields.contains(&field));
    let mut response = render(FormPage {
        revision,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "tax_zones",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        id,
        values,
        classes,
        countries,
        can_write: session.allows(Permission::TaxesWrite),
        error: error.map(i18n::tax_error).unwrap_or_default(),
        saved,
        name_error: invalid(Field::Name) || matches!(error, Some(TaxError::Duplicate)),
        description_error: invalid(Field::Description),
        class_error: invalid(Field::Class) || matches!(error, Some(TaxError::InactiveClass)),
        countries_error: invalid(Field::Countries) || matches!(error, Some(TaxError::CountryInUse)),
    });
    if let Some(error) = error {
        *response.status_mut() = super::taxes::status(error);
    }
    Ok(response)
}
async fn new_page(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    form_response(
        &state,
        &session,
        0,
        &ZoneValues {
            active: true,
            ..ZoneValues::default()
        },
        None,
        false,
        revision,
    )
    .await
}
#[derive(Deserialize)]
struct SavedQuery {
    saved: Option<String>,
}
async fn edit(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<SavedQuery>,
) -> Result<Response> {
    let zone = state.taxes.zone(&session, id).await?;
    form_response(
        &state,
        &session,
        id,
        &zone.values(),
        None,
        q.saved.as_deref() == Some("1"),
        revision,
    )
    .await
}
#[derive(Deserialize)]
struct ZoneForm {
    csrf: String,
    name: String,
    #[serde(default)]
    description: String,
    rules: std::collections::BTreeMap<i64, i64>,
    revision: i64,
    countries: String,
    #[serde(default)]
    active: bool,
}
async fn save(state: AppState, session: Session, id: Option<i64>, f: ZoneForm) -> Result<Response> {
    let revision = f.revision;
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let values = ZoneValues {
        name: f.name,
        description: f.description,
        rules: f.rules,
        countries: f.countries,
        active: f.active,
    };
    match state
        .taxes
        .save_zone(&session, &f.csrf, id, &values, revision)
        .await
    {
        Ok(id) => Ok(Redirect::to(&format!("{PATH}/{id}?saved=1")).into_response()),
        Err(TaxError::Technical(error)) => Err(error.into()),
        Err(TaxError::Access(error)) => Err(error.into()),
        Err(TaxError::Identity(error)) => Err(error.into()),
        Err(TaxError::NotFound) => Err(TaxError::NotFound.into()),
        Err(error) => {
            form_response(
                &state,
                &session,
                id.unwrap_or(0),
                &values,
                Some(&error),
                false,
                revision,
            )
            .await
        }
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    save(state, session, None, zone_form(fields)?).await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    save(state, session, Some(id), zone_form(fields)?).await
}
#[derive(Deserialize)]
struct DeleteForm {
    revision: i64,
    csrf: String,
    #[serde(default)]
    confirm: bool,
}
async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<DeleteForm>,
) -> Result<Response> {
    let revision = f.revision;
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    if !f.confirm {
        let zone = state.taxes.zone(&session, id).await?;
        return form_response(
            &state,
            &session,
            id,
            &zone.values(),
            Some(&TaxError::Invalid(vec![])),
            false,
            revision,
        )
        .await;
    }
    state
        .taxes
        .delete_zone(&session, &f.csrf, id, revision)
        .await?;
    Ok(Redirect::to(PATH).into_response())
}

fn zone_form(fields: Vec<(String, String)>) -> Result<ZoneForm> {
    let mut countries = vec![];
    let mut rules = std::collections::BTreeMap::new();
    let mut seen_rules = std::collections::BTreeSet::new();
    let mut scalar = std::collections::BTreeMap::new();
    for (key, value) in fields {
        if let Some(class) = key.strip_prefix("rule_") {
            let class = class
                .parse::<i64>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or(TaxError::Invalid(vec![Field::Class]))?;
            if !seen_rules.insert(class) {
                return Err(TaxError::Invalid(vec![Field::Class]).into());
            }
            if !value.is_empty() {
                let rate = value
                    .parse::<i64>()
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or(TaxError::Invalid(vec![Field::Class]))?;
                rules.insert(class, rate);
            }
        } else if key == "countries" {
            countries.push(value);
        } else if !["csrf", "name", "description", "revision", "active"].contains(&key.as_str())
            || scalar.insert(key, value).is_some()
        {
            return Err(TaxError::Invalid(vec![]).into());
        }
    }
    let active = match scalar.remove("active").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        _ => return Err(TaxError::Invalid(vec![]).into()),
    };
    Ok(ZoneForm {
        csrf: scalar.remove("csrf").unwrap_or_default(),
        name: scalar.remove("name").unwrap_or_default(),
        description: scalar.remove("description").unwrap_or_default(),
        rules,
        revision: scalar
            .remove("revision")
            .and_then(|v| v.parse().ok())
            .ok_or(TaxError::Stale)?,
        active,
        countries: countries.join(", "),
    })
}
