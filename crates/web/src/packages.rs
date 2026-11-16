//! Owner-only package management and authenticated forwarding to installed executables.
use super::*;
use axum::{
    extract::{Form, Multipart},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::packages::{self, Manager};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/modules/preview",
            axum::routing::post(preview).layer(DefaultBodyLimit::max(packages::LIMIT + 40000)),
        )
        .route("/admin/modules/install", axum::routing::post(install))
        .route("/admin/modules/update", axum::routing::post(update))
        .route("/admin/modules/retry", axum::routing::post(retry))
        .route("/admin/modules/restart", axum::routing::post(restart))
        .route("/admin/modules/uninstall", axum::routing::post(uninstall))
        .route("/admin/modules/toggle", axum::routing::post(toggle))
        .route("/admin/modules/critical", axum::routing::post(critical))
}
fn manager(state: &AppState) -> std::result::Result<&Manager, packages::Error> {
    state.packages.as_ref().ok_or(packages::Error::Incompatible)
}
async fn require_panel(state: &AppState) -> Result<(), access::IdentityErrorResponse> {
    if !state.panel_install().await? {
        return Err(talocart_platform::identity::IdentityError::Forbidden.into());
    }
    Ok(())
}
fn error(error: &packages::Error) -> &'static str {
    match error {
        packages::Error::Invalid => "packages-error-invalid",
        packages::Error::Incompatible => "packages-error-incompatible",
        packages::Error::Signature => "packages-error-signature",
        packages::Error::Conflict => "packages-error-conflict",
        packages::Error::Busy => "packages-error-busy",
        packages::Error::Preview => "packages-error-preview",
        packages::Error::Version => "packages-error-version",
        packages::Error::Schema => "packages-error-schema",
        packages::Error::Incomplete => "packages-error-incomplete",
        packages::Error::Started => "packages-error-started",
        packages::Error::RolledBack => "packages-error-rolled-back",
        packages::Error::Active => "packages-error-active",
        _ => "packages-error-unavailable",
    }
}
async fn result(
    state: &AppState,
    session: &Session,
    result: std::result::Result<(), packages::Error>,
) -> Result<Response, access::IdentityErrorResponse> {
    match result {
        Ok(()) => Ok(Redirect::to("/admin/modules").into_response()),
        Err(source_error) => {
            modules::page(
                state,
                session,
                None,
                i18n::text(error(&source_error)),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
    }
}
async fn preview(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    mut multipart: Multipart,
) -> Result<Response, access::IdentityErrorResponse> {
    state.identity.authorize_owner(&session).await?;
    require_panel(&state).await?;
    let parsed = async {
        let mut csrf = false;
        let mut bytes = None;
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|_| packages::Error::Invalid)?
        {
            match field.name() {
                Some("csrf") if !csrf && bytes.is_none() => {
                    let value = field.text().await.map_err(|_| packages::Error::Invalid)?;
                    session
                        .check_csrf(&value)
                        .map_err(|_| packages::Error::Invalid)?;
                    csrf = true;
                }
                Some("package") if csrf && bytes.is_none() => {
                    bytes = Some(
                        field
                            .bytes()
                            .await
                            .map_err(|_| packages::Error::Invalid)?
                            .to_vec(),
                    )
                }
                _ => return Err(packages::Error::Invalid),
            }
        }
        if !csrf {
            return Err(packages::Error::Invalid);
        }
        bytes.ok_or(packages::Error::Invalid)
    }
    .await;
    // Uploads and content checks must not retain authorization from a revoked session.
    state.identity.authorize_owner(&session).await?;
    let preview = match parsed {
        Ok(bytes) => match manager(&state) {
            Ok(m) => m.preview(session.token_hash.clone(), bytes).await,
            Err(source_error) => Err(source_error),
        },
        Err(source_error) => Err(source_error),
    };
    match preview {
        Ok(p) => modules::page(&state, &session, Some(p), String::new(), StatusCode::OK).await,
        Err(source_error) => {
            modules::page(
                &state,
                &session,
                None,
                i18n::text(error(&source_error)),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
    }
}
#[derive(Deserialize)]
struct Confirm {
    csrf: String,
    preview: String,
    #[serde(default)]
    password: String,
}
async fn install(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Confirm>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    require_panel(&state).await?;
    state
        .identity
        .reauthenticate_owner(&session, &form.csrf, form.password)
        .await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(m) => {
                m.install(&session.token_hash, &form.preview, &state.pool)
                    .await
            }
            Err(error) => Err(error),
        },
    )
    .await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Confirm>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    require_panel(&state).await?;
    state
        .identity
        .reauthenticate_owner(&session, &form.csrf, form.password)
        .await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(m) => {
                m.update(&session.token_hash, &form.preview, &state.pool)
                    .await
            }
            Err(error) => Err(error),
        },
    )
    .await
}
#[derive(Deserialize)]
struct Target {
    csrf: String,
    id: String,
    #[serde(default)]
    password: String,
}
async fn restart(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Target>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(manager) => manager.restart(&form.id).await,
            Err(error) => Err(error),
        },
    )
    .await
}
async fn retry(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Target>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    require_panel(&state).await?;
    state
        .identity
        .reauthenticate_owner(&session, &form.csrf, form.password)
        .await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(m) => m.retry(&form.id, &state.pool).await,
            Err(error) => Err(error),
        },
    )
    .await
}
async fn uninstall(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Target>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(m) => m.uninstall(&form.id).await,
            Err(error) => Err(error),
        },
    )
    .await
}
#[derive(Deserialize)]
struct Toggle {
    csrf: String,
    id: String,
    enabled: bool,
}
async fn toggle(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Toggle>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(m) => m.set_enabled(&form.id, form.enabled).await,
            Err(error) => Err(error),
        },
    )
    .await
}
#[derive(Deserialize)]
struct Critical {
    csrf: String,
    id: String,
    critical: bool,
}
async fn critical(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Critical>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state.identity.authorize_owner(&session).await?;
    result(
        &state,
        &session,
        match manager(&state) {
            Ok(m) => m.set_critical(&form.id, form.critical).await,
            Err(error) => Err(error),
        },
    )
    .await
}
/// Core routes always win. Only validated module namespaces can reach an executable.
pub(super) async fn fallback(
    State(state): State<AppState>,
    request: axum::http::Request<Body>,
) -> Response {
    let path = request.uri().path();
    let Some(manager) = state.packages.as_ref() else {
        return error_pages::not_found().await;
    };
    // Pick a candidate ID, then re-read every route/permission under its gate.
    // A lifecycle operation may replace or remove it while this request waits.
    let candidate = manager.inventory().ok().and_then(|items| {
        items.into_iter().find(|p| {
            p.enabled
                && p.manifest
                    .paths
                    .iter()
                    .any(|prefix| packages::matches_path(prefix, path))
        })
    });
    let Some(candidate) = candidate else {
        return error_pages::not_found().await;
    };
    // Reject anonymous callers before waiting for requests or lifecycle work.
    // Routes and current permissions are still re-read under the module gate.
    let Some(session) = request
        .extensions()
        .get::<Session>()
        .filter(|s| s.administrator_id.is_some())
    else {
        let mut response = Redirect::to("/admin/login").into_response();
        if request
            .headers()
            .get("hx-request")
            .is_some_and(|v| v == "true")
        {
            *response.status_mut() = StatusCode::UNAUTHORIZED;
            response
                .headers_mut()
                .insert("hx-redirect", HeaderValue::from_static("/admin/login"));
        }
        return response;
    };
    let mutation = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    // Settings must exclude payment leases through the host's SQL commit, not only
    // in-flight handlers inside the module. Both admission paths have a bounded wait.
    let admission = if mutation {
        manager.mutation_guard(&candidate.manifest.id).await
    } else {
        manager.request_guard(&candidate.manifest.id).await
    };
    let Ok(guard) = admission else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let item = manager.inventory().ok().and_then(|items| {
        items.into_iter().find(|p| {
            p.manifest.id == candidate.manifest.id
                && p.enabled
                && p.manifest
                    .paths
                    .iter()
                    .any(|prefix| packages::matches_path(prefix, path))
        })
    });
    let Some(item) = item else {
        return error_pages::not_found().await;
    };
    let permitted = item
        .manifest
        .permissions
        .iter()
        .filter_map(|p| Permission::parse(p).ok())
        .any(|p| session.allows(p));
    if !permitted {
        return access::IdentityErrorResponse::from(
            talocart_platform::identity::IdentityError::Forbidden,
        )
        .into_response();
    }
    // Only after authentication and permissions: anonymous callers learn nothing about packages.
    if !item.available() {
        // Known route of a package awaiting a compatible or completed update, or a reinstall.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
            i18n::text(if item.damaged() {
                "packages-state-damaged"
            } else {
                "packages-error-requires-update"
            }),
        )
            .into_response();
    }
    let navigation = extension_context::encode(&state, session);
    let forwarded = async {
        let (port, token) = guard.endpoint().await?;
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, 16 * 1024 * 1024)
            .await
            .map_err(|_| packages::Error::Invalid)?;
        let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
        let parsed = talocart_platform::packages::proxy_url(port, path)?;
        let mut authentication =
            HeaderValue::from_str(&token).map_err(|_| packages::Error::Execution)?;
        authentication.set_sensitive(true);
        let mut call = manager
            .client()
            .request(parts.method, parsed)
            .body(body)
            .header("x-talocart-module", authentication);
        if let Some(navigation) = navigation {
            call = call.header(extension_context::HEADER, navigation);
        }
        if let Some(mut cookie) = admin_cookie(&parts.headers) {
            cookie.set_sensitive(true);
            call = call.header(header::COOKIE, cookie);
        }
        // A fixed allowlist excludes Host, forwarded addresses, hop-by-hop and caller-supplied module authentication.
        for name in [
            "accept",
            "content-type",
            "accept-language",
            "hx-request",
            "hx-target",
            "hx-trigger",
            "hx-current-url",
        ] {
            if let Some(value) = parts.headers.get(name) {
                call = call.header(name, value.clone());
            }
        }
        let mut upstream = call.send().await.map_err(|_| packages::Error::Execution)?;
        let status = upstream.status();
        let headers = upstream.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = upstream
            .chunk()
            .await
            .map_err(|_| packages::Error::Execution)?
        {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err(packages::Error::Execution);
            }
            bytes.extend_from_slice(&chunk);
        }
        if mutation {
            manager.forget_payment_status(&item.manifest.id);
        }
        let restart = headers
            .get("x-talocart-module-restart")
            .is_some_and(|v| v == "1")
            .then_some((port, token));
        let mut response = (status, bytes).into_response();
        for name in [
            "content-type",
            "content-disposition",
            "location",
            "hx-redirect",
            "hx-refresh",
            "content-language",
            "vary",
        ] {
            if let Some(value) = headers.get(name) {
                response.headers_mut().insert(name, value.clone());
            }
        }
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok::<_, packages::Error>((response, restart))
    }
    .await;
    // Release this request before a requested restart waits for the others to drain.
    drop(guard);
    match forwarded {
        Ok((response, restart)) => {
            if let Some((port, token)) = restart {
                manager
                    .stop_generation(&item.manifest.id, port, &token)
                    .await;
            }
            response
        }
        Err(error) => {
            if matches!(error, packages::Error::Execution) {
                manager.record_request_failure(&item.manifest.id);
            }
            (
                StatusCode::SERVICE_UNAVAILABLE,
                i18n::text("packages-error-unavailable"),
            )
                .into_response()
        }
    }
}

fn admin_cookie(headers: &HeaderMap) -> Option<HeaderValue> {
    let token = access::session_token(headers)?;
    HeaderValue::from_str(&format!("talocart_session={token}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_forwards_only_one_valid_administrative_cookie() {
        let token = talocart_platform::identity::token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "customer_session=private; talocart_session={token}; tracking=private"
            ))
            .unwrap(),
        );
        assert_eq!(
            admin_cookie(&headers).unwrap(),
            format!("talocart_session={token}")
        );
        headers.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("talocart_session={token}")).unwrap(),
        );
        assert!(admin_cookie(&headers).is_none());
    }

    use std::{fs, future::Future, io::Read, task::Poll};

    /// Poll the real proxy while a lifecycle operation owns its gate. The module
    /// is disabled before the request resumes, so neither old permissions nor
    /// old availability may determine its response.
    #[tokio::test]
    async fn proxy_waits_for_lifecycle_before_reading_routes_permissions_and_availability() {
        let folder = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "talocart-proxy-gate-{}",
                talocart_platform::identity::token().unwrap()
            ));
        let root = folder.join("packages");
        fs::create_dir_all(root.join("installed/fixture/1.0.0")).unwrap();
        let key = folder.join("publisher.pk8");
        let publisher = packages::build::keygen(&key).unwrap();
        let manifest = packages::Manifest {
            format: packages::sdk::MANIFEST_FORMAT,
            protocol: packages::protocol::VERSION,
            id: "fixture".into(),
            version: "1.0.0".into(),

            sdk_version: packages::sdk::VERSION.into(),
            database: talocart_platform::database::contract(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            title: packages::Labels {
                en: "Fixture".into(),
                it: "Prova".into(),
            },
            description: packages::Labels {
                en: "Test only".into(),
                it: "Solo test".into(),
            },
            notices: "Synthetic".into(),
            payment: None,
            menu: "/admin/extensions/fixture".into(),
            paths: vec!["/admin/extensions/fixture".into()],
            permissions: vec!["products.read".into()],
            ledger: "_fixture_migrations".into(),
            migrations: vec![packages::Migration {
                version: 1,
                checksum: "00".repeat(48),
            }],
            publisher: String::new(),
            size: 0,
            sha256: String::new(),
        };
        // This executable is never started: the regression concerns requests
        // queued during recovery, when an incomplete module may be deactivated.
        let mut executable_header = [0; 64];
        fs::File::open(std::env::current_exe().unwrap())
            .unwrap()
            .read_exact(&mut executable_header)
            .unwrap();
        let mut other = manifest.clone();
        other.id = "independent".into();
        other.ledger = "_independent_migrations".into();
        other.menu = "/admin/extensions/independent".into();
        other.paths = vec![other.menu.clone()];
        let other_signed =
            packages::sign(other, &executable_header, &fs::read(&key).unwrap()).unwrap();
        let other_verified =
            packages::verify(&other_signed, std::slice::from_ref(&publisher)).unwrap();
        fs::create_dir_all(root.join("installed/independent/1.0.0")).unwrap();
        fs::write(
            root.join("installed/independent/1.0.0/module"),
            &other_verified.executable,
        )
        .unwrap();
        let other_installed = serde_json::json!({"manifest":other_verified.manifest, "raw":other_verified.manifest_bytes,
            "signature":other_verified.signature, "enabled":true});
        let signed = packages::sign(manifest, &executable_header, &fs::read(key).unwrap()).unwrap();
        let verified = packages::verify(&signed, std::slice::from_ref(&publisher)).unwrap();
        fs::write(
            root.join("installed/fixture/1.0.0/module"),
            &verified.executable,
        )
        .unwrap();
        let release = serde_json::json!({"manifest": verified.manifest, "raw": verified.manifest_bytes, "signature": verified.signature});
        let mut installed = release.clone();
        installed["enabled"] = true.into();
        installed["update"] = release;
        let registry = serde_json::to_vec(
            &serde_json::json!({"publishers":[publisher], "installed":[installed, other_installed]}),
        )
        .unwrap();
        let mut results = Vec::new();
        for locale in ["en", "it"] {
            for permissions in [vec![], vec!["products.read".to_owned()]] {
                fs::write(root.join("registry.json"), &registry).unwrap();
                let url = "postgres://unused@127.0.0.1:1/unreachable_test";
                let manager = Manager::open(packages::RuntimeConfig {
                    root: root.clone(),
                    database_url: url.into(),
                    media_root: folder.join("media"),
                    store_name: "Synthetic".into(),
                    public_origin: "http://127.0.0.1:1".into(),
                    secure_cookies: false,
                    compiled: vec![],
                    serve: true,
                })
                .await
                .unwrap();
                let pool = sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy(url)
                    .unwrap();
                let state = AppState::configured(
                    pool,
                    "Synthetic".into(),
                    crate::AppStateConfig {
                        packages: Some(manager.clone()),
                        ..Default::default()
                    },
                );
                let session = Session {
                    token_hash: "synthetic".into(),
                    csrf: "synthetic".into(),
                    administrator_id: Some(1),
                    role: Some("viewer".into()),
                    permissions,
                    display_name: "Fixture".into(),
                    email: None,
                    has_avatar: false,
                    avatar_version: 0,
                    locale: locale.into(),
                };
                // A payment lease holds shared host admission across its SQL commit,
                // after the module's Status handler has already released its own lock.
                // Exercise the real proxy: reads may overlap, settings mutations must wait.
                for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
                    let payment_transaction = manager.request_guard("fixture").await.unwrap();
                    let request = axum::http::Request::builder()
                        .method(method)
                        .uri("/admin/extensions/fixture")
                        .extension(session.clone())
                        .body(Body::empty())
                        .unwrap();
                    let future = i18n::scope(
                        talocart_platform::locale::Locale::parse(locale).unwrap(),
                        fallback(State(state.clone()), request),
                    );
                    tokio::pin!(future);
                    let initial =
                        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await;
                    let waited = initial.is_pending();
                    drop(payment_transaction);
                    let response = match initial {
                        Poll::Pending => future.await,
                        Poll::Ready(response) => response,
                    };
                    assert_eq!(waited, !matches!(method, "GET" | "HEAD"), "{method}");
                    assert_eq!(
                        response.status(),
                        if session.permissions.is_empty() {
                            StatusCode::FORBIDDEN
                        } else {
                            StatusCode::SERVICE_UNAVAILABLE
                        },
                        "Permissions and incomplete-package checks still apply after admission"
                    );
                }
                let request = axum::http::Request::builder()
                    .uri("/admin/extensions/fixture")
                    .extension(session)
                    .body(Body::empty())
                    .unwrap();
                let guard = manager.module_guard("fixture").await.unwrap();
                for htmx in [false, true] {
                    let mut anonymous =
                        axum::http::Request::builder().uri("/admin/extensions/fixture");
                    if htmx {
                        anonymous = anonymous.header("hx-request", "true");
                    }
                    let rejected = tokio::time::timeout(
                        Duration::from_secs(1),
                        fallback(State(state.clone()), anonymous.body(Body::empty()).unwrap()),
                    )
                    .await
                    .expect("Anonymous requests must not wait for the module lifecycle gate");
                    assert_eq!(
                        rejected.status(),
                        if htmx {
                            StatusCode::UNAUTHORIZED
                        } else {
                            StatusCode::SEE_OTHER
                        }
                    );
                    assert_eq!(
                        rejected.headers()[if htmx { "hx-redirect" } else { "location" }],
                        "/admin/login"
                    );
                }
                // A blocked request/update for fixture must not queue a different package.
                let other_request = axum::http::Request::builder()
                    .uri("/admin/extensions/independent")
                    .body(Body::empty())
                    .unwrap();
                let other = tokio::time::timeout(
                    Duration::from_secs(1),
                    fallback(State(state.clone()), other_request),
                )
                .await
                .expect("Independent module must reach authentication while fixture is locked");
                assert_eq!(other.status(), StatusCode::SEE_OTHER);
                let mutation = manager.set_enabled("fixture", false);
                tokio::pin!(mutation);
                assert!(
                    std::future::poll_fn(|cx| Poll::Ready(mutation.as_mut().poll(cx)))
                        .await
                        .is_pending()
                );
                // This current-thread test yields to the spawned lifecycle task
                // so it queues on the held gate before the request is polled.
                tokio::task::yield_now().await;
                let future = i18n::scope(
                    talocart_platform::locale::Locale::parse(locale).unwrap(),
                    fallback(State(state), request),
                );
                tokio::pin!(future);
                let initial =
                    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await;
                drop(guard);
                mutation.await.unwrap();
                let (waited, response) = match initial {
                    Poll::Pending => (true, future.await),
                    Poll::Ready(response) => (false, response),
                };
                results.push((waited, response.status()));
            }
        }
        fs::remove_dir_all(folder).unwrap();
        assert_eq!(
            results,
            vec![(true, StatusCode::NOT_FOUND); 4],
            "Queued requests must observe the post-operation registry in both languages, regardless of old permissions"
        );
    }
}
