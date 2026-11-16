use super::*;
use axum::{
    extract::{Form, Path},
    response::Redirect,
};
use talocart_commerce::payments::{
    BankDetailsInput, BankTransferInput, CashOnDeliveryInput, CommonInput, FeeKind, MethodCode,
    PaymentError, Payments, Snapshot,
};
type Result<T> = std::result::Result<T, PaymentResponseError>;
struct PaymentResponseError(PaymentError);
impl From<PaymentError> for PaymentResponseError {
    fn from(error: PaymentError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for PaymentResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for PaymentResponseError {
    fn into_response(self) -> Response {
        if let PaymentError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let PaymentError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let PaymentError::Identity(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let (status, key) = match self.0 {
            PaymentError::NotFound => (StatusCode::NOT_FOUND, "payment-not-found"),
            PaymentError::Stale => (StatusCode::CONFLICT, "payment-stale"),
            _ => (StatusCode::UNPROCESSABLE_ENTITY, "payment-invalid"),
        };
        (status, i18n::text(key)).into_response()
    }
}
const PATH: &str = "/admin/settings/payments";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route(&format!("{PATH}/{{id}}"), get(edit).post(update))
}
fn service(state: &AppState) -> Payments {
    let online = state
        .online
        .as_ref()
        .is_some_and(|service| service.enabled());
    let providers = state.packages.as_ref().map(|m| {
        m.payments()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.manifest.id)
            .collect()
    });
    Payments::new(state.pool.clone(), state.identity.clone())
        .with_online(online)
        .with_providers(providers)
}
struct Entry {
    id: i64,
    name: String,
    fee: String,
    tax_class: String,
    minimum: String,
    active: bool,
    status_name: String,
    status_color: String,
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
#[template(path = "payments.html")]
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
    input: CommonInput,
    fields: Vec<Field>,
    error: String,
    entries: Vec<Entry>,
    tax_classes: Vec<talocart_commerce::taxes::charges::ClassChoice>,
    tax_class_invalid: bool,
    tax_class_unknown: bool,
    price_mode_help: String,
    statuses: Vec<talocart_commerce::order_statuses::OrderStatus>,
    status_invalid: bool,
}
struct Editor {
    bank: BankDetailsInput,
    code: MethodCode,
    id: Option<i64>,
    revision: i64,
    input: CommonInput,
    error: Option<PaymentError>,
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
        statuses,
        tax_classes,
        price_mode,
    } = snapshot;
    let entries = methods
        .into_iter()
        .filter(|_| editor.is_none())
        .map(|m| {
            let mut input = m.common_input(currency.clone(), price_mode);
            input.fee_value = i18n::decimal(input.fee_value.replace('.', ","));
            input.minimum = i18n::decimal(input.minimum.replace('.', ","));
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
                    FeeKind::None => i18n::text("payment-none"),
                    FeeKind::Percent => format!("{}%", input.fee_value),
                    FeeKind::Fixed => format!("{} {}", input.fee_value, currency),
                },
                minimum: if m.minimum_cents == 0 {
                    i18n::text("payment-always")
                } else {
                    format!("{} {}", input.minimum, currency)
                },
                active: m.active,
                status_name: statuses
                    .iter()
                    .find(|status| status.id == m.initial_order_status_id)
                    .map(|status| status.name(i18n::locale()).to_owned())
                    .unwrap_or_default(),
                status_color: statuses
                    .iter()
                    .find(|status| status.id == m.initial_order_status_id)
                    .map(|status| status.color.clone())
                    .unwrap_or_else(|| "#64748b".into()),
            }
        })
        .collect();
    let is_editor = editor.is_some();
    let editor_state = editor.unwrap_or(Editor {
        code: MethodCode::BankTransfer,
        bank: BankDetailsInput::default(),
        id: None,
        revision,
        input: CommonInput::default(),
        error: None,
    });
    let status = match &editor_state.error {
        None => StatusCode::OK,
        Some(PaymentError::Stale) => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let error = match &editor_state.error {
        None => String::new(),
        Some(PaymentError::Stale) => i18n::text("payment-stale"),
        _ => i18n::text(if editor_state.code == MethodCode::OnlineCheckout {
            "online-payment-method-invalid"
        } else if editor_state.code == MethodCode::CashOnDelivery {
            "payment-cod-invalid"
        } else {
            "payment-invalid"
        }),
    };
    let bad = match &editor_state.error {
        Some(PaymentError::Invalid(fields)) => fields.as_slice(),
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
        ("beneficiary", &editor_state.bank.beneficiary, false, 200),
        ("iban", &editor_state.bank.iban, false, 64),
        ("bic", &editor_state.bank.bic, false, 11),
        ("fee_value", &editor_state.input.fee_value, false, 24),
        ("minimum", &editor_state.input.minimum, false, 24),
        ("sort_order", &editor_state.input.sort_order, false, 4),
    ] {
        if editor_state.code != MethodCode::BankTransfer
            && matches!(name, "beneficiary" | "iban" | "bic")
        {
            continue;
        }
        fields.push(Field {
            name,
            label: i18n::text(&format!("payment-{name}")),
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
        section: "payments",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::SettingsWrite),
        editor: is_editor,
        help: i18n::text(if editor_state.code == MethodCode::OnlineCheckout {
            "online-payment-method-help"
        } else if editor_state.code == MethodCode::CashOnDelivery {
            "payment-cod-help"
        } else {
            "payment-help"
        }),
        method_name: if editor_state.code == MethodCode::OnlineCheckout {
            if i18n::locale() == "it" {
                editor_state.input.name_it.clone()
            } else {
                editor_state.input.name_en.clone()
            }
        } else {
            i18n::text(if editor_state.code == MethodCode::CashOnDelivery {
                "payment-cod"
            } else {
                "payment-bank"
            })
        },
        action: editor_state
            .id
            .map_or_else(|| PATH.to_owned(), |id| format!("{PATH}/{id}")),
        revision: editor_state.revision,
        input: editor_state.input,
        fields,
        error,
        status_invalid: bad.contains(&"initial_order_status_id"),
        statuses,
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
        .ok_or(PaymentError::NotFound)?;
    page(
        &state,
        &session,
        Some(Editor {
            code: m.code,
            bank: m.bank_input(),
            id: Some(id),
            revision: *revision,
            input: m.common_input(currency.clone(), *price_mode),
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
        .ok_or(PaymentError::NotFound)?
        .code;
    let mut input = CommonInput::default();
    let mut bank = BankDetailsInput::default();
    let mut csrf = String::new();
    let mut revision = String::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut invalid = false;
    for (k, v) in fields {
        if !seen.insert(k.clone()) {
            invalid = true;
        }
        if code != MethodCode::BankTransfer && matches!(k.as_str(), "beneficiary" | "iban" | "bic")
        {
            invalid = true;
            continue;
        }
        match k.as_str() {
            "csrf" => csrf = v,
            "revision" => revision = v,
            "name_it" => input.name_it = v,
            "name_en" => input.name_en = v,
            "instructions_it" => input.instructions_it = v,
            "instructions_en" => input.instructions_en = v,
            "beneficiary" => bank.beneficiary = v,
            "iban" => bank.iban = v,
            "bic" => bank.bic = v,
            "fee_kind" => input.fee_kind = v,
            "tax_class_id" => input.tax_class_id = v,
            "fee_value" => input.fee_value = v,
            "minimum" => input.minimum = v,
            "sort_order" => input.sort_order = v,
            "currency" => input.currency = v,
            "price_mode" => input.price_mode = v,
            "initial_order_status_id" => input.initial_order_status_id = v,
            "active" => {
                input.active = v == "true";
                invalid |= v != "true";
            }
            _ => invalid = true,
        }
    }
    session.check_csrf(&csrf)?;
    let revision = revision.parse::<i64>().map_err(|_| PaymentError::Stale)?;
    let result = if invalid {
        Err(PaymentError::Invalid(vec![]))
    } else {
        match code {
            MethodCode::BankTransfer => {
                service(state)
                    .save_bank_transfer(
                        session,
                        &csrf,
                        id,
                        revision,
                        &BankTransferInput {
                            common: input.clone(),
                            bank: bank.clone(),
                        },
                    )
                    .await
            }
            MethodCode::OnlineCheckout => {
                service(state)
                    .save_online(
                        session,
                        &csrf,
                        id,
                        revision,
                        &input,
                        match &state.online {
                            Some(s) => match s.for_method(Some(id)).await {
                                Ok(s) => s.accepting().await,
                                Err(_) => false,
                            },
                            None => false,
                        },
                    )
                    .await
            }
            MethodCode::CashOnDelivery => {
                service(state)
                    .save_cash_on_delivery(
                        session,
                        &csrf,
                        id,
                        revision,
                        &CashOnDeliveryInput {
                            common: input.clone(),
                        },
                    )
                    .await
            }
        }
    };
    match result {
        Ok(_) => Ok(Redirect::to(PATH).into_response()),
        Err(source_error @ (PaymentError::Invalid(_) | PaymentError::Stale)) => {
            page(
                state,
                session,
                Some(Editor {
                    code,
                    bank,
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
