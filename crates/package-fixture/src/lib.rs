//! Synthetic installable module for package update tests. It speaks protocol v3 like a
//! real module but performs no session or permission checks: never package it for use.
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    routing::{any, post},
};
use sqlx::{
    PgPool,
    migrate::{Migration, MigrationType, Migrator},
    postgres::PgPoolOptions,
};
use std::{
    borrow::Cow,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use talocart_platform::{
    database,
    packages::{
        self,
        protocol::{self, Action},
    },
};
use tokio::io::{AsyncReadExt, BufReader};

pub const ID: &str = "fixture-module";
pub const LEDGER: &str = "_fixture_module_migrations";
pub const M1: &str =
    "CREATE TABLE fixture_module_jobs(id BIGSERIAL PRIMARY KEY, status TEXT NOT NULL)";
pub const M1_CHANGED: &str =
    "CREATE TABLE fixture_module_jobs(id BIGSERIAL PRIMARY KEY, status TEXT NOT NULL, note TEXT)";
pub const M2: &str =
    "CREATE TABLE fixture_module_notes(id BIGSERIAL PRIMARY KEY, body TEXT NOT NULL)";
pub const M3: &str =
    "ALTER TABLE fixture_module_notes ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now()";
pub const M4_FAILING: &str = "ALTER TABLE fixture_module_missing ADD COLUMN value INT";
pub const M4_FIXED: &str =
    "CREATE INDEX fixture_module_notes_created ON fixture_module_notes(created_at)";

/// One compiled release: each test binary selects a version, a migration list and
/// whether its serving process starts.
pub struct Variant {
    pub version: &'static str,
    pub migrations: &'static [(i64, &'static str)],
    pub serves: bool,
}
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn migrator(variant: &Variant) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            variant
                .migrations
                .iter()
                .map(|(version, sql)| {
                    Migration::new(
                        *version,
                        Cow::Borrowed("fixture"),
                        MigrationType::Simple,
                        sqlx::SqlStr::from_static(sql),
                        false,
                    )
                })
                .collect(),
        ),
        table_name: Cow::Borrowed(LEDGER),
        ..Migrator::DEFAULT
    }
}
pub fn manifest(variant: &Variant) -> packages::Result<packages::Manifest> {
    let sdk = packages::sdk::VERSION;
    Ok(packages::Manifest {
        format: packages::sdk::MANIFEST_FORMAT,
        protocol: protocol::VERSION,
        id: ID.into(),
        version: variant.version.into(),

        sdk_version: sdk.into(),
        database: database::contract(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        title: packages::Labels {
            en: "Fixture module".into(),
            it: "Modulo di prova".into(),
        },
        description: packages::Labels {
            en: "Synthetic module for update tests.".into(),
            it: "Modulo sintetico per i test di aggiornamento.".into(),
        },
        notices: "MIT; synthetic test fixture".into(),
        payment: None,
        menu: format!("/admin/extensions/{ID}"),
        paths: vec![format!("/admin/extensions/{ID}")],
        permissions: vec!["products.read".into()],
        ledger: LEDGER.into(),
        migrations: migrator(variant)
            .iter()
            .map(|m| packages::Migration {
                version: m.version,
                checksum: packages::hex(&m.checksum),
            })
            .collect(),
        publisher: String::new(),
        size: 0,
        sha256: String::new(),
    })
}
pub async fn main(variant: Variant) -> ExitCode {
    match run(variant).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
fn reply(variant: &Variant, port: Option<u16>, busy: bool) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&protocol::Reply {
            protocol: protocol::VERSION,
            sdk_version: packages::sdk::VERSION.into(),
            database_contract: database::CONTRACT_VERSION,
            id: ID.into(),
            version: variant.version.into(),
            port,
            busy,
        })?
    );
    Ok(())
}
async fn busy(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM fixture_module_jobs WHERE status IN ('pending','running'))",
    )
    .fetch_one(pool)
    .await?)
}
struct Serving {
    token: String,
    version: &'static str,
    quiesced: AtomicBool,
    pool: PgPool,
}
fn authenticated(state: &Serving, headers: &HeaderMap) -> bool {
    headers
        .get("x-talocart-module")
        .is_some_and(|v| v.as_bytes() == state.token.as_bytes())
}
async fn page(State(state): State<Arc<Serving>>, headers: HeaderMap) -> (StatusCode, String) {
    if !authenticated(&state, &headers) {
        return (StatusCode::FORBIDDEN, String::new());
    }
    if state.quiesced.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, String::new());
    }
    (StatusCode::OK, format!("{ID} {}", state.version))
}
async fn control(
    State(state): State<Arc<Serving>>,
    headers: HeaderMap,
    body: String,
) -> StatusCode {
    if !authenticated(&state, &headers) {
        return StatusCode::FORBIDDEN;
    }
    match body.as_str() {
        "quiesce" => match busy(&state.pool).await {
            Ok(false) => {
                state.quiesced.store(true, Ordering::SeqCst);
                StatusCode::OK
            }
            Ok(true) => StatusCode::CONFLICT,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        },
        "resume" => {
            state.quiesced.store(false, Ordering::SeqCst);
            StatusCode::OK
        }
        _ => StatusCode::BAD_REQUEST,
    }
}
async fn health(
    State(state): State<Arc<Serving>>,
    headers: HeaderMap,
    method: Method,
) -> StatusCode {
    if !authenticated(&state, &headers) {
        StatusCode::FORBIDDEN
    } else if method != Method::GET {
        StatusCode::METHOD_NOT_ALLOWED
    } else {
        StatusCode::NO_CONTENT
    }
}
async fn run(variant: Variant) -> Result<()> {
    let arg = std::env::args_os().nth(1);
    if arg.as_deref() == Some(std::ffi::OsStr::new("--describe-package")) {
        println!("{}", serde_json::to_string(&manifest(&variant)?)?);
        return Ok(());
    }
    if arg.as_deref() != Some(std::ffi::OsStr::new("--module-protocol")) {
        return Err("Unsupported fixture command".into());
    }
    let mut stdin = BufReader::new(tokio::io::stdin());
    let start: protocol::Start = serde_json::from_slice(&protocol::read_line(&mut stdin).await?)?;
    let descriptor = manifest(&variant)?;
    start.validate_for(&descriptor)?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&start.database_url)
        .await?;
    database::check_module(&pool, &start.database, &descriptor.database).await?;
    let migrator = migrator(&variant);
    match start.action {
        Action::Install => {
            migrator.run(&pool).await?;
            database::check_migrations(&pool, &migrator).await?;
            reply(&variant, None, false)
        }
        Action::CanStop => {
            database::check_migrations(&pool, &migrator).await?;
            reply(&variant, None, busy(&pool).await?)
        }
        Action::Serve => {
            database::check_migrations(&pool, &migrator).await?;
            if !variant.serves {
                return Err("Synthetic start failure".into());
            }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let state = Arc::new(Serving {
                token: start.token,
                version: variant.version,
                quiesced: AtomicBool::new(false),
                pool,
            });
            let app = Router::new()
                .route("/__talocart_package/control", post(control))
                .route(protocol::HEALTH_PATH, any(health))
                .fallback(any(page))
                .with_state(state);
            reply(&variant, Some(listener.local_addr()?.port()), false)?;
            // The host keeps stdin open as a lifeline; EOF stops this process.
            let mut sink = Vec::new();
            tokio::select! {
                result = axum::serve(listener, app) => result?,
                _ = stdin.read_to_end(&mut sink) => {}
            }
            Ok(())
        }
    }
}
