use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::orders::{
    ListFilters, ListOrder, ListSort, Order, OrderError, Orders, Summary,
};
#[path = "orders_fulfillment.rs"]
mod fulfillment;
pub(super) fn service(state: &AppState) -> Orders {
    Orders::new(state.pool.clone(), state.identity.clone())
}
pub(crate) use crate::http_policy::private;
pub(crate) fn error(error: OrderError) -> Response {
    let error = match error {
        OrderError::Technical(error)
        | OrderError::Cart(talocart_commerce::cart::CartError::Technical(error)) => {
            return private(access::IdentityErrorResponse::from(error).into_response());
        }
        OrderError::Access(error)
        | OrderError::Cart(talocart_commerce::cart::CartError::Access(error)) => {
            return private(access::IdentityErrorResponse::from(error).into_response());
        }

        OrderError::Identity(error)
        | OrderError::Cart(talocart_commerce::cart::CartError::Identity(error)) => {
            return private(access::IdentityErrorResponse::from(error).into_response());
        }
        error => error,
    };
    let status = match error {
        OrderError::EditBlocked => StatusCode::CONFLICT,
        OrderError::NotFound => StatusCode::NOT_FOUND,
        OrderError::Shipped
        | OrderError::FulfillmentTransition
        | OrderError::FulfillmentPayment
        | OrderError::FulfillmentConflict => StatusCode::CONFLICT,
        OrderError::Cart(talocart_commerce::cart::CartError::Forbidden) => StatusCode::FORBIDDEN,
        OrderError::Cart(talocart_commerce::cart::CartError::Stale) | OrderError::Review => {
            StatusCode::CONFLICT
        }
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let key = if matches!(error, OrderError::ListFilters) {
        "admin-order-filters-error"
    } else if matches!(error, OrderError::Shipped) {
        "fulfillment-return-required"
    } else {
        "ord-error"
    };
    private((status, i18n::text(key)).into_response())
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(super::orders_editor::routes())
        .route("/admin/orders", get(index))
        .route("/admin/orders/{id}", get(detail))
        .route("/admin/orders/{id}/status", post(update_status))
        .route("/admin/orders/{id}/cancel", post(cancel))
        .merge(
            Router::new()
                .route("/admin/orders/{id}/fulfillment", post(fulfillment::update))
                .layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route("/admin/orders/{id}/payment", post(record_payment))
        .route("/admin/orders/{id}/refund", post(refund))
        .route(
            "/admin/orders/{id}/attempts/{attempt}/reconcile",
            post(reconcile),
        )
}
pub(crate) fn public_routes(prefix: &str) -> Router<AppState> {
    Router::new()
        .route(
            &format!("{prefix}/checkout/orders/{{reference}}/payment"),
            get(payment_page),
        )
        .route(
            &format!("{prefix}/checkout/orders/{{reference}}/session"),
            post(element_session),
        )
        .route(
            &format!("{prefix}/checkout/orders/{{reference}}/payment-status"),
            get(payment_status),
        )
        .route(
            &format!("{prefix}/checkout/orders/{{reference}}/pay"),
            post(pay),
        )
        .route(
            &format!("{prefix}/checkout/orders/{{reference}}"),
            get(receipt),
        )
}
#[derive(Template)]
#[template(path = "default/templates/order-receipt.html")]
struct Receipt<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    order: Order,
    csrf: String,
    online_available: bool,
    payment_pending: bool,
    show_initial_status: bool,
    receipt_state: ReceiptState,
    status_url: String,
    payment_url: String,
}
#[derive(Deserialize)]
struct ReceiptQuery {
    payment_return: Option<u8>,
}
#[derive(PartialEq)]
enum ReceiptState {
    Other,
    Due,
    Checking,
    Paid,
    Refunded,
    Cancelled,
    Review,
}
impl ReceiptState {
    fn for_order(order: &Order, returned: bool) -> Self {
        if !order.is_online() {
            if order.state == "cancelled" {
                Self::Cancelled
            } else {
                Self::Other
            }
        } else if order.is_refunded() {
            Self::Refunded
        } else if order.payment_review_blocks_confirmation() {
            Self::Review
        } else if order.state == "cancelled" {
            Self::Cancelled
        } else if order.is_paid() {
            Self::Paid
        } else if order.payment_verifying()
            || (returned
                && !matches!(
                    order.online_attempt_state.as_deref(),
                    Some("expired" | "failed")
                ))
        {
            Self::Checking
        } else {
            Self::Due
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::Due => "due",
            Self::Checking => "checking",
            Self::Paid => "paid",
            Self::Refunded => "refunded",
            Self::Cancelled => "cancelled",
            Self::Review => "review",
        }
    }
    fn title(&self) -> &'static str {
        match self {
            Self::Due => "online-payment-due",
            Self::Checking => "online-payment-checking",
            Self::Paid => "ord-confirmed",
            Self::Refunded => "online-payment-state-refunded",
            Self::Cancelled => "ord-cancelled-title",
            Self::Review => "online-payment-state-review",
            _ => "ord-received",
        }
    }
    fn polling(&self) -> bool {
        matches!(self, Self::Due | Self::Checking)
    }
    fn tone(&self) -> &'static str {
        match self {
            Self::Refunded | Self::Cancelled => "closed",
            Self::Due | Self::Checking | Self::Review => "pending",
            Self::Paid | Self::Other => "done",
        }
    }
    fn icon(&self) -> &'static str {
        match self {
            Self::Due => "credit-card",
            Self::Checking => "refresh-cw",
            Self::Review => "shield-check",
            Self::Refunded => "rotate-ccw",
            Self::Cancelled => "x",
            Self::Paid | Self::Other => "check",
        }
    }
}
async fn receipt(
    State(state): State<AppState>,
    Path(reference): Path<String>,
    Query(query): Query<ReceiptQuery>,
    cart_context: cart::CartContext,
) -> Response {
    let result = async {
        let (guest, order) = order_context(&state, &reference, &cart_context).await?;
        let state = state.localized();
        let payment_url = if order.is_online() && state.embedded_payments() {
            store_language::path(format!(
                "/checkout/orders/{reference}/payment?payment_return=1"
            ))
        } else {
            String::new()
        };
        if !payment_url.is_empty()
            && query.payment_return == Some(1)
            && order.state == "open"
            && !order.is_paid()
            && !order.is_refunded()
            && !order.online_review
        {
            return Ok(private(Redirect::to(&payment_url).into_response()));
        }
        let name = current_name(&state).await?;
        let online_form = order.is_online();
        let receipt_state = ReceiptState::for_order(&order, query.payment_return == Some(1));
        let payment_pending = receipt_state == ReceiptState::Checking;
        let status_url = store_language::path(format!(
            "/checkout/orders/{}{}",
            order.reference,
            if query.payment_return == Some(1) {
                "?payment_return=1"
            } else {
                ""
            }
        ));
        let payment_provider = match &state.online {
            Some(s) if order.is_online() => s.for_order(order.id).await.ok(),
            _ => None,
        };
        let online_available = match &payment_provider {
            Some(s) => s.accepting().await,
            None => false,
        };
        let provider_id = payment_provider.as_ref().map(|s| s.provider_id());
        let mut response = private(render(Receipt {
            store_name: &name,
            logo: current_logo(&state).await?,
            theme: store_language::settings(&state).await?,
            category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
            search_query: "",
            search_action: "/search",
            search_filters: navigation::SearchFilters::default(),
            order,
            csrf: guest.csrf.clone(),
            online_available,
            payment_pending,
            show_initial_status: !online_form,
            receipt_state,
            status_url,
            payment_url,
        }));
        if online_form && let Some(ui) = &state.payment_ui {
            response.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&ui.receipt_policy_for(provider_id.as_deref()))
                    .map_err(|_| OrderError::Invalid)?,
            );
        }
        Ok::<_, OrderError>(response)
    }
    .await;
    result.unwrap_or_else(error)
}
/// Preserve host-only guest cookies when the local listener is reached by an alias.
/// Public HTTPS uses its configured authority; local aliases remain loopback-only.
/// Forwarded headers never select an origin.
fn payment_origin(listener: &str, headers: &HeaderMap) -> Result<String, OrderError> {
    use axum::http::{Uri, uri::Authority};
    fn loopback(host: &str) -> bool {
        matches!(host, "localhost" | "127.0.0.1" | "[::1]")
    }
    let configured: Uri = listener.parse().map_err(|_| OrderError::Invalid)?;
    let scheme = configured.scheme_str().ok_or(OrderError::Invalid)?;
    let authority = configured.authority().ok_or(OrderError::Invalid)?;
    if !matches!(scheme, "http" | "https")
        || (scheme != "https" && !loopback(authority.host()))
        || authority.as_str().contains('@')
        || configured.path() != "/"
        || configured.query().is_some()
    {
        return Err(OrderError::Invalid);
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    fn port(authority: &Authority, default: u16) -> Result<u16, OrderError> {
        if authority.port().is_some() {
            authority.port_u16().ok_or(OrderError::Invalid)
        } else {
            Ok(default)
        }
    }
    let values: Vec<_> = headers.get_all(header::HOST).iter().collect();
    let request_authority = match values.as_slice() {
        [] => authority.clone(),
        [value] => value
            .to_str()
            .map_err(|_| OrderError::Invalid)?
            .parse::<Authority>()
            .map_err(|_| OrderError::Invalid)?,
        _ => return Err(OrderError::Invalid),
    };
    if (if loopback(authority.host()) {
        !loopback(request_authority.host())
    } else {
        !request_authority
            .host()
            .eq_ignore_ascii_case(authority.host())
    }) || request_authority.as_str().contains('@')
        || port(&request_authority, default_port)? != port(authority, default_port)?
    {
        return Err(OrderError::Invalid);
    }
    let origin = if loopback(authority.host()) {
        format!("{scheme}://{request_authority}")
    } else {
        listener.trim_end_matches('/').to_owned()
    };
    let origins: Vec<_> = headers.get_all(header::ORIGIN).iter().collect();
    match origins.as_slice() {
        [] => {}
        [value] if value.to_str().ok() == Some(origin.as_str()) => {}
        _ => return Err(OrderError::Invalid),
    }
    Ok(origin)
}
// This remains the second checkout step; the order is now immutable and unpaid.
#[derive(Template)]
#[template(path = "default/templates/checkout-payment.html")]
struct PaymentPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    payment_body: String,
    scripts: Vec<String>,
    page_title: String,
}
async fn payment_page(
    State(state): State<AppState>,
    Path(reference): Path<String>,
    Query(query): Query<ReceiptQuery>,
    cart_context: cart::CartContext,
) -> Response {
    let result = async {
        let (guest, order) = order_context(&state, &reference, &cart_context).await?;
        if !order.is_online() {
            return Err(OrderError::PaymentMethod);
        }
        if order.state != "open" || order.payment_state != "unpaid" || order.online_review {
            return Ok(private(
                Redirect::to(&store_language::path(format!(
                    "/checkout/orders/{reference}"
                )))
                .into_response(),
            ));
        }
        let state = state.localized();
        let name = current_name(&state).await?;
        let resumable = matches!(
            order.online_attempt_state.as_deref(),
            Some("expired" | "failed")
        );
        let submitted = query.payment_return == Some(1) && !resumable;
        let waiting = submitted || order.payment_verifying();
        let selected = state
            .online
            .as_ref()
            .ok_or(OrderError::PaymentMethod)?
            .for_order(order.id)
            .await?;
        let online_available = selected.accepting().await;
        let mut order = order;
        order.frozen.payment.provider_id = Some(selected.provider_id());
        let ui = state.payment_ui.as_ref().ok_or(OrderError::PaymentMethod)?;
        let payment_body = ui
            .render(crate::payment_extensions::PaymentBody {
                order: &order,
                csrf: &guest.csrf,
                online_available,
                waiting,
                submitted,
                resumable,
            })
            .await
            .map_err(|_| talocart_platform::identity::IdentityError::Unavailable)?;
        let mut response = private(render(PaymentPage {
            store_name: &name,
            logo: current_logo(&state).await?,
            theme: store_language::settings(&state).await?,
            category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
            search_query: "",
            search_action: "/search",
            search_filters: navigation::SearchFilters::default(),
            payment_body: payment_body.body,
            scripts: payment_body.scripts,
            page_title: payment_body.title,
        }));
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(&payment_body.policy).map_err(|_| OrderError::Invalid)?,
        );
        Ok(response)
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(serde::Serialize)]
struct PaymentStatus {
    state: &'static str,
    can_resume: bool,
    receipt_url: Option<String>,
}
// Read-only: ownership and persisted settlement are the only authority, never SDK output.
async fn payment_status(
    State(state): State<AppState>,
    Path(reference): Path<String>,
    cart_context: cart::CartContext,
) -> Response {
    let result = async {
        let (_, order) = order_context(&state, &reference, &cart_context).await?;
        if !order.is_online() {
            return Err(OrderError::PaymentMethod);
        }
        let status = ReceiptState::for_order(&order, false);
        Ok::<_, OrderError>(private(
            axum::Json(PaymentStatus {
                state: status.code(),
                can_resume: matches!(
                    order.online_attempt_state.as_deref(),
                    Some("expired" | "failed")
                ),
                receipt_url: (status == ReceiptState::Paid)
                    .then(|| store_language::path(format!("/checkout/orders/{reference}"))),
            })
            .into_response(),
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(serde::Serialize)]
#[serde(untagged)]
enum ElementResponse {
    Secret { client_secret: String },
    Redirect { redirect: String },
    Pending { pending: bool },
}
async fn element_session(
    State(state): State<AppState>,
    Path(reference): Path<String>,
    headers: HeaderMap,
    cart_context: cart::CartContext,
    Form(input): Form<PayInput>,
) -> Response {
    let result = async {
        let (guest, _) = order_context(&state, &reference, &cart_context).await?;
        guest.check_csrf(&input.csrf).map_err(OrderError::from)?;
        let online = state.online.as_ref().ok_or(OrderError::PaymentMethod)?;
        let origin = payment_origin(&state.public_origin, &headers)?;
        if !state.embedded_payments() {
            return Err(OrderError::PaymentMethod);
        }
        let id = online
            .start_elements(&guest, &input.csrf, &reference, input.revision, &origin)
            .await?;
        online.process_attempt(id).await?;
        if let Some(secret) = online.element_secret(&guest, &reference).await? {
            return Ok(private(
                axum::Json(ElementResponse::Secret {
                    client_secret: secret,
                })
                .into_response(),
            ));
        }
        // An upgrade must resume the exact hosted session already associated with the order.
        if let Some(url) = online.checkout(&guest, &reference).await? {
            return Ok(private(
                axum::Json(ElementResponse::Redirect { redirect: url }).into_response(),
            ));
        }
        let order = service(&state).for_guest(&guest, &reference).await?;
        if order.payment_state != "unpaid" || order.state != "open" || order.online_review {
            return Ok(private(
                axum::Json(ElementResponse::Redirect {
                    redirect: store_language::path(format!("/checkout/orders/{reference}")),
                })
                .into_response(),
            ));
        }
        Ok(private(
            (
                StatusCode::ACCEPTED,
                axum::Json(ElementResponse::Pending { pending: true }),
            )
                .into_response(),
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "first")]
    page: i64,
    #[serde(flatten)]
    filters: ListFilters,
}

fn first() -> i64 {
    1
}
struct OrderSortHeader {
    label: &'static str,
    href: String,
    sort: super::table_sort::SortState,
    class: &'static str,
}
fn order_list_link(filters: &ListFilters, order: ListOrder) -> String {
    format!(
        "/admin/orders?reference={}&date={}&customer={}&state={}&payment={}&fulfillment={}&total={}&total_op={}&sort={}&dir={}&per_page={}",
        catalog::encoded(&filters.reference),
        catalog::encoded(&filters.date),
        catalog::encoded(&filters.customer),
        catalog::encoded(&filters.state),
        catalog::encoded(&filters.payment),
        catalog::encoded(&filters.fulfillment),
        catalog::encoded(&filters.total),
        catalog::encoded(&filters.total_op),
        order.column.key(),
        order.direction(),
        catalog::encoded(&filters.per_page)
    )
}
fn order_sort_headers(filters: &ListFilters, order: ListOrder) -> Vec<OrderSortHeader> {
    [
        (ListSort::Reference, "admin-order-number", ""),
        (ListSort::Created, "ord-created", ""),
        (ListSort::Customer, "admin-order-customer", ""),
        (ListSort::State, "ord-state", ""),
        (ListSort::Payment, "admin-order-payment", "ui-table__center"),
        (ListSort::Total, "admin-total", "ui-table__num"),
    ]
    .into_iter()
    .map(|(column, label, class)| {
        let sort = super::table_sort::SortState::new(
            column == order.column,
            order.descending,
            i18n::text(label),
        );
        let descending = sort.next_descending;
        OrderSortHeader {
            label,
            class,
            href: order_list_link(filters, ListOrder { column, descending }),
            sort,
        }
    })
    .collect()
}
#[derive(Template)]
#[template(path = "orders.html")]
struct ListPage<'a> {
    can_write: bool,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    entries: Vec<Summary>,
    statuses: Vec<talocart_commerce::order_statuses::OrderStatus>,
    filters: ListFilters,
    headers: Vec<OrderSortHeader>,
    filtered: bool,
    sort: &'static str,
    direction: &'static str,
    pagination: super::pagination::Pagination,
    page_size: u16,
    page_sizes: [u16; 4],
    filter_error: bool,
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(mut q): Query<ListQuery>,
    headers: HeaderMap,
) -> Response {
    let explicit_size = q.filters.per_page.clone();
    let preference = super::page_preference::AdminList::Orders;
    let size = preference.resolve(&explicit_size, &headers).ok();
    if let Some(size) = size {
        q.filters.per_page = size.value().to_string();
    }
    let result = async {
        let (entries, filter_error) = match service(&state)
            .filtered_list(&session, &q.filters, q.page)
            .await
        {
            Ok(entries) => (entries, false),
            Err(OrderError::ListFilters) => (
                talocart_commerce::orders::ListPage {
                    entries: Vec::new(),
                    total: 0,
                    page: 1,
                    order: ListOrder::parse(&q.filters.sort, &q.filters.dir).unwrap_or(ListOrder {
                        column: ListSort::Created,
                        descending: true,
                    }),
                    page_size: q.filters.page_size().unwrap_or_default(),
                },
                true,
            ),
            Err(source_error) => return Err(source_error),
        };
        let order = entries.order;
        let link = order_list_link(&q.filters, order);
        let page_size = entries.page_size.value();
        let admin_shell = AdminShell::load(&state, &session).await?;
        let name = admin_shell.store_name;
        let mut response = private(render(ListPage {
            can_write: session.allows(Permission::OrdersWrite),
            store_name: &name,
            logo: admin_shell.logo,
            csrf: admin_shell.csrf,
            section: "orders",
            theme: admin_shell.theme,
            nav: admin_shell.nav,
            account: admin_shell.account,
            pagination: super::pagination::Pagination::new(
                entries.total,
                entries.page,
                page_size,
                &i18n::text("ord-title"),
                &i18n::text("ord-pages"),
                |p| format!("{link}&page={p}"),
            ),
            statuses: service(&state).status_choices(&session).await?,
            entries: entries.entries,
            headers: order_sort_headers(&q.filters, order),
            filtered: q.filters.active(),
            sort: order.column.key(),
            direction: order.direction(),
            filters: q.filters,
            page_size,
            page_sizes: talocart_platform::pagination::ADMIN_PAGE_SIZES,
            filter_error,
        }));
        if filter_error {
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        } else if let Some(size) = size {
            response = preference.remember(response, &explicit_size, size, state.secure_cookies)?;
        }
        Ok::<_, OrderError>(response)
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Template)]
#[template(path = "order-admin.html")]
struct Detail<'a> {
    edit_history: Vec<super::orders_editor::History>,
    edit_pagination: super::pagination::Pagination,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    order: Order,
    can_write: bool,
    error: String,
    payment_form: PaymentForm,
    attempts: Vec<talocart_commerce::orders::Attempt>,
    online_available: bool,
    payment_pending: bool,
    fulfillment_form: fulfillment::View,
    statuses: Vec<talocart_commerce::order_statuses::OrderStatus>,
    status_history: Vec<talocart_commerce::orders::StatusEvent>,
    status_error: bool,
    status_pagination: super::pagination::Pagination,
}
#[derive(Default)]
struct PaymentForm {
    edit_page: i64,
    summary_sent: bool,
    reference: String,
    invalid_reference: bool,
    invalid_confirmation: bool,
    saved: bool,
}
async fn show(
    state: &AppState,
    session: &Session,
    id: i64,
    message: &str,
    status: StatusCode,
    payment_form: PaymentForm,
) -> std::result::Result<Response, OrderError> {
    show_with_fulfillment(state, session, id, message, status, payment_form, None).await
}
async fn show_with_fulfillment(
    state: &AppState,
    session: &Session,
    id: i64,
    message: &str,
    status: StatusCode,
    payment_form: PaymentForm,
    fulfillment_context: Option<fulfillment::Context>,
) -> std::result::Result<Response, OrderError> {
    let order = service(state).get(session, id).await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let history_page = fulfillment_context.as_ref().map_or(1, |c| c.page.max(1));
    let history = service(state)
        .status_history(session, id, history_page)
        .await?;
    let edits = super::orders_editor::history(state, session, id, payment_form.edit_page).await?;
    let mut r = render(Detail {
        edit_pagination: super::pagination::Pagination::new(
            edits.total,
            edits.page,
            talocart_commerce::orders::editor::EDITOR_PAGE_SIZE,
            &i18n::text("oe-history"),
            &i18n::text("oe-history"),
            |page| format!("/admin/orders/{id}?edit_page={page}#order-edit-history"),
        ),
        edit_history: edits.entries,
        statuses: service(state).status_choices(session).await?,
        status_pagination: super::pagination::Pagination::new(
            history.total,
            history.page,
            20,
            &i18n::text("status-history"),
            &i18n::text("status-history"),
            |p| format!("/admin/orders/{id}?history_page={p}#status-history"),
        ),
        status_history: history.entries,
        status_error: message == i18n::text("status-change-error"),
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "orders",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        payment_pending: order.payment_verifying(),
        fulfillment_form: fulfillment::View::load(state, session, &order, fulfillment_context)
            .await?,
        order,
        can_write: session.allows(Permission::OrdersWrite),
        error: message.into(),
        payment_form,
        attempts: service(state).attempts(session, id).await?,
        online_available: state.online.is_some(),
    });
    *r.status_mut() = status;
    Ok(private(r))
}
async fn detail(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(query): Query<DetailQuery>,
) -> Response {
    show_with_fulfillment(
        &state,
        &session,
        id,
        "",
        StatusCode::OK,
        PaymentForm {
            edit_page: query.edit_page.unwrap_or(1),
            summary_sent: query.summary == Some(1),
            saved: query.payment == Some(1),
            ..Default::default()
        },
        Some(fulfillment::Context {
            saved: query.fulfillment == Some(1),
            page: query.history_page.unwrap_or(1),
            ..Default::default()
        }),
    )
    .await
    .unwrap_or_else(error)
}
#[derive(Deserialize)]
struct DetailQuery {
    edit_page: Option<i64>,
    summary: Option<u8>,
    payment: Option<u8>,
    fulfillment: Option<u8>,
    history_page: Option<i64>,
}
#[derive(Deserialize)]
struct PaymentInput {
    csrf: String,
    revision: i64,
    #[serde(default)]
    reference: String,
    #[serde(default)]
    confirm: bool,
}
async fn record_payment(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(input): Form<PaymentInput>,
) -> Response {
    if let Err(source_error) = state
        .identity
        .authorize(&session, Permission::OrdersWrite)
        .await
    {
        return error(source_error.into());
    }
    if let Err(source_error) = session.check_csrf(&input.csrf) {
        return error(source_error.into());
    }
    let result = if input.confirm {
        service(&state)
            .record_payment(&session, &input.csrf, id, input.revision, &input.reference)
            .await
    } else {
        return show(
            &state,
            &session,
            id,
            "",
            StatusCode::UNPROCESSABLE_ENTITY,
            PaymentForm {
                reference: input.reference,
                invalid_confirmation: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(error);
    };
    let (status, key, invalid_reference) = match result {
        Ok(_) => {
            return private(Redirect::to(&format!("/admin/orders/{id}?payment=1")).into_response());
        }
        Err(OrderError::PaymentReference) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "ord-payment-reference-error",
            true,
        ),
        Err(OrderError::AlreadyPaid) => (StatusCode::CONFLICT, "ord-payment-already", false),
        Err(OrderError::PaymentCancelled) => (StatusCode::CONFLICT, "ord-payment-cancelled", false),
        Err(OrderError::PaymentMethod) => (StatusCode::CONFLICT, "ord-payment-method-error", false),
        Err(OrderError::Cart(talocart_commerce::cart::CartError::Stale)) => {
            (StatusCode::CONFLICT, "ord-payment-stale", false)
        }
        Err(OrderError::Notification) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ord-payment-queue-error",
            false,
        ),
        Err(source_error) => return error(source_error),
    };
    show(
        &state,
        &session,
        id,
        &if invalid_reference {
            String::new()
        } else {
            i18n::text(key)
        },
        status,
        PaymentForm {
            reference: input.reference,
            edit_page: 1,
            summary_sent: false,
            invalid_reference,
            invalid_confirmation: false,
            saved: false,
        },
    )
    .await
    .unwrap_or_else(error)
}
#[derive(Deserialize)]
struct Cancellation {
    csrf: String,
    revision: i64,
    #[serde(default)]
    confirm: bool,
}
async fn cancel(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<Cancellation>,
) -> Response {
    if let Err(source_error) = state
        .identity
        .authorize(&session, Permission::OrdersWrite)
        .await
    {
        return error(source_error.into());
    }
    if let Err(source_error) = session.check_csrf(&f.csrf) {
        return error(source_error.into());
    }
    if !f.confirm {
        return show(
            &state,
            &session,
            id,
            &i18n::text("ord-cancel-required"),
            StatusCode::UNPROCESSABLE_ENTITY,
            PaymentForm::default(),
        )
        .await
        .unwrap_or_else(error);
    }
    match service(&state)
        .cancel(&session, &f.csrf, id, f.revision)
        .await
    {
        Ok(_) => private(Redirect::to(&format!("/admin/orders/{id}")).into_response()),
        Err(OrderError::Restore) => show(
            &state,
            &session,
            id,
            &i18n::text("ord-restore-error"),
            StatusCode::CONFLICT,
            PaymentForm::default(),
        )
        .await
        .unwrap_or_else(error),
        Err(OrderError::Notification) => show(
            &state,
            &session,
            id,
            &i18n::text("ord-error"),
            StatusCode::SERVICE_UNAVAILABLE,
            PaymentForm::default(),
        )
        .await
        .unwrap_or_else(error),
        Err(OrderError::PaidCancellation) => show(
            &state,
            &session,
            id,
            &i18n::text("ord-paid-cancel-blocked"),
            StatusCode::CONFLICT,
            PaymentForm::default(),
        )
        .await
        .unwrap_or_else(error),
        Err(source_error) => error(source_error),
    }
}

#[derive(Deserialize)]
struct PayInput {
    csrf: String,
    revision: i64,
}
async fn pay(
    State(state): State<AppState>,
    Path(reference): Path<String>,
    headers: HeaderMap,
    cart_context: cart::CartContext,
    Form(input): Form<PayInput>,
) -> Response {
    let result = async {
        let (guest, _) = order_context(&state, &reference, &cart_context).await?;
        let online = state.online.as_ref().ok_or(OrderError::PaymentMethod)?;
        let origin = payment_origin(&state.public_origin, &headers)?;
        if state.embedded_payments() {
            guest.check_csrf(&input.csrf).map_err(OrderError::from)?;
            let order = service(&state).for_guest(&guest, &reference).await?;
            if !order.is_online()
                || order.state != "open"
                || order.payment_state != "unpaid"
                || order.online_review
            {
                return Err(OrderError::PaymentMethod);
            }
            return Ok(private(
                Redirect::to(&store_language::path(format!(
                    "/checkout/orders/{reference}/payment"
                )))
                .into_response(),
            ));
        }
        let attempt = online
            .start(&guest, &input.csrf, &reference, input.revision, &origin)
            .await?;
        // Claiming commits before I/O; the persistent background worker can also win.
        online.process_attempt(attempt).await?;
        let destination = online
            .checkout(&guest, &reference)
            .await?
            .unwrap_or_else(|| store_language::path(format!("/checkout/orders/{reference}")));
        Ok::<_, OrderError>(private(Redirect::to(&destination).into_response()))
    }
    .await;
    result.unwrap_or_else(error)
}
async fn refund(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(input): Form<Cancellation>,
) -> Response {
    let result = async {
        state
            .identity
            .authorize(&session, Permission::OrdersWrite)
            .await?;
        session.check_csrf(&input.csrf)?;
        if !input.confirm {
            return Err(OrderError::Invalid);
        }
        state
            .online
            .as_ref()
            .ok_or(OrderError::PaymentMethod)?
            .request_refund(&session, &input.csrf, id, input.revision)
            .await?;
        Ok::<_, OrderError>(private(
            Redirect::to(&format!("/admin/orders/{id}")).into_response(),
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
struct ReconcileInput {
    csrf: String,
    #[serde(default)]
    session_id: String,
}
async fn reconcile(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, attempt)): Path<(i64, i64)>,
    Form(input): Form<ReconcileInput>,
) -> Response {
    let result = async {
        if !input.session_id.trim().is_empty() {
            state
                .online
                .as_ref()
                .ok_or(OrderError::PaymentMethod)?
                .attach_session(&session, &input.csrf, id, attempt, input.session_id.trim())
                .await?;
        }
        state
            .online
            .as_ref()
            .ok_or(OrderError::PaymentMethod)?
            .reconcile(&session, &input.csrf, id, attempt)
            .await?;
        Ok::<_, OrderError>(private(
            Redirect::to(&format!("/admin/orders/{id}")).into_response(),
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
pub(crate) fn webhook_routes(state: &AppState) -> Router<AppState> {
    let Some(online) = state.online.as_ref() else {
        return Router::new();
    };
    Router::new().route(
        online.webhook_path(),
        post(webhook).layer(DefaultBodyLimit::max(
            talocart_platform::payment_gateway::BODY_LIMIT,
        )),
    )
}
async fn webhook(
    State(state): State<AppState>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(online) = state.online.as_ref() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Some(names) = online.webhook_headers(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut selected = std::collections::BTreeMap::new();
    for name in names {
        let values = headers.get_all(&name);
        if values.iter().count() > 1 {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let value = headers
            .get(&name)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if value.len() > 4096 {
            return StatusCode::BAD_REQUEST.into_response();
        }
        selected.insert(name, value.to_owned());
    }
    match online
        .webhook_with_headers(uri.path(), &body, &selected)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(OrderError::Invalid) => StatusCode::BAD_REQUEST.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Preserve the original guest capability for receipts created before login.
/// Only the two capabilities in this request are considered; email is never used.
async fn order_context(
    state: &AppState,
    reference: &str,
    cart_context: &cart::CartContext,
) -> Result<(talocart_commerce::cart::Guest, Order), OrderError> {
    let active = cart_context.guest().ok();
    let has_active = active.is_some();
    if let Some(active) = active {
        match service(state).for_guest(&active, reference).await {
            Ok(order) => return Ok((active, order)),
            Err(OrderError::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    if let Some(source) = cart_context.source()? {
        let order = service(state).for_guest(&source, reference).await?;
        Ok((source, order))
    } else if has_active {
        Err(OrderError::NotFound)
    } else {
        Err(talocart_commerce::cart::CartError::Forbidden.into())
    }
}

#[derive(Deserialize)]
struct StatusChange {
    csrf: String,
    revision: i64,
    status_id: i64,
}
async fn update_status(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<StatusChange>,
) -> Response {
    match service(&state)
        .set_status(&session, &f.csrf, id, f.revision, f.status_id)
        .await
    {
        Ok(_) => private(Redirect::to(&format!("/admin/orders/{id}")).into_response()),
        Err(
            source_error @ (OrderError::Technical(_)
            | OrderError::Access(_)
            | OrderError::Identity(_)),
        ) => error(source_error),
        Err(source_error) => show(
            &state,
            &session,
            id,
            &i18n::text("status-change-error"),
            if matches!(
                source_error,
                OrderError::Cart(talocart_commerce::cart::CartError::Stale)
            ) {
                StatusCode::CONFLICT
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            },
            PaymentForm::default(),
        )
        .await
        .unwrap_or_else(error),
    }
}

#[cfg(test)]
mod return_origin_tests {
    use super::*;
    #[test]
    fn configured_https_origin_accepts_only_its_own_host_and_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("shop.example.test"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://shop.example.test"),
        );
        headers.insert("x-forwarded-host", HeaderValue::from_static("evil.test"));
        assert_eq!(
            payment_origin("https://shop.example.test", &headers).unwrap(),
            "https://shop.example.test"
        );
        for host in ["evil.test", "localhost:3000", "shop.example.test:444"] {
            headers.insert(header::HOST, host.parse().unwrap());
            assert!(payment_origin("https://shop.example.test", &headers).is_err());
        }
        headers.insert(header::HOST, HeaderValue::from_static("shop.example.test"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.test"),
        );
        assert!(payment_origin("https://shop.example.test", &headers).is_err());
        assert!(payment_origin("http://shop.example.test", &HeaderMap::new()).is_err());
    }
    #[test]
    fn local_aliases_preserve_host_and_configured_scheme_port() {
        for (listener, host, expected) in [
            (
                "http://127.0.0.1:3000",
                "localhost:3000",
                "http://localhost:3000",
            ),
            (
                "http://localhost:3000",
                "127.0.0.1:3000",
                "http://127.0.0.1:3000",
            ),
            ("http://[::1]:3000", "[::1]:3000", "http://[::1]:3000"),
            ("https://127.0.0.1", "localhost", "https://localhost"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, host.parse().unwrap());
            headers.insert(header::ORIGIN, expected.parse().unwrap());
            assert_eq!(payment_origin(listener, &headers).unwrap(), expected);
        }
        assert_eq!(
            payment_origin("http://127.0.0.1:3000", &HeaderMap::new()).unwrap(),
            "http://127.0.0.1:3000"
        );
    }
    #[test]
    fn return_origin_rejects_untrusted_or_ambiguous_authorities() {
        for host in [
            "evil.example:3000",
            "localhost.evil.example:3000",
            "localhost:9999",
            "user@localhost:3000",
            "localhost:65536",
            "localhost:abc",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, host.parse().unwrap());
            assert!(
                payment_origin("http://127.0.0.1:3000", &headers).is_err(),
                "{host}"
            );
        }
        let mut headers = HeaderMap::new();
        headers.append(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.append(header::HOST, HeaderValue::from_static("127.0.0.1:3000"));
        assert!(payment_origin("http://127.0.0.1:3000", &headers).is_err());
        headers.remove(header::HOST);
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        for origin in [
            "http://evil.example",
            "http://localhost:9999",
            "https://localhost:3000",
            "null",
        ] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(payment_origin("http://127.0.0.1:3000", &headers).is_err());
        }
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:3000"),
        );
        headers.append(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:3000"),
        );
        assert!(payment_origin("http://127.0.0.1:3000", &headers).is_err());
    }
}
