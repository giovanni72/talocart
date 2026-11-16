use super::fiscal_forms::FiscalRevision;
use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::{
    pricing::PriceMode,
    taxes::{Entry, Field, Kind, PAGE_SIZE, Preferences, Preview, Quote, TaxError, Values},
};

struct FormStatus {
    preview_only: bool,
    saved: bool,
    revision: i64,
}

type Result<T> = std::result::Result<T, TaxResponseError>;
pub struct TaxResponseError(TaxError);
impl From<TaxError> for TaxResponseError {
    fn from(error: TaxError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for TaxResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
pub(super) fn status(error: &TaxError) -> StatusCode {
    match error {
        TaxError::Invalid(_) | TaxError::Limit | TaxError::Overflow => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        TaxError::Stale
        | TaxError::CurrencyInUse
        | TaxError::CountryInUse
        | TaxError::InactiveClass
        | TaxError::ActiveZones
        | TaxError::ProductPrices
        | TaxError::Duplicate
        | TaxError::InUse
        | TaxError::InactiveRate
        | TaxError::ActiveClasses
        | TaxError::DefaultInUse
        | TaxError::InvalidDefault => StatusCode::CONFLICT,
        TaxError::NotFound => StatusCode::NOT_FOUND,
        TaxError::Technical(source_error) => crate::access::technical_status(source_error),
        TaxError::Access(source_error) => crate::access::access_status(source_error),
        TaxError::Identity(source_error) => access::IdentityErrorResponse::status_for(source_error),
    }
}
impl IntoResponse for TaxResponseError {
    fn into_response(self) -> Response {
        if let TaxError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let TaxError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let mut response = (status(&self.0), i18n::tax_error(&self.0)).into_response();
        if let TaxError::Identity(error) = &self.0 {
            access::IdentityErrorResponse::retry_for(error, &mut response);
        }
        response
    }
}
pub fn routes() -> Router<AppState> {
    let mut router = Router::new()
        .route(
            "/admin/settings/taxes/coverage",
            get(super::tax_coverage::index),
        )
        .route("/admin/settings/taxes", get(settings).post(preferences))
        .route(
            "/admin/settings/taxes/preview",
            get(preview_page).post(preview),
        );
    for kind in [Kind::Rate, Kind::Class] {
        let path = kind.path();
        router = router.merge(
            Router::new()
                .route(path, get(index))
                .route(&format!("{path}/new"), get(new_page).post(create))
                .route(&format!("{path}/{{id}}"), get(edit_page).post(update))
                .route(&format!("{path}/{{id}}/delete"), post(delete))
                .layer(Extension(kind)),
        );
    }
    router
}
struct Choice {
    id: i64,
    name: String,
    active: bool,
    selected: bool,
}
fn choices(entries: Vec<Entry>, selected: &str) -> Vec<Choice> {
    entries
        .into_iter()
        .map(|entry| Choice {
            id: entry.id,
            selected: entry.id.to_string() == selected,
            name: entry.name,
            active: entry.active && entry.rate_active,
        })
        .collect()
}
fn field(error: Option<&TaxError>, field: Field) -> bool {
    match error {
        Some(TaxError::Invalid(fields)) => fields.contains(&field),
        Some(TaxError::Duplicate) => field == Field::Name,
        Some(TaxError::InactiveRate) => field == Field::Rate,
        Some(TaxError::InvalidDefault) => field == Field::DefaultClass,
        Some(TaxError::Overflow) => field == Field::Amount,
        _ => false,
    }
}
#[derive(Deserialize, Default)]
struct SavedQuery {
    saved: Option<String>,
}
#[derive(Template)]
#[template(path = "tax-settings.html")]
struct SettingsPage<'a> {
    preview_only: bool,
    revision: i64,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    preferences: &'a Preferences,
    preview: &'a Preview,
    defaults: Vec<Choice>,
    classes: Vec<Choice>,
    quote: Option<Quote>,
    currencies: Vec<i18n::CodeChoice>,
    countries: Vec<i18n::CodeChoice>,
    preview_countries: Vec<i18n::CodeChoice>,
    currency_error: bool,
    country_error: bool,
    preview_country_error: bool,
    can_write: bool,
    saved: bool,
    error: String,
    mode_error: bool,
    default_error: bool,
    amount_error: bool,
    quantity_error: bool,
    class_error: bool,
}
async fn settings_response(
    state: &AppState,
    session: &Session,
    preferences: &Preferences,
    preview: &Preview,
    quote: Option<Quote>,
    error: Option<&TaxError>,
    form_status: FormStatus,
) -> Result<Response> {
    let preview_only = form_status.preview_only;
    let FormStatus {
        saved, revision, ..
    } = form_status;
    let entries = state.taxes.choices(session, Kind::Class).await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut response = render(SettingsPage {
        preview_only,
        revision,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "settings",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        preferences,
        preview,
        defaults: choices(entries.clone(), &preferences.default_class_id),
        classes: choices(entries, &preview.class_id),
        quote,
        currencies: i18n::code_choices(
            talocart_platform::geography::CURRENCIES,
            "currency",
            &preferences.currency,
        ),
        countries: i18n::code_choices(
            talocart_platform::geography::COUNTRIES,
            "country",
            &preferences.country,
        ),
        preview_countries: i18n::code_choices(
            talocart_platform::geography::COUNTRIES,
            "country",
            &preview.country,
        ),
        currency_error: field(error, Field::Currency),
        country_error: field(error, Field::Country) && preview.country == "store",
        preview_country_error: field(error, Field::Country) && preview.country != "store",
        can_write: session.allows(Permission::TaxesWrite),
        saved,
        error: error.map(i18n::tax_error).unwrap_or_default(),
        mode_error: field(error, Field::Mode),
        default_error: field(error, Field::DefaultClass),
        amount_error: field(error, Field::Amount),
        quantity_error: field(error, Field::Quantity),
        class_error: field(error, Field::Class),
    });
    if let Some(source_error) = error {
        *response.status_mut() = status(source_error);
    }
    Ok(response)
}
async fn settings(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<SavedQuery>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::TaxesRead)
        .await?;
    if session.allows(Permission::SettingsRead) {
        return unified_settings::show(
            &state,
            &session,
            "taxes",
            query.saved.as_deref() == Some("1"),
        )
        .await
        .map_err(|error| TaxResponseError(error.0.into()));
    }
    preview_page(
        Extension(FiscalRevision(revision)),
        State(state),
        Extension(session),
        Query(query),
    )
    .await
}
async fn preview_page(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<SavedQuery>,
) -> Result<Response> {
    let preferences = state.taxes.settings(&session).await?.values();
    let preview = Preview {
        country: "store".into(),
        class_id: preferences.default_class_id.clone(),
        ..Preview::default()
    };
    settings_response(
        &state,
        &session,
        &preferences,
        &preview,
        None,
        None,
        FormStatus {
            preview_only: session.allows(Permission::SettingsRead),
            saved: query.saved.as_deref() == Some("1"),
            revision,
        },
    )
    .await
}
#[derive(Deserialize)]
struct PreferencesForm {
    revision: i64,
    currency: Option<String>,
    country: Option<String>,
    csrf: String,
    mode: String,
    #[serde(default)]
    default_class_id: String,
}
async fn preferences(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<PreferencesForm>,
) -> Result<Response> {
    let revision = form.revision;
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let current = state.taxes.settings(&session).await?;
    let values = Preferences {
        currency: form.currency.unwrap_or(current.currency),
        country: form.country.unwrap_or(current.country),
        mode: form.mode,
        default_class_id: form.default_class_id,
    };
    match state
        .taxes
        .save_preferences(&session, &form.csrf, &values, revision)
        .await
    {
        Ok(()) => Ok(Redirect::to("/admin/settings/taxes?saved=1").into_response()),
        Err(TaxError::Technical(error)) => Err(error.into()),
        Err(TaxError::Access(error)) => Err(error.into()),
        Err(TaxError::Identity(error)) => Err(error.into()),
        Err(error) => {
            settings_response(
                &state,
                &session,
                &values,
                &Preview::default(),
                None,
                Some(&error),
                FormStatus {
                    preview_only: false,
                    saved: false,
                    revision,
                },
            )
            .await
        }
    }
}
#[derive(Deserialize)]
struct PreviewForm {
    #[serde(default = "store_country")]
    country: String,
    csrf: String,
    amount: String,
    quantity: String,
    class_id: String,
}
async fn preview(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<PreviewForm>,
) -> Result<Response> {
    session.check_csrf(&form.csrf)?;
    let preferences = state.taxes.settings(&session).await?.values();
    let values = Preview {
        country: form.country,
        amount: form.amount,
        quantity: form.quantity,
        class_id: form.class_id,
    };
    match state.taxes.preview(&session, &form.csrf, &values).await {
        Ok(quote) => {
            settings_response(
                &state,
                &session,
                &preferences,
                &values,
                Some(quote),
                None,
                FormStatus {
                    preview_only: session.allows(Permission::SettingsRead),
                    saved: false,
                    revision,
                },
            )
            .await
        }
        Err(TaxError::Technical(error)) => Err(error.into()),
        Err(TaxError::Access(error)) => Err(error.into()),
        Err(TaxError::Identity(error)) => Err(error.into()),
        Err(error) => {
            settings_response(
                &state,
                &session,
                &preferences,
                &values,
                None,
                Some(&error),
                FormStatus {
                    preview_only: session.allows(Permission::SettingsRead),
                    saved: false,
                    revision,
                },
            )
            .await
        }
    }
}
#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: String,
    #[serde(default = "first_page")]
    page: i64,
    deleted: Option<String>,
}
fn first_page() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "tax-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rate: bool,
    path: &'static str,
    title: &'static str,
    entries: Vec<Entry>,
    can_write: bool,
    q: &'a str,
    filter: &'a str,
    pagination: super::pagination::Pagination,
    deleted: bool,
}
fn title(kind: Kind) -> &'static str {
    match kind {
        Kind::Rate => "admin-tax-rates",
        Kind::Class => "admin-tax-classes",
    }
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let active = match query.status.as_str() {
        "" => None,
        "active" => Some(true),
        "inactive" => Some(false),
        _ => return Err(TaxError::Invalid(vec![]).into()),
    };
    let result = state
        .taxes
        .list_counted(&session, kind, &query.q, active, query.page)
        .await?;
    let entries = result.entries;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let link = |page| {
        format!(
            "{}?q={}&status={}&page={page}",
            kind.path(),
            super::catalog::encoded(&query.q),
            query.status
        )
    };
    Ok(render(ListPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: if kind == Kind::Rate {
            "tax_rates"
        } else {
            "tax_classes"
        },
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rate: kind == Kind::Rate,
        path: kind.path(),
        title: title(kind),
        entries,
        can_write: session.allows(Permission::TaxesWrite),
        q: &query.q,
        filter: &query.status,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            PAGE_SIZE as u16,
            &i18n::text(title(kind)),
            &i18n::text(match kind {
                Kind::Rate => "admin-tax-rate-pages",
                Kind::Class => "admin-tax-class-pages",
            }),
            link,
        ),
        deleted: query.deleted.as_deref() == Some("1"),
    }))
}
#[derive(Template)]
#[template(path = "tax-form.html")]
struct FormPage<'a> {
    revision: i64,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rate: bool,
    path: &'static str,
    title: &'static str,
    id: i64,
    values: &'a Values,
    rates: Vec<Choice>,
    can_write: bool,
    error: String,
    saved: bool,
    name_error: bool,
    description_error: bool,
    percentage_error: bool,
    rate_error: bool,
}
async fn form_response(
    state: &AppState,
    session: &Session,
    kind: Kind,
    id: i64,
    values: &Values,
    error: Option<&TaxError>,
    form_status: FormStatus,
) -> Result<Response> {
    let FormStatus {
        saved, revision, ..
    } = form_status;
    state
        .identity
        .authorize(session, Permission::TaxesRead)
        .await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut response = render(FormPage {
        revision,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: if kind == Kind::Rate {
            "tax_rates"
        } else {
            "tax_classes"
        },
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rate: kind == Kind::Rate,
        path: kind.path(),
        title: title(kind),
        id,
        values,
        rates: if kind == Kind::Class {
            choices(
                state.taxes.choices(session, Kind::Rate).await?,
                &values.rate_id,
            )
        } else {
            vec![]
        },
        can_write: session.allows(Permission::TaxesWrite),
        error: error.map(i18n::tax_error).unwrap_or_default(),
        saved,
        name_error: field(error, Field::Name),
        description_error: field(error, Field::Description),
        percentage_error: field(error, Field::Percentage),
        rate_error: field(error, Field::Rate),
    });
    if let Some(source_error) = error {
        *response.status_mut() = status(source_error);
    }
    Ok(response)
}
async fn new_page(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    form_response(
        &state,
        &session,
        kind,
        0,
        &Values {
            active: true,
            ..Values::default()
        },
        None,
        FormStatus {
            preview_only: false,
            saved: false,
            revision,
        },
    )
    .await
}
async fn edit_page(
    Extension(FiscalRevision(revision)): Extension<FiscalRevision>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Path(id): Path<i64>,
    Query(query): Query<SavedQuery>,
) -> Result<Response> {
    let entry = state.taxes.entry(&session, kind, id).await?;
    form_response(
        &state,
        &session,
        kind,
        id,
        &entry.values(),
        None,
        FormStatus {
            preview_only: false,
            saved: query.saved.as_deref() == Some("1"),
            revision,
        },
    )
    .await
}
#[derive(Deserialize)]
struct EntryForm {
    revision: i64,
    csrf: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    percentage: String,
    #[serde(default)]
    rate_id: String,
    #[serde(default)]
    active: bool,
}
async fn save_response(
    state: AppState,
    session: Session,
    kind: Kind,
    id: Option<i64>,
    form: EntryForm,
) -> Result<Response> {
    let revision = form.revision;
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let values = Values {
        name: form.name,
        description: form.description,
        percentage: form.percentage,
        rate_id: form.rate_id,
        active: form.active,
    };
    match state
        .taxes
        .save(&session, &form.csrf, kind, id, &values, revision)
        .await
    {
        Ok(id) => Ok(Redirect::to(&format!("{}/{id}?saved=1", kind.path())).into_response()),
        Err(TaxError::Technical(error)) => Err(error.into()),
        Err(TaxError::Access(error)) => Err(error.into()),
        Err(TaxError::Identity(error)) => Err(error.into()),
        Err(TaxError::NotFound) => Err(TaxError::NotFound.into()),
        Err(error) => {
            form_response(
                &state,
                &session,
                kind,
                id.unwrap_or(0),
                &values,
                Some(&error),
                FormStatus {
                    preview_only: false,
                    saved: false,
                    revision,
                },
            )
            .await
        }
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Form(form): Form<EntryForm>,
) -> Result<Response> {
    save_response(state, session, kind, None, form).await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Path(id): Path<i64>,
    Form(form): Form<EntryForm>,
) -> Result<Response> {
    save_response(state, session, kind, Some(id), form).await
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
    Extension(kind): Extension<Kind>,
    Path(id): Path<i64>,
    Form(form): Form<DeleteForm>,
) -> Result<Response> {
    let revision = form.revision;
    state
        .identity
        .authorize(&session, Permission::TaxesWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let entry = state.taxes.entry(&session, kind, id).await?;
    if !form.confirm {
        return form_response(
            &state,
            &session,
            kind,
            id,
            &entry.values(),
            Some(&TaxError::Invalid(vec![])),
            FormStatus {
                preview_only: false,
                saved: false,
                revision,
            },
        )
        .await;
    }
    match state
        .taxes
        .delete(&session, &form.csrf, kind, id, revision)
        .await
    {
        Ok(()) => Ok(Redirect::to(&format!("{}?deleted=1", kind.path())).into_response()),
        Err(TaxError::Technical(error)) => Err(error.into()),
        Err(TaxError::Access(error)) => Err(error.into()),
        Err(TaxError::Identity(error)) => Err(error.into()),
        Err(TaxError::NotFound) => Err(TaxError::NotFound.into()),
        Err(error) => {
            form_response(
                &state,
                &session,
                kind,
                id,
                &entry.values(),
                Some(&error),
                FormStatus {
                    preview_only: false,
                    saved: false,
                    revision,
                },
            )
            .await
        }
    }
}

fn store_country() -> String {
    "store".into()
}

impl From<talocart_platform::technical::TechnicalError> for TaxResponseError {
    fn from(error: talocart_platform::technical::TechnicalError) -> Self {
        Self(TaxError::Technical(error))
    }
}

impl From<talocart_platform::technical::AccessError> for TaxResponseError {
    fn from(error: talocart_platform::technical::AccessError) -> Self {
        Self(TaxError::Access(error))
    }
}
