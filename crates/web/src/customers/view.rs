//! Typed customer screens, shared field policy and Askama presentation.
use super::*;
struct Field {
    name: &'static str,
    label: String,
    value: String,
    kind: &'static str,
    max: usize,
    required: bool,
    error: bool,
    error_text: String,
    autocomplete: &'static str,
    private_mode: &'static str,
    business_mode: &'static str,
    hidden: bool,
}
fn field(
    name: &'static str,
    key: &str,
    value: &str,
    kind: &'static str,
    max: usize,
    autocomplete: &'static str,
    required: bool,
) -> Field {
    Field {
        name,
        label: i18n::text(key),
        value: value.into(),
        kind,
        max,
        required,
        error: false,
        error_text: String::new(),
        autocomplete,
        private_mode: if required { "required" } else { "optional" },
        business_mode: if required { "required" } else { "optional" },
        hidden: false,
    }
}
fn profile_fields(kind: &str, profile: &CustomerProfile) -> Vec<Field> {
    let mut fields = Vec::new();
    if ["register", "account"].contains(&kind) {
        fields.push(field(
            "first_name",
            "co-field-first-name",
            &profile.first_name,
            "text",
            100,
            "given-name",
            true,
        ));
        fields.push(field(
            "last_name",
            "co-field-last-name",
            &profile.last_name,
            "text",
            100,
            "family-name",
            true,
        ));
    }
    if ["register", "email", "login", "confirmation"].contains(&kind) {
        fields.push(field(
            "email",
            "co-field-email",
            &profile.email,
            "email",
            254,
            "email",
            true,
        ));
    }
    fields
}
fn credential_fields(kind: &str) -> Vec<Field> {
    let mut fields = Vec::new();
    if ["register", "login", "password", "reset"].contains(&kind) {
        if kind == "password" {
            fields.push(field(
                "current_password",
                "customer-current-password",
                "",
                "password",
                128,
                "current-password",
                true,
            ));
        }
        if kind == "reset" {
            fields.push(field(
                "token",
                "customer-reset-token",
                "",
                "text",
                64,
                "off",
                true,
            ));
        }
        fields.push(field(
            "password",
            if kind == "login" {
                "customer-password"
            } else {
                "customer-new-password"
            },
            "",
            "password",
            128,
            if kind == "login" {
                "current-password"
            } else {
                "new-password"
            },
            true,
        ));
        if kind != "login" {
            fields.push(field(
                "password_confirmation",
                "customer-password-confirmation",
                "",
                "password",
                128,
                "new-password",
                true,
            ));
        }
    }
    if kind == "email" {
        fields.push(field(
            "current_password",
            "customer-current-password",
            "",
            "password",
            128,
            "current-password",
            true,
        ));
    }
    fields
}
fn address_fields(kind: &str, address: &AddressInput) -> Vec<Field> {
    let mut fields = Vec::new();
    if kind == "address" {
        let a = address;
        for (name, key, value, max, auto, required) in [
            (
                "label",
                "customer-address-label",
                a.label.as_str(),
                80,
                "off",
                true,
            ),
            (
                "first_name",
                "co-field-recipient-first-name",
                a.first_name.as_str(),
                100,
                "given-name",
                true,
            ),
            (
                "last_name",
                "co-field-recipient-last-name",
                a.last_name.as_str(),
                100,
                "family-name",
                true,
            ),
            (
                "company",
                "co-field-company",
                a.company.as_str(),
                160,
                "organization",
                false,
            ),
            ("phone", "co-field-phone", a.phone.as_str(), 40, "tel", true),
            (
                "street",
                "co-field-street",
                a.street.as_str(),
                160,
                "address-line1",
                true,
            ),
            (
                "number",
                "co-field-number",
                a.number.as_str(),
                20,
                "off",
                true,
            ),
            (
                "extra",
                "co-field-extra",
                a.extra.as_str(),
                160,
                "address-line2",
                false,
            ),
            (
                "postal",
                "co-field-postal",
                a.postal.as_str(),
                20,
                "postal-code",
                true,
            ),
            (
                "city",
                "co-field-city",
                a.city.as_str(),
                100,
                "address-level2",
                true,
            ),
            (
                "region",
                "co-field-region",
                a.region.as_str(),
                100,
                "address-level1",
                false,
            ),
            (
                "country",
                "co-field-country",
                a.country.as_str(),
                2,
                "country",
                true,
            ),
        ] {
            fields.push(field(name, key, value, "text", max, auto, required));
        }
    }
    fields
}
#[derive(Template)]
#[template(path = "default/templates/customer.html")]
struct Page<'a> {
    confirmation_token: String,
    confirmation_required: bool,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    kind: PageKind,
    next: String,
    title: String,
    error: String,
    notice: String,
    csrf: String,
    revision: i64,
    action: String,
    fields: Vec<Field>,
    account: Option<CustomerAccount>,
    addresses: Vec<Address>,
    address_default: bool,
    history: Vec<Summary>,
    pagination: pagination::Pagination,
    order: Option<Order>,
    receipt_available: bool,
    countries: Vec<(String, String)>,
    customer_type: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthPage {
    Login,
    Register,
    Confirm,
    Confirmation,
    Reset,
}
impl AuthPage {
    fn key(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Register => "register",
            Self::Confirm => "confirm",
            Self::Confirmation => "confirmation",
            Self::Reset => "reset",
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum AuthAction {
    Login,
    Register,
}
impl AuthAction {
    pub(super) fn page(self) -> AuthPage {
        match self {
            Self::Login => AuthPage::Login,
            Self::Register => AuthPage::Register,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum ProfilePage {
    Account,
    Email,
    Password,
}
impl ProfilePage {
    fn key(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Email => "email",
            Self::Password => "password",
        }
    }
}
#[derive(Clone, Copy)]
enum PageKind {
    Authentication(AuthPage),
    Profile(ProfilePage),
    Addresses,
    Address,
    History,
    Order,
}
impl PageKind {
    fn key(self) -> &'static str {
        match self {
            Self::Authentication(kind) => kind.key(),
            Self::Profile(kind) => kind.key(),
            Self::Addresses => "addresses",
            Self::Address => "address",
            Self::History => "orders",
            Self::Order => "order",
        }
    }
}
/// Each screen owns only its valid input. Order references, address targets and history pages
/// cannot be attached accidentally to authentication/profile forms.
pub(super) enum View {
    Authentication {
        kind: AuthPage,
        input: Box<Input>,
        notice: &'static str,
    },
    Profile {
        kind: ProfilePage,
        input: Box<Input>,
        notice: &'static str,
    },
    Addresses,
    Address {
        target: Option<i64>,
        revision: i64,
        data: Box<AddressInput>,
    },
    History {
        page: i64,
    },
    Order {
        reference: String,
    },
}
impl View {
    pub(super) fn authentication(kind: AuthPage, input: Input, notice: &'static str) -> Self {
        Self::Authentication {
            kind,
            input: Box::new(input),
            notice,
        }
    }
    pub(super) fn profile(kind: ProfilePage, input: Input, notice: &'static str) -> Self {
        Self::Profile {
            kind,
            input: Box::new(input),
            notice,
        }
    }
    pub(super) fn address(target: Option<i64>, revision: i64, data: AddressInput) -> Self {
        Self::Address {
            target,
            revision,
            data: Box::new(data),
        }
    }
}

pub(super) async fn page(
    state: &AppState,
    session: &CustomerSession,
    cart_context: &CartContext,
    view: View,
    error: Option<&CustomerError>,
) -> Result<Response> {
    let s = session.clone();
    let account = if s.customer_id().is_some() {
        Some(state.customers().account(&s).await?)
    } else {
        None
    };
    let (page_kind, input, address, target, requested_page, reference, notice) = match view {
        View::Authentication {
            kind,
            input,
            notice,
        } => (
            PageKind::Authentication(kind),
            *input,
            AddressInput::default(),
            None,
            1,
            String::new(),
            notice,
        ),
        View::Profile {
            kind,
            input,
            notice,
        } => (
            PageKind::Profile(kind),
            *input,
            AddressInput::default(),
            None,
            1,
            String::new(),
            notice,
        ),
        View::Addresses => (
            PageKind::Addresses,
            Input::default(),
            AddressInput::default(),
            None,
            1,
            String::new(),
            "",
        ),
        View::Address {
            target,
            revision,
            data,
        } => (
            PageKind::Address,
            Input {
                revision,
                ..Default::default()
            },
            *data,
            target,
            1,
            String::new(),
            "",
        ),
        View::History { page } => (
            PageKind::History,
            Input::default(),
            AddressInput::default(),
            None,
            page,
            String::new(),
            "",
        ),
        View::Order { reference } => (
            PageKind::Order,
            Input::default(),
            AddressInput::default(),
            None,
            1,
            reference,
            "",
        ),
    };
    let kind = page_kind.key();
    let mut fields = Vec::new();
    let mut revision = input.revision;
    let mut profile = input.profile;
    if ["account", "email"].contains(&kind)
        && error.is_none()
        && input.action != "refresh"
        && let Some(c) = &account
    {
        profile = CustomerProfile {
            email: c.email.clone(),
            first_name: c.first_name.clone(),
            last_name: c.last_name.clone(),
            customer_type: c.customer_type.clone(),
        };
        revision = c.revision;
    }
    fields.extend(profile_fields(kind, &profile));
    fields.extend(credential_fields(kind));
    fields.extend(address_fields(kind, &address));
    let policy = talocart_platform::customer_fields::load(&state.pool).await?;
    let customer_type = if kind == "address" {
        account
            .as_ref()
            .map(|c| c.customer_type.clone())
            .unwrap_or_else(|| "private".into())
    } else {
        profile.customer_type.clone()
    };
    for f in &mut fields {
        let key = if kind == "address" {
            match f.name {
                "first_name" => "recipient_first_name",
                "last_name" => "recipient_last_name",
                "company" => "shipping_company",
                "country" => "shipping_country",
                "street" => "shipping_street",
                "number" => "shipping_number",
                "extra" => "shipping_extra",
                "postal" => "shipping_postal",
                "city" => "shipping_city",
                "region" => "shipping_region",
                _ => f.name,
            }
        } else {
            f.name
        };
        if (kind == "address" || matches!(kind, "register" | "account"))
            && talocart_platform::customer_fields::field(key).is_some()
        {
            f.private_mode = policy.mode(key, "private").key();
            f.business_mode = policy.mode(key, "business").key();
            let mode = policy.mode(key, &customer_type);
            f.hidden = mode == talocart_platform::customer_fields::Mode::Hidden;
            f.required = mode == talocart_platform::customer_fields::Mode::Required;
            if kind == "address" {
                let context = talocart_commerce::checkout_details::Details {
                    customer_type: customer_type.clone(),
                    shipping_country: address.country.clone(),
                    ..Default::default()
                };
                f.required = context.field_required(key, &policy);
            }
            if kind == "address"
                && matches!(f.name, "first_name" | "last_name")
                && !policy.overridden(key, &customer_type)
            {
                f.required = true;
            }
            if f.hidden {
                f.value.clear();
            }
        }
    }

    if let Some(CustomerError::Fields(errors)) = error {
        for f in &mut fields {
            f.error = errors.iter().any(|field_error| {
                *field_error == f.name
                    || field_error.strip_prefix("shipping_") == Some(f.name)
                    || field_error.strip_prefix("recipient_") == Some(f.name)
            });
        }
    }
    if matches!(error, Some(CustomerError::Duplicate)) {
        for f in &mut fields {
            f.error = f.name == "email";
        }
    }
    if matches!(
        error,
        Some(CustomerError::Identity(IdentityError::PasswordMismatch))
    ) {
        for f in &mut fields {
            f.error = f.name == "password_confirmation";
        }
    }
    if matches!(
        error,
        Some(CustomerError::Identity(IdentityError::CurrentPassword))
    ) {
        for f in &mut fields {
            f.error = f.name == "current_password";
        }
    }
    for f in &mut fields {
        if f.error {
            f.error_text = if matches!(
                error,
                Some(CustomerError::Duplicate)
                    | Some(CustomerError::Identity(
                        IdentityError::PasswordMismatch | IdentityError::CurrentPassword
                    ))
            ) {
                error.map(message).unwrap_or_default()
            } else {
                i18n::text(match f.name {
                    "email" => "customer-error-email",
                    "first_name" | "last_name" => "customer-error-name",
                    "password" => "customer-password-help",
                    "phone" => "customer-error-phone",
                    "postal" => "customer-error-postal",
                    "country" => "customer-error-country",
                    "label" => "customer-error-label",
                    "street" => "customer-error-street",
                    "number" => "customer-error-number",
                    "city" => "customer-error-city",
                    _ => "customer-field-error",
                })
            };
        }
    }
    let addresses = if kind == "addresses" {
        CustomerAddresses::new(state.pool.clone()).list(&s).await?
    } else {
        vec![]
    };
    let orders = Orders::new(state.pool.clone(), state.identity.clone());
    let history = if kind == "orders" {
        Some(
            orders
                .customer_history(&s, requested_page)
                .await
                .map_err(order_error)?,
        )
    } else {
        None
    };
    let p = pagination::Pagination::new(
        history.as_ref().map_or(0, |p| p.total),
        history.as_ref().map_or(1, |p| p.page),
        20,
        &i18n::text("customer-orders"),
        &i18n::text("customer-orders"),
        |p| store_language::path(format!("/account/orders?page={p}")),
    );
    let order = if kind == "order" {
        Some(
            orders
                .for_customer(&s, &reference)
                .await
                .map_err(order_error)?,
        )
    } else {
        None
    };
    let receipt_available = if let (Some(order), Ok(cart)) = (&order, cart_context.guest()) {
        match orders.for_guest(&cart, &order.reference).await {
            Ok(_) => true,
            Err(talocart_commerce::orders::OrderError::NotFound) => false,
            Err(source_error) => return Err(order_error(source_error)),
        }
    } else {
        false
    };
    let title = i18n::text(match kind {
        "confirm" => "customer-confirm-email",
        "confirmation" => "customer-confirmation-title",
        "register" => "customer-register",
        "login" => "customer-login",
        "reset" => "customer-reset",
        "password" => "customer-change-password",
        "email" => "customer-change-email",
        "addresses" | "address" => "customer-addresses",
        "orders" | "order" => "customer-orders",
        _ => "customer-account",
    });
    let action = match kind {
        "account" => "/account".into(),
        "address" => match target {
            Some(id) => format!("/account/addresses/{id}"),
            None => "/account/addresses/new".into(),
        },
        _ => format!("/account/{kind}"),
    };
    let name = current_name(state).await?;
    let mut countries: Vec<_> = talocart_platform::geography::COUNTRIES
        .iter()
        .map(|code| {
            (
                (*code).to_owned(),
                i18n::text(&format!("country-{}", code.to_ascii_lowercase())),
            )
        })
        .collect();
    countries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut r = render(Page {
        confirmation_token: input.token,
        confirmation_required:
            talocart_platform::identity::customer_registration::RegistrationSettings::load(
                &state.pool,
            )
            .await?
            .require_email_confirmation,
        store_name: &name,
        logo: current_logo(state).await?,
        theme: store_language::settings(state).await?,
        category_menu: navigation::CategoryMenu::load(state, "", false, "", false).await?,
        search_query: "",
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        kind: page_kind,
        next: return_target(&input.next),
        title,
        error: error.map(message).unwrap_or_default(),
        notice: if notice.is_empty() {
            String::new()
        } else {
            i18n::text(notice)
        },
        csrf: s.csrf,
        revision,
        action: store_language::path(action),
        fields,
        account,
        addresses,
        address_default: address.is_default,
        history: history.map(|p| p.entries).unwrap_or_default(),
        pagination: p,
        order,
        receipt_available,
        countries,
        customer_type,
    });
    if let Some(source_error) = error {
        *r.status_mut() = status(source_error);
        retry_for(source_error, &mut r);
    }
    Ok(private(r))
}
fn order_error(error: talocart_commerce::orders::OrderError) -> CustomerError {
    match error {
        talocart_commerce::orders::OrderError::NotFound => CustomerError::NotFound,
        talocart_commerce::orders::OrderError::Technical(error) => error.into(),
        talocart_commerce::orders::OrderError::Access(error) => error.into(),
        talocart_commerce::orders::OrderError::Identity(error) => error.into(),
        _ => IdentityError::Unavailable.into(),
    }
}
