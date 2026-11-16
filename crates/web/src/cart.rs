//! Guest HTTP boundary. Native forms work equally with and without JavaScript.
use super::*;
use axum::{
    extract::{Form, Path},
    http::Request,
    response::Redirect,
};
use cookie::{Cookie, SameSite, time::Duration};
use serde::Deserialize;
use talocart_commerce::cart::{CartError, Carts, Guest, Issue, Quote, SESSION_SECONDS};
const COOKIE: &str = "talocart_cart";
#[derive(Clone)]
struct Context {
    guest: Option<Guest>,
    cookie: Option<String>,
    source: Option<Guest>,
}
/// Mutable capabilities and deferred cookie for exactly one request. Never held in a task-local.
#[derive(Clone)]
pub(crate) struct CartContext(std::sync::Arc<std::sync::Mutex<Context>>);
impl CartContext {
    fn read<T>(&self, read: impl FnOnce(&Context) -> T) -> Result<T, CartError> {
        let context = self.0.lock().map_err(|_| {
            CartError::Identity(talocart_platform::identity::IdentityError::Unavailable)
        })?;
        Ok(read(&context))
    }
    fn update<T>(&self, write: impl FnOnce(&mut Context) -> T) -> Result<T, CartError> {
        let mut context = self.0.lock().map_err(|_| {
            CartError::Identity(talocart_platform::identity::IdentityError::Unavailable)
        })?;
        Ok(write(&mut context))
    }
    pub(crate) fn guest(&self) -> Result<Guest, CartError> {
        self.read(|c| c.guest.clone())?.ok_or(CartError::Forbidden)
    }
    pub(crate) fn source(&self) -> Result<Option<Guest>, CartError> {
        self.read(|c| c.source.clone())
    }
    fn publish(&self) -> Result<(), CartError> {
        let guest = self.read(|c| c.guest.clone())?;
        super::store_presentation::update_cart(guest.as_ref());
        Ok(())
    }
}
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for CartContext {
    type Rejection = Error;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Self>().cloned().ok_or({
            Error(CartError::Identity(
                talocart_platform::identity::IdentityError::Unavailable,
            ))
        })
    }
}
pub(crate) async fn after_login(
    state: &AppState,
    raw: &str,
    context: &CartContext,
) -> Result<(), CartError> {
    let session = state
        .customers()
        .session(raw)
        .await
        .map_err(|error| match error {
            talocart_platform::identity::CustomerError::Technical(error) => {
                CartError::Technical(error)
            }
            talocart_platform::identity::CustomerError::Access(error) => CartError::Access(error),
            talocart_platform::identity::CustomerError::Identity(error) => {
                CartError::Identity(error)
            }
            _ => CartError::Forbidden,
        })?
        .ok_or(CartError::Forbidden)?;
    let carts = Carts::new(state.pool.clone());
    if let Some(source) = context.source()? {
        carts
            .merge_customer(
                &session,
                &session.csrf,
                &source,
                &source.csrf,
                source.revision,
            )
            .await
    } else {
        carts.ensure_customer(&session).await.map(|_| ())
    }
}

fn token(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for parsed in Cookie::split_parse(header.to_str().ok()?) {
            let cookie = parsed.ok()?;
            if cookie.name() == COOKIE {
                if found.is_some() || !talocart_platform::identity::valid_token(cookie.value()) {
                    return None;
                }
                found = Some(cookie.value().to_owned());
            }
        }
    }
    found
}
/// GET handlers call this after resource/input validation; POST and HEAD never create identity.
pub(crate) async fn establish(
    state: &AppState,
    context: &CartContext,
    customer: Option<&talocart_platform::identity::CustomerSession>,
) -> Result<(), talocart_platform::identity::IdentityError> {
    let identity_error = |error| match error {
        CartError::Technical(error) => error.into(),
        CartError::Access(error) => error.into(),
        CartError::Identity(error) => error,
        _ => talocart_platform::identity::IdentityError::Forbidden,
    };
    if context
        .read(|c| c.guest.is_some())
        .map_err(identity_error)?
    {
        return Ok(());
    }
    if let Some(session) = customer.filter(|s| s.customer_id().is_some()) {
        let cart = Carts::new(state.pool.clone())
            .ensure_customer(session)
            .await
            .map_err(identity_error)?;
        context
            .update(|c| c.guest = Some(cart))
            .map_err(identity_error)?;
    } else {
        let (raw, guest) = Carts::new(state.pool.clone()).create().await?;
        context
            .update(|c| {
                c.source = Some(guest.clone());
                c.guest = Some(guest);
                c.cookie = Some(raw);
            })
            .map_err(identity_error)?;
    }
    context.publish().map_err(identity_error)
}
/// Existing session capabilities are loaded once into request extensions.
pub(crate) async fn context(
    state: &AppState,
    mut request: Request<Body>,
    next: Next,
    known: bool,
) -> Response {
    if !known {
        return next.run(request).await;
    }
    let guest = match token(request.headers()) {
        Some(raw) => match Carts::new(state.pool.clone()).session(&raw).await {
            Ok(g) => g,
            Err(error) => return Error(error).into_response(),
        },
        None => None,
    };
    let source = guest.clone();
    let Some(customer) = request.extensions().get::<customers::OptionalCustomer>() else {
        return Error(CartError::Identity(
            talocart_platform::identity::IdentityError::Unavailable,
        ))
        .into_response();
    };
    let guest = if let Some(session) = customer.authenticated() {
        match Carts::new(state.pool.clone()).customer_cart(session).await {
            Ok(g) => g,
            Err(error) => return Error(error).into_response(),
        }
    } else {
        guest
    };
    let presentation = super::store_presentation::Presentation::new(
        guest.as_ref(),
        customer.0.as_ref().and_then(|s| s.customer_id()),
    );
    let context = CartContext(std::sync::Arc::new(std::sync::Mutex::new(Context {
        guest,
        source,
        cookie: None,
    })));
    request.extensions_mut().insert(context.clone());
    super::store_presentation::scope(presentation, async {
        let mut response = next.run(request).await;
        let raw = match context.update(|c| c.cookie.take()) {
            Ok(raw) => raw,
            Err(error) => return Error(error).into_response(),
        };
        if let Some(raw) = raw {
            let cookie = Cookie::build((COOKIE, raw))
                .path("/")
                .http_only(true)
                .same_site(SameSite::Lax)
                .secure(state.secure_cookies)
                .max_age(Duration::seconds(SESSION_SECONDS))
                .build();
            match HeaderValue::from_str(&cookie.to_string()) {
                Ok(value) => {
                    response.headers_mut().append(header::SET_COOKIE, value);
                }
                Err(_) => {
                    return Error(CartError::Identity(
                        talocart_platform::identity::IdentityError::Unavailable,
                    ))
                    .into_response();
                }
            }
        }
        response
    })
    .await
}

pub(crate) fn routes(prefix: &str) -> Router<AppState> {
    Router::new()
        .route(
            &format!("{prefix}/cart/coupon"),
            axum::routing::post(coupon),
        )
        .route(&format!("{prefix}/cart"), get(page).head(head))
        .route(&format!("{prefix}/cart/add"), axum::routing::post(add))
        .route(&format!("{prefix}/cart/merge"), axum::routing::post(merge))
        .route(
            &format!("{prefix}/cart/items/{{id}}"),
            axum::routing::post(update),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}
pub(crate) struct Error(CartError);
fn error_key(error: &CartError) -> &'static str {
    match error {
        CartError::Coupon => "coupon-unavailable",
        CartError::Invalid => "store-cart-invalid",
        CartError::Forbidden => "store-cart-expired",
        CartError::Stale => "store-cart-stale",
        CartError::Methods => "cm-invalid",
        CartError::Unavailable => "store-cart-unavailable",
        CartError::Quantity => "store-cart-quantity-error",
        CartError::Stock => "store-cart-stock-error",
        CartError::Limit => "store-cart-limit",
        CartError::Identity(_) | CartError::Technical(_) | CartError::Access(_) => {
            "store-cart-service-error"
        }
    }
}
fn status(error: &CartError) -> StatusCode {
    match error {
        CartError::Forbidden => StatusCode::FORBIDDEN,
        CartError::Stale => StatusCode::CONFLICT,
        CartError::Identity(_) | CartError::Technical(_) => StatusCode::SERVICE_UNAVAILABLE,
        CartError::Access(source_error) => access::access_status(source_error),
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if let CartError::Technical(source_error) = self.0 {
            return access::IdentityErrorResponse::from(source_error).into_response();
        }
        if let CartError::Access(source_error) = self.0 {
            return access::IdentityErrorResponse::from(source_error).into_response();
        }
        if let CartError::Identity(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        (status(&self.0), i18n::text(error_key(&self.0))).into_response()
    }
}
impl From<CartError> for Error {
    fn from(error: CartError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for Error {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(CartError::Identity(error))
    }
}
#[derive(Template)]
#[template(path = "default/templates/cart.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    category_menu: navigation::CategoryMenu,
    search_query: &'a str,
    search_action: &'a str,
    search_filters: navigation::SearchFilters,
    quote: Quote,
    csrf: String,
    error: String,
    saved: bool,
    stale: bool,
    submitted: Option<Input>,
    failed_id: i64,
    pending: Option<Pending>,
    customer_csrf: String,
    coupon_error: bool,
}
struct Pending {
    source: Guest,
    quote: Quote,
}
#[derive(Deserialize, Default)]
pub(crate) struct Input {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    variant_id: String,
    #[serde(default)]
    quantity: String,
    #[serde(default)]
    action: String,
    /// `minicart` asks HTMX requests for the refreshed dialog fragment instead of the page.
    #[serde(default)]
    view: String,
}
/// Rendering feedback belongs to the cart page; request capabilities stay explicit.
struct Feedback {
    error: Option<CartError>,
    submitted: Option<Input>,
    failed_id: i64,
    saved: bool,
    coupon_input: Option<String>,
}
async fn response(
    state: &AppState,
    cart_context: &CartContext,
    customer: &customers::OptionalCustomer,
    feedback: Feedback,
) -> Result<Response, Error> {
    let Feedback {
        error,
        submitted,
        failed_id,
        saved,
        coupon_input,
    } = feedback;
    let guest = cart_context.guest()?;
    let mut quote = Carts::new(state.pool.clone())
        .quote(&guest, store_language::content_language())
        .await?;
    if let Some(code) = coupon_input {
        quote.coupon_code = code;
    }
    let pending = if let Some(source) = cart_context
        .source()?
        .filter(|g| customer.authenticated().is_some() && g.count > 0)
    {
        let quote = Carts::new(state.pool.clone())
            .quote(&source, store_language::content_language())
            .await?;
        Some(Pending { source, quote })
    } else {
        None
    };
    let state = state.localized();
    let name = current_name(&state).await?;
    let category_menu = navigation::CategoryMenu::load(&state, "", false, "", false).await?;
    let code = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    let mut response = render(Page {
        store_name: &name,
        logo: current_logo(&state).await?,
        theme: store_language::settings(&state).await?,
        category_menu,
        search_query: "",
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        quote,
        csrf: guest.csrf,
        error: error
            .as_ref()
            .map(|entry| i18n::text(error_key(entry)))
            .unwrap_or_default(),
        saved,
        stale: matches!(error, Some(CartError::Stale)),
        submitted,
        failed_id,
        pending,
        coupon_error: matches!(error, Some(CartError::Coupon)),
        customer_csrf: customer
            .0
            .as_ref()
            .map(|s| s.csrf.clone())
            .unwrap_or_default(),
    });
    *response.status_mut() = code;
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    Ok(response)
}
#[derive(Deserialize)]
pub(crate) struct Saved {
    saved: Option<String>,
    preview: Option<String>,
}
pub(crate) async fn page(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<Saved>,
    cart_context: CartContext,
    customer: customers::OptionalCustomer,
) -> Result<Response, Error> {
    if query.preview.as_deref() == Some("1") {
        return preview(&state, &cart_context, None).await;
    }
    establish(&state, &cart_context, customer.0.as_ref()).await?;
    response(
        &state,
        &cart_context,
        &customer,
        Feedback {
            error: None,
            submitted: None,
            failed_id: 0,
            saved: query.saved.as_deref() == Some("1"),
            coupon_input: None,
        },
    )
    .await
}
#[derive(Template)]
#[template(path = "default/templates/minicart.html")]
struct MiniCart {
    quote: Option<Quote>,
    count: i64,
    csrf: String,
    /// Outcome of a dialog action: errors stay visible, success is announced only.
    notice: Option<(bool, String)>,
}
async fn preview(
    state: &AppState,
    cart_context: &CartContext,
    notice: Option<(bool, String)>,
) -> Result<Response, Error> {
    let (quote, csrf) = match cart_context.guest() {
        Ok(guest) => (
            Some(
                Carts::new(state.pool.clone())
                    .quote(&guest, store_language::content_language())
                    .await?,
            ),
            guest.csrf,
        ),
        Err(_) => (None, String::new()),
    };
    let count = quote.as_ref().map_or(0, |quote| {
        quote
            .lines
            .iter()
            .map(|line| i64::from(line.quantity))
            .sum()
    });
    let mut response = render(MiniCart {
        quote,
        count,
        csrf,
        notice,
    });
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    Ok(response)
}
async fn head(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<Saved>,
    cart_context: CartContext,
    customer: customers::OptionalCustomer,
) -> Result<Response, Error> {
    if query.preview.as_deref() == Some("1") {
        return preview(&state, &cart_context, None).await;
    }
    if cart_context.guest().is_ok() {
        return response(
            &state,
            &cart_context,
            &customer,
            Feedback {
                error: None,
                submitted: None,
                failed_id: 0,
                saved: query.saved.as_deref() == Some("1"),
                coupon_input: None,
            },
        )
        .await;
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                axum::http::HeaderName::from_static("x-robots-tag"),
                "noindex, nofollow",
            ),
        ],
        "",
    )
        .into_response())
}
fn parse<T: std::str::FromStr>(value: &str) -> Result<T, CartError> {
    value.parse().map_err(|_| CartError::Invalid)
}
pub(crate) async fn add(
    State(state): State<AppState>,
    cart_context: CartContext,
    customer: customers::OptionalCustomer,
    Form(input): Form<Input>,
) -> Result<Response, Error> {
    let guest = cart_context.guest()?;
    guest.check_csrf(&input.csrf)?;
    let result = async {
        Carts::new(state.pool.clone())
            .add(
                &guest,
                &input.csrf,
                parse(&input.revision)?,
                (
                    parse(&input.product_id)?,
                    if input.variant_id.is_empty() {
                        0
                    } else {
                        parse(&input.variant_id)?
                    },
                ),
                parse(&input.quantity)?,
                store_language::content_language(),
            )
            .await
    }
    .await;
    finish(&state, &cart_context, &customer, result, input, 0).await
}
pub(crate) async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    cart_context: CartContext,
    customer: customers::OptionalCustomer,
    Form(input): Form<Input>,
) -> Result<Response, Error> {
    let guest = cart_context.guest()?;
    guest.check_csrf(&input.csrf)?;
    let result = async {
        let quantity = match input.action.as_str() {
            "remove" => None,
            "update" => Some(parse(&input.quantity)?),
            _ => return Err(CartError::Invalid),
        };
        Carts::new(state.pool.clone())
            .update(
                &guest,
                &input.csrf,
                parse(&input.revision)?,
                id,
                quantity,
                store_language::content_language(),
            )
            .await
    }
    .await;
    let htmx = headers
        .get("hx-request")
        .is_some_and(|value| value == "true");
    if htmx && input.view == "minicart" {
        return match result {
            Ok(()) => {
                preview(
                    &state,
                    &cart_context,
                    Some((false, i18n::text("store-minicart-removed"))),
                )
                .await
            }
            Err(
                source_error @ (CartError::Forbidden
                | CartError::Identity(_)
                | CartError::Technical(_)
                | CartError::Access(_)),
            ) => Err(Error(source_error)),
            Err(source_error) => {
                preview(
                    &state,
                    &cart_context,
                    Some((true, i18n::text(error_key(&source_error)))),
                )
                .await
            }
        };
    }
    finish(&state, &cart_context, &customer, result, input, id).await
}
async fn finish(
    state: &AppState,
    cart_context: &CartContext,
    customer: &customers::OptionalCustomer,
    result: Result<(), CartError>,
    input: Input,
    id: i64,
) -> Result<Response, Error> {
    match result {
        Ok(()) => Ok(Redirect::to(&store_language::path("/cart?saved=1")).into_response()),
        Err(
            source_error @ (CartError::Forbidden
            | CartError::Identity(_)
            | CartError::Technical(_)
            | CartError::Access(_)),
        ) => Err(Error(source_error)),
        Err(source_error) => {
            response(
                state,
                cart_context,
                customer,
                Feedback {
                    error: Some(source_error),
                    submitted: Some(input),
                    failed_id: id,
                    saved: false,
                    coupon_input: None,
                },
            )
            .await
        }
    }
}
pub(crate) fn issue(issue: &Option<Issue>) -> String {
    match issue {
        Some(Issue::Unavailable) => i18n::text("store-cart-unavailable"),
        Some(Issue::Quantity) => i18n::text("store-cart-quantity-error"),
        Some(Issue::Stock) => i18n::text("store-cart-stock-error"),
        None => String::new(),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MergeInput {
    csrf: String,
    source_csrf: String,
    revision: String,
}
async fn merge(
    State(state): State<AppState>,
    cart_context: CartContext,
    customer: customers::RequiredCustomer,
    Form(input): Form<MergeInput>,
) -> Result<Response, Error> {
    let revision = input
        .revision
        .parse::<i64>()
        .map_err(|_| CartError::Invalid)?;
    let session = customers::required(&customer).map_err(|_| CartError::Forbidden)?;
    let source = cart_context.source()?.ok_or(CartError::Forbidden)?;
    match Carts::new(state.pool.clone())
        .merge_customer(session, &input.csrf, &source, &input.source_csrf, revision)
        .await
    {
        Ok(()) => Ok(Redirect::to(&store_language::path("/cart?saved=1")).into_response()),
        Err(CartError::Forbidden) => Err(CartError::Forbidden.into()),
        Err(source_error) => {
            response(
                &state,
                &cart_context,
                &customers::OptionalCustomer(Some(session.clone())),
                Feedback {
                    error: Some(source_error),
                    submitted: None,
                    failed_id: 0,
                    saved: false,
                    coupon_input: None,
                },
            )
            .await
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CouponInput {
    csrf: String,
    revision: String,
    code: String,
    action: String,
}
async fn coupon(
    State(state): State<AppState>,
    cart_context: CartContext,
    customer: customers::OptionalCustomer,
    Form(input): Form<CouponInput>,
) -> Result<Response, Error> {
    let g = cart_context.guest()?;
    g.check_csrf(&input.csrf)?;
    let result = async {
        let code = match input.action.as_str() {
            "remove" => "",
            "apply" => &input.code,
            _ => return Err(CartError::Invalid),
        };
        Carts::new(state.pool.clone())
            .set_coupon(
                &g,
                &input.csrf,
                parse(&input.revision)?,
                code,
                store_language::content_language(),
            )
            .await
    }
    .await;
    match result {
        Ok(()) => Ok(Redirect::to(&store_language::path("/cart?saved=1")).into_response()),
        Err(
            source_error @ (CartError::Forbidden
            | CartError::Identity(_)
            | CartError::Technical(_)
            | CartError::Access(_)),
        ) => Err(Error(source_error)),
        Err(source_error) => {
            response(
                &state,
                &cart_context,
                &customer,
                Feedback {
                    error: Some(source_error),
                    submitted: None,
                    failed_id: 0,
                    saved: false,
                    coupon_input: Some(input.code),
                },
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_cookie_is_unambiguous_and_independent_from_admin_identity() {
        let raw = "a".repeat(64);
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("talocart_session={raw}; talocart_cart={raw}")).unwrap(),
        );
        assert_eq!(token(&headers), Some(raw.clone()));
        headers.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("talocart_cart={raw}")).unwrap(),
        );
        assert_eq!(token(&headers), None);
        for value in [
            "talocart_cart=bad",
            "talocart_cart=",
            "talocart_session=bad",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::COOKIE, HeaderValue::from_static(value));
            assert_eq!(token(&headers), None);
        }
    }
}
