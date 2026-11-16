use super::*;
use axum::{
    extract::{Form, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_commerce::{
    cart::{CartError, Carts},
    checkout_details::Details,
    checkout_methods::{Quote as CheckoutQuote, Selection},
    countries::CheckoutCountries,
    customer_addresses::{Address, CustomerAddresses},
};
use talocart_platform::{
    customer_fields::{self, Mode, Policy},
    geography,
};
#[derive(Deserialize)]
struct Submission {
    csrf: String,
    revision: i64,
    #[serde(default)]
    action: String,
    #[serde(default)]
    address_id: String,
    #[serde(flatten)]
    data: Details,
}
#[derive(Deserialize)]
struct MethodSubmission {
    csrf: String,
    revision: i64,
    #[serde(default)]
    shipping_method_id: String,
    #[serde(default)]
    payment_method_id: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    review: String,
    #[serde(default, deserialize_with = "customer_reference")]
    customer_id: Option<i64>,
}
fn customer_reference<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<i64>, D::Error> {
    let value = i64::deserialize(deserializer)?;
    Ok((value != 0).then_some(value))
}
#[derive(Deserialize, Default)]
struct MethodQuery {
    changed: Option<String>,
}
pub(crate) fn routes(prefix: &str) -> Router<AppState> {
    Router::new()
        .route(
            &format!("{prefix}/checkout"),
            get(index).head(head).post(save),
        )
        .route(
            &format!("{prefix}/checkout/methods"),
            get(methods_index).head(head).post(methods_save),
        )
        .merge(super::orders::public_routes(prefix))
        .layer(DefaultBodyLimit::max(128 * 1024))
}
async fn head() -> Response {
    private(StatusCode::OK.into_response())
}
use crate::http_policy::private;
fn failure(error: CartError) -> Response {
    let status = match error {
        CartError::Forbidden => StatusCode::FORBIDDEN,
        CartError::Stale => StatusCode::CONFLICT,
        CartError::Identity(_) | CartError::Technical(_) => StatusCode::SERVICE_UNAVAILABLE,
        CartError::Access(error) => access::access_status(&error),
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    private((status, i18n::text("co-error")).into_response())
}
struct Field {
    name: &'static str,
    label: String,
    value: String,
    kind: &'static str,
    autocomplete: &'static str,
    max: usize,
    error: bool,
    countries: Vec<Choice>,
    private_mode: &'static str,
    business_mode: &'static str,
    hidden: bool,
    required: bool,
}
struct Choice {
    code: String,
    label: String,
}
struct Group {
    key: &'static str,
    title: String,
    fields: Vec<Field>,
}
#[derive(Template)]
#[template(path = "default/templates/checkout-details.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    data: Details,
    groups: Vec<Group>,
    addresses: Vec<Address>,
    selected_address_id: i64,
    csrf: String,
    revision: i64,
    error: String,
    stale: bool,
    destinations_empty: bool,
    checkout: CheckoutQuote,
}
#[derive(Template)]
#[template(path = "default/templates/checkout-methods-body.html")]
struct MethodsBody {
    embedded_selected: bool,
    data: Details,
    checkout: CheckoutQuote,
    csrf: String,
    error: String,
    stale: bool,
}
#[derive(Template)]
#[template(path = "default/templates/checkout-methods.html")]
struct MethodsPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    view: MethodsBody,
}
fn redirect_details() -> Response {
    private(Redirect::to(&store_language::path("/checkout")).into_response())
}
fn selection(shipping: &str, payment: &str) -> Result<Selection, CartError> {
    fn parse(value: &str) -> Result<Option<i64>, CartError> {
        if value.is_empty() {
            return Ok(None);
        }
        value
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .map(Some)
            .ok_or(CartError::Invalid)
    }
    Ok(Selection {
        shipping_method_id: parse(shipping)?,
        payment_method_id: parse(payment)?,
    })
}
fn groups(data: &Details, errors: &[&str], shipping: &[String], policy: &Policy) -> Vec<Group> {
    type FieldSpec = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        usize,
    );
    let specifications: &[(&str, &[FieldSpec])] = &[
        (
            "contact",
            &[
                ("email", "email", "email", "email", 254),
                ("first_name", "first_name", "text", "given-name", 100),
                ("last_name", "last_name", "text", "family-name", 100),
                ("phone", "phone", "tel", "tel", 40),
            ],
        ),
        (
            "shipping",
            &[
                (
                    "recipient_first_name",
                    "recipient_first_name",
                    "text",
                    "shipping given-name",
                    100,
                ),
                (
                    "recipient_last_name",
                    "recipient_last_name",
                    "text",
                    "shipping family-name",
                    100,
                ),
                (
                    "shipping_company",
                    "company",
                    "text",
                    "shipping organization",
                    160,
                ),
                (
                    "shipping_country",
                    "country",
                    "country",
                    "shipping country",
                    2,
                ),
                (
                    "shipping_street",
                    "street",
                    "text",
                    "shipping address-line1",
                    160,
                ),
                ("shipping_number", "number", "text", "off", 20),
                (
                    "shipping_extra",
                    "extra",
                    "text",
                    "shipping address-line2",
                    160,
                ),
                (
                    "shipping_postal",
                    "postal",
                    "text",
                    "shipping postal-code",
                    20,
                ),
                (
                    "shipping_city",
                    "city",
                    "text",
                    "shipping address-level2",
                    100,
                ),
                (
                    "shipping_region",
                    "region",
                    "text",
                    "shipping address-level1",
                    100,
                ),
                ("delivery_notes", "delivery_notes", "textarea", "off", 1000),
            ],
        ),
        (
            "private",
            &[
                (
                    "billing_first_name",
                    "billing_first_name",
                    "text",
                    "billing given-name",
                    100,
                ),
                (
                    "billing_last_name",
                    "billing_last_name",
                    "text",
                    "billing family-name",
                    100,
                ),
            ],
        ),
        (
            "business",
            &[
                (
                    "billing_company",
                    "billing_company",
                    "text",
                    "billing organization",
                    160,
                ),
                ("vat_id", "vat_id", "text", "off", 40),
            ],
        ),
        (
            "billing",
            &[
                (
                    "billing_country",
                    "country",
                    "country",
                    "billing country",
                    2,
                ),
                (
                    "billing_street",
                    "street",
                    "text",
                    "billing address-line1",
                    160,
                ),
                ("billing_number", "number", "text", "off", 20),
                (
                    "billing_extra",
                    "extra",
                    "text",
                    "billing address-line2",
                    160,
                ),
                (
                    "billing_postal",
                    "postal",
                    "text",
                    "billing postal-code",
                    20,
                ),
                (
                    "billing_city",
                    "city",
                    "text",
                    "billing address-level2",
                    100,
                ),
                (
                    "billing_region",
                    "region",
                    "text",
                    "billing address-level1",
                    100,
                ),
            ],
        ),
        ("fiscal", &[("tax_id", "tax_id", "text", "off", 40)]),
        (
            "code",
            &[("recipient_code", "recipient_code", "text", "off", 7)],
        ),
        ("pec", &[("pec", "pec", "email", "off", 254)]),
    ];
    specifications
        .iter()
        .map(|(key, fs)| Group {
            key,
            title: i18n::text(&format!("co-group-{key}")),
            fields: fs
                .iter()
                .map(|(name, label, kind, autocomplete, max)| {
                    let codes: Vec<String> = if *name == "shipping_country" {
                        shipping.to_vec()
                    } else {
                        geography::COUNTRIES
                            .iter()
                            .map(|c| (*c).to_owned())
                            .collect()
                    };
                    Field {
                        name,
                        label: i18n::text(&format!("co-field-{}", label.replace('_', "-"))),
                        value: data.value(name).to_owned(),
                        kind,
                        autocomplete,
                        max: *max,
                        error: errors.contains(name),
                        private_mode: policy.mode(name, "private").key(),
                        business_mode: policy.mode(name, "business").key(),
                        hidden: policy.mode(name, &data.customer_type) == Mode::Hidden,
                        required: data.field_required(name, policy),
                        countries: if *kind == "country" {
                            codes
                                .into_iter()
                                .map(|code| Choice {
                                    label: i18n::text(&format!(
                                        "country-{}",
                                        code.to_ascii_lowercase()
                                    )),
                                    code,
                                })
                                .collect()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect(),
        })
        .collect()
}
async fn page(
    state: &AppState,
    submitted: Option<Details>,
    errors: &[&str],
    error: &str,
    status: StatusCode,
    cart_context: &cart::CartContext,
    customer: &super::customers::OptionalCustomer,
) -> Result<Response, CartError> {
    let guest = cart_context.guest()?;
    let carts = Carts::new(state.pool.clone())
        .with_online(state.online_available().await)
        .with_online_providers(state.online_providers().await)
        .with_online_limits(
            state
                .online
                .as_ref()
                .and_then(|online| online.amount_limits()),
        );
    let (mut data, saved_draft) = match submitted {
        Some(d) => (d, false),
        None => match carts.details(&guest).await? {
            Some(d) => (d, true),
            None => {
                let mut d = Details::default();
                if let Some(session) = customer.0.as_ref()
                    && session.customer_id().is_some()
                {
                    d = talocart_commerce::customer_billing::CustomerBilling::new(
                        state.pool.clone(),
                        state.identity.clone(),
                    )
                    .initial(session)
                    .await
                    .map_err(customer_error)?;
                }
                (d, false)
            }
        },
    };
    let addresses = match customer.authenticated() {
        Some(session) => CustomerAddresses::new(state.pool.clone())
            .list(session)
            .await
            .map_err(customer_error)?,
        None => Vec::new(),
    };
    let policy = customer_fields::load(&state.pool).await?;
    // Only a persisted checkout draft has already been submitted. Profile
    // prefill and non-validating form actions can legitimately be incomplete.
    let detected = if errors.is_empty() && saved_draft {
        data.clone()
            .validate_with(&policy)
            .err()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let errors = if errors.is_empty() {
        detected.as_slice()
    } else {
        errors
    };
    // A shipping field may be hidden/optional while billing requires it. If the
    // shared address cannot satisfy billing, reveal an independent postal form.
    let postal = [
        "country", "street", "number", "extra", "postal", "city", "region",
    ];
    if data.billing_same
        && errors.iter().any(|key| {
            key.strip_prefix("billing_").is_some_and(|suffix| {
                let shipping = format!("shipping_{suffix}");
                postal.contains(&suffix)
                    && policy.mode(&shipping, &data.customer_type) != Mode::Required
                    && (policy.mode(&shipping, &data.customer_type) == Mode::Hidden
                        || data.value(&shipping).trim().is_empty())
            })
        })
    {
        let normalized = data.clone().normalize_with(&policy);
        for suffix in postal {
            let key = format!("billing_{suffix}");
            if let Some(value) = data.value_mut(&key) {
                *value = normalized.value(&key).to_owned();
            }
        }
        data.billing_same = false;
    }
    let detected_message = i18n::text("cf-review-fields");
    let error = if error.is_empty() && !errors.is_empty() {
        detected_message.as_str()
    } else {
        error
    };
    let checkout = carts
        .checkout_quote(
            &guest,
            store_language::content_language(),
            &data.shipping_country,
            None,
        )
        .await?;
    let shipping = CheckoutCountries::new(state.pool.clone(), state.identity.clone())
        .destinations()
        .await
        .map_err(|_| CartError::Unavailable)?;
    let state = state.localized();
    let name = current_name(&state).await?;
    let revision = guest.revision;
    let mut r = render(Page {
        store_name: &name,
        logo: current_logo(&state).await?,
        theme: store_language::settings(&state).await?,
        category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
        search_query: "",
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        groups: groups(&data, errors, &shipping, &policy),
        selected_address_id: addresses
            .iter()
            .find(|a| a.matches_checkout(&data))
            .map(|a| a.id)
            .unwrap_or(0),
        addresses,
        data,
        csrf: guest.csrf,
        revision,
        checkout,
        stale: status == StatusCode::CONFLICT,
        destinations_empty: shipping.is_empty(),
        error: error.into(),
    });
    *r.status_mut() = status;
    Ok(private(r))
}
fn customer_error(error: talocart_platform::identity::CustomerError) -> CartError {
    use talocart_platform::identity::CustomerError;
    match error {
        CustomerError::Technical(error) => CartError::Technical(error),
        CustomerError::Access(error) => CartError::Access(error),
        CustomerError::Identity(error) => CartError::Identity(error),
        _ => CartError::Forbidden,
    }
}
async fn index(
    State(state): State<AppState>,
    cart_context: cart::CartContext,
    customer: super::customers::OptionalCustomer,
) -> Response {
    if let Err(error) = cart::establish(&state, &cart_context, customer.0.as_ref()).await {
        return failure(error.into());
    }
    page(
        &state,
        None,
        &[],
        "",
        StatusCode::OK,
        &cart_context,
        &customer,
    )
    .await
    .unwrap_or_else(failure)
}
async fn save(
    State(state): State<AppState>,
    cart_context: cart::CartContext,
    customer: super::customers::OptionalCustomer,
    Form(mut f): Form<Submission>,
) -> Response {
    let guest = match cart_context.guest() {
        Ok(g) => g,
        Err(error) => return failure(error),
    };
    if let Err(error) = guest.check_csrf(&f.csrf) {
        return failure(error);
    }
    if f.action == "address" {
        let Some(session) = customer.authenticated() else {
            return failure(CartError::Forbidden);
        };
        let Some(id) = f.address_id.parse::<i64>().ok().filter(|id| *id > 0) else {
            return page(
                &state,
                Some(f.data),
                &[],
                &i18n::text("co-address-required"),
                StatusCode::UNPROCESSABLE_ENTITY,
                &cart_context,
                &customer,
            )
            .await
            .unwrap_or_else(failure);
        };
        let address = match CustomerAddresses::new(state.pool.clone())
            .get(session, id)
            .await
        {
            Ok(address) => address,
            Err(error) => return super::customers::failure(error),
        };
        address.apply_to_checkout(&mut f.data);
        return page(
            &state,
            Some(f.data),
            &[],
            "",
            StatusCode::OK,
            &cart_context,
            &customer,
        )
        .await
        .unwrap_or_else(failure);
    }
    if f.action == "refresh" {
        return page(
            &state,
            Some(f.data),
            &[],
            "",
            StatusCode::OK,
            &cart_context,
            &customer,
        )
        .await
        .unwrap_or_else(failure);
    }
    if !matches!(f.action.as_str(), "" | "continue" | "save") {
        return failure(CartError::Invalid);
    }
    let policy = match customer_fields::load(&state.pool).await {
        Ok(p) => p,
        Err(error) => return failure(error.into()),
    };
    let data = match f.data.clone().validate_with(&policy) {
        Ok(d) => d,
        Err(fields) => {
            return page(
                &state,
                Some(f.data),
                &fields,
                &i18n::text("co-error"),
                StatusCode::UNPROCESSABLE_ENTITY,
                &cart_context,
                &customer,
            )
            .await
            .unwrap_or_else(failure);
        }
    };
    match Carts::new(state.pool.clone())
        .save_details(&guest, &f.csrf, f.revision, data)
        .await
    {
        Ok(()) => private(Redirect::to(&store_language::path("/checkout/methods")).into_response()),
        Err(CartError::Invalid) => {
            let current = match customer_fields::load(&state.pool).await {
                Ok(p) => p,
                Err(error) => return failure(error.into()),
            };
            let errors = f
                .data
                .clone()
                .validate_with(&current)
                .err()
                .unwrap_or_default();
            page(
                &state,
                Some(f.data),
                &errors,
                &i18n::text("cf-review-fields"),
                StatusCode::UNPROCESSABLE_ENTITY,
                &cart_context,
                &customer,
            )
            .await
            .unwrap_or_else(failure)
        }
        Err(CartError::Stale) => page(
            &state,
            Some(f.data),
            &[],
            &i18n::text("co-stale"),
            StatusCode::CONFLICT,
            &cart_context,
            &customer,
        )
        .await
        .unwrap_or_else(failure),
        Err(CartError::Unavailable) => page(
            &state,
            Some(f.data),
            &["shipping_country"],
            &i18n::text("co-country-unavailable"),
            StatusCode::UNPROCESSABLE_ENTITY,
            &cart_context,
            &customer,
        )
        .await
        .unwrap_or_else(failure),
        Err(error) => failure(error),
    }
}
fn fragment(headers: &HeaderMap) -> bool {
    headers.get("hx-request").is_some_and(|v| v == "true")
}
async fn methods_page(
    state: &AppState,
    checkout: Option<(Details, CheckoutQuote)>,
    selected: Option<Selection>,
    error: &str,
    status: StatusCode,
    partial: bool,
    cart_context: &cart::CartContext,
) -> Result<Response, CartError> {
    let guest = cart_context.guest()?;
    let carts = Carts::new(state.pool.clone())
        .with_online(state.online_available().await)
        .with_online_providers(state.online_providers().await)
        .with_online_limits(
            state
                .online
                .as_ref()
                .and_then(|online| online.amount_limits()),
        );
    let (data, checkout) = match checkout {
        Some(pair) => pair,
        None => match carts
            .checkout_step(&guest, store_language::content_language(), selected)
            .await?
        {
            Some(pair) => pair,
            None => return Ok(redirect_details()),
        },
    };
    let view = MethodsBody {
        embedded_selected: state.embedded_payments()
            && checkout.payments.iter().any(|m| m.online && m.selected),
        data,
        checkout,
        csrf: guest.csrf,
        error: error.into(),
        stale: status == StatusCode::CONFLICT && error == i18n::text("co-stale"),
    };
    let mut response = if partial {
        render(view)
    } else {
        let state = state.localized();
        let name = current_name(&state).await?;
        render(MethodsPage {
            store_name: &name,
            logo: current_logo(&state).await?,
            theme: store_language::settings(&state).await?,
            category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
            search_query: "",
            search_action: "/search",
            search_filters: navigation::SearchFilters::default(),
            view,
        })
    };
    *response.status_mut() = status;
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("HX-Request"));
    Ok(private(response))
}
async fn methods_index(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MethodQuery>,
    cart_context: cart::CartContext,
) -> Response {
    let guest = match cart_context.guest() {
        Ok(g) => g,
        Err(_) => return redirect_details(),
    };
    let checkout = if query.changed.as_deref() == Some("1") {
        match Carts::new(state.pool.clone())
            .with_online(state.online_available().await)
            .with_online_providers(state.online_providers().await)
            .with_online_limits(
                state
                    .online
                    .as_ref()
                    .and_then(|online| online.amount_limits()),
            )
            .checkout_step(&guest, store_language::content_language(), None)
            .await
        {
            Ok(Some((data, mut quote))) => {
                // Presentation-only notice after a native update normalized stale choices.
                quote.invalid_selection = true;
                Some((data, quote))
            }
            Ok(None) => return redirect_details(),
            Err(error) => return failure(error),
        }
    } else {
        None
    };
    methods_page(
        &state,
        checkout,
        None,
        "",
        StatusCode::OK,
        fragment(&headers),
        &cart_context,
    )
    .await
    .unwrap_or_else(failure)
}
async fn methods_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    cart_context: cart::CartContext,
    customer: super::customers::OptionalCustomer,
    Form(f): Form<MethodSubmission>,
) -> Response {
    use talocart_commerce::orders::{Confirmation, OrderError, Orders};
    let guest = match cart_context.guest() {
        Ok(g) => g,
        Err(error) => return failure(error),
    };
    if let Err(error) = guest.check_csrf(&f.csrf) {
        return failure(error);
    }
    let selected = match selection(&f.shipping_method_id, &f.payment_method_id) {
        Ok(s) => s,
        Err(error) => return failure(error),
    };
    let partial = fragment(&headers);
    let carts = Carts::new(state.pool.clone());
    if f.action == "update" {
        let carts = carts
            .with_online(state.online_available().await)
            .with_online_providers(state.online_providers().await)
            .with_online_limits(
                state
                    .online
                    .as_ref()
                    .and_then(|online| online.amount_limits()),
            );
        return match carts
            .update_checkout_choices(
                &guest,
                &f.csrf,
                f.revision,
                selected,
                store_language::content_language(),
            )
            .await
        {
            Ok((_, quote)) if !partial => {
                let path = if quote.invalid_selection {
                    "/checkout/methods?changed=1"
                } else {
                    "/checkout/methods"
                };
                let mut response =
                    private(Redirect::to(&store_language::path(path)).into_response());
                response
                    .headers_mut()
                    .append(header::VARY, HeaderValue::from_static("HX-Request"));
                response
            }
            Ok(quote) => methods_page(
                &state,
                Some(quote),
                None,
                "",
                StatusCode::OK,
                true,
                &cart_context,
            )
            .await
            .unwrap_or_else(failure),
            Err(CartError::Stale) => methods_page(
                &state,
                None,
                Some(selected),
                &i18n::text("co-stale"),
                StatusCode::CONFLICT,
                partial,
                &cart_context,
            )
            .await
            .unwrap_or_else(failure),
            Err(CartError::Unavailable) => methods_page(
                &state,
                None,
                Some(selected),
                &i18n::text("co-country-unavailable"),
                StatusCode::UNPROCESSABLE_ENTITY,
                partial,
                &cart_context,
            )
            .await
            .unwrap_or_else(failure),
            Err(CartError::Invalid) => redirect_details(),
            Err(error) => failure(error),
        };
    }
    if f.action != "confirm" {
        return failure(CartError::Invalid);
    }
    let policy = match customer_fields::load(&state.pool).await {
        Ok(p) => p,
        Err(error) => return failure(error.into()),
    };
    let data = match carts.details(&guest).await {
        Ok(Some(d)) => match d.validate_with(&policy) {
            Ok(d) => d,
            Err(_) => return redirect_details(),
        },
        Ok(None) => return redirect_details(),
        Err(error) => return failure(error),
    };
    let customer_session = customer.0.as_ref();
    if customer_session.and_then(|s| s.customer_id()) != f.customer_id {
        return failure(CartError::Forbidden);
    }
    let orders = Orders::new(state.pool.clone(), state.identity.clone());
    // Read presentation availability before taking the lease: never reacquire a module
    // request gate while a writer may be queued behind this confirmation.
    let mut providers = state.online_providers().await;
    // Protect the provider generation until the order is committed. No external I/O in SQL transactions.
    let (provider_guard, online_available) = match state.online.as_ref().filter(|s| s.enabled()) {
        Some(service) => match service
            .confirmation_gate_for(selected.payment_method_id)
            .await
        {
            Ok((guard, accepting)) => (Some(guard), accepting),
            Err(error) => return orders::error(error),
        },
        None => (None, false),
    };
    if online_available && let (Some(providers), Some(service)) = (&mut providers, &state.online) {
        let service = match service.for_method(selected.payment_method_id).await {
            Ok(service) => service,
            Err(error) => return orders::error(error),
        };
        providers.insert(service.provider_id(), service.amount_limits());
    }
    let orders = orders.with_online_providers(providers).with_online_limits(
        state
            .online
            .as_ref()
            .and_then(|online| online.amount_limits()),
    );
    let input = Confirmation {
        guest: &guest,
        csrf: &f.csrf,
        revision: f.revision,
        data,
        selection: selected,
        review: &f.review,
        language: store_language::content_language(),
        online_available,
    };
    let result = if let Some(customer) = customer_session.filter(|s| s.customer_id().is_some()) {
        orders.confirm_for_customer(input, customer).await
    } else {
        orders.confirm(input).await
    };
    // Rendering may query the provider again; the transaction has finished.
    drop(provider_guard);
    match result {
        Ok(order) => private(
            Redirect::to(&store_language::path(format!(
                "/checkout/orders/{}{}",
                order.reference,
                if order.is_online() && state.embedded_payments() {
                    "/payment"
                } else {
                    ""
                }
            )))
            .into_response(),
        ),
        Err(OrderError::Cart(CartError::Forbidden)) => failure(CartError::Forbidden),
        Err(error) => {
            let (key, status) = match error {
                OrderError::Review => ("ord-review", StatusCode::CONFLICT),
                OrderError::Cart(CartError::Stale) => ("co-stale", StatusCode::CONFLICT),
                OrderError::Notification
                | OrderError::Identity(_)
                | OrderError::Technical(_)
                | OrderError::Access(_)
                | OrderError::Cart(CartError::Identity(_))
                | OrderError::Cart(CartError::Technical(_))
                | OrderError::Cart(CartError::Access(_))
                | OrderError::Invalid => ("ord-error", StatusCode::SERVICE_UNAVAILABLE),
                _ => ("ord-recheck", StatusCode::UNPROCESSABLE_ENTITY),
            };
            methods_page(
                &state,
                None,
                Some(selected),
                &i18n::text(key),
                status,
                partial,
                &cart_context,
            )
            .await
            .unwrap_or_else(failure)
        }
    }
}
