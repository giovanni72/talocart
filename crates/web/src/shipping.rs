use super::*;
use axum::{
    extract::{Form, Path},
    response::Redirect,
};
use talocart_commerce::shipping::{FeeKind, Input, MethodCode, Shipping, ShippingError, Snapshot};
type Result<T> = std::result::Result<T, ShippingResponseError>;
struct ShippingResponseError(ShippingError);
impl From<ShippingError> for ShippingResponseError {
    fn from(error: ShippingError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for ShippingResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for ShippingResponseError {
    fn into_response(self) -> Response {
        if let ShippingError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let ShippingError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let ShippingError::Identity(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let (status, key) = match self.0 {
            ShippingError::NotFound => (StatusCode::NOT_FOUND, "shipping-not-found"),
            ShippingError::Stale => (StatusCode::CONFLICT, "shipping-stale"),
            _ => (StatusCode::UNPROCESSABLE_ENTITY, "shipping-invalid"),
        };
        (status, i18n::text(key)).into_response()
    }
}
const PATH: &str = "/admin/settings/shipping";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route(&format!("{PATH}/{{id}}"), get(edit).post(update))
}
fn service(state: &AppState) -> Shipping {
    Shipping::new(state.pool.clone(), state.identity.clone())
}
struct CountryChoice {
    code: String,
    name: String,
    selected: bool,
}
struct Entry {
    id: i64,
    name: String,
    fee: String,
    tax_class: String,
    free_from: String,
    countries: String,
    delivery_days: Option<i32>,
    free_method: bool,
    active: bool,
}
struct Field {
    name: &'static str,
    label: String,
    value: String,
    multiline: bool,
    invalid: bool,
    max: usize,
}
#[derive(Template)]
#[template(path = "shipping.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    can_write: bool,
    editor: bool,
    help: String,
    method_name: String,
    action: String,
    revision: i64,
    input: Input,
    fields: Vec<Field>,
    error: String,
    entries: Vec<Entry>,
    tax_classes: Vec<talocart_commerce::taxes::charges::ClassChoice>,
    tax_class_invalid: bool,
    tax_class_unknown: bool,
    price_mode_help: String,
    countries: Vec<CountryChoice>,
    countries_invalid: bool,
    fee_kind_invalid: bool,
    free_method: bool,
}
struct Editor {
    code: MethodCode,
    id: Option<i64>,
    revision: i64,
    input: Input,
    error: Option<ShippingError>,
}
async fn page(
    state: &AppState,
    session: &Session,
    editor: Option<Editor>,
    snapshot: Snapshot,
) -> Result<Response> {
    let Snapshot {
        revision,
        currency,
        methods,
        enabled_countries,
        tax_classes,
        price_mode,
    } = snapshot;
    let entries = methods
        .into_iter()
        .filter(|_| editor.is_none())
        .map(|m| {
            let mut input = m.input(currency.clone(), price_mode);
            input.fee_value = i18n::decimal(input.fee_value.replace('.', ","));
            input.free_from = i18n::decimal(input.free_from.replace('.', ","));
            Entry {
                tax_class: m
                    .tax_class_id
                    .and_then(|id| tax_classes.iter().find(|c| c.id == id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| i18n::text("charge-tax-unselected")),
                id: m.id,
                name: if i18n::locale() == "it" {
                    m.name_it
                } else {
                    m.name_en
                },
                fee: match m.fee_kind {
                    FeeKind::Percent => format!("{}%", input.fee_value),
                    FeeKind::Fixed => format!("{} {}", input.fee_value, currency),
                },
                free_from: if m.free_from_cents == 0 {
                    i18n::text("shipping-disabled")
                } else {
                    format!("{} {}", input.free_from, currency)
                },
                active: m.active,
                delivery_days: m.delivery_days,
                free_method: m.code == MethodCode::FreeShipping,
                countries: m
                    .countries
                    .iter()
                    .map(|c| i18n::text(&format!("country-{}", c.to_ascii_lowercase())))
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        })
        .collect();
    let is_editor = editor.is_some();
    let editor_state = editor.unwrap_or(Editor {
        code: MethodCode::NationalExpress,
        id: None,
        revision,
        input: Input::default(),
        error: None,
    });
    let status = match &editor_state.error {
        None => StatusCode::OK,
        Some(ShippingError::Stale) => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let error = match &editor_state.error {
        None => String::new(),
        Some(ShippingError::Stale) => i18n::text("shipping-stale"),
        _ => i18n::text("shipping-invalid"),
    };
    let bad = match &editor_state.error {
        Some(ShippingError::Invalid(fields)) => fields.as_slice(),
        _ => &[],
    };
    let mut fields = Vec::new();
    for (name, value, multiline, max) in [
        ("name_it", &editor_state.input.name_it, false, 120),
        ("name_en", &editor_state.input.name_en, false, 120),
        (
            "instructions_it",
            &editor_state.input.instructions_it,
            true,
            4000,
        ),
        (
            "instructions_en",
            &editor_state.input.instructions_en,
            true,
            4000,
        ),
        ("fee_value", &editor_state.input.fee_value, false, 24),
        ("free_from", &editor_state.input.free_from, false, 24),
        ("sort_order", &editor_state.input.sort_order, false, 4),
        ("delivery_days", &editor_state.input.delivery_days, false, 3),
    ] {
        if editor_state.code == MethodCode::FreeShipping
            && matches!(name, "fee_value" | "free_from")
        {
            continue;
        }
        fields.push(Field {
            name,
            label: i18n::text(&format!("shipping-{name}")),
            value: value.clone(),
            multiline,
            invalid: bad.contains(&name),
            max,
        });
    }
    let displayed_mode =
        talocart_commerce::pricing::PriceMode::parse(&editor_state.input.price_mode)
            .unwrap_or(price_mode);
    let tax_class_unknown = !editor_state.input.tax_class_id.is_empty()
        && !tax_classes
            .iter()
            .any(|c| c.id.to_string() == editor_state.input.tax_class_id);
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut response = render(Page {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "shipping",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::SettingsWrite),
        editor: is_editor,
        help: i18n::text(if editor_state.code == MethodCode::FreeShipping {
            "shipping-free-help"
        } else {
            "shipping-help"
        }),
        method_name: i18n::text(if editor_state.code == MethodCode::FreeShipping {
            "shipping-free"
        } else {
            "shipping-express"
        }),
        free_method: editor_state.code == MethodCode::FreeShipping,
        action: editor_state
            .id
            .map_or_else(|| PATH.to_owned(), |id| format!("{PATH}/{id}")),
        revision: editor_state.revision,
        countries: enabled_countries
            .into_iter()
            .map(|code| CountryChoice {
                selected: editor_state.input.countries.contains(&code),
                name: i18n::text(&format!("country-{}", code.to_ascii_lowercase())),
                code,
            })
            .collect(),
        input: editor_state.input,
        fields,
        error,
        countries_invalid: bad.contains(&"countries"),
        fee_kind_invalid: bad.contains(&"fee_kind"),
        entries,
        tax_classes,
        tax_class_unknown,
        tax_class_invalid: bad.contains(&"tax_class_id"),
        price_mode_help: i18n::text(
            if displayed_mode == talocart_commerce::pricing::PriceMode::Inclusive {
                "charge-price-inclusive"
            } else {
                "charge-price-exclusive"
            },
        ),
    });
    *response.status_mut() = status;
    Ok(response)
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    let snapshot = service(&state).snapshot(&session).await?;
    page(&state, &session, None, snapshot).await
}
async fn edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let snapshot = service(&state).snapshot(&session).await?;
    let Snapshot {
        revision,
        currency,
        methods,
        price_mode,
        ..
    } = &snapshot;
    let m = methods
        .iter()
        .find(|m| m.id == id)
        .ok_or(ShippingError::NotFound)?;
    page(
        &state,
        &session,
        Some(Editor {
            code: m.code,
            id: Some(id),
            revision: *revision,
            input: m.input(currency.clone(), *price_mode),
            error: None,
        }),
        snapshot,
    )
    .await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    save(&state, &session, id, fields).await
}
async fn save(
    state: &AppState,
    session: &Session,
    id: i64,
    fields: Vec<(String, String)>,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::SettingsWrite)
        .await?;
    let snapshot = service(state).snapshot(session).await?;
    let code = snapshot
        .methods
        .iter()
        .find(|m| m.id == id)
        .ok_or(ShippingError::NotFound)?
        .code;
    let mut input = Input::default();
    let mut csrf = String::new();
    let mut revision = String::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut invalid = false;
    for (k, v) in fields {
        if k != "countries" && !seen.insert(k.clone()) {
            invalid = true;
        }
        match k.as_str() {
            "csrf" => csrf = v,
            "revision" => revision = v,
            "name_it" => input.name_it = v,
            "name_en" => input.name_en = v,
            "instructions_it" => input.instructions_it = v,
            "instructions_en" => input.instructions_en = v,
            "fee_kind" => input.fee_kind = v,
            "tax_class_id" => input.tax_class_id = v,
            "fee_value" => input.fee_value = v,
            "free_from" => input.free_from = v,
            "countries" => input.countries.push(v),
            "sort_order" => input.sort_order = v,
            "delivery_days" => input.delivery_days = v,
            "currency" => input.currency = v,
            "price_mode" => input.price_mode = v,
            "active" => {
                input.active = v == "true";
                invalid |= v != "true";
            }
            _ => invalid = true,
        }
    }
    session.check_csrf(&csrf)?;
    let revision = revision.parse::<i64>().map_err(|_| ShippingError::Stale)?;
    let result = if invalid {
        Err(ShippingError::Invalid(vec![]))
    } else {
        service(state)
            .save(session, &csrf, id, revision, &input)
            .await
    };
    match result {
        Ok(_) => Ok(Redirect::to(PATH).into_response()),
        Err(source_error @ (ShippingError::Invalid(_) | ShippingError::Stale)) => {
            page(
                state,
                session,
                Some(Editor {
                    code,
                    id: Some(id),
                    revision,
                    input,
                    error: Some(source_error),
                }),
                snapshot,
            )
            .await
        }
        Err(source_error) => Err(source_error.into()),
    }
}
