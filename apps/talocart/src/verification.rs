//! Narrow HTTP surface for maintenance probes. Business writes and module routes stay closed.
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(crate) async fn read_only(request: Request<Body>, next: Next) -> Response {
    let cart_preview =
        request.uri().path() == "/cart" && request.uri().query() == Some("preview=1");
    let allowed = matches!(request.method(), &Method::GET | &Method::HEAD)
        && (cart_preview
            || matches!(
                request.uri().path(),
                "/health/live" | "/health/ready" | "/" | "/en" | "/catalog" | "/assets/ui.css"
            ));
    if !allowed {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let ready = request.uri().path() == "/health/ready" && request.method() == Method::GET;
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-talocart-verification",
        axum::http::HeaderValue::from_static("1"),
    );
    if ready && response.status() == StatusCode::OK {
        response.headers_mut().remove(header::CONTENT_LENGTH);
        response = response.map(|_| Body::from("verification-ready\n"));
    }
    response
}
