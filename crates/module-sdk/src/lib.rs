//! Shared native-process transport. Modules retain their database and business hooks.
mod diagnostics;
pub use diagnostics::Diagnostics;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderValue, Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::future::Future;
use subtle::ConstantTimeEq;
use talocart_platform::packages::{Manifest, protocol};
use tokio::{io::BufReader, sync::RwLock};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Module-owned pending work is checked only after exclusive request drainage.
pub trait Hooks: Clone + Send + Sync + 'static {
    fn token(&self) -> &str;
    fn gate(&self) -> &RwLock<bool>;
    fn busy(&self) -> impl Future<Output = std::result::Result<bool, ()>> + Send;
    fn shared(&self, method: &Method, _path: &str) -> bool {
        matches!(*method, Method::GET | Method::HEAD)
    }
    fn restart(&self) {}
    fn no_store(&self) -> bool {
        false
    }
}

/// Parse the single command and bounded handshake before opening module services.
pub async fn start(
    manifest: &Manifest,
) -> Result<Option<(protocol::Start, BufReader<tokio::io::Stdin>)>> {
    let arg = std::env::args_os().nth(1);
    if arg.as_deref() == Some(std::ffi::OsStr::new("--describe-package")) {
        println!("{}", serde_json::to_string(manifest)?);
        return Ok(None);
    }
    if arg.as_deref() != Some(std::ffi::OsStr::new("--module-protocol")) {
        return Err("Unsupported module command".into());
    }
    let mut stdin = BufReader::new(tokio::io::stdin());
    let start: protocol::Start = serde_json::from_slice(&protocol::read_line(&mut stdin).await?)?;
    start.validate_for(manifest)?;
    Ok(Some((start, stdin)))
}

pub fn reply(manifest: &Manifest, port: Option<u16>, busy: bool) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&protocol::Reply {
            protocol: manifest.protocol,
            sdk_version: manifest.sdk_version.clone(),
            database_contract: manifest.database.version,
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            port,
            busy,
        })?
    );
    Ok(())
}

/// Serve only on loopback; the caller supplies its stdin lifeline and worker shutdown.
pub async fn serve(
    manifest: Manifest,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    reply(&manifest, Some(listener.local_addr()?.port()), false).map_err(std::io::Error::other)?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

pub async fn authenticate<H: Hooks>(
    State(hooks): State<H>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request
        .headers()
        .get("x-talocart-module")
        .is_none_or(|v| !bool::from(hooks.token().as_bytes().ct_eq(v.as_bytes())))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Health never waits on admission and never changes quiescence.
    if request.uri().path() == protocol::HEALTH_PATH {
        return if request.method() == Method::GET {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::METHOD_NOT_ALLOWED
        }
        .into_response();
    }
    if request.uri().path() == "/__talocart_package/control" {
        if request.method() != Method::POST {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let mut quiesced = hooks.gate().write().await;
        let Ok(body) = axum::body::to_bytes(request.into_body(), 16).await else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        if body == "resume" {
            *quiesced = false;
            return StatusCode::OK.into_response();
        }
        if body != "quiesce" {
            return StatusCode::BAD_REQUEST.into_response();
        }
        return match hooks.busy().await {
            Ok(false) => {
                *quiesced = true;
                StatusCode::OK.into_response()
            }
            Ok(true) => StatusCode::CONFLICT.into_response(),
            Err(()) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    }
    let mut response = if hooks.shared(request.method(), request.uri().path()) {
        let quiesced = hooks.gate().read().await;
        if *quiesced {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let response = next.run(request).await;
        if wants_restart(&response) {
            drop(quiesced);
            *hooks.gate().write().await = true;
            hooks.restart();
        }
        response
    } else {
        let mut quiesced = hooks.gate().write().await;
        if *quiesced {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let response = next.run(request).await;
        if wants_restart(&response) {
            *quiesced = true;
            hooks.restart();
        }
        response
    };
    if hooks.no_store() {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}
fn wants_restart(response: &Response) -> bool {
    response
        .headers()
        .get("x-talocart-module-restart")
        .is_some_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    };
    use tower::ServiceExt;
    #[derive(Clone)]
    struct Fixture {
        gate: Arc<RwLock<bool>>,
        busy: Arc<AtomicU8>,
    }
    impl Hooks for Fixture {
        fn token(&self) -> &str {
            "synthetic"
        }
        fn gate(&self) -> &RwLock<bool> {
            &self.gate
        }
        async fn busy(&self) -> std::result::Result<bool, ()> {
            match self.busy.load(Ordering::SeqCst) {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(()),
            }
        }
    }
    async fn call(app: &Router, method: &str, path: &str, token: &str, body: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("x-talocart-module", token)
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }
    #[tokio::test]
    async fn health_control_and_requests_share_one_authenticated_lifecycle() {
        let state = Fixture {
            gate: Arc::new(RwLock::new(false)),
            busy: Arc::new(AtomicU8::new(0)),
        };
        let app = Router::new()
            .route("/page", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                authenticate::<Fixture>,
            ));
        let guard = state.gate.write().await;
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                call(&app, "GET", protocol::HEALTH_PATH, "synthetic", "")
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(&app, "GET", protocol::HEALTH_PATH, "wrong", "").await,
            StatusCode::FORBIDDEN
        );
        drop(guard);
        let control = "/__talocart_package/control";
        assert_eq!(
            call(&app, "GET", control, "synthetic", "").await,
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            call(&app, "POST", control, "synthetic", "invalid").await,
            StatusCode::BAD_REQUEST
        );
        for (value, status) in [
            (1, StatusCode::CONFLICT),
            (2, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            state.busy.store(value, Ordering::SeqCst);
            assert_eq!(
                call(&app, "POST", control, "synthetic", "quiesce").await,
                status
            );
            assert!(!*state.gate.read().await);
        }
        state.busy.store(0, Ordering::SeqCst);
        assert_eq!(
            call(&app, "POST", control, "synthetic", "quiesce").await,
            StatusCode::OK
        );
        assert_eq!(
            call(&app, "GET", "/page", "synthetic", "").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            call(&app, "POST", control, "synthetic", "resume").await,
            StatusCode::OK
        );
        assert_eq!(
            call(&app, "GET", "/page", "synthetic", "").await,
            StatusCode::OK
        );
    }
}
