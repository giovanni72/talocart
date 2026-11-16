//! Package updates against a dedicated PostgreSQL `_test` database, with real signed
//! fixture executables, isolated schemas and private temporary package roots.
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use talocart_package_fixture::{ID, LEDGER};
use talocart_platform::packages::{Error, Manager, RuntimeConfig, build, protocol};

struct Fixture {
    folder: PathBuf,
    url: String,
    pool: PgPool,
    owner: PgPool,
    schema: String,
    public: String,
    key: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        // Direct HTTP probes must also work when no earlier test opened a Manager.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let base = std::env::var("TALOCART_TEST_DATABASE_URL").expect("Dedicated test URL");
        let parsed = url::Url::parse(&base).expect("Test URL");
        assert!(
            matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
                && parsed.path().ends_with("_test"),
            "A dedicated local _test database is required"
        );
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let schema = format!(
            "talocart_package_update_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let owner = PgPool::connect(&base).await.expect("Test database");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&owner)
            .await
            .expect("Isolated schema");
        let mut scoped = parsed.clone();
        scoped
            .query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let url = scoped.to_string();
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(&url)
            .await
            .expect("Scoped pool");
        talocart_platform::database::migrate(&pool)
            .await
            .expect("Base schema");
        let folder = std::fs::canonicalize(std::env::temp_dir())
            .expect("Temporary directory")
            .join(&schema);
        std::fs::create_dir(&folder).expect("Private test folder");
        std::fs::create_dir(folder.join("media")).expect("Media folder");
        let key = folder.join("publisher.pk8");
        let public = build::keygen(&key).expect("Synthetic publisher key");
        Self {
            folder,
            url,
            pool,
            owner,
            schema,
            public,
            key,
        }
    }
    fn config(&self) -> RuntimeConfig {
        RuntimeConfig {
            root: self.folder.join("packages"),
            database_url: self.url.clone(),
            media_root: self.folder.join("media"),
            store_name: "Synthetic".into(),
            public_origin: "http://127.0.0.1:1".into(),
            secure_cookies: false,
            compiled: vec![],
            serve: true,
        }
    }
    async fn manager(&self) -> Manager {
        let manager = Manager::open(self.config()).await.expect("Package host");
        manager.trust(self.public.clone()).await.expect("Trust");
        manager
    }
    /// Sign one fixture binary with the trusted key (or another one).
    async fn package(&self, binary: &str, key: &Path) -> Vec<u8> {
        // `binary` is absolute: joining it whole would write beside target/debug.
        let name = Path::new(binary)
            .file_name()
            .expect("Fixture binary name")
            .to_string_lossy();
        let output = self.folder.join(format!(
            "{name}-{}.talopkg",
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        build::package(Path::new(binary), key, &output)
            .await
            .expect("Package build");
        std::fs::read(output).expect("Package bytes")
    }
    async fn ledger(&self) -> Vec<i64> {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT version FROM {LEDGER} ORDER BY version"
        )))
        .fetch_all(&self.pool)
        .await
        .expect("Module ledger")
    }
    async fn finish(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.owner)
        .await
        .expect("Drop isolated schema");
        self.owner.close().await;
        std::fs::remove_dir_all(&self.folder).expect("Remove private folder");
    }
}
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
const V1: &str = env!("CARGO_BIN_EXE_fixture-v1");
const V1_BROKEN: &str = env!("CARGO_BIN_EXE_fixture-v1-1-broken");
const V2: &str = env!("CARGO_BIN_EXE_fixture-v2");
const V2_BROKEN: &str = env!("CARGO_BIN_EXE_fixture-v2-broken");
const V2_CHANGED: &str = env!("CARGO_BIN_EXE_fixture-v2-changed");
const V3_FAILING: &str = env!("CARGO_BIN_EXE_fixture-v3-failing");
const V3_FIXED: &str = env!("CARGO_BIN_EXE_fixture-v3-fixed");

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn keyless_install_checks_the_real_descriptor_before_schema_changes_and_survives_restart() {
    let f = Fixture::new().await;
    let manager = Manager::open(f.config()).await.unwrap();
    let output = f.folder.join("keyless.talopkg");
    build::bundle(Path::new(V1), &output).await.unwrap();
    let bytes = std::fs::read(output).unwrap();
    let mut package = talocart_platform::packages::inspect(&bytes).unwrap();
    assert!(package.manifest.publisher.is_empty());
    package.manifest.title.en = "Not the executable's descriptor".into();
    let changed = talocart_platform::packages::pack(package.manifest, &package.executable).unwrap();
    let preview = manager.preview("owner".into(), changed).await.unwrap();
    assert!(matches!(
        manager.install("owner", &preview.token, &f.pool).await,
        Err(Error::Incompatible)
    ));
    let ledger: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(LEDGER)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        ledger.is_none(),
        "Descriptor rejection precedes private migrations"
    );
    assert!(manager.inventory().unwrap().is_empty());
    assert!(manager.publishers().unwrap().is_empty());
    let preview = manager.preview("owner".into(), bytes).await.unwrap();
    manager
        .install("owner", &preview.token, &f.pool)
        .await
        .unwrap();
    assert_eq!(f.ledger().await, vec![1]);
    assert!(manager.publishers().unwrap().is_empty());
    manager.set_enabled(ID, true).await.unwrap();
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    manager.shutdown().await;
    drop(manager);
    let reopened = Manager::open(f.config()).await.unwrap();
    assert_eq!(serving(&reopened).await.unwrap(), format!("{ID} 1.0.0"));
    assert!(reopened.publishers().unwrap().is_empty());
    let next = f.folder.join("keyless-v2.talopkg");
    build::bundle(Path::new(V2), &next).await.unwrap();
    let preview = reopened
        .preview("owner".into(), std::fs::read(next).unwrap())
        .await
        .unwrap();
    reopened
        .update("owner", &preview.token, &f.pool)
        .await
        .unwrap();
    assert_eq!(f.ledger().await, vec![1, 2]);
    assert_eq!(serving(&reopened).await.unwrap(), format!("{ID} 1.1.0"));
    reopened.shutdown().await;
    drop(reopened);
    f.finish().await;
}

async fn install(f: &Fixture, manager: &Manager, binary: &str) {
    let preview = manager
        .preview("owner".into(), f.package(binary, &f.key).await)
        .await
        .expect("Install preview");
    assert!(preview.current.is_none());
    manager
        .install("owner", &preview.token, &f.pool)
        .await
        .expect("Installation");
}
async fn update(f: &Fixture, manager: &Manager, binary: &str) -> Result<(), Error> {
    let preview = manager
        .preview("owner".into(), f.package(binary, &f.key).await)
        .await?;
    assert!(
        preview.current.is_some(),
        "An installed ID previews as update"
    );
    manager.update("owner", &preview.token, &f.pool).await
}
async fn serving(manager: &Manager) -> Result<String, Error> {
    let (port, token) = manager.endpoint(ID).await?;
    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/admin/extensions/{ID}"))
        .header("x-talocart-module", token)
        .send()
        .await
        .map_err(|_| Error::Execution)?;
    response.text().await.map_err(|_| Error::Execution)
}
fn version(manager: &Manager) -> String {
    manager.inventory().expect("Inventory")[0]
        .manifest
        .version
        .clone()
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn update_checks_work_publisher_versions_schema_and_rolls_back_only_without_migrations() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.expect("Activate");
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    sqlx::query("INSERT INTO fixture_module_jobs(status) VALUES('pending'),('done')")
        .execute(&f.pool)
        .await
        .unwrap();
    // Pending work: refused before any executable or schema change; the module keeps serving.
    assert!(matches!(update(&f, &manager, V2).await, Err(Error::Busy)));
    assert_eq!(version(&manager), "1.0.0");
    assert_eq!(f.ledger().await, vec![1]);
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    sqlx::query("DELETE FROM fixture_module_jobs WHERE status='pending'")
        .execute(&f.pool)
        .await
        .unwrap();
    // A publisher change does not block a contract-compatible, explicitly confirmed update.
    let other = f.folder.join("other.pk8");
    manager.trust(build::keygen(&other).unwrap()).await.unwrap();
    assert!(
        manager
            .preview("owner".into(), f.package(V2, &other).await)
            .await
            .is_ok()
    );
    assert!(matches!(
        update(&f, &manager, V1).await,
        Err(Error::Version)
    ));
    assert!(matches!(
        update(&f, &manager, V2_CHANGED).await,
        Err(Error::Schema)
    ));
    // Same schema: a release that cannot start is replaced by the previous executable.
    assert!(matches!(
        update(&f, &manager, V1_BROKEN).await,
        Err(Error::RolledBack)
    ));
    assert_eq!(version(&manager), "1.0.0");
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    assert!(
        !f.folder
            .join("packages/installed")
            .join(ID)
            .join("1.0.1")
            .exists()
    );
    update(&f, &manager, V2).await.expect("Update");
    let item = &manager.inventory().unwrap()[0];
    assert_eq!(
        (item.manifest.version.as_str(), item.previous_version()),
        ("1.1.0", Some("1.0.0"))
    );
    assert!(item.enabled && item.available());
    assert_eq!(f.ledger().await, vec![1, 2]);
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.1.0"));
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM fixture_module_jobs")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(kept, 1, "Module data survive the update");
    manager
        .check_database(&f.pool)
        .await
        .expect("Consistent ledger");
    // Persisted across a host restart.
    manager.shutdown().await;
    drop(manager);
    let reopened = f.manager().await;
    assert_eq!(version(&reopened), "1.1.0");
    reopened.check_database(&f.pool).await.unwrap();
    assert_eq!(serving(&reopened).await.unwrap(), format!("{ID} 1.1.0"));
    reopened.shutdown().await;
    drop(reopened);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn start_failure_after_new_migrations_keeps_the_new_release() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.unwrap();
    assert!(matches!(
        update(&f, &manager, V2_BROKEN).await,
        Err(Error::Started)
    ));
    assert_eq!(version(&manager), "1.1.0");
    assert_eq!(f.ledger().await, vec![1, 2]);
    assert!(serving(&manager).await.is_err());
    manager.check_database(&f.pool).await.unwrap();
    manager.shutdown().await;
    drop(manager);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn failed_migration_is_incomplete_survives_restart_and_completes_with_a_fix() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.unwrap();
    assert!(matches!(
        update(&f, &manager, V3_FAILING).await,
        Err(Error::Incomplete)
    ));
    // Migrations 2 and 3 committed; 4 failed in its own transaction. Nothing is reverted.
    assert_eq!(f.ledger().await, vec![1, 2, 3]);
    let item = manager.inventory().unwrap().remove(0);
    assert!(item.incomplete() && !item.available());
    assert_eq!(item.pending_version(), Some("1.2.0"));
    assert!(matches!(serving(&manager).await, Err(Error::Incomplete)));
    manager
        .set_enabled(ID, false)
        .await
        .expect("An incomplete module can be deactivated");
    assert!(matches!(
        manager.set_enabled(ID, true).await,
        Err(Error::Incomplete)
    ));
    manager
        .check_database(&f.pool)
        .await
        .expect("Interrupted ledger accepted");
    // A restart finds the same write-ahead intent.
    drop(manager);
    let manager = f.manager().await;
    assert!(manager.inventory().unwrap()[0].incomplete());
    manager.check_database(&f.pool).await.unwrap();
    assert!(matches!(
        manager.retry(ID, &f.pool).await,
        Err(Error::Incomplete)
    ));
    assert_eq!(f.ledger().await, vec![1, 2, 3]);
    update(&f, &manager, V3_FIXED)
        .await
        .expect("Corrected package");
    let item = manager.inventory().unwrap().remove(0);
    assert_eq!(
        (item.manifest.version.as_str(), item.incomplete()),
        ("1.2.1", false)
    );
    assert_eq!(f.ledger().await, vec![1, 2, 3, 4]);
    manager.set_enabled(ID, true).await.unwrap();
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.2.1"));
    manager.shutdown().await;
    drop(manager);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn concurrent_confirmations_apply_one_update() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    let bytes = f.package(V2, &f.key).await;
    let a = manager.preview("a".into(), bytes.clone()).await.unwrap();
    let b = manager.preview("b".into(), bytes).await.unwrap();
    let (first, second) = tokio::join!(
        manager.update("a", &a.token, &f.pool),
        manager.update("b", &b.token, &f.pool)
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .iter()
            .filter(|ok| **ok)
            .count(),
        1
    );
    assert!(matches!(
        first.and(second),
        Err(Error::Version | Error::Conflict)
    ));
    assert_eq!(version(&manager), "1.1.0");
    assert_eq!(f.ledger().await, vec![1, 2]);
    drop(manager);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn retry_after_base_upgrade_rejects_incompatible_intent_without_running_migrations() {
    use ring::signature::Ed25519KeyPair;
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    assert!(matches!(
        update(&f, &manager, V3_FAILING).await,
        Err(Error::Incomplete)
    ));
    assert_eq!(f.ledger().await, vec![1, 2, 3]);
    drop(manager);
    // Make the failed migration executable: any retry that runs it will change the ledger.
    sqlx::query("CREATE TABLE fixture_module_missing(id BIGINT)")
        .execute(&f.pool)
        .await
        .unwrap();
    let path = f.folder.join("packages/registry.json");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let pending = &mut registry["installed"][0]["update"];
    pending["manifest"]["sdk_version"] = "0.0.1".into();
    let raw = serde_json::to_vec(&pending["manifest"]).unwrap();
    let pair = Ed25519KeyPair::from_pkcs8(&std::fs::read(&f.key).unwrap()).unwrap();
    pending["signature"] = pair
        .sign(&[b"talocart-package-v1\0".as_slice(), &raw].concat())
        .as_ref()
        .to_vec()
        .into();
    pending["raw"] = raw.into();
    let before = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&path, &before).unwrap();
    let manager = f.manager().await;
    manager.check_database(&f.pool).await.unwrap();
    let result = manager.retry(ID, &f.pool).await;
    let after = std::fs::read(&path).unwrap();
    let ledger = f.ledger().await;
    drop(manager);
    f.finish().await;
    assert!(
        matches!(result, Err(Error::Incompatible)),
        "Retry must reject an old SDK before executing its installer: {result:?}"
    );
    assert_eq!(ledger, vec![1, 2, 3], "No migration may run");
    assert_eq!(
        after, before,
        "The signed pending intent must remain available for a compatible corrective package"
    );
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn package_for_another_sdk_does_not_block_startup_and_can_be_updated() {
    use ring::signature::Ed25519KeyPair;
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.unwrap();
    manager.shutdown().await;
    drop(manager);
    // Re-sign the installed manifest as if it were built for an earlier base release.
    let path = f.folder.join("packages/registry.json");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let item = &mut registry["installed"][0];
    item["manifest"]["sdk_version"] = "0.0.1".into();
    let raw = serde_json::to_vec(&item["manifest"]).unwrap();
    let pair = Ed25519KeyPair::from_pkcs8(&std::fs::read(&f.key).unwrap()).unwrap();
    let signature = pair.sign(&[b"talocart-package-v1\0".as_slice(), &raw].concat());
    item["raw"] = raw.into();
    item["signature"] = signature.as_ref().to_vec().into();
    std::fs::write(&path, serde_json::to_vec(&registry).unwrap()).unwrap();
    let manager = f.manager().await;
    let item = manager.inventory().unwrap().remove(0);
    assert!(item.enabled && !item.compatible());
    assert!(matches!(serving(&manager).await, Err(Error::Incompatible)));
    manager.check_database(&f.pool).await.unwrap();
    update(&f, &manager, V2)
        .await
        .expect("Update from incompatible");
    assert!(manager.inventory().unwrap()[0].available());
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.1.0"));
    manager.shutdown().await;
    drop(manager);
    // Integrity failures stay fatal: a tampered signature blocks startup.
    let mut registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    registry["installed"][0]["manifest"]["sdk_version"] = "0.0.2".into();
    std::fs::write(&path, serde_json::to_vec(&registry).unwrap()).unwrap();
    assert!(matches!(
        Manager::open(f.config()).await,
        Err(Error::Signature)
    ));
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn uninstall_requires_inactive_module_and_preserves_schema_data_and_settings() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    update(&f, &manager, V2).await.unwrap();
    manager.set_enabled(ID, true).await.unwrap();
    sqlx::query("INSERT INTO fixture_module_notes(body) VALUES('kept')")
        .execute(&f.pool)
        .await
        .unwrap();
    let settings = f
        .folder
        .join("packages/data")
        .join(ID)
        .join("settings.toml");
    std::fs::write(&settings, "kept = true\n").unwrap();
    assert!(matches!(manager.uninstall(ID).await, Err(Error::Active)));
    manager.set_enabled(ID, false).await.unwrap();
    manager.uninstall(ID).await.expect("Uninstall");
    assert!(manager.inventory().unwrap().is_empty());
    assert!(!f.folder.join("packages/installed").join(ID).exists());
    assert!(settings.exists());
    assert_eq!(f.ledger().await, vec![1, 2]);
    let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM fixture_module_notes")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(notes, 1);
    assert!(matches!(manager.uninstall(ID).await, Err(Error::Invalid)));
    // Reinstallation reuses the kept schema; an older schema list is a downgrade.
    let older = manager
        .preview("owner".into(), f.package(V1, &f.key).await)
        .await
        .unwrap();
    assert!(matches!(
        manager.install("owner", &older.token, &f.pool).await,
        Err(Error::Schema)
    ));
    install(&f, &manager, V2).await;
    assert_eq!(version(&manager), "1.1.0");
    manager.check_database(&f.pool).await.unwrap();
    drop(manager);
    let reopened = f.manager().await;
    assert_eq!(reopened.publishers().unwrap().len(), 1);
    drop(reopened);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn unchanged_native_module_survives_a_compatible_host_sdk_and_database_upgrade() {
    use sqlx::migrate::{Migration, MigrationType, Migrator};
    use std::{borrow::Cow, process::Stdio, time::Duration};
    use talocart_platform::{
        database,
        packages::{self, protocol},
    };
    use tokio::io::{AsyncWriteExt, BufReader};
    let f = Fixture::new().await;
    let binary_before = std::fs::read(V1).unwrap();
    let mut migrations = database::MIGRATOR.iter().cloned().collect::<Vec<_>>();
    let next = migrations.last().unwrap().version + 1;
    migrations.push(Migration::new(
        next,
        Cow::Borrowed("compatible future host"),
        MigrationType::Simple,
        sqlx::SqlStr::from_static("CREATE TABLE fixture_future_host(id BIGINT PRIMARY KEY)"),
        false,
    ));
    let host = Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    };
    host.run(&f.pool).await.unwrap();
    let token = "ab".repeat(32);
    for action in [protocol::Action::Install, protocol::Action::Serve] {
        let start = protocol::Start {
            protocol: protocol::VERSION,
            sdk_version: {
                let [major, minor, _] = packages::version(packages::sdk::VERSION).unwrap();
                format!("{major}.{}.0", minor + 1)
            },
            application: talocart_platform::version::ApplicationIdentity::current(),
            database: database::contract_for(&host),
            action,
            database_url: f.url.clone(),
            media_root: f.folder.join("media"),
            data_root: f.folder.clone(),
            store_name: "Synthetic future host".into(),
            public_origin: "http://127.0.0.1:1".into(),
            secure_cookies: false,
            token: token.clone(),
        };
        let mut child = tokio::process::Command::new(V1)
            .arg("--module-protocol")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut raw = serde_json::to_vec(&start).unwrap();
        raw.push(b'\n');
        stdin.write_all(&raw).await.unwrap();
        let reply = protocol::read_line(&mut BufReader::new(child.stdout.take().unwrap()))
            .await
            .unwrap();
        let reply: protocol::Reply = serde_json::from_slice(&reply).unwrap();
        assert_eq!(reply.protocol, protocol::VERSION);
        assert_eq!(
            reply.sdk_version,
            packages::sdk::VERSION,
            "The module was not rebuilt for the newer host SDK"
        );
        if action == protocol::Action::Serve {
            let port = reply.port.unwrap();
            let response = reqwest::Client::new()
                .get(format!("http://127.0.0.1:{port}/admin/extensions/{ID}"))
                .header("x-talocart-module", &token)
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success());
            assert_eq!(response.text().await.unwrap(), format!("{ID} 1.0.0"));
        }
        drop(stdin);
        assert!(
            tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
    assert_eq!(f.ledger().await, vec![1]);
    assert_eq!(
        std::fs::read(V1).unwrap(),
        binary_before,
        "The exact same native executable must serve the upgraded host"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn obsolete_registry_is_rejected_without_executing_or_rewriting_it() {
    use ring::signature::Ed25519KeyPair;
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.unwrap();
    manager.shutdown().await;
    drop(manager);
    let path = f.folder.join("packages/registry.json");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let item = &mut registry["installed"][0];
    item["manifest"]["format"] = 1.into();
    item["manifest"]["protocol"] = 1.into();
    item["manifest"]["base_major"] = 9.into();
    item["manifest"]["sdk_version"] = "9.3.0".into();
    item["manifest"].as_object_mut().unwrap().remove("database");
    let raw = serde_json::to_vec(&item["manifest"]).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(&std::fs::read(&f.key).unwrap()).unwrap();
    item["signature"] = key
        .sign(&[b"talocart-package-v1\0".as_slice(), &raw].concat())
        .as_ref()
        .to_vec()
        .into();
    item["raw"] = raw.into();
    let original = serde_json::to_vec(&registry).unwrap();
    std::fs::write(&path, &original).unwrap();
    assert!(Manager::open(f.config()).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn live_update_quiesce_preserves_activation_and_refuses_pending_work() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.unwrap();
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    let (port, token) = manager.endpoint(ID).await.unwrap();
    for authentication in ["wrong", token.as_str()] {
        let probe = manager
            .client()
            .get(format!("http://127.0.0.1:{port}{}", protocol::HEALTH_PATH))
            .header("x-talocart-module", authentication)
            .send()
            .await
            .unwrap();
        assert_eq!(
            probe.status(),
            if authentication == token {
                reqwest::StatusCode::NO_CONTENT
            } else {
                reqwest::StatusCode::FORBIDDEN
            }
        );
    }
    assert_eq!(
        serving(&manager).await.unwrap(),
        format!("{ID} 1.0.0"),
        "A health probe cannot quiesce a module"
    );
    sqlx::query("INSERT INTO fixture_module_jobs(status) VALUES('pending')")
        .execute(&f.pool)
        .await
        .unwrap();
    let snapshot = manager.update_snapshot().unwrap();
    assert!(matches!(
        manager.prepare_update(snapshot.inventory_sha256).await,
        Err(Error::Busy)
    ));
    assert!(!manager.update_snapshot().unwrap().suspended);
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fixture_module_jobs WHERE status='pending'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "Quiesce must preserve pending work");
    sqlx::query("DELETE FROM fixture_module_jobs WHERE status='pending'")
        .execute(&f.pool)
        .await
        .unwrap();
    for enabled in [true, false] {
        manager.set_enabled(ID, enabled).await.unwrap();
        let before = std::fs::read(f.config().root.join("registry.json")).unwrap();
        let snapshot = manager.update_snapshot().unwrap();
        let paused = manager
            .prepare_update(snapshot.inventory_sha256.clone())
            .await
            .unwrap();
        assert!(paused.suspended);
        assert_eq!(paused.modules[0].enabled, enabled);
        assert_eq!(
            std::fs::read(f.config().root.join("registry.json")).unwrap(),
            before
        );
        assert!(matches!(manager.endpoint(ID).await, Err(Error::Busy)));
        assert!(matches!(
            manager.set_enabled(ID, !enabled).await,
            Err(Error::Busy)
        ));
        assert!(!manager.resume_update().await.unwrap().suspended);
        assert_eq!(manager.inventory().unwrap()[0].enabled, enabled);
        if enabled {
            assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
        }
    }
    manager.shutdown().await;
    drop(manager);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn maintenance_admission_does_not_break_a_module_update_already_in_flight() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    manager.set_enabled(ID, true).await.unwrap();
    assert_eq!(serving(&manager).await.unwrap(), format!("{ID} 1.0.0"));
    let snapshot = manager.update_snapshot().unwrap();
    let preview = manager
        .preview("owner".into(), f.package(V2, &f.key).await)
        .await
        .unwrap();
    let mut lock = f.pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "LOCK TABLE {LEDGER} IN ACCESS EXCLUSIVE MODE"
    )))
    .execute(&mut *lock)
    .await
    .unwrap();
    let update = tokio::spawn({
        let manager = manager.clone();
        let pool = f.pool.clone();
        async move { manager.update("owner", &preview.token, &pool).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation=$1::text::regclass AND NOT granted)",
            )
            .bind(LEDGER)
            .fetch_one(&f.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Update reached the ledger check while holding lifecycle locks");
    let maintenance = tokio::spawn({
        let manager = manager.clone();
        async move { manager.prepare_update(snapshot.inventory_sha256).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !manager.update_snapshot().unwrap().suspended {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    lock.rollback().await.unwrap();
    let updated = update.await.unwrap();
    let prepared = maintenance.await.unwrap();
    // Collect both results before asserting, so a red regression still cleans up.
    manager.resume_update().await.unwrap();
    let actual_version = version(&manager);
    manager.shutdown().await;
    drop(manager);
    f.finish().await;
    assert!(
        updated.is_ok(),
        "Existing lifecycle update must finish: {updated:?}"
    );
    assert!(
        matches!(prepared, Err(Error::Conflict)),
        "Changed inventory must invalidate maintenance"
    );
    assert_eq!(actual_version, "1.1.0");
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn offline_update_holds_host_lease_preserves_activation_and_never_serves() {
    let f = Fixture::new().await;
    let serving_host = f.manager().await;
    install(&f, &serving_host, V1).await;
    serving_host.set_enabled(ID, true).await.unwrap();
    assert!(matches!(
        Manager::open_offline(f.config()).await,
        Err(Error::Conflict)
    ));
    let live = serving_host.update_snapshot().unwrap();
    let proof = serving_host
        .prepare_update(live.inventory_sha256)
        .await
        .unwrap();
    serving_host.shutdown().await;
    drop(serving_host);
    let manager = Manager::open_offline(f.config()).await.unwrap();
    assert!(matches!(manager.endpoint(ID).await, Err(Error::Conflict)));
    let original = manager.update_snapshot().unwrap();
    assert!(matches!(
        manager
            .suspend_offline(original.inventory_sha256.clone(), None)
            .await,
        Err(Error::Conflict)
    ));
    let mut invalid = proof.clone();
    invalid.suspended = false;
    assert!(matches!(
        manager
            .suspend_offline(original.inventory_sha256.clone(), Some(invalid))
            .await,
        Err(Error::Conflict)
    ));
    let suspended = manager
        .suspend_offline(original.inventory_sha256.clone(), Some(proof))
        .await
        .unwrap();
    assert_eq!(suspended.modules[0].enabled, original.modules[0].enabled);
    assert!(!manager.inventory().unwrap()[0].enabled);
    let path = f.folder.join("replacement.talopkg");
    std::fs::write(&path, f.package(V2, &f.key).await).unwrap();
    manager.apply_offline(vec![path], &f.pool).await.unwrap();
    manager.verify_offline(&f.pool).await.unwrap();
    assert_eq!(f.ledger().await, vec![1, 2]);
    manager.restore_activation_offline(original).await.unwrap();
    assert!(manager.inventory().unwrap()[0].enabled);
    assert!(matches!(manager.endpoint(ID).await, Err(Error::Conflict)));
    drop(manager);
    let host = Manager::open(f.config()).await.unwrap();
    assert_eq!(serving(&host).await.unwrap(), format!("{ID} 1.1.0"));
    host.shutdown().await;
    drop(host);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn verification_connections_reject_writes_and_preserve_scoped_database_url() {
    let f = Fixture::new().await;
    let config = talocart_platform::config::DatabaseConfig {
        url: f.url.clone(),
        max_connections: 2,
        timeout_seconds: 3,
    };
    let readonly = talocart_platform::database::connect_read_only(&config)
        .await
        .unwrap();
    let native_url = talocart_platform::database::read_only_url(&f.url).unwrap();
    let native = PgPool::connect(&native_url).await.unwrap();
    for pool in [&readonly, &native] {
        let name: String = sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(name, f.schema);
        let error = sqlx::query("UPDATE identity_state SET installed=installed WHERE id=1")
            .execute(pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("25006")
        );
    }
    readonly.close().await;
    native.close().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn changed_offline_archive_after_planning_is_rejected_without_silent_skip() {
    let f = Fixture::new().await;
    // Hash real executable bytes, without making the synchronization window depend
    // on Cargo's debug-symbol volume. Never strip Cargo's original executables.
    let mut compact = Vec::new();
    for source in [V1, V2] {
        let copy = f.folder.join(Path::new(source).file_name().unwrap());
        std::fs::copy(source, &copy).unwrap();
        assert!(
            std::process::Command::new("strip")
                .arg(&copy)
                .status()
                .unwrap()
                .success()
        );
        compact.push(copy);
    }
    let v1 = compact[0].to_str().unwrap();
    let v2 = compact[1].to_str().unwrap();
    let host = f.manager().await;
    install(&f, &host, v1).await;
    drop(host);
    let manager = Manager::open_offline(f.config()).await.unwrap();
    let path = f.folder.join("replacement.talopkg");
    std::fs::write(&path, f.package(v2, &f.key).await).unwrap();
    let previous = f.package(v1, &f.key).await;
    let mut lock = f.pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "LOCK TABLE {LEDGER} IN ACCESS EXCLUSIVE MODE"
    )))
    .execute(&mut *lock)
    .await
    .unwrap();
    let applying = tokio::spawn({
        let manager = manager.clone();
        let path = path.clone();
        let pool = f.pool.clone();
        async move { manager.apply_offline(vec![path], &pool).await }
    });
    let waiting = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation=$1::text::regclass AND NOT granted)")
                .bind(LEDGER).fetch_one(&f.pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    if waiting.is_err() {
        let finished = applying.is_finished();
        lock.rollback().await.unwrap();
        let result = applying.await.unwrap();
        drop(manager);
        f.finish().await;
        panic!(
            "Update did not reach the locked ledger; completed before unlock: {finished}, result: {result:?}"
        );
    }
    std::fs::write(&path, previous).unwrap();
    lock.rollback().await.unwrap();
    let result = applying.await.unwrap();
    let actual = version(&manager);
    drop(manager);
    f.finish().await;
    assert!(
        matches!(result, Err(Error::Signature)),
        "Changed artifact must not be skipped: {result:?}"
    );
    assert_eq!(actual, "1.0.0");
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn offline_verification_preserves_pending_work_of_a_disabled_module() {
    let f = Fixture::new().await;
    let host = f.manager().await;
    install(&f, &host, V1).await;
    drop(host);
    sqlx::query("INSERT INTO fixture_module_jobs(status) VALUES('pending')")
        .execute(&f.pool)
        .await
        .unwrap();
    let manager = Manager::open_offline(f.config()).await.unwrap();
    let original = manager.update_snapshot().unwrap();
    assert!(!original.modules[0].enabled);
    manager.verify_offline(&f.pool).await.unwrap();
    manager.restore_activation_offline(original).await.unwrap();
    assert!(!manager.inventory().unwrap()[0].enabled);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fixture_module_jobs WHERE status='pending'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    drop(manager);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn update_database_fingerprint_binds_migrations_and_ignores_business_rows() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1).await;
    let before = manager.update_database_fingerprint(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO fixture_module_jobs(status) VALUES('pending')")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        manager.update_database_fingerprint(&f.pool).await.unwrap(),
        before
    );
    // A dirty private migration must never produce a usable attestation.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {LEDGER} SET success=false WHERE version=1"
    )))
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(matches!(
        manager.update_database_fingerprint(&f.pool).await,
        Err(Error::Schema)
    ));
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {LEDGER} SET success=true WHERE version=1"
    )))
    .execute(&f.pool)
    .await
    .unwrap();
    // Finish pending work before applying the ordinary lifecycle update.
    sqlx::query("DELETE FROM fixture_module_jobs")
        .execute(&f.pool)
        .await
        .unwrap();
    update(&f, &manager, V2).await.unwrap();
    assert_ne!(
        manager.update_database_fingerprint(&f.pool).await.unwrap(),
        before
    );
    // Name validation occurs before constructing any SQL identifier.
    assert!(
        talocart_platform::database::migration_snapshot(
            &f.pool,
            &["bad; DROP SCHEMA public".into()]
        )
        .await
        .is_err()
    );
    assert_eq!(f.ledger().await, vec![1, 2]);
    manager.shutdown().await;
    drop(manager);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn controlled_activation_warms_only_enabled_modules_and_never_hides_failures() {
    let f = Fixture::new().await;
    let manager = f.manager().await;
    install(&f, &manager, V1_BROKEN).await;
    // This fixture installs correctly but fails Serve. Disabled modules must not start.
    let disabled = manager.update_snapshot().unwrap();
    assert!(!disabled.modules[0].enabled);
    manager
        .warm_update(disabled.inventory_sha256.clone())
        .await
        .unwrap();
    assert!(matches!(
        manager.warm_update("0".repeat(64)).await,
        Err(Error::Conflict)
    ));
    drop(manager);
    // Simulate an already enabled module whose controlled restart now fails.
    let registry = f.config().root.join("registry.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
    value["installed"][0]["enabled"] = true.into();
    std::fs::write(&registry, serde_json::to_vec(&value).unwrap()).unwrap();
    let host = Manager::open(f.config()).await.unwrap();
    let enabled = host.update_snapshot().unwrap();
    assert!(host.warm_update(enabled.inventory_sha256).await.is_err());
    assert!(
        host.inventory().unwrap()[0].enabled,
        "Failed activation must not silently disable a module"
    );
    host.shutdown().await;
    drop(host);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn controlled_activation_accepts_old_unchanged_enabled_module_and_rejects_pause() {
    let f = Fixture::new().await;
    let host = f.manager().await;
    install(&f, &host, V1).await;
    host.set_enabled(ID, true).await.unwrap();
    let original = host.update_snapshot().unwrap();
    host.shutdown().await;
    drop(host);
    let host = Manager::open(f.config()).await.unwrap();
    let result = host
        .warm_update(original.inventory_sha256.clone())
        .await
        .unwrap();
    assert_eq!(result.modules, original.modules);
    assert_eq!(serving(&host).await.unwrap(), format!("{ID} 1.0.0"));
    host.prepare_update(original.inventory_sha256.clone())
        .await
        .unwrap();
    assert!(matches!(
        host.warm_update(original.inventory_sha256).await,
        Err(Error::Busy)
    ));
    host.resume_update().await.unwrap();
    host.shutdown().await;
    drop(host);
    f.finish().await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn controlled_activation_rejects_registry_changed_outside_the_live_manager() {
    let f = Fixture::new().await;
    let host = f.manager().await;
    install(&f, &host, V1).await;
    host.set_enabled(ID, true).await.unwrap();
    let original = host.update_snapshot().unwrap();
    let registry = f.config().root.join("registry.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
    value["installed"][0]["enabled"] = false.into();
    std::fs::write(&registry, serde_json::to_vec(&value).unwrap()).unwrap();
    let result = host.warm_update(original.inventory_sha256).await;
    host.shutdown().await;
    drop(host);
    f.finish().await;
    assert!(
        matches!(result, Err(Error::Conflict)),
        "A changed on-disk activation must invalidate the attempt: {result:?}"
    );
}
