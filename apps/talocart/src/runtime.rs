//! HTTP/worker composition and ordered shutdown.
use crate::{diagnostics, preflight};
use talocart_platform::{
    config::Config, database::PgPool, identity::Identity, media::MediaStore, modules,
    packages::Manager,
};

pub(crate) struct RuntimeExtensions {}

pub(crate) async fn serve(
    config: Config,
    pool: PgPool,
    media: MediaStore,
    packages: Option<Manager>,
    public_origin: String,
    verification: bool,
    extensions: RuntimeExtensions,
) -> Result<(), Box<dyn std::error::Error>> {
    let RuntimeExtensions {} = extensions;
    talocart_web::validate_catalogues().map_err(str::to_owned)?;
    if let Some(manager) = &packages {
        for item in manager.inventory()?.iter().filter(|p| !p.available()) {
            if item.damaged() {
                tracing::warn!(
                    package = %item.manifest.id,
                    version = %item.manifest.version,
                    "Package cannot run: executable missing or modified"
                );
                continue;
            }
            tracing::warn!(
                package = %item.manifest.id,
                version = %item.manifest.version,
                sdk = %item.manifest.sdk_version,
                incomplete = item.incomplete(),
                "Package cannot run: update required"
            );
        }
    }
    if !verification && let Some(manager) = &packages {
        crate::schema::check_schema(&pool).await?;
        manager.register_payment_methods(&pool).await?;
    }
    let online = if verification {
        None
    } else {
        packages.as_ref().map(|manager| {
            talocart_commerce::orders::OnlinePayments::new(
                pool.clone(),
                std::sync::Arc::new(talocart_platform::packages::payments::NativeGateway::new(
                    manager.clone(),
                )),
                true,
            )
        })
    };
    preflight::database(
        &pool,
        (!verification)
            .then(|| {
                packages
                    .as_ref()
                    .map(|m| m.payments())
                    .transpose()
                    .map(|items| {
                        items
                            .unwrap_or_default()
                            .into_iter()
                            .map(|p| p.manifest.id)
                            .collect()
                    })
            })
            .transpose()?,
    )
    .await?;
    if !verification && let Some(manager) = &packages {
        for item in manager.payments()? {
            manager.endpoint(&item.manifest.id).await?;
        }
    }
    let listener = tokio::net::TcpListener::bind(config.server.listen).await?;
    let address = listener.local_addr()?;
    let payment_ui = if verification {
        None
    } else {
        packages.as_ref().map(|manager| {
            std::sync::Arc::new(talocart_web::payment_extensions::NativePresentation::new(
                manager.clone(),
            )) as std::sync::Arc<dyn talocart_web::payment_extensions::PaymentUi>
        })
    };
    let mail_key = if verification {
        None
    } else {
        preflight::mail_key(&pool, &config.media.root, preflight::MailPolicy::Prepare).await?
    };
    let state = talocart_web::AppState::configured(
        pool.clone(),
        config.store.name.clone(),
        talocart_web::AppStateConfig {
            media: media.clone(),
            mail_key: mail_key.clone(),
            payment_ui,
            online: online.clone(),
            packages: packages.clone(),
            package_install: config.packages.install,
            public_origin,
            secure_cookies: config.server.secure_cookies,
        },
    );
    let state = state.with_error_logs(talocart_platform::error_logs::ErrorLogs::new(
        config.logging.root.clone(),
    ));
    let state = state.with_updates(talocart_platform::updates::Updates::new(
        config.updates.root.clone(),
    ));
    modules::validate(&state.module_inventory())?;
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let app = talocart_web::router(state.with_runtime_health(ready.clone()));
    let app = if verification {
        app.layer(axum::middleware::from_fn(crate::verification::read_only))
    } else {
        app
    };
    let update_control = match (&config.updates.root, &packages) {
        (Some(root), Some(manager)) if !verification => Some(
            crate::update_control::Control::bind(
                root.clone(),
                manager.clone(),
                config.source_sha256.clone(),
            )
            .await?,
        ),
        _ => None,
    };
    let mut signals = Signals::new()?;
    let mut workers = crate::workers::Workers::new();
    if let Some(control) = update_control {
        workers.spawn("update-control", false, control.run());
    }
    if !verification {
        if let Some(manager) = &packages {
            manager.enable_supervision()?;
            let manager = manager.clone();
            workers.spawn("native-packages", false, async move {
                if let Err(error) = manager.supervise().await {
                    tracing::error!(target: "talocart::packages", %error, "Package supervisor stopped");
                }
            });
        }
        let mail_service =
            talocart_platform::mail::Mail::new(pool.clone(), Identity::new(pool.clone()));
        let key = mail_key.ok_or(talocart_platform::mail::MailError::Key)?;
        let mut mail_stopping = workers.stopping();
        workers.spawn("email", true, async move {
            loop {
                if *mail_stopping.borrow() {
                    break;
                }
                let active = match mail_service.process_pending(&key).await {
                    Ok(active) => active,
                    Err(error) => {
                        tracing::warn!(%error, "Email worker deferred");
                        false
                    }
                };
                if !active {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                        _ = mail_stopping.changed() => break,
                    }
                }
            }
        });
        let online_service = online.clone();
        let mut online_stopping = workers.stopping();
        if let Some(service) = online_service {
            workers.spawn("payments", true, async move {
            loop {
                if *online_stopping.borrow() {
                    break;
                }
                let active = match service.process_one().await {
                    Ok(active) => active,
                    Err(error) => {
                        tracing::warn!(%error,"Online payment reconciliation deferred");
                        false
                    }
                };
                if !active {
                    tokio::select! {_ = tokio::time::sleep(std::time::Duration::from_secs(1))=>{},_ = online_stopping.changed()=>break}
                }
            }
        });
        }
        let cache_media = media.clone();
        let cache_pool = pool.clone();
        workers.spawn("media-cache", false, async move {
            loop {
                match cache_media.process_cache_job(&cache_pool).await {
                    Ok(true) => {}
                    Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
                    Err(error) => {
                        tracing::warn!(%error,"Image cache maintenance deferred");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });
        let maintenance_media = media.clone();
        let discovery_media = media.clone();
        let discovery_pool = pool.clone();
        let discovery_identity = Identity::new(pool.clone());
        let discovery_interval = config.media.discovery_interval_seconds;
        workers.spawn("media-discovery", false, async move {
            let mut ticks =
                tokio::time::interval(std::time::Duration::from_secs(discovery_interval));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticks.tick().await;
                if !matches!(discovery_identity.installed().await, Ok(true)) {
                    continue;
                }
                match discovery_media.discover_files(&discovery_pool).await {
                    Ok(report) if report.acquired > 0 || report.rejected > 0 => tracing::info!(
                        acquired = report.acquired,
                        existing = report.existing,
                        deferred = report.deferred,
                        rejected = report.rejected,
                        "Media library source discovery"
                    ),
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error,"Media library source discovery deferred")
                    }
                }
            }
        });
        let maintenance_pool = pool.clone();
        let carts = talocart_commerce::cart::Carts::new(pool.clone());
        workers.spawn("maintenance", false, async move {
            // interval ticks immediately: expired carts need not wait an hour after restart.
            let mut ticks = tokio::time::interval(std::time::Duration::from_secs(60));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut next_media = tokio::time::Instant::now() + std::time::Duration::from_secs(3600);
            loop {
                ticks.tick().await;
                match carts.prune().await {
                    Ok(report) => tracing::info!(
                        deleted = report.deleted,
                        remaining_at_least = report.remaining_at_least,
                        "Expired cart cleanup"
                    ),
                    Err(error) => tracing::warn!(%error,"Cart cleanup deferred"),
                }
                if tokio::time::Instant::now() >= next_media {
                    match maintenance_media.prune(&maintenance_pool, false).await {
                        Ok(report) => tracing::info!(
                            objects = report.objects,
                            files = report.files,
                            busy = report.busy,
                            "Media pruning"
                        ),
                        Err(error) => tracing::warn!(%error,"Media pruning deferred"),
                    }
                    next_media = tokio::time::Instant::now() + std::time::Duration::from_secs(3600);
                }
            }
        });
        workers.adopt("diagnostics", false, diagnostics::spawn(pool.clone()));
    }
    tracing::info!(%address, version = talocart_platform::version::DISPLAY, verification, "Talocart started in local preview");
    run_host(
        listener,
        app,
        &mut workers,
        ready,
        signals.recv(),
        packages.as_ref(),
        SHUTDOWN_GRACE,
    )
    .await
}

const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

async fn run_host(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    workers: &mut crate::workers::Workers,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stop_requested: impl std::future::Future<Output = std::io::Result<()>>,
    packages: Option<&Manager>,
    grace: std::time::Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let (stop, stopping) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = stopping.await;
    });
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);
    let mut server_finished = false;
    let failure: Option<Box<dyn std::error::Error>> = tokio::select! {
        result = &mut server => {
            server_finished = true;
            result.err().map(|error| Box::new(error) as Box<dyn std::error::Error>)
        },
        result = stop_requested => result.err().map(|error| Box::new(error) as Box<dyn std::error::Error>),
        error = workers.failed() => {
            tracing::error!(target: "talocart::workers", %error, "Worker failure: stopping host");
            Some(Box::new(error))
        },
    };
    ready.store(false, std::sync::atomic::Ordering::Release);
    if let Some(manager) = packages {
        manager.begin_shutdown();
    }
    let _ = stop.send(());
    tracing::info!("Graceful shutdown requested");
    let deadline = tokio::time::Instant::now() + grace;
    let drain_http = async {
        if server_finished {
            return Ok(());
        }
        tokio::time::timeout_at(deadline, &mut server)
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "HTTP shutdown deadline exceeded",
                )
            })?
    };
    let drain_transport = async {
        let http = drain_http.await;
        // Let active HTTP handlers finish before killing their module processes.
        let packages = async {
            if let Some(manager) = packages {
                tokio::time::timeout_at(deadline, manager.shutdown())
                    .await
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "Package shutdown deadline exceeded",
                        )
                    })?;
            }
            Ok::<_, std::io::Error>(())
        }
        .await;
        (http, packages)
    };
    let ((http, packages), background) = tokio::join!(drain_transport, workers.shutdown(deadline));
    if let Err(error) = &http {
        tracing::error!(%error, "HTTP shutdown failed");
    }
    if let Err(error) = &background {
        tracing::error!(%error, "Worker shutdown failed");
    }
    if let Err(error) = &packages {
        tracing::error!(%error, "Package shutdown failed");
    }
    if let Some(error) = failure {
        return Err(error);
    }
    http?;
    background?;
    packages?;
    Ok(())
}

/// Register signals before starting any background tasks; registration failure is recoverable.
struct Signals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}
impl Signals {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            interrupt: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
            #[cfg(unix)]
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }
    async fn recv(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            tokio::select! { _ = self.interrupt.recv() => {}, _ = self.terminate.recv() => {} }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    #[tokio::test]
    async fn worker_failure_drains_host_and_marks_readiness_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let healthy = Arc::new(AtomicBool::new(true));
        let mut workers = crate::workers::Workers::new();
        workers.spawn("synthetic", true, async {});
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            run_host(
                listener,
                axum::Router::new(),
                &mut workers,
                healthy.clone(),
                std::future::pending(),
                None,
                Duration::from_millis(50),
            ),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("synthetic"));
        assert!(!healthy.load(Ordering::Acquire));
        assert!(
            tokio::net::TcpListener::bind(address).await.is_ok(),
            "Failed host releases its listener"
        );
    }

    #[tokio::test]
    async fn normal_stop_joins_active_work_and_absent_optional_workers_are_not_failures() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let healthy = Arc::new(AtomicBool::new(true));
        let mut workers = crate::workers::Workers::new();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let mut stop = workers.stopping();
        workers.spawn("email", true, async move {
            stop.changed().await.unwrap();
            done.store(true, Ordering::Release);
        });
        run_host(
            listener,
            axum::Router::new(),
            &mut workers,
            healthy.clone(),
            async { Ok(()) },
            None,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(finished.load(Ordering::Acquire));
        assert!(!healthy.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn stalled_http_and_worker_share_the_shutdown_deadline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let incoming = started.clone();
        let app = axum::Router::new().route(
            "/slow",
            axum::routing::get(move || async move {
                incoming.notify_one();
                std::future::pending::<()>().await;
                "unreachable"
            }),
        );
        let mut workers = crate::workers::Workers::new();
        workers.spawn("stalled", true, std::future::pending());
        let healthy = Arc::new(AtomicBool::new(true));
        let host = run_host(
            listener,
            app,
            &mut workers,
            healthy.clone(),
            async {
                started.notified().await;
                Ok(())
            },
            None,
            Duration::from_millis(30),
        );
        let client = async {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket
                .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut bytes = Vec::new();
            let _ =
                tokio::time::timeout(Duration::from_millis(150), socket.read_to_end(&mut bytes))
                    .await;
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(1), async { tokio::join!(host, client) })
                .await
                .unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("shutdown deadline")
        );
        assert!(!healthy.load(Ordering::Acquire));
    }
}
