//! Customer cookies, typed session requirements and request middleware.
use super::*;
const COOKIE: &str = "talocart_customer";
/// Absence, anonymous session and authenticated customer are distinct states.
#[derive(Clone, Default)]
pub(crate) struct OptionalCustomer(pub(crate) Option<CustomerSession>);
#[derive(Clone)]
pub(crate) struct AuthenticatedCustomer(pub(crate) CustomerSession);
pub(crate) enum SessionRejection {
    Middleware,
    Unauthenticated { login: Option<String>, htmx: bool },
}
impl SessionRejection {
    fn error(&self) -> CustomerError {
        match self {
            Self::Middleware => IdentityError::Unavailable,
            Self::Unauthenticated { .. } => IdentityError::Forbidden,
        }
        .into()
    }
}
impl IntoResponse for SessionRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthenticated {
                login: Some(login),
                htmx,
            } => {
                let mut response = private(Redirect::to(&login).into_response());
                if htmx {
                    *response.status_mut() = StatusCode::UNAUTHORIZED;
                    if let Ok(value) = HeaderValue::from_str(&login) {
                        response.headers_mut().insert("hx-redirect", value);
                    }
                }
                response
            }
            other => failure(other.error()),
        }
    }
}
/// Result captures rejection until the handler runs, preserving Form/Query rejection order.
/// POST handlers validate at their existing authorization boundary after native body parsing.
pub(crate) type RequiredCustomer = std::result::Result<AuthenticatedCustomer, SessionRejection>;
pub(crate) fn required(customer: &RequiredCustomer) -> Result<&CustomerSession> {
    match customer {
        Ok(customer) => Ok(&customer.0),
        Err(error) => Err(error.error()),
    }
}
impl OptionalCustomer {
    pub(super) fn session(&self) -> Result<&CustomerSession> {
        self.0
            .as_ref()
            .ok_or_else(|| IdentityError::Forbidden.into())
    }
    pub(crate) fn authenticated(&self) -> Option<&CustomerSession> {
        self.0.as_ref().filter(|s| s.customer_id().is_some())
    }
}
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for OptionalCustomer {
    type Rejection = SessionRejection;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(SessionRejection::Middleware)
    }
}
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for AuthenticatedCustomer {
    type Rejection = SessionRejection;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        let optional = OptionalCustomer::from_request_parts(parts, state).await?;
        match optional.authenticated() {
            Some(session) => Ok(Self(session.clone())),
            None => Err(SessionRejection::Unauthenticated {
                login: matches!(
                    parts.method,
                    axum::http::Method::GET | axum::http::Method::HEAD
                )
                .then(|| auth_link("login", &store_language::request_target())),
                htmx: parts.headers.get("hx-request").is_some_and(|h| h == "true"),
            }),
        }
    }
}

pub(super) fn token(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for h in headers.get_all(header::COOKIE) {
        for c in Cookie::split_parse(h.to_str().ok()?) {
            let c = c.ok()?;
            if c.name() == COOKIE {
                if found.is_some() || !talocart_platform::identity::valid_token(c.value()) {
                    return None;
                }
                found = Some(c.value().to_owned());
            }
        }
    }
    found
}

pub(super) fn set_cookie(r: &mut Response, raw: &str, secure: bool, seconds: i64) -> Result<()> {
    let c = Cookie::build((COOKIE, raw.to_owned()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(Duration::seconds(seconds))
        .build();
    r.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&c.to_string()).map_err(|_| IdentityError::Unavailable)?,
    );
    Ok(())
}

pub(super) fn forget_cart(r: &mut Response, secure: bool) -> Result<()> {
    let c = Cookie::build(("talocart_cart", ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(Duration::ZERO)
        .build();
    r.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&c.to_string()).map_err(|_| IdentityError::Unavailable)?,
    );
    Ok(())
}

pub(super) use crate::http_policy::private;

pub(super) fn redirect(path: &str) -> Response {
    private(Redirect::to(&store_language::path(path)).into_response())
}

/// Allow only readable customer/checkout destinations; no arbitrary redirect URLs.
pub(super) fn return_target(value: &str) -> String {
    if value.len() > 512
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_?=&".contains(&b))
    {
        return "/account".into();
    }
    let value = value
        .strip_prefix("/it/")
        .or_else(|| value.strip_prefix("/en/"))
        .map_or_else(|| value.to_owned(), |v| format!("/{v}"));
    let (path, query) = value
        .split_once('?')
        .map_or((value.as_str(), None), |(p, q)| (p, Some(q)));
    let allowed = matches!(
        path,
        "/account"
            | "/account/email"
            | "/account/password"
            | "/account/addresses"
            | "/account/addresses/new"
            | "/account/orders"
            | "/cart"
            | "/checkout"
            | "/checkout/methods"
    ) || path
        .strip_prefix("/account/addresses/")
        .is_some_and(|id| id.parse::<i64>().is_ok_and(|id| id > 0))
        || path.strip_prefix("/account/orders/").is_some_and(|r| {
            !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    let valid_query = query.is_none_or(|q| {
        path == "/account/orders"
            && q.strip_prefix("page=")
                .is_some_and(|p| p.parse::<i64>().is_ok_and(|p| p > 0))
    });
    if allowed && valid_query {
        value
    } else {
        "/account".into()
    }
}

pub(crate) fn auth_link(kind: &str, target: &str) -> String {
    let target = return_target(target);
    let path = if target == "/account" {
        format!("/account/{kind}")
    } else {
        format!("/account/{kind}?next={}", catalog::encoded(&target))
    };
    store_language::path(path)
}

pub(crate) fn public_path(path: &str) -> bool {
    matches!(
        path,
        "/account"
            | "/account/login"
            | "/account/register"
            | "/account/confirm"
            | "/account/confirmation"
            | "/account/logout"
            | "/account/reset"
            | "/account/password"
            | "/account/email"
            | "/account/addresses"
            | "/account/addresses/new"
            | "/account/orders"
    ) || path
        .strip_prefix("/account/addresses/")
        .is_some_and(|tail| {
            let id = tail
                .strip_suffix("/delete")
                .or_else(|| tail.strip_suffix("/use"))
                .unwrap_or(tail);
            id.parse::<i64>().is_ok()
        })
        || path
            .strip_prefix("/account/orders/")
            .is_some_and(|r| !r.is_empty() && !r.contains('/'))
}

pub(crate) async fn context(
    state: &AppState,
    mut request: axum::http::Request<Body>,
    next: Next,
    known: bool,
) -> Response {
    if !known {
        return cart::context(state, request, next, false).await;
    }
    let s = if let Some(raw) = token(request.headers()) {
        match state.customers().session(&raw).await {
            Ok(s) => s,
            Err(error) => return failure(error),
        }
    } else {
        None
    };
    // Gate new checkout steps only; historical order receipts/payments stay usable.
    let path = request.uri().path();
    let path = path
        .strip_prefix("/it")
        .or_else(|| path.strip_prefix("/en"))
        .unwrap_or(path);
    if matches!(path, "/checkout" | "/checkout/methods")
        && s.as_ref()
            .and_then(|session| session.customer_id())
            .is_none()
    {
        match talocart_platform::identity::customer_registration::RegistrationSettings::load(
            &state.pool,
        )
        .await
        {
            Ok(policy) if !policy.allow_guest_checkout => {
                let login = auth_link("login", "/checkout");
                return SessionRejection::Unauthenticated {
                    login: Some(login),
                    htmx: request
                        .headers()
                        .get("hx-request")
                        .is_some_and(|h| h == "true"),
                }
                .into_response();
            }
            Ok(_) => {}
            Err(error) => return failure(error.into()),
        }
    }
    // The binary supplies the TCP peer; forwarded headers are intentionally ignored.
    // The configured HTTPS proxy owns per-client limits. Counting its loopback
    // connection here would let one visitor exhaust every customer's budget.
    if let Some(peer) = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        let origin = peer.0.ip().to_canonical();
        let proxy_limits = origin.is_loopback()
            && state.secure_cookies
            && state.public_origin.starts_with("https://");
        let path = request.uri().path();
        let action = match (request.method(), path.rsplit('/').next()) {
            (&axum::http::Method::POST, Some("login")) if path.ends_with("/account/login") => {
                Some(CustomerAuthAction::Login)
            }
            (&axum::http::Method::POST, Some("register"))
                if path.ends_with("/account/register") =>
            {
                Some(CustomerAuthAction::Register)
            }
            (&axum::http::Method::POST, Some("reset" | "confirm" | "confirmation")) => {
                Some(CustomerAuthAction::Reset)
            }
            (
                &axum::http::Method::GET,
                Some("login" | "register" | "reset" | "confirm" | "confirmation"),
            ) if s.is_none()
                && [
                    "/account/login",
                    "/account/register",
                    "/account/reset",
                    "/account/confirm",
                    "/account/confirmation",
                ]
                .iter()
                .any(|tail| path.ends_with(tail)) =>
            {
                Some(CustomerAuthAction::Anonymous)
            }
            _ => None,
        };
        if let Some(action) = action
            && !proxy_limits
            && let Err(error) = state.customers().limit_origin(origin, action).await
        {
            return failure(error);
        }
    }
    request.extensions_mut().insert(OptionalCustomer(s));
    cart::context(state, request, next, known).await
}

#[cfg(test)]
mod extractor_tests {
    use super::*;
    use axum::extract::FromRequestParts;
    #[tokio::test]
    async fn missing_middleware_is_distinct_from_an_absent_session() {
        let (mut parts, _) = axum::http::Request::new(()).into_parts();
        assert_eq!(
            OptionalCustomer::from_request_parts(&mut parts, &())
                .await
                .err()
                .expect("missing middleware")
                .into_response()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        parts.extensions.insert(OptionalCustomer(None));
        let optional = OptionalCustomer::from_request_parts(&mut parts, &())
            .await
            .ok()
            .expect("optional context");
        assert!(optional.0.is_none());
        for method in [axum::http::Method::GET, axum::http::Method::HEAD] {
            parts.method = method;
            let response = AuthenticatedCustomer::from_request_parts(&mut parts, &())
                .await
                .err()
                .expect("login required")
                .into_response();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            parts
                .headers
                .insert("hx-request", HeaderValue::from_static("true"));
            let response = AuthenticatedCustomer::from_request_parts(&mut parts, &())
                .await
                .err()
                .expect("login required")
                .into_response();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(response.headers().contains_key("hx-redirect"));
            parts.headers.remove("hx-request");
        }
        parts.method = axum::http::Method::POST;
        assert_eq!(
            AuthenticatedCustomer::from_request_parts(&mut parts, &())
                .await
                .err()
                .expect("login required")
                .into_response()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}
