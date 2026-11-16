//! Native administrative editor; domain services own all data access and publication.
use super::*;
use crate::http_policy::private;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use talocart_commerce::{
    checkout_details::Details,
    orders::{
        OrderError,
        editor::{
            Draft, DraftData, DraftLine, DraftSummary, EDITOR_PAGE_SIZE, EditorPage, MethodChoice,
            Preview, ProductChoice, Publish, RevisionSnapshot,
        },
    },
};
use talocart_platform::{
    customer_fields::{self, Mode},
    identity::customer_admin::{Customer, CustomerAdmin},
};
fn service(state: &AppState) -> talocart_commerce::orders::Orders {
    super::orders::service(state)
}
fn invalid() -> OrderError {
    OrderError::EditorFields(vec!["form".into()])
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/orders/new", post(create))
        .route("/admin/orders/{id}/edit", post(edit))
        .route("/admin/orders/{id}/summary", post(summary))
        .route("/admin/orders/drafts", get(index))
        .route("/admin/orders/drafts/{id}", get(show).post(save))
        .route("/admin/orders/drafts/{id}/confirm", post(publish))
        .layer(DefaultBodyLimit::max(256 * 1024))
}
#[derive(Deserialize)]
struct Start {
    csrf: String,
    #[serde(default)]
    revision: i64,
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(input): Form<Start>,
) -> Response {
    match service(&state)
        .create_draft(&session, &input.csrf, None, i18n::locale())
        .await
    {
        Ok(id) => private(Redirect::to(&format!("/admin/orders/drafts/{id}")).into_response()),
        Err(e) => super::orders::error(e),
    }
}
async fn edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(input): Form<Start>,
) -> Response {
    match service(&state)
        .create_draft(
            &session,
            &input.csrf,
            Some((id, input.revision)),
            i18n::locale(),
        )
        .await
    {
        Ok(id) => private(Redirect::to(&format!("/admin/orders/drafts/{id}")).into_response()),
        Err(e) => super::orders::error(e),
    }
}
#[derive(Template)]
#[template(path = "order-drafts.html")]
struct DraftsPage<'a> {
    pagination: super::pagination::Pagination,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    entries: Vec<DraftSummary>,
}
#[derive(Default, Deserialize)]
struct DraftsQuery {
    page: Option<i64>,
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<DraftsQuery>,
) -> Response {
    let result = async {
        let drafts = service(&state)
            .drafts(&session, query.page.unwrap_or(1))
            .await?;
        let shell = AdminShell::load(&state, &session).await?;
        Ok::<_, OrderError>(private(render(DraftsPage {
            store_name: &shell.store_name,
            logo: shell.logo,
            csrf: shell.csrf,
            section: "orders",
            theme: shell.theme,
            nav: shell.nav,
            account: shell.account,
            pagination: super::pagination::Pagination::new(
                drafts.total,
                drafts.page,
                EDITOR_PAGE_SIZE,
                &i18n::text("oe-drafts"),
                &i18n::text("oe-drafts"),
                |page| format!("/admin/orders/drafts?page={page}"),
            ),
            entries: drafts.entries,
        })))
    }
    .await;
    result.unwrap_or_else(super::orders::error)
}
#[derive(Default, Deserialize)]
struct EditorQuery {
    #[serde(default)]
    products: String,
    #[serde(default)]
    customers: String,
    preview: Option<u8>,
    saved: Option<u8>,
}
struct Country {
    code: String,
    label: String,
}
struct Field {
    name: &'static str,
    label: String,
    value: String,
    kind: &'static str,
    invalid: bool,
    required: bool,
}
struct Group {
    label: String,
    fields: Vec<Field>,
}
#[derive(Template)]
#[template(path = "order-editor.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    draft: Draft,
    groups: Vec<Group>,
    countries: Vec<Country>,
    shipping: Vec<MethodChoice>,
    payments: Vec<MethodChoice>,
    products: Vec<ProductChoice>,
    customers: Vec<Customer>,
    query: EditorQuery,
    preview: Option<Preview>,
    can_customers: bool,
    error: String,
    invalid: Vec<String>,
    title: String,
}
fn message(error: &OrderError) -> &'static str {
    match error {
        OrderError::Notification => "oe-mail-unavailable",
        OrderError::Review => "oe-review-changed",
        OrderError::Cart(talocart_commerce::cart::CartError::Stale) => "oe-stale",
        OrderError::Cart(talocart_commerce::cart::CartError::Stock) => "oe-stock-error",
        OrderError::Cart(talocart_commerce::cart::CartError::Quantity) => "oe-quantity-error",
        OrderError::Cart(talocart_commerce::cart::CartError::Coupon) => "oe-coupon-error",
        OrderError::Cart(talocart_commerce::cart::CartError::Methods) => "oe-method-error",
        OrderError::Cart(talocart_commerce::cart::CartError::Unavailable) => "oe-unavailable",
        OrderError::EditBlocked => "oe-blocked",
        _ => "oe-fields-error",
    }
}
fn recoverable(error: &OrderError) -> bool {
    matches!(
        error,
        OrderError::EditorFields(_)
            | OrderError::Notification
            | OrderError::EditBlocked
            | OrderError::Review
            | OrderError::Cart(
                talocart_commerce::cart::CartError::Stale
                    | talocart_commerce::cart::CartError::Stock
                    | talocart_commerce::cart::CartError::Quantity
                    | talocart_commerce::cart::CartError::Coupon
                    | talocart_commerce::cart::CartError::Methods
                    | talocart_commerce::cart::CartError::Unavailable
            )
    )
}
fn status(error: &OrderError) -> StatusCode {
    if matches!(
        error,
        OrderError::Review
            | OrderError::EditBlocked
            | OrderError::Cart(talocart_commerce::cart::CartError::Stale)
    ) {
        StatusCode::CONFLICT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    }
}
async fn page(
    state: &AppState,
    session: &Session,
    draft: Draft,
    query: EditorQuery,
    failure: Option<OrderError>,
) -> Result<Response, OrderError> {
    let mut failure = failure;
    let mut preview = None;
    if query.preview == Some(1) && failure.is_none() {
        match service(state).preview_draft(session, draft.id).await {
            Ok(value) => preview = Some(value),
            Err(e) if recoverable(&e) => failure = Some(e),
            Err(e) => return Err(e),
        }
    }
    let invalid = match &failure {
        Some(OrderError::EditorFields(names)) => names.clone(),
        _ => vec![],
    };
    let policy = customer_fields::load(&state.pool).await?;
    let groups = [
        ("customer", "ca-profile"),
        ("shipping", "co-group-shipping"),
        ("billing", "ord-invoice"),
    ]
    .into_iter()
    .map(|(scope, key)| Group {
        label: i18n::text(key),
        fields: customer_fields::FIELDS
            .iter()
            .filter(|f| {
                f.scope == scope
                    && policy.mode(f.name, &draft.data.customer.customer_type) != Mode::Hidden
            })
            .map(|f| Field {
                name: f.name,
                label: i18n::text(f.label),
                value: draft.data.customer.value(f.name).into(),
                kind: if f.name.ends_with("country") {
                    "country"
                } else if f.name == "delivery_notes" {
                    "textarea"
                } else if f.name == "email" || f.name == "pec" {
                    "email"
                } else {
                    "text"
                },
                invalid: invalid.iter().any(|n| n == f.name),
                required: draft.data.customer.field_required(f.name, &policy),
            })
            .collect(),
    })
    .collect();
    let countries = talocart_platform::geography::COUNTRIES
        .iter()
        .map(|code| Country {
            code: (*code).into(),
            label: i18n::text(&format!("country-{}", code.to_ascii_lowercase())),
        })
        .collect();
    let (shipping, payments) = service(state)
        .editor_methods(session, i18n::locale())
        .await?;
    let products = if query.products.is_empty() {
        vec![]
    } else {
        service(state)
            .editor_products(session, &query.products)
            .await?
    };
    let can_customers =
        session.allows(Permission::CustomersRead) && draft.target_order_id.is_none();
    let customers = if can_customers && !query.customers.is_empty() {
        CustomerAdmin::new(state.pool.clone(), state.identity.clone())
            .search(session, &query.customers)
            .await
            .map_err(OrderError::from)?
    } else {
        vec![]
    };
    let shell = AdminShell::load(state, session).await?;
    let title = i18n::text(if preview.is_some() {
        "oe-preview"
    } else if draft.target_order_id.is_some() {
        "oe-edit"
    } else {
        "oe-new"
    });
    let http_status = failure.as_ref().map_or(StatusCode::OK, status);
    let error = failure
        .as_ref()
        .map_or(String::new(), |e| i18n::text(message(e)));
    let mut response = private(render(Page {
        store_name: &shell.store_name,
        logo: shell.logo,
        csrf: shell.csrf,
        section: "orders",
        theme: shell.theme,
        nav: shell.nav,
        account: shell.account,
        draft,
        groups,
        countries,
        shipping,
        payments,
        products,
        customers,
        query,
        preview,
        can_customers,
        error,
        invalid,
        title,
    }));
    *response.status_mut() = http_status;
    Ok(response)
}
async fn show(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(query): Query<EditorQuery>,
) -> Response {
    let result = async {
        let draft = service(&state).draft(&session, id).await?;
        if let Some(id) = draft.committed_order_id {
            return Ok(private(
                Redirect::to(&format!("/admin/orders/{id}")).into_response(),
            ));
        }
        page(&state, &session, draft, query, None).await
    }
    .await;
    result.unwrap_or_else(super::orders::error)
}
fn form_map(values: Vec<(String, String)>) -> Result<BTreeMap<String, String>, OrderError> {
    let mut map = BTreeMap::new();
    for (key, value) in values {
        if map.insert(key, value).is_some() {
            return Err(invalid());
        }
    }
    Ok(map)
}
fn value<'a>(map: &'a BTreeMap<String, String>, name: &str) -> &'a str {
    map.get(name).map_or("", String::as_str)
}
fn integer(map: &BTreeMap<String, String>, name: &str) -> Result<i64, OrderError> {
    value(map, name).parse().map_err(|_| invalid())
}
fn optional(map: &BTreeMap<String, String>, name: &str) -> Result<Option<i64>, OrderError> {
    if value(map, name).is_empty() {
        Ok(None)
    } else {
        integer(map, name).map(Some)
    }
}
fn parse_data(
    map: &BTreeMap<String, String>,
    previous: &DraftData,
) -> Result<DraftData, OrderError> {
    let mut customer = Details::default();
    for field in customer_fields::FIELDS {
        if let Some(target) = customer.value_mut(field.name) {
            *target = value(map, field.name).into();
        }
    }
    customer.customer_type = value(map, "customer_type").into();
    customer.billing_same = value(map, "billing_same") == "true";
    customer.invoice_requested = value(map, "invoice_requested") == "true";
    let count: usize = integer(map, "line_count")?
        .try_into()
        .map_err(|_| invalid())?;
    if count > 100 {
        return Err(invalid());
    }
    let mut lines = vec![];
    for i in 0..count {
        let product_id = integer(map, &format!("product_{i}"))?;
        let variant_id = integer(map, &format!("variant_{i}"))?;
        let stored = previous.lines.get(i).ok_or_else(invalid)?;
        if product_id != stored.product_id || variant_id != stored.variant_id {
            return Err(invalid());
        }
        lines.push(DraftLine {
            quantity: integer(map, &format!("quantity_{i}"))?
                .try_into()
                .map_err(|_| invalid())?,
            unit_price: value(map, &format!("price_{i}")).into(),
            ..stored.clone()
        });
    }
    Ok(DraftData {
        customer_id: previous.customer_id,
        customer,
        locale: value(map, "locale").into(),
        lines,
        shipping_method_id: optional(map, "shipping_method_id")?,
        payment_method_id: optional(map, "payment_method_id")?,
        shipping_price: value(map, "shipping_price").into(),
        payment_price: value(map, "payment_price").into(),
        coupon_code: value(map, "coupon_code").into(),
        reason: value(map, "reason").into(),
    })
}
async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(values): Form<Vec<(String, String)>>,
) -> Response {
    let result = async {
        let map = form_map(values)?;
        session.check_csrf(value(&map, "csrf"))?;
        let mut draft = service(&state).draft(&session, id).await?;
        let revision = integer(&map, "revision")?;
        if revision != draft.revision || draft.committed_order_id.is_some() {
            return page(
                &state,
                &session,
                draft,
                EditorQuery::default(),
                Some(talocart_commerce::cart::CartError::Stale.into()),
            )
            .await;
        }
        let mut data = parse_data(&map, &draft.data)?;
        let action = value(&map, "action");
        if let Some(pick) = action.strip_prefix("add:") {
            let (p, v) = pick.split_once(':').ok_or_else(invalid)?;
            let item = service(&state)
                .editor_product(
                    &session,
                    p.parse().map_err(|_| invalid())?,
                    v.parse().map_err(|_| invalid())?,
                    &data.locale,
                )
                .await?;
            if !data
                .lines
                .iter()
                .any(|l| l.product_id == item.product_id && l.variant_id == item.variant_id)
            {
                data.lines.push(item);
            }
        } else if let Some(index) = action.strip_prefix("remove:") {
            let index: usize = index.parse().map_err(|_| invalid())?;
            if index >= data.lines.len() {
                return Err(invalid());
            }
            data.lines.remove(index);
        } else if let Some(customer) = action.strip_prefix("customer:") {
            if draft.target_order_id.is_some() {
                return Err(invalid());
            }
            let customer_id = customer.parse().map_err(|_| invalid())?;
            let profile = CustomerAdmin::new(state.pool.clone(), state.identity.clone())
                .get(&session, customer_id)
                .await
                .map_err(OrderError::from)?;
            data.customer_id = Some(customer_id);
            data.customer = Details {
                email: profile.email,
                first_name: profile.first_name,
                last_name: profile.last_name,
                phone: profile.phone,
                customer_type: profile.customer_type,
                ..Default::default()
            };
            let addresses =
                talocart_commerce::customer_addresses::CustomerAddresses::new(state.pool.clone())
                    .admin_list(&state.identity, &session, customer_id)
                    .await
                    .map_err(OrderError::from)?;
            if let Some(address) = addresses.first() {
                address.apply_to_checkout(&mut data.customer);
            }
            if let Some(billing) = talocart_commerce::customer_billing::CustomerBilling::new(
                state.pool.clone(),
                state.identity.clone(),
            )
            .get(&session, customer_id)
            .await
            .map_err(OrderError::from)?
            {
                billing.input.apply(&mut data.customer);
            }
        } else if action == "guest" {
            if draft.target_order_id.is_some() {
                return Err(invalid());
            }
            data.customer_id = None;
        } else if ![
            "save",
            "preview",
            "search-products",
            "search-customers",
            "refresh",
        ]
        .contains(&action)
        {
            return Err(invalid());
        }
        let saved = service(&state)
            .save_draft(&session, value(&map, "csrf"), id, revision, &data)
            .await;
        if let Err(error) = saved {
            if !recoverable(&error) {
                return Err(error);
            }
            draft.data = data;
            return page(&state, &session, draft, EditorQuery::default(), Some(error)).await;
        }
        let suffix = if action == "preview" {
            "?preview=1".into()
        } else if action == "search-products" {
            format!(
                "?products={}",
                catalog::encoded(value(&map, "product_search"))
            )
        } else if action == "search-customers" {
            format!(
                "?customers={}",
                catalog::encoded(value(&map, "customer_search"))
            )
        } else {
            "?saved=1".into()
        };
        Ok(private(
            Redirect::to(&format!("/admin/orders/drafts/{id}{suffix}")).into_response(),
        ))
    }
    .await;
    result.unwrap_or_else(super::orders::error)
}
#[derive(Deserialize)]
struct Publication {
    csrf: String,
    revision: i64,
    review: String,
    #[serde(default)]
    notify: Option<String>,
    #[serde(default)]
    confirm: Option<String>,
}
async fn publish(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(input): Form<Publication>,
) -> Response {
    let result = async {
        if input.confirm.as_deref() != Some("true") {
            return Err(invalid());
        }
        match service(&state)
            .publish_draft(Publish {
                session: &session,
                csrf: &input.csrf,
                draft_id: id,
                revision: input.revision,
                review: &input.review,
                notify: input.notify.as_deref() == Some("true"),
            })
            .await
        {
            Ok(order) => Ok(private(
                Redirect::to(&format!("/admin/orders/{}", order.id)).into_response(),
            )),
            Err(error) if recoverable(&error) => {
                let draft = service(&state).draft(&session, id).await?;
                page(&state, &session, draft, EditorQuery::default(), Some(error)).await
            }
            Err(error) => Err(error),
        }
    }
    .await;
    result.unwrap_or_else(super::orders::error)
}
#[derive(Deserialize)]
struct SummarySubmission {
    csrf: String,
    revision: i64,
    #[serde(default)]
    confirm: Option<String>,
}
async fn summary(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(input): Form<SummarySubmission>,
) -> Response {
    if input.confirm.as_deref() != Some("true") {
        return super::orders::error(invalid());
    }
    match service(&state)
        .send_summary(&session, &input.csrf, id, input.revision)
        .await
    {
        Ok(()) => private(Redirect::to(&format!("/admin/orders/{id}?summary=1")).into_response()),
        Err(e) => super::orders::error(e),
    }
}
pub(super) struct History {
    pub administrator_id: i64,
    pub revision: i64,
    pub administrator_name: String,
    pub reason: String,
    pub created_at: String,
    pub before: Option<RevisionSnapshot>,
    pub after: RevisionSnapshot,
}
pub(super) async fn history(
    state: &AppState,
    session: &Session,
    id: i64,
    page: i64,
) -> Result<EditorPage<History>, OrderError> {
    let events = service(state).edit_history(session, id, page).await?;
    let entries = events
        .entries
        .into_iter()
        .map(|e| {
            Ok(History {
                revision: e.revision,
                administrator_id: e.administrator_id,
                administrator_name: e.administrator_name,
                reason: e.reason,
                created_at: e.created_at,
                before: if e.before_data.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&e.before_data).map_err(|_| OrderError::Invalid)?)
                },
                after: serde_json::from_str(&e.after_data).map_err(|_| OrderError::Invalid)?,
            })
        })
        .collect::<Result<Vec<_>, OrderError>>()?;
    Ok(EditorPage {
        entries,
        total: events.total,
        page: events.page,
    })
}
