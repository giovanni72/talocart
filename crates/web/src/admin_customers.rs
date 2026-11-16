//! Full administrator customer card. All writes use administrator capabilities.
use super::*;
use axum::{
    extract::{Form, Path},
    response::Redirect,
    routing::post,
};
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use talocart_commerce::{
    customer_addresses::{Address, AddressInput, CustomerAddresses},
    customer_billing::{BillingInput, CustomerBilling},
    orders::{OrderError, Orders, Summary},
};
use talocart_platform::{
    customer_fields::{self, Mode, Policy},
    identity::{
        CustomerError, IdentityError,
        customer_admin::{Customer, CustomerAdmin, Profile},
        customer_groups::{CustomerGroup, CustomerGroups, GroupError},
    },
};
type Result<T> = std::result::Result<T, Error>;
pub(super) enum Error {
    Customer(CustomerError),
    Order(OrderError),
}
impl From<CustomerError> for Error {
    fn from(error: CustomerError) -> Self {
        Self::Customer(error)
    }
}
impl From<IdentityError> for Error {
    fn from(error: IdentityError) -> Self {
        Self::Customer(error.into())
    }
}
impl From<OrderError> for Error {
    fn from(error: OrderError) -> Self {
        Self::Order(error)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::Order(error) => private(super::orders::error(error)),
            Self::Customer(CustomerError::Technical(error)) => {
                private(access::IdentityErrorResponse::from(error).into_response())
            }
            Self::Customer(CustomerError::Access(error)) => {
                private(access::IdentityErrorResponse::from(error).into_response())
            }
            Self::Customer(CustomerError::Identity(error)) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            Self::Customer(error) => private((status(&error), message(&error)).into_response()),
        }
    }
}
fn status(error: &CustomerError) -> StatusCode {
    match error {
        CustomerError::Stale | CustomerError::Duplicate | CustomerError::AddressLimit => {
            StatusCode::CONFLICT
        }
        CustomerError::NotFound => StatusCode::NOT_FOUND,
        CustomerError::Technical(error) => crate::access::technical_status(error),
        CustomerError::Access(error) => crate::access::access_status(error),
        CustomerError::Identity(error) => access::IdentityErrorResponse::status_for(error),
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
fn message(error: &CustomerError) -> String {
    i18n::text(match error {
        CustomerError::Stale => "ca-stale",
        CustomerError::Duplicate => "ca-duplicate",
        CustomerError::AddressLimit => "customer-address-limit",
        CustomerError::NotFound => "customer-not-found",
        _ => "ca-invalid",
    })
}
fn private(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r
}
fn admin(state: &AppState) -> CustomerAdmin {
    CustomerAdmin::new(state.pool.clone(), state.identity.clone())
}
fn billing(state: &AppState) -> CustomerBilling {
    CustomerBilling::new(state.pool.clone(), state.identity.clone())
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/customers/new",
            get(new_customer).post(create_customer),
        )
        .route("/admin/customers/{id}/profile", post(save_profile))
        .route("/admin/customers/{id}/billing", post(save_billing))
        .route("/admin/customers/{id}/billing/delete", post(delete_billing))
        .route("/admin/customers/{id}/access", post(access_action))
        .route(
            "/admin/customers/{id}/addresses/new",
            get(new_address).post(create_address),
        )
        .route(
            "/admin/customers/{id}/addresses/{address}",
            get(edit_address).post(update_address),
        )
        .route(
            "/admin/customers/{id}/addresses/{address}/delete",
            post(delete_address),
        )
        .layer(DefaultBodyLimit::max(32 * 1024))
}
#[derive(Template)]
#[template(path = "customer-admin-new.html")]
struct NewCustomer<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    profile: Profile,
    fields: Vec<Field>,
    groups: Vec<CustomerGroup>,
    countries: Vec<Country>,
    selected: i64,
    invalid_group: bool,
    invalid_type: bool,
    error: String,
    pending: bool,
}
async fn new_customer(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    creation_page(
        &state,
        &session,
        Profile {
            customer_type: "private".into(),
            active: true,
            ..Profile::default()
        },
        None,
        false,
    )
    .await
}
async fn creation_page(
    state: &AppState,
    session: &Session,
    profile: Profile,
    error: Option<CustomerError>,
    pending: bool,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::CustomersWrite)
        .await?;
    let groups = CustomerGroups::new(state.pool.clone(), state.identity.clone())
        .choices(session)
        .await
        .map_err(|source_error| match source_error {
            GroupError::Technical(source_error) => Error::Customer(source_error.into()),
            GroupError::Access(source_error) => Error::Customer(source_error.into()),
            GroupError::Identity(source_error) => Error::Customer(source_error.into()),
            _ => Error::Customer(IdentityError::Unavailable.into()),
        })?;
    let selected = profile.group_id.unwrap_or_else(|| {
        groups
            .iter()
            .find(|g| g.is_default)
            .map(|g| g.id)
            .unwrap_or(0)
    });
    let errors = match &error {
        Some(CustomerError::Fields(fields)) => fields.clone(),
        Some(CustomerError::Duplicate) => vec!["email"],
        _ => vec![],
    };
    let policy = customer_fields::load(&state.pool)
        .await?
        .for_administration();
    let mut fields = fields_profile(&profile, &policy, &errors);
    fields.push(field(
        "password",
        "ca-initial-password",
        "",
        "password",
        128,
        true,
        &errors,
    ));
    fields.push(field(
        "password_confirmation",
        "ca-confirm-password",
        "",
        "password",
        128,
        true,
        &errors,
    ));
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut response = render(NewCustomer {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "customers",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        invalid_group: errors.contains(&"group_id") || !groups.iter().any(|g| g.id == selected),
        invalid_type: errors.contains(&"customer_type"),
        profile,
        fields,
        groups,
        countries: vec![],
        selected,
        error: error.as_ref().map(message).unwrap_or_default(),
        pending,
    });
    *response.status_mut() = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    Ok(private(response))
}
async fn create_customer(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    // New accounts have no revision or confirmation actions. Reject ambiguous input.
    let extra = fields
        .iter()
        .any(|(k, _)| ["revision", "confirm"].contains(&k.as_str()));
    let mut f = submitted(
        &state,
        &session,
        fields,
        &[
            "email",
            "first_name",
            "last_name",
            "customer_type",
            "phone",
            "group_id",
            "active",
            "password",
            "password_confirmation",
        ],
    )
    .await?;
    let secret = f.values.remove("password").unwrap_or_default();
    let confirmation = f.values.remove("password_confirmation").unwrap_or_default();
    let input = Profile {
        group_id: f.values.get("group_id").and_then(|v| v.parse().ok()),
        email: f.values.get("email").cloned().unwrap_or_default(),
        first_name: f.values.get("first_name").cloned().unwrap_or_default(),
        last_name: f.values.get("last_name").cloned().unwrap_or_default(),
        phone: f.values.get("phone").cloned().unwrap_or_default(),
        customer_type: f.values.get("customer_type").cloned().unwrap_or_default(),
        active: f.values.get("active").is_some_and(|v| v == "true"),
    };
    if f.action() == "refresh" && !f.malformed && !extra {
        return creation_page(&state, &session, input, None, true).await;
    }
    let result = if f.malformed || extra || f.action() != "save" {
        Err(CustomerError::Fields(
            if f.values
                .get("group_id")
                .is_some_and(|v| v.parse::<i64>().is_err())
            {
                vec!["group_id"]
            } else {
                vec![]
            },
        ))
    } else if input.group_id.is_none() {
        Err(CustomerError::Fields(vec!["group_id"]))
    } else if secret != confirmation {
        Err(CustomerError::Fields(vec!["password_confirmation"]))
    } else {
        admin(&state)
            .create(&session, &f.csrf, &input, secret)
            .await
    };
    match result {
        Ok(id) => Ok(redirect(id, "customer-profile")),
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        ) => Err(error.into()),
        Err(error) => creation_page(&state, &session, input, Some(error), true).await,
    }
}
#[derive(Default, Deserialize)]
pub(super) struct CardQuery {
    #[serde(default = "first")]
    pub orders_page: i64,
    pub saved: Option<String>,
}
fn first() -> i64 {
    1
}
struct Field {
    name: &'static str,
    label: String,
    value: String,
    kind: &'static str,
    max: usize,
    required: bool,
    configured_required: bool,
    private_required: bool,
    business_required: bool,
    error_text: String,
    condition: &'static str,
    invalid: bool,
}
fn field(
    name: &'static str,
    label: &str,
    value: &str,
    kind: &'static str,
    max: usize,
    required: bool,
    errors: &[&str],
) -> Field {
    Field {
        name,
        label: i18n::text(label),
        value: value.into(),
        kind,
        max,
        required,
        configured_required: required,
        private_required: required,
        business_required: required,
        error_text: i18n::text(match name {
            "email" | "pec" => "ca-error-email",
            "password" => "customer-password-help",
            "password_confirmation" => "ca-password-mismatch",
            "phone" => "ca-error-phone",
            "billing_postal" | "postal" => "ca-error-postal",
            "tax_id" => "ca-error-tax",
            "recipient_code" => "ca-error-sdi",
            "vat_id" => "ca-error-vat",
            _ => "ca-field-error",
        }),
        condition: "always",
        invalid: errors.contains(&name),
    }
}
fn fields_profile(p: &Profile, policy: &Policy, errors: &[&str]) -> Vec<Field> {
    [
        (
            "first_name",
            "co-field-first-name",
            p.first_name.as_str(),
            "text",
            100,
        ),
        (
            "last_name",
            "co-field-last-name",
            p.last_name.as_str(),
            "text",
            100,
        ),
        ("email", "co-field-email", p.email.as_str(), "email", 254),
        ("phone", "ca-primary-phone", p.phone.as_str(), "tel", 40),
    ]
    .into_iter()
    .map(|(n, l, v, k, m)| {
        let mut f = field(
            n,
            l,
            v,
            k,
            m,
            n != "phone" && policy.mode(n, &p.customer_type) == Mode::Required,
            errors,
        );
        f.private_required = n != "phone" && policy.mode(n, "private") == Mode::Required;
        f.business_required = n != "phone" && policy.mode(n, "business") == Mode::Required;
        f
    })
    .collect()
}
const BILLING_FIELDS: &[(&str, &str, usize)] = &[
    ("billing_first_name", "co-field-billing-first-name", 100),
    ("billing_last_name", "co-field-billing-last-name", 100),
    ("billing_company", "co-field-billing-company", 160),
    ("billing_country", "co-field-country", 2),
    ("billing_street", "co-field-street", 160),
    ("billing_number", "co-field-number", 20),
    ("billing_extra", "co-field-extra", 160),
    ("billing_postal", "co-field-postal", 20),
    ("billing_city", "co-field-city", 100),
    ("billing_region", "co-field-region", 100),
    ("vat_id", "co-field-vat-id", 40),
    ("tax_id", "co-field-tax-id", 40),
    ("recipient_code", "co-field-recipient-code", 7),
    ("pec", "co-field-pec", 254),
];
fn fields_billing(
    input: &BillingInput,
    kind: &str,
    policy: &Policy,
    errors: &[&str],
) -> Vec<Field> {
    let values = serde_json::to_value(input).unwrap_or_default();
    BILLING_FIELDS
        .iter()
        .map(|&(n, l, m)| {
            let conditional = match n {
                "billing_region" => {
                    ["IT", "US", "CA", "AU"].contains(&input.billing_country.as_str())
                }
                "tax_id" | "recipient_code" | "pec" => input.billing_country == "IT",
                _ => true,
            };
            let mut field = field(
                n,
                l,
                values.get(n).and_then(|v| v.as_str()).unwrap_or(""),
                if n == "billing_country" {
                    "country"
                } else if n == "pec" {
                    "email"
                } else {
                    "text"
                },
                m,
                conditional && policy.mode(n, kind) == Mode::Required,
                errors,
            );
            field.configured_required = policy.mode(n, kind) == Mode::Required;
            field.condition = match n {
                "billing_region" => "region",
                "tax_id" | "recipient_code" | "pec" => "it",
                _ => "always",
            };
            field
        })
        .collect()
}
#[derive(Default)]
enum Draft {
    #[default]
    None,
    Profile(i64, Profile),
    Billing(i64, Box<BillingInput>),
    Group(i64, i64),
}
#[derive(Default)]
struct Feedback {
    draft: Draft,
    error: String,
    invalid: Vec<&'static str>,
    notice: &'static str,
    recovery: String,
    code: Option<StatusCode>,
}
impl Feedback {
    fn failure(draft: Draft, source_error: CustomerError) -> Self {
        Self {
            draft,
            error: message(&source_error),
            invalid: match &source_error {
                CustomerError::Fields(f) => f.clone(),
                CustomerError::Duplicate => vec!["email"],
                _ => vec![],
            },
            code: Some(status(&source_error)),
            ..Self::default()
        }
    }
}
#[derive(Template)]
#[template(path = "customer-admin-form.html")]
struct Card<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    member: Customer,
    groups: Vec<CustomerGroup>,
    selected: i64,
    invalid: bool,
    billing_open: bool,
    profile_pending: bool,
    initial_section: &'static str,
    profile: Profile,
    profile_revision: i64,
    profile_fields: Vec<Field>,
    billing: BillingInput,
    billing_revision: i64,
    billing_fields: Vec<Field>,
    countries: Vec<Country>,
    addresses: Vec<Address>,
    orders: Vec<Summary>,
    pagination: super::pagination::Pagination,
    can_orders: bool,
    can_write: bool,
    error: String,
    notice: String,
    recovery: String,
}
struct Country {
    code: String,
    label: String,
}
fn countries() -> Vec<Country> {
    let mut countries: Vec<Country> = talocart_platform::geography::COUNTRIES
        .iter()
        .map(|c| Country {
            code: (*c).into(),
            label: i18n::text(&format!("country-{}", c.to_ascii_lowercase())),
        })
        .collect();
    countries.sort_by_cached_key(|c| c.label.to_lowercase());
    countries
}
pub(super) async fn card(
    state: &AppState,
    session: &Session,
    id: i64,
    q: CardQuery,
) -> Result<Response> {
    let notice = if q.saved.as_deref() == Some("1") {
        "ca-saved"
    } else {
        ""
    };
    page(
        state,
        session,
        id,
        q.orders_page,
        Feedback {
            notice,
            ..Feedback::default()
        },
    )
    .await
}
pub(super) async fn group_form(
    state: &AppState,
    session: &Session,
    id: i64,
    submitted: Option<(i64, i64, GroupError)>,
) -> Result<Response> {
    let f = if let Some((selected, revision, source_error)) = submitted {
        let (code, key) = match source_error {
            GroupError::Stale => (StatusCode::CONFLICT, "cg-stale"),
            GroupError::NotFound => return Err(CustomerError::NotFound.into()),
            GroupError::Technical(source_error) => {
                return Err(Error::Customer(source_error.into()));
            }
            GroupError::Access(source_error) => return Err(Error::Customer(source_error.into())),
            GroupError::Identity(source_error) => return Err(source_error.into()),
            _ => (StatusCode::UNPROCESSABLE_ENTITY, "cg-invalid"),
        };
        Feedback {
            draft: Draft::Group(selected, revision),
            error: i18n::text(key),
            code: Some(code),
            ..Feedback::default()
        }
    } else {
        Feedback::default()
    };
    page(state, session, id, 1, f).await
}
async fn page(
    state: &AppState,
    session: &Session,
    id: i64,
    orders_page: i64,
    f: Feedback,
) -> Result<Response> {
    if !(1..=1_000_000).contains(&orders_page) {
        return Err(OrderError::ListFilters.into());
    }
    let member = admin(state).get(session, id).await?;
    let (group, groups) = CustomerGroups::new(state.pool.clone(), state.identity.clone())
        .member_editor(session, id)
        .await
        .map_err(|source_error| match source_error {
            GroupError::Technical(source_error) => Error::Customer(source_error.into()),
            GroupError::Access(source_error) => Error::Customer(source_error.into()),
            GroupError::Identity(source_error) => Error::Customer(source_error.into()),
            _ => Error::Customer(CustomerError::NotFound),
        })?;
    let stored = billing(state).get(session, id).await?;
    let mut billing_revision = stored.as_ref().map(|b| b.revision).unwrap_or(-1);
    let mut billing = stored.map(|b| b.input).unwrap_or_default();
    let mut profile = member.profile();
    let mut profile_revision = member.revision;
    let mut selected = group.group_id;
    let initial_section = match &f.draft {
        Draft::Billing(..) => "customer-billing",
        _ if matches!(f.notice, "ca-recovery-created" | "ca-revoked") => "customer-access",
        _ => "customer-profile",
    };
    let profile_pending = matches!(&f.draft, Draft::Profile(..) | Draft::Group(..));
    let billing_open = matches!(&f.draft, Draft::Billing(..));
    match f.draft {
        Draft::Profile(r, p) => {
            selected = p.group_id.unwrap_or(if f.invalid.contains(&"group_id") {
                0
            } else {
                selected
            });
            profile = p;
            profile_revision = r
        }
        Draft::Billing(r, b) => {
            billing = *b;
            billing_revision = r
        }
        Draft::Group(g, r) => {
            selected = g;
            profile_revision = r
        }
        Draft::None => {}
    }
    let policy = customer_fields::load(&state.pool)
        .await?
        .for_administration();
    let profile_fields = fields_profile(&profile, &policy, &f.invalid);
    let billing_fields = fields_billing(&billing, &member.customer_type, &policy, &f.invalid);
    let addresses = CustomerAddresses::new(state.pool.clone())
        .admin_list(&state.identity, session, id)
        .await?;
    let can_orders = session.allows(Permission::OrdersRead);
    let history = if can_orders {
        Orders::new(state.pool.clone(), state.identity.clone())
            .admin_customer_history(session, id, orders_page)
            .await
            .map_err(Error::Order)?
    } else {
        talocart_platform::pagination::Page {
            entries: vec![],
            total: 0,
            page: 1,
        }
    };
    let pagination = super::pagination::Pagination::new(
        history.total,
        history.page,
        20,
        &i18n::text("ca-orders"),
        &i18n::text("ca-orders-pages"),
        |p| format!("/admin/customers/{id}?orders_page={p}#customer-orders"),
    );
    let invalid = f.invalid.contains(&"group_id") || !groups.iter().any(|g| g.id == selected);
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let mut response = render(Card {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "customers",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        member,
        groups,
        selected,
        invalid,
        billing_open,
        profile_pending,
        initial_section,
        profile,
        profile_revision,
        profile_fields,
        billing,
        billing_revision,
        billing_fields,
        countries: countries(),
        addresses,
        orders: history.entries,
        pagination,
        can_orders,
        can_write: session.allows(Permission::CustomersWrite),
        error: f.error,
        notice: if f.notice.is_empty() {
            String::new()
        } else {
            i18n::text(f.notice)
        },
        recovery: f.recovery,
    });
    *response.status_mut() = f.code.unwrap_or(StatusCode::OK);
    Ok(private(response))
}
struct Submitted {
    csrf: String,
    revision: i64,
    values: BTreeMap<String, String>,
    malformed: bool,
}
impl Submitted {
    fn input<T: DeserializeOwned + Default>(&self) -> T {
        let mut object = serde_json::Map::new();
        for (k, v) in &self.values {
            if ["action", "confirm"].contains(&k.as_str()) {
                continue;
            }
            object.insert(
                k.clone(),
                if k == "group_id" {
                    v.parse::<i64>()
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null)
                } else if ["active", "invoice_requested", "is_default"].contains(&k.as_str()) {
                    serde_json::Value::Bool(v == "true")
                } else {
                    serde_json::Value::String(v.clone())
                },
            );
        }
        serde_json::from_value(serde_json::Value::Object(object)).unwrap_or_default()
    }
    fn confirm(&self) -> bool {
        self.values.get("confirm").is_some_and(|v| v == "true")
    }
    fn action(&self) -> &str {
        self.values
            .get("action")
            .map(String::as_str)
            .unwrap_or("save")
    }
}
async fn submitted(
    state: &AppState,
    session: &Session,
    fields: Vec<(String, String)>,
    allowed: &[&str],
) -> Result<Submitted> {
    state
        .identity
        .authorize(session, Permission::CustomersWrite)
        .await?;
    let mut values = BTreeMap::new();
    let mut malformed = false;
    for (k, v) in fields {
        if !["csrf", "revision", "action", "confirm"].contains(&k.as_str())
            && !allowed.contains(&k.as_str())
        {
            malformed = true;
        }
        if values.insert(k, v).is_some() {
            malformed = true;
        }
    }
    if values
        .get("group_id")
        .is_some_and(|v| v.parse::<i64>().is_err())
    {
        malformed = true;
    }
    let csrf = values.remove("csrf").unwrap_or_default();
    session.check_csrf(&csrf)?;
    let revision = values
        .remove("revision")
        .and_then(|v| v.parse().ok())
        .unwrap_or(i64::MIN);
    for key in ["active", "invoice_requested", "is_default", "confirm"] {
        if values.get(key).is_some_and(|v| v != "true") {
            malformed = true;
        }
    }
    Ok(Submitted {
        csrf,
        revision,
        values,
        malformed,
    })
}
fn redirect(id: i64, section: &str) -> Response {
    private(Redirect::to(&format!("/admin/customers/{id}?saved=1#{section}")).into_response())
}
async fn save_profile(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let f = submitted(
        &state,
        &session,
        fields,
        &[
            "first_name",
            "last_name",
            "email",
            "group_id",
            "customer_type",
            "phone",
            "active",
        ],
    )
    .await?;
    let input: Profile = f.input();
    if f.action() == "refresh" && !f.malformed {
        return page(
            &state,
            &session,
            id,
            1,
            Feedback {
                draft: Draft::Profile(f.revision, input),
                ..Feedback::default()
            },
        )
        .await;
    }
    let result = if f.malformed || f.action() != "save" {
        Err(CustomerError::Fields(
            if f.values
                .get("group_id")
                .is_some_and(|v| v.parse::<i64>().is_err())
            {
                vec!["group_id"]
            } else {
                vec![]
            },
        ))
    } else {
        admin(&state)
            .save(&session, &f.csrf, id, f.revision, &input)
            .await
    };
    match result {
        Ok(()) => Ok(redirect(id, "customer-profile")),
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        )
        | Err(error @ CustomerError::NotFound) => Err(error.into()),
        Err(error) => {
            page(
                &state,
                &session,
                id,
                1,
                Feedback::failure(Draft::Profile(f.revision, input), error),
            )
            .await
        }
    }
}
async fn save_billing(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let mut allowed: Vec<_> = BILLING_FIELDS.iter().map(|f| f.0).collect();
    allowed.push("invoice_requested");
    let f = submitted(&state, &session, fields, &allowed).await?;
    let input: BillingInput = f.input();
    if !f.malformed && f.action() == "refresh" {
        return page(
            &state,
            &session,
            id,
            1,
            Feedback {
                draft: Draft::Billing(f.revision, Box::new(input)),
                ..Feedback::default()
            },
        )
        .await;
    }
    let result = if f.malformed || f.action() != "save" {
        Err(CustomerError::Fields(vec![]))
    } else {
        billing(&state)
            .save(&session, &f.csrf, id, f.revision, &input)
            .await
    };
    match result {
        Ok(()) => Ok(redirect(id, "customer-billing")),
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        )
        | Err(error @ CustomerError::NotFound) => Err(error.into()),
        Err(error) => {
            page(
                &state,
                &session,
                id,
                1,
                Feedback::failure(Draft::Billing(f.revision, Box::new(input)), error),
            )
            .await
        }
    }
}
async fn delete_billing(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let f = submitted(&state, &session, fields, &[]).await?;
    let result = if f.malformed || !f.confirm() || f.action() != "save" {
        Err(CustomerError::Fields(vec!["confirm"]))
    } else {
        billing(&state)
            .delete(&session, &f.csrf, id, f.revision)
            .await
    };
    match result {
        Ok(()) => Ok(redirect(id, "customer-billing")),
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        )
        | Err(error @ CustomerError::NotFound) => Err(error.into()),
        Err(error) => {
            page(
                &state,
                &session,
                id,
                1,
                Feedback::failure(Draft::None, error),
            )
            .await
        }
    }
}
async fn access_action(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let f = submitted(&state, &session, fields, &[]).await?;
    let result = if f.malformed || !f.confirm() || !["recovery", "revoke"].contains(&f.action()) {
        Err(CustomerError::Fields(vec!["confirm"]))
    } else {
        admin(&state)
            .access(&session, &f.csrf, id, f.revision, f.action() == "recovery")
            .await
    };
    match result {
        Ok(raw) => {
            page(
                &state,
                &session,
                id,
                1,
                Feedback {
                    notice: if raw.is_some() {
                        "ca-recovery-created"
                    } else {
                        "ca-revoked"
                    },
                    recovery: raw.unwrap_or_default(),
                    ..Feedback::default()
                },
            )
            .await
        }
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        )
        | Err(error @ CustomerError::NotFound) => Err(error.into()),
        Err(error) => {
            page(
                &state,
                &session,
                id,
                1,
                Feedback::failure(Draft::None, error),
            )
            .await
        }
    }
}
#[derive(Template)]
#[template(path = "customer-admin-address.html")]
struct AddressPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    member: Customer,
    fields: Vec<Field>,
    countries: Vec<Country>,
    revision: i64,
    action: String,
    error: String,
    can_write: bool,
    address_default: bool,
}
const ADDRESS_FIELDS: &[(&str, &str, &str, usize)] = &[
    ("label", "customer-address-label", "label", 80),
    (
        "first_name",
        "co-field-recipient-first-name",
        "recipient_first_name",
        100,
    ),
    (
        "last_name",
        "co-field-recipient-last-name",
        "recipient_last_name",
        100,
    ),
    ("company", "co-field-company", "shipping_company", 160),
    ("phone", "co-field-phone", "phone", 40),
    ("country", "co-field-country", "shipping_country", 2),
    ("street", "co-field-street", "shipping_street", 160),
    ("number", "co-field-number", "shipping_number", 20),
    ("extra", "co-field-extra", "shipping_extra", 160),
    ("postal", "co-field-postal", "shipping_postal", 20),
    ("city", "co-field-city", "shipping_city", 100),
    ("region", "co-field-region", "shipping_region", 100),
];
async fn address_page(
    state: &AppState,
    session: &Session,
    id: i64,
    address: Option<i64>,
    draft: Option<(i64, AddressInput)>,
    error: Option<CustomerError>,
) -> Result<Response> {
    let member = admin(state).get(session, id).await?;
    let (revision, input) = if let Some(d) = draft {
        d
    } else if let Some(target) = address {
        let a = CustomerAddresses::new(state.pool.clone())
            .admin_list(&state.identity, session, id)
            .await?
            .into_iter()
            .find(|a| a.id == target)
            .ok_or(CustomerError::NotFound)?;
        (a.revision, a.input())
    } else {
        (-1, AddressInput::default())
    };
    let policy = customer_fields::load(&state.pool)
        .await?
        .for_administration();
    let values = serde_json::to_value(&input).unwrap_or_default();
    let errors = match &error {
        Some(CustomerError::Fields(f)) => f.clone(),
        _ => vec![],
    };
    let fields = ADDRESS_FIELDS
        .iter()
        .map(|&(n, l, key, max)| {
            let conditional = key != "shipping_region"
                || ["IT", "US", "CA", "AU"].contains(&input.country.as_str());
            let mut f = field(
                n,
                l,
                values.get(n).and_then(|v| v.as_str()).unwrap_or(""),
                if n == "country" {
                    "country"
                } else if n == "phone" {
                    "tel"
                } else {
                    "text"
                },
                max,
                n == "label"
                    || conditional && policy.mode(key, &member.customer_type) == Mode::Required,
                &errors,
            );
            f.configured_required =
                n == "label" || policy.mode(key, &member.customer_type) == Mode::Required;
            f.condition = if key == "shipping_region" {
                "region"
            } else {
                "always"
            };
            f.invalid |= errors.contains(&key);
            f
        })
        .collect();
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let code = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    let mut response = render(AddressPage {
        address_default: input.is_default,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "customers",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        member,
        fields,
        countries: countries(),
        revision,
        action: format!(
            "/admin/customers/{id}/addresses/{}",
            address.map(|v| v.to_string()).unwrap_or("new".into())
        ),
        error: error.as_ref().map(message).unwrap_or_default(),
        can_write: session.allows(Permission::CustomersWrite),
    });
    *response.status_mut() = code;
    Ok(private(response))
}
async fn new_address(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::CustomersWrite)
        .await?;
    address_page(&state, &session, id, None, None, None).await
}
async fn edit_address(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, address)): Path<(i64, i64)>,
) -> Result<Response> {
    address_page(&state, &session, id, Some(address), None, None).await
}
async fn address_save(
    state: AppState,
    session: Session,
    id: i64,
    address: Option<i64>,
    fields: Vec<(String, String)>,
) -> Result<Response> {
    let mut allowed: Vec<_> = ADDRESS_FIELDS.iter().map(|f| f.0).collect();
    allowed.push("is_default");
    let f = submitted(&state, &session, fields, &allowed).await?;
    let input: AddressInput = f.input();
    if !f.malformed && f.action() == "refresh" {
        return address_page(
            &state,
            &session,
            id,
            address,
            Some((f.revision, input)),
            None,
        )
        .await;
    }
    let result = if f.malformed || f.action() != "save" {
        Err(CustomerError::Fields(vec![]))
    } else if address.is_some() && f.revision < 0 || address.is_none() && f.revision != -1 {
        Err(CustomerError::Stale)
    } else {
        CustomerAddresses::new(state.pool.clone())
            .admin_save(
                &session,
                &f.csrf,
                id,
                address.map(|a| (a, f.revision)),
                &input,
            )
            .await
    };
    match result {
        Ok(_) => Ok(redirect(id, "customer-addresses")),
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        )
        | Err(error @ CustomerError::NotFound) => Err(error.into()),
        Err(error) => {
            address_page(
                &state,
                &session,
                id,
                address,
                Some((f.revision, input)),
                Some(error),
            )
            .await
        }
    }
}
async fn create_address(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    address_save(state, session, id, None, fields).await
}
async fn update_address(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, address)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    address_save(state, session, id, Some(address), fields).await
}
async fn delete_address(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, address)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let f = submitted(&state, &session, fields, &[]).await?;
    let result = if f.malformed || !f.confirm() || f.action() != "save" {
        Err(CustomerError::Fields(vec!["confirm"]))
    } else {
        CustomerAddresses::new(state.pool.clone())
            .admin_delete(&session, &f.csrf, id, address, f.revision)
            .await
    };
    match result {
        Ok(()) => Ok(redirect(id, "customer-addresses")),
        Err(
            error @ (CustomerError::Identity(_)
            | CustomerError::Technical(_)
            | CustomerError::Access(_)),
        )
        | Err(error @ CustomerError::NotFound) => Err(error.into()),
        Err(error) => address_page(&state, &session, id, Some(address), None, Some(error)).await,
    }
}
