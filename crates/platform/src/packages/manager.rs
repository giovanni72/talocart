//! Package lifecycle orchestration. Model, storage, verification and processes have explicit owners.
mod maintenance;
mod model;
mod offline;
mod planning;
mod process;
mod storage;
mod supervision;
mod validation;
use super::{Error, Manifest, Result, format, protocol};
pub use maintenance::{Activation, UpdateSnapshot};
pub use model::{Changes, Installed, Preview, RuntimeConfig};
use model::{Pending, Registry, Release};
pub use planning::{ModulePlan, PlanAction, PlanBlocker, UpdatePlan};
pub use process::ModuleGuard;
use process::ModuleRuntime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, RwLock},
    time::Duration,
};
use storage::*;
pub use supervision::{RuntimeCause, RuntimeFailure, RuntimePhase, RuntimeState, RuntimeStatus};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::Mutex,
};
use validation::*;

struct Inner {
    config: RuntimeConfig,
    _lease: Option<File>,
    offline: bool,
    offline_operation: Mutex<()>,
    registry: RwLock<Registry>,
    operation: Mutex<()>,
    update_preparing: std::sync::atomic::AtomicBool,
    update_pause: Mutex<Option<maintenance::Pause>>,
    pending: Mutex<HashMap<String, Pending>>,
    runtimes: RwLock<HashMap<String, Arc<ModuleRuntime>>>,
    client: reqwest::Client,
    payment_requests: Arc<tokio::sync::Semaphore>,
    supervised: std::sync::atomic::AtomicBool,
    stopping: std::sync::atomic::AtomicBool,
}
#[derive(Clone)]
pub struct Manager(Arc<Inner>);

async fn blocking<T: Send + 'static>(
    run: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|_| Error::Storage)?
}
impl Manager {
    pub(super) fn admit_payment(
        &self,
    ) -> crate::payment_gateway::Result<tokio::sync::OwnedSemaphorePermit> {
        self.0
            .payment_requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| crate::payment_gateway::Error::Unavailable)
    }
    pub(super) fn payment_client(&self) -> crate::payment_gateway::Result<&reqwest::Client> {
        Ok(&self.0.client)
    }

    pub async fn open(config: RuntimeConfig) -> Result<Self> {
        Self::open_mode(config, false).await
    }
    /// Administrative CLI: exclusive host lease, no HTTP/native serving processes.
    pub async fn open_offline(mut config: RuntimeConfig) -> Result<Self> {
        if !config.compiled.is_empty() {
            return Err(Error::Incompatible);
        }
        config.serve = true;
        Self::open_mode(config, true).await
    }
    async fn open_mode(config: RuntimeConfig, offline: bool) -> Result<Self> {
        if !config.root.is_absolute()
            || !config.media_root.is_absolute()
            || config
                .root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || config.root.starts_with(&config.media_root)
            || config.media_root.starts_with(&config.root)
        {
            return Err(Error::Storage);
        }
        let root = config.root.clone();
        let media = config.media_root.clone();
        blocking(move || {
            let root = location(&root)?;
            let media = location(&media)?;
            if root.starts_with(&media) || media.starts_with(&root) {
                return Err(Error::Storage);
            }
            Ok(())
        })
        .await?;
        let lease = if config.serve {
            let root = config.root.clone();
            Some(
                blocking(move || {
                    directory(&root)?;
                    let path = root.join("host.lock");
                    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                        return Err(Error::Storage);
                    }
                    let mut options = OpenOptions::new();
                    options.read(true).write(true).create(true).truncate(false);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let file = options.open(path).map_err(|_| Error::Storage)?;
                    file.try_lock().map_err(|_| Error::Conflict)?;
                    Ok(file)
                })
                .await?,
            )
        } else {
            None
        };
        let root = config.root.clone();
        let registry = blocking(move || load(&root)).await?;
        if registry
            .installed
            .iter()
            .any(|item| config.compiled.contains(&item.manifest.id))
        {
            return Err(Error::Conflict);
        }
        // reqwest is built with rustls-no-provider: initialize independently of payment providers/SMTP.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            // Allow bounded upstream reads and pool acquisition to finish so packages
            // can render localized provider failures before the gateway deadline.
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| Error::Execution)?;
        Ok(Self(Arc::new(Inner {
            config,
            _lease: lease,
            offline,
            offline_operation: Mutex::new(()),
            registry: RwLock::new(registry),
            operation: Mutex::new(()),
            update_preparing: std::sync::atomic::AtomicBool::new(false),
            update_pause: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            runtimes: RwLock::new(HashMap::new()),
            client,
            payment_requests: Arc::new(tokio::sync::Semaphore::new(32)),
            supervised: std::sync::atomic::AtomicBool::new(false),
            stopping: std::sync::atomic::AtomicBool::new(false),
        })))
    }
    /// Legacy metadata API retained for older tooling. Installation never consults this list.
    pub async fn trust(&self, public_key: String) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        tokio::spawn(async move { manager.trust_inner(public_key).await })
            .await
            .map_err(|_| Error::Execution)?
    }
    async fn trust_inner(&self, public_key: String) -> Result<()> {
        format::unhex(&public_key, 32)?;
        let _op = self.0.operation.lock().await;
        self.ensure_mutable()?;
        let _disk = self.lock_disk().await?;
        let mut registry = self.registry()?;
        if !registry.publishers.contains(&public_key) {
            if registry.publishers.len() >= 100 {
                return Err(Error::Conflict);
            }
            registry.publishers.push(public_key);
            self.persist(registry).await?;
        }
        Ok(())
    }
    pub async fn preview(&self, owner: String, bytes: Vec<u8>) -> Result<Preview> {
        self.ensure_mutable()?;
        let (bytes, manifest) = blocking(move || {
            let verified = format::inspect(&bytes)?;
            Ok((bytes, verified.manifest))
        })
        .await?;
        let current = self
            .inventory()?
            .into_iter()
            .find(|i| i.manifest.id == manifest.id);
        match &current {
            Some(installed) => self.upgradable(installed, &manifest)?,
            None => self.allowed(&manifest)?,
        }
        let token = crate::identity::token().map_err(|_| Error::Storage)?;
        let mut pending = self.0.pending.lock().await;
        pending.retain(|_, p| p.expires > std::time::Instant::now() && p.owner != owner);
        if pending.len() >= 2 {
            return Err(Error::Busy);
        }
        pending.insert(
            token.clone(),
            Pending {
                owner,
                module_id: manifest.id.clone(),
                bytes,
                expires: std::time::Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(Preview {
            token,
            manifest,
            current: current.map(|i| i.manifest),
        })
    }
    async fn pending_bytes(&self, owner: &str, token: &str) -> Result<Vec<u8>> {
        let pending = self.0.pending.lock().await;
        Ok(pending
            .get(token)
            .filter(|p| p.owner == owner && p.expires > std::time::Instant::now())
            .ok_or(Error::Preview)?
            .bytes
            .clone())
    }
    pub async fn install(&self, owner: &str, token: &str, pool: &sqlx::PgPool) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        let owner = owner.to_owned();
        let token = token.to_owned();
        let pool = pool.clone();
        tokio::spawn(async move { manager.install_inner(&owner, &token, &pool).await })
            .await
            .map_err(|_| Error::Execution)?
    }
    async fn install_inner(&self, owner: &str, token: &str, pool: &sqlx::PgPool) -> Result<()> {
        let _op = self.0.operation.lock().await;
        self.ensure_mutable()?;
        let _disk = self.lock_disk().await?;
        let bytes = self.pending_bytes(owner, token).await?;
        let verified = blocking(move || format::inspect(&bytes)).await?;
        self.allowed(&verified.manifest)?;
        // Schema kept by an earlier uninstallation is accepted only as an exact prefix.
        self.ledger(pool, &verified.manifest).await?;
        let path = self
            .publish(&verified.manifest, verified.executable)
            .await?;
        verify_descriptor(&path, &verified.manifest).await?;
        self.command(&path, &verified.manifest, protocol::Action::Install)
            .await?;
        self.register_new_payment_method(pool, &verified.manifest)
            .await?;
        let mut registry = self.registry()?;
        registry.installed.push(Installed {
            critical: true,
            manifest: verified.manifest,
            enabled: false,
            raw: verified.manifest_bytes,
            signature: verified.signature,
            previous: None,
            update: None,
            damaged: false,
        });
        self.persist(registry).await?;
        self.0.pending.lock().await.remove(token);
        Ok(())
    }
    pub async fn update(&self, owner: &str, token: &str, pool: &sqlx::PgPool) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        let owner = owner.to_owned();
        let token = token.to_owned();
        let pool = pool.clone();
        tokio::spawn(async move { manager.update_inner(&owner, &token, &pool).await })
            .await
            .map_err(|_| Error::Execution)?
    }
    async fn update_inner(&self, owner: &str, token: &str, pool: &sqlx::PgPool) -> Result<()> {
        // Queue only the small, already verified ID, never a copy of the payload.
        let id = self
            .0
            .pending
            .lock()
            .await
            .get(token)
            .filter(|p| p.owner == owner && p.expires > std::time::Instant::now())
            .ok_or(Error::Preview)?
            .module_id
            .clone();
        // No module request may start between the work check and the executable swap.
        let _requests = self.module_guard(&id).await?;
        let _op = self.0.operation.lock().await;
        self.ensure_mutable()?;
        let _disk = self.lock_disk().await?;
        // Revalidate ownership/expiry after waiting, before copying and hashing.
        let bytes = self.pending_bytes(owner, token).await?;
        let verified = blocking(move || format::inspect(&bytes)).await?;
        if verified.manifest.id != id {
            return Err(Error::Signature);
        }
        let installed = self
            .inventory()?
            .into_iter()
            .find(|i| i.manifest.id == verified.manifest.id)
            .ok_or(Error::Invalid)?;
        self.upgradable(&installed, &verified.manifest)?;
        if self.ledger(pool, &verified.manifest).await?.len() < installed.manifest.migrations.len()
        {
            return Err(Error::Schema);
        }
        let path = self
            .publish(&verified.manifest, verified.executable)
            .await?;
        verify_descriptor(&path, &verified.manifest).await?;
        if installed.enabled && !installed.incomplete() {
            self.quiesce(&installed).await?;
        }
        self.stop(&installed.manifest.id).await;
        let release = Release {
            manifest: verified.manifest,
            raw: verified.manifest_bytes,
            signature: verified.signature,
        };
        self.record(&installed.manifest.id, |item| {
            item.update = Some(release.clone())
        })
        .await?;
        self.0.pending.lock().await.remove(token);
        self.complete(&installed.manifest.id, pool).await
    }
    /// Resume an interrupted update from its verified intent; no new upload is needed.
    pub async fn retry(&self, id: &str, pool: &sqlx::PgPool) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        let id = id.to_owned();
        let pool = pool.clone();
        tokio::spawn(async move {
            let _requests = manager.module_guard(&id).await?;
            let _op = manager.0.operation.lock().await;
            manager.ensure_mutable()?;
            let _disk = manager.lock_disk().await?;
            manager.complete(&id, &pool).await
        })
        .await
        .map_err(|_| Error::Execution)?
    }
    async fn record(&self, id: &str, change: impl FnOnce(&mut Installed)) -> Result<()> {
        let mut registry = self.registry()?;
        let item = registry
            .installed
            .iter_mut()
            .find(|p| p.manifest.id == id)
            .ok_or(Error::Invalid)?;
        change(item);
        self.persist(registry).await
    }
    /// Apply the intent's migrations, then switch releases. Caller holds every lock.
    async fn complete(&self, id: &str, pool: &sqlx::PgPool) -> Result<()> {
        let item = self
            .inventory()?
            .into_iter()
            .find(|p| p.manifest.id == id)
            .ok_or(Error::Invalid)?;
        let release = item.update.clone().ok_or(Error::Invalid)?;
        // An intent can outlive the base that accepted it. Never execute an
        // installer for a different SDK, protocol or host when retrying it.
        release.manifest.validate()?;
        let root = self.0.config.root.clone();
        let manifest = release.manifest.clone();
        let path = blocking(move || verify_executable(&root, &manifest)).await?;
        self.ledger(pool, &release.manifest).await?;
        if let Err(error) = self
            .command(&path, &release.manifest, protocol::Action::Install)
            .await
        {
            // SQLx applies each migration in its own transaction: nothing is rolled back.
            let applied = self.ledger(pool, &release.manifest).await;
            if applied.is_ok_and(|a| a.len() == item.manifest.migrations.len()) {
                self.record(id, |item| item.update = None).await?;
                return Err(error);
            }
            return Err(Error::Incomplete);
        }
        self.register_new_payment_method(pool, &release.manifest)
            .await?;
        let old = item.release();
        let added = release.manifest.migrations.len() > old.manifest.migrations.len();
        let (current, previous) = (
            release.manifest.version.clone(),
            old.manifest.version.clone(),
        );
        self.record(id, |item| {
            item.replace(release);
            item.previous = Some(old.clone());
            item.update = None;
        })
        .await?;
        self.prune_logged(id, &[&current, &previous]).await;
        if !item.enabled {
            return Ok(());
        }
        let started = self.ensure_running(id).await;
        if started.is_ok() {
            return Ok(());
        }
        if added {
            // The previous release requires its exact schema: it cannot be restored.
            return Err(Error::Started);
        }
        let root = self.0.config.root.clone();
        let manifest = old.manifest.clone();
        blocking(move || verify_executable(&root, &manifest)).await?;
        self.record(id, |item| {
            item.replace(old);
            item.previous = None;
        })
        .await?;
        self.prune_logged(id, &[&previous]).await;
        // The rejected release's retry budget must not delay the restored executable.
        self.stop(id).await;
        let _ = self.ensure_running(id).await;
        Err(Error::RolledBack)
    }
    /// Remove an inactive module from the registry and delete its executables only.
    /// Schema, ledger, data, settings and legacy publisher metadata are preserved.
    pub async fn uninstall(&self, id: &str) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move { manager.uninstall_inner(&id).await })
            .await
            .map_err(|_| Error::Execution)?
    }
    async fn uninstall_inner(&self, id: &str) -> Result<()> {
        let _requests = self.module_guard(id).await?;
        let _op = self.0.operation.lock().await;
        self.ensure_mutable()?;
        let _disk = self.lock_disk().await?;
        let mut registry = self.registry()?;
        let index = registry
            .installed
            .iter()
            .position(|p| p.manifest.id == id)
            .ok_or(Error::Invalid)?;
        if registry.installed[index].enabled {
            return Err(Error::Active);
        }
        self.stop(id).await;
        registry.installed.remove(index);
        self.persist(registry).await?;
        let folder = self.0.config.root.join("installed").join(id);
        let removed = blocking(move || match fs::symlink_metadata(&folder) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                fs::remove_dir_all(&folder).map_err(|_| Error::Storage)
            }
            Ok(_) => Err(Error::Storage),
            Err(_) => Ok(()),
        })
        .await;
        if removed.is_err() {
            tracing::warn!("Failed to remove uninstalled module executables");
        }
        Ok(())
    }
    /// Owner policy only: never stops a process or changes activation/data.
    pub async fn set_critical(&self, id: &str, critical: bool) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _op = manager.0.operation.lock().await;
            manager.ensure_mutable()?;
            let _disk = manager.lock_disk().await?;
            let mut registry = manager.registry()?;
            let item = registry
                .installed
                .iter_mut()
                .find(|p| p.manifest.id == id)
                .ok_or(Error::Invalid)?;
            if item.critical == critical {
                return Ok(());
            }
            item.critical = critical;
            manager.persist(registry).await
        })
        .await
        .map_err(|_| Error::Execution)?
    }
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.ensure_mutable()?;
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move { manager.set_enabled_inner(&id, enabled).await })
            .await
            .map_err(|_| Error::Execution)?
    }
    async fn set_enabled_inner(&self, id: &str, enabled: bool) -> Result<()> {
        let _requests = self.module_guard(id).await?;
        let _op = self.0.operation.lock().await;
        self.ensure_mutable()?;
        let _disk = self.lock_disk().await?;
        let mut registry = self.registry()?;
        let item = registry
            .installed
            .iter_mut()
            .find(|p| p.manifest.id == id)
            .ok_or(Error::Invalid)?;
        if item.enabled == enabled {
            return Ok(());
        }
        if enabled && item.incomplete() {
            return Err(Error::Incomplete);
        }
        if enabled && !item.compatible() {
            return Err(Error::Incompatible);
        }
        if enabled && item.damaged() {
            return Err(Error::Signature);
        }
        // An incomplete update already quiesced the module and nothing has served it since.
        if !enabled && !item.incomplete() {
            self.quiesce(item).await?;
        }
        // Stop any process before changing its persisted activation. Pending work was checked above.
        self.stop(id).await;
        item.enabled = enabled;
        self.persist(registry).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_payment_admission_is_bounded_and_released_after_requests() {
        let options = config();
        let root = options.root.clone();
        let manager = Manager::open(options).await.unwrap();
        let mut permits = Vec::new();
        for _ in 0..32 {
            permits.push(manager.admit_payment().unwrap());
        }
        assert!(matches!(
            manager.admit_payment(),
            Err(crate::payment_gateway::Error::Unavailable)
        ));
        permits.pop();
        let released = manager.admit_payment().unwrap();
        drop(released);
        drop(permits);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    #[ignore = "Requires dedicated test PostgreSQL"]
    async fn payment_registration_preserves_existing_owner_before_offline_install() {
        use crate::packages::payments::{Descriptor, VERSION};
        use std::str::FromStr;
        let url = crate::config::environment("TALOCART_TEST_DATABASE_URL")
            .unwrap()
            .unwrap();
        let owner = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("talocart_payment_registration_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&owner)
            .await
            .unwrap();
        let options = sqlx::postgres::PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let test_pool = pool.clone();
        let result = tokio::spawn(async move {
            let previous = sqlx::migrate::Migrator {
                migrations: std::borrow::Cow::Owned(crate::database::MIGRATOR.iter().filter(|m| m.version < 4).cloned().collect()),
                ..sqlx::migrate::Migrator::DEFAULT
            };
            previous.run(&test_pool).await.unwrap();
            // A retired provider's completed attempts must not claim the current method.
            sqlx::query("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents) VALUES('ORD-RETIRED',987654,0,$1,'status_id = 1','EUR',100,0,0,100,0,100)").bind("a".repeat(64)).execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO payment_attempts(provider_id,order_id,request_key,amount_cents,currency,title,return_url,locale,state,action) SELECT 'retired',id,$1,100,'EUR','Legacy','http://127.0.0.1/','en','paid','none' FROM orders WHERE reference='ORD-RETIRED'").bind("b".repeat(64)).execute(&test_pool).await.unwrap();
            crate::database::migrate(&test_pool).await.unwrap();
            sqlx::query(
                "UPDATE payment_methods SET name_en='My original card',active=true WHERE id=3",
            )
            .execute(&test_pool)
            .await
            .unwrap();
            let options = config();
            let root = options.root.clone();
            let manager = Manager::open(options).await.unwrap();
            let mut first = format::fixture_manifest();
            first.payment = Some(Descriptor {
                amount_limits: None,
                protocol: VERSION,
                retry_window_seconds: 3600,
                webhook_path: "/payments/fixture/webhook".into(),
                webhook_headers: vec!["fixture-signature".into()],
                signature_header: None,
                prefixes: None,
                checkout_host: "first.example.test".into(),
                receipt_policy: "default-src 'none'".into(),
            });
            manager
                .0
                .registry
                .write()
                .unwrap()
                .installed
                .push(Installed {
                    critical: true,
                    manifest: first.clone(),
                    enabled: false,
                    raw: vec![],
                    signature: vec![],
                    previous: None,
                    update: None,
                    damaged: false,
                });
            let mut second = first;
            second.id = "second".into();
            second.title.en = "Second".into();
            second.title.it = "à".repeat(121);
            second.ledger = "_second_migrations".into();
            second.paths = vec!["/admin/extensions/second".into()];
            second.menu = "/admin/extensions/second".into();
            second.payment.as_mut().unwrap().webhook_path = "/payments/second/webhook".into();
            second.validate().unwrap();
            manager
                .register_new_payment_method(&test_pool, &second)
                .await
                .unwrap();
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT provider_id FROM payment_attempts").fetch_one(&test_pool).await.unwrap(), "retired");
            let original: (String, String, bool) =
                sqlx::query_as("SELECT provider_id,name_en,active FROM payment_methods WHERE id=3")
                    .fetch_one(&test_pool)
                    .await
                    .unwrap();
            assert_eq!(
                original,
                ("fixture".into(), "My original card".into(), true)
            );
            let new: (String, bool) = sqlx::query_as(
                "SELECT name_en,active FROM payment_methods WHERE provider_id='second'",
            )
            .fetch_one(&test_pool)
            .await
            .unwrap();
            assert_eq!(new, ("Second".into(), false));
            assert_eq!(
                sqlx::query_scalar::<_, i32>(
                    "SELECT char_length(name_it) FROM payment_methods WHERE provider_id='second'"
                )
                .fetch_one(&test_pool)
                .await
                .unwrap(),
                120
            );
            let revision: i64 =
                sqlx::query_scalar("SELECT revision FROM payment_configuration WHERE id=1")
                    .fetch_one(&test_pool)
                    .await
                    .unwrap();
            manager
                .register_new_payment_method(&test_pool, &second)
                .await
                .unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT revision FROM payment_configuration WHERE id=1"
                )
                .fetch_one(&test_pool)
                .await
                .unwrap(),
                revision
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_methods")
                    .fetch_one(&test_pool)
                    .await
                    .unwrap(),
                4
            );
            drop(manager);
            fs::remove_dir_all(root).unwrap();
        })
        .await;
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&owner)
            .await
            .unwrap();
        result.unwrap();
    }

    #[tokio::test]
    async fn multiple_native_providers_require_explicit_identity_and_keep_routes_separate() {
        use crate::packages::payments::{Descriptor, NativeGateway, VERSION};
        use crate::payment_gateway::Gateway;
        let options = config();
        let root = options.root.clone();
        let manager = Manager::open(options).await.unwrap();
        for id in ["first", "second"] {
            let mut manifest = format::fixture_manifest();
            manifest.id = id.into();
            manifest.ledger = format!("_{id}_migrations");
            manifest.paths = vec![format!("/admin/extensions/{id}")];
            manifest.menu = format!("/admin/extensions/{id}");
            manifest.payment = Some(Descriptor {
                amount_limits: None,
                protocol: VERSION,
                retry_window_seconds: 3600,
                webhook_path: format!("/payments/{id}/webhook"),
                webhook_headers: vec![format!("{id}-signature")],
                signature_header: None,
                prefixes: None,
                checkout_host: format!("{id}.example.test"),
                receipt_policy: "default-src 'none'".into(),
            });
            if let Some(first) = manager.inventory().unwrap().first() {
                assert!(!conflict(&first.manifest, &manifest));
            }
            manager
                .0
                .registry
                .write()
                .unwrap()
                .installed
                .push(Installed {
                    critical: true,
                    manifest,
                    enabled: true,
                    raw: vec![],
                    signature: vec![],
                    previous: None,
                    update: None,
                    damaged: false,
                });
        }
        assert!(
            manager.payment().unwrap().is_none(),
            "Ambiguous calls must never select the first provider"
        );
        let gateway = NativeGateway::new(manager.clone());
        assert_eq!(gateway.providers(), vec!["first", "second"]);
        let second = gateway.select("second").unwrap();
        assert_eq!(second.provider_id(), "second");
        assert!(second.webhook_headers("/payments/first/webhook").is_none());
        assert!(second.webhook_headers("/payments/second/webhook").is_some());
        assert!(second.checkout_url("https://second.example.test/pay"));
        assert!(!second.checkout_url("https://first.example.test/pay"));
        assert!(!gateway.select("missing").unwrap().available());
        drop(second);
        drop(gateway);
        drop(manager);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_payment_rpc_preserves_requests_errors_versions_and_lifecycle_gate() {
        use crate::packages::payments::{self, Envelope, NativeGateway, Request};
        use crate::payment_gateway::{Create, Gateway};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let options = config();
        let root = options.root.clone();
        let manager = Manager::open(options).await.unwrap();
        let mut manifest = format::fixture_manifest();
        manifest.payment = Some(payments::Descriptor {
            amount_limits: None,
            protocol: payments::VERSION,
            retry_window_seconds: 23 * 3600,
            webhook_path: "/payments/fixture/webhook".into(),
            webhook_headers: vec!["fixture-signature".into()],
            signature_header: None,
            prefixes: None,
            checkout_host: "checkout.example.test".into(),
            receipt_policy: "default-src 'none'".into(),
        });
        manager
            .0
            .registry
            .write()
            .unwrap()
            .installed
            .push(Installed {
                critical: true,
                manifest,
                enabled: true,
                raw: vec![],
                signature: vec![],
                previous: None,
                update: None,
                damaged: false,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let runtime = manager.runtime("fixture").unwrap();
        *runtime.process.lock().await = Some(process::Process {
            child,
            _stdin: stdin,
            port: listener.local_addr().unwrap().port(),
            token: "a".repeat(64),
        });
        let worker = tokio::spawn(async move {
            for index in 0..10 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                let (headers, body) = loop {
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..end].to_vec())
                            .unwrap()
                            .to_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break (headers, bytes[end + 4..end + 4 + length].to_vec());
                        }
                    }
                };
                assert!(headers.starts_with("post /__talocart_package/payments "));
                assert!(headers.contains(&format!("x-talocart-module: {}", "a".repeat(64))));
                let request: Envelope<Request> = serde_json::from_slice(&body).unwrap();
                assert_eq!(request.protocol, payments::VERSION);
                let value = match request.value {
                    Request::Status => serde_json::json!(true),
                    Request::Create { input } => {
                        assert_eq!(input.amount, 123);
                        assert_eq!(input.key, "b".repeat(64));
                        serde_json::json!({"id":"cs_fixture","ui_mode":"hosted","state":"open","amount":123,"currency":"eur","reference":"ORD-test","request_key":"key"})
                    }
                    Request::Session { id } => {
                        assert_eq!(id, "cs_fixture");
                        serde_json::json!({"id":id,"ui_mode":"hosted","state":"open","amount":123,"currency":"eur","reference":"ORD-test","request_key":"key"})
                    }
                    Request::Refund {
                        payment_id,
                        amount,
                        key,
                    } => {
                        assert_eq!(payment_id, "pi_fixture");
                        assert_eq!(amount, 123);
                        assert_eq!(key, "refund-key");
                        serde_json::json!({"id":"re_fixture","payment_id":payment_id,"amount":amount,"currency":"eur","state":"succeeded"})
                    }
                    Request::Refunds { payment_id } => {
                        assert_eq!(payment_id, "pi_fixture");
                        serde_json::json!({"data":[],"has_more":false})
                    }
                    Request::RefundStatus { id } => {
                        assert_eq!(id, "re_fixture");
                        serde_json::json!({"id":id,"payment_id":"pi_fixture","amount":123,"currency":"eur","state":"succeeded"})
                    }
                    Request::Event { body, headers } => {
                        assert_eq!(body, b"signed-body");
                        assert_eq!(headers.get("fixture-signature").unwrap(), "signature");
                        serde_json::json!({"id":"evt_fixture","kind":"test","request_key":"","object_id":"","payment_id":"","payload_hash":"hash"})
                    }
                    Request::Asset { .. } => unreachable!(),
                };
                let reply = match index {
                    7 => serde_json::json!({"protocol":payments::VERSION,"value":{"Err":"Unavailable"}}).to_string(),
                    8 => serde_json::json!({"protocol":99,"value":{"Ok":true}}).to_string(),
                    _ => serde_json::json!({"protocol":payments::VERSION,"value":{"Ok":value}}).to_string(),
                };
                let length = if index == 9 {
                    payments::LIMIT + 1
                } else {
                    reply.len()
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{reply}").as_bytes()).await.unwrap();
            }
        });
        let gateway = NativeGateway::new(manager.clone());
        assert!(gateway.accepting().await.unwrap());
        // Served by the presentation cache: the worker would otherwise see one more request.
        assert!(gateway.accepting().await.unwrap());
        let input = Create {
            email: "synthetic@example.test".into(),
            ui_mode: "elements".into(),
            key: "b".repeat(64),
            reference: "ORD-TEST".into(),
            title: "Test".into(),
            currency: "EUR".into(),
            amount: 123,
            return_url: "http://localhost/".into(),
            locale: "en".into(),
        };
        assert_eq!(gateway.create(&input).await.unwrap().id, "cs_fixture");
        assert_eq!(
            gateway.session("cs_fixture").await.unwrap().id,
            "cs_fixture"
        );
        assert_eq!(
            gateway
                .refund("pi_fixture", 123, "refund-key")
                .await
                .unwrap()
                .amount,
            123
        );
        assert!(!gateway.refunds("pi_fixture").await.unwrap().has_more);
        assert_eq!(
            gateway.refund_status("re_fixture").await.unwrap().id,
            "re_fixture"
        );
        assert_eq!(
            gateway
                .authenticated_headers(
                    b"signed-body",
                    &std::collections::BTreeMap::from([(
                        "fixture-signature".into(),
                        "signature".into()
                    )])
                )
                .await
                .unwrap()
                .id,
            "evt_fixture"
        );
        // Failures are never cached, so one invalidation reaches all three replies.
        manager.forget_payment_status("fixture");
        assert!(matches!(
            gateway.accepting().await,
            Err(crate::payment_gateway::Error::Unavailable)
        ));
        assert!(matches!(
            gateway.accepting().await,
            Err(crate::payment_gateway::Error::Invalid)
        ));
        assert!(matches!(
            gateway.accepting().await,
            Err(crate::payment_gateway::Error::Invalid)
        ));
        worker.await.unwrap();
        let guard = gateway.lease().await.unwrap();
        // Reads and other payment calls remain concurrent while SQL retains the lease.
        let read = manager.request_guard("fixture").await.unwrap();
        drop(read);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), manager.module_guard("fixture"))
                .await
                .is_err()
        );
        {
            let mutation = manager.mutation_guard("fixture");
            tokio::pin!(mutation);
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut mutation)
                    .await
                    .is_err(),
                "Settings cannot change until the host payment transaction commits"
            );
            drop(guard);
            drop(
                tokio::time::timeout(Duration::from_secs(1), mutation)
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        manager.module_guard("fixture").await.unwrap();
        let pinned = gateway.pinned().unwrap();
        manager.0.registry.write().unwrap().installed[0].manifest.id = "replacement".into();
        assert_eq!(pinned.provider_id(), "fixture");
        assert_eq!(gateway.provider_id(), "replacement");
        assert!(!pinned.available());
        assert!(matches!(
            pinned.session("old-provider-id").await,
            Err(crate::payment_gateway::Error::Unavailable)
        ));
        manager.0.registry.write().unwrap().installed[0].manifest.id = "fixture".into();
        manager.0.registry.write().unwrap().installed[0].enabled = false;
        assert!(!gateway.available());
        assert!(!gateway.accepting().await.unwrap());
        manager.shutdown().await;
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn queued_updates_defer_payload_verification_and_recheck_preview_expiry() {
        use ring::signature::KeyPair;
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config.clone()).await.unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .unwrap();
        let key = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
        manager
            .trust(format::hex(pair.public_key().as_ref()))
            .await
            .unwrap();
        for module_busy in [true, false] {
            let bytes = format::sign(
                format::fixture_manifest(),
                &format::fixture_payload(),
                key.as_ref(),
            )
            .unwrap();
            let preview = manager.preview("owner".into(), bytes).await.unwrap();
            let module = if module_busy {
                Some(manager.module_guard(&preview.manifest.id).await.unwrap())
            } else {
                None
            };
            let operation = if module_busy {
                None
            } else {
                Some(manager.0.operation.lock().await)
            };
            // A sentinel detects premature verification without allocating a large package.
            // Production previews are immutable after their validated ID has been validated.
            *manager
                .0
                .pending
                .lock()
                .await
                .get_mut(&preview.token)
                .unwrap()
                .bytes
                .last_mut()
                .unwrap() ^= 1;
            let confirmations = async {
                tokio::join!(
                    manager.update_inner("owner", &preview.token, &pool),
                    manager.update_inner("owner", &preview.token, &pool)
                )
            };
            tokio::pin!(confirmations);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut confirmations)
                    .await
                    .is_err(),
                "Queued confirmations must not verify or copy payloads before both locks"
            );
            manager
                .0
                .pending
                .lock()
                .await
                .get_mut(&preview.token)
                .unwrap()
                .expires = std::time::Instant::now();
            drop(module);
            drop(operation);
            let (first, second) = confirmations.await;
            assert!(matches!(first, Err(Error::Preview)));
            assert!(matches!(second, Err(Error::Preview)));
        }
        drop(manager);
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn unrelated_modules_do_not_wait_for_a_slow_request_or_lifecycle() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let first = manager.module_guard("first").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), manager.module_guard("second"))
                .await
                .is_ok(),
            "A slow module must not block an unrelated module"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), manager.module_guard("first"))
                .await
                .is_err(),
            "Lifecycle operations on the same module remain exclusive"
        );
        drop(first);
        manager.module_guard("first").await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn requests_share_the_gate_and_transitions_wait_for_them() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let first = manager.request_guard("first").await.unwrap();
        let second =
            tokio::time::timeout(Duration::from_millis(50), manager.request_guard("first"))
                .await
                .expect("Ordinary requests to one module run concurrently")
                .unwrap();
        let transition = manager.module_guard("first");
        tokio::pin!(transition);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut transition)
                .await
                .is_err(),
            "A transition waits for in-flight requests"
        );
        drop((first, second));
        let exclusive = tokio::time::timeout(Duration::from_secs(1), transition)
            .await
            .expect("A transition proceeds once requests drain")
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), manager.request_guard("first"))
                .await
                .is_err(),
            "A request waits behind a transition"
        );
        let other =
            tokio::time::timeout(Duration::from_millis(50), manager.request_guard("second"))
                .await
                .expect("Another module is unaffected")
                .unwrap();
        drop((exclusive, other));
        manager.request_guard("first").await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn requested_restart_stops_only_the_answering_generation() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let mut item = installed(format::fixture_manifest());
        item.enabled = true;
        manager.0.registry.write().unwrap().installed.push(item);
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let runtime = manager.runtime("fixture").unwrap();
        let token = "a".repeat(64);
        *runtime.process.lock().await = Some(process::Process {
            child,
            _stdin: stdin,
            port: 40000,
            token: token.clone(),
        });
        manager.stop_generation("fixture", 40001, &token).await;
        manager
            .stop_generation("fixture", 40000, &"b".repeat(64))
            .await;
        assert!(
            runtime.process.lock().await.is_some(),
            "A replaced generation is never stopped by an old response"
        );
        manager.stop_generation("fixture", 40000, &token).await;
        assert!(runtime.process.lock().await.is_none());
        manager.shutdown().await;
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn slow_process_startup_does_not_hold_other_modules_processes() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        for id in ["first", "second"] {
            let mut manifest = format::fixture_manifest();
            manifest.id = id.into();
            let mut item = installed(manifest);
            item.enabled = true;
            manager.0.registry.write().unwrap().installed.push(item);
        }
        let runtime = manager.runtime("first").unwrap();
        let _slow_start = runtime.process.lock().await;
        let first = manager.endpoint("first");
        tokio::pin!(first);
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
                first.as_mut(),
                cx
            )))
            .await
            .is_pending()
        );
        let second = tokio::time::timeout(Duration::from_secs(1), manager.endpoint("second"))
            .await
            .expect("Other module must reach its own startup without waiting for first");
        assert!(
            matches!(second, Err(Error::Storage)),
            "Synthetic second executable is missing, rather than blocked by first"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn cancelled_waiters_and_idle_slots_do_not_split_the_module_gate() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let first = manager.module_guard("first").await.unwrap();
        for _ in 0..3 {
            assert!(
                tokio::time::timeout(Duration::from_millis(5), manager.module_guard("first"))
                    .await
                    .is_err()
            );
            manager.module_guard("second").await.unwrap();
        }
        assert_eq!(manager.0.runtimes.read().unwrap().len(), 2);
        drop(first);
        for id in 0..100 {
            manager
                .module_guard(&format!("temporary-{id}"))
                .await
                .unwrap();
        }
        assert!(
            manager.0.runtimes.read().unwrap().len() <= 2,
            "Idle historical slots are reclaimed"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_executable_write_can_be_retried_without_publishing_partial_bytes() {
        let folder = config().root;
        fs::create_dir(&folder).unwrap();
        let path = folder.join("module");
        let result = publish_executable_with(&path, |file| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("synthetic disk full"))
        });
        assert!(result.is_err());
        assert!(
            !path.exists(),
            "A failed write must never publish partial executable bytes"
        );
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
        publish_executable_with(&path, |file| file.write_all(b"complete")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete");
        assert!(publish_executable_with(&path, |file| file.write_all(b"replacement")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"complete");
        fs::remove_dir_all(folder).unwrap();
    }
    pub(super) fn config() -> RuntimeConfig {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "talocart-packages-{}",
                crate::identity::token().unwrap()
            ));
        RuntimeConfig {
            root,
            media_root: PathBuf::from("/synthetic/media"),
            database_url: "postgres://fixture:fixture@127.0.0.1:1/synthetic_test".into(),
            store_name: "Synthetic".into(),
            public_origin: "http://127.0.0.1:1".into(),
            secure_cookies: false,
            compiled: vec![],
            serve: true,
        }
    }
    #[tokio::test]
    async fn trust_preview_ownership_persistence_and_exclusive_host() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config.clone()).await.unwrap();
        // Preview ownership is checked before any database access.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/unreachable_test")
            .unwrap();
        assert!(matches!(
            Manager::open(config.clone()).await,
            Err(Error::Conflict)
        ));
        let key = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .unwrap();
        use ring::signature::KeyPair;
        let public = format::hex(
            ring::signature::Ed25519KeyPair::from_pkcs8(key.as_ref())
                .unwrap()
                .public_key()
                .as_ref(),
        );
        let bytes = format::sign(
            format::fixture_manifest(),
            &format::fixture_payload(),
            key.as_ref(),
        )
        .unwrap();
        assert!(manager.preview("owner".into(), bytes.clone()).await.is_ok());
        assert!(manager.publishers().unwrap().is_empty());
        manager.trust(public.clone()).await.unwrap();
        let preview = manager
            .preview("owner".into(), bytes.clone())
            .await
            .unwrap();
        assert!(matches!(
            manager.install("other", &preview.token, &pool).await,
            Err(Error::Preview)
        ));
        let newer = manager
            .preview("owner".into(), bytes.clone())
            .await
            .unwrap();
        assert!(matches!(
            manager.install("owner", &preview.token, &pool).await,
            Err(Error::Preview)
        ));
        manager
            .0
            .pending
            .lock()
            .await
            .get_mut(&newer.token)
            .unwrap()
            .expires = std::time::Instant::now();
        assert!(matches!(
            manager.install("owner", &newer.token, &pool).await,
            Err(Error::Preview)
        ));
        assert!(manager.inventory().unwrap().is_empty());
        drop(manager);
        let reopened = Manager::open(config.clone()).await.unwrap();
        assert_eq!(reopened.publishers().unwrap(), vec![public]);
        assert!(reopened.inventory().unwrap().is_empty());
        drop(reopened);
        let mut readonly = config;
        readonly.serve = false;
        let reader = Manager::open(readonly).await.unwrap();
        assert!(matches!(
            reader.trust("00".repeat(32)).await,
            Err(Error::Conflict)
        ));
        drop(reader);
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn optional_modules_preserve_strict_readiness_and_persist_owner_policy() {
        let options = config();
        let root = options.root.clone();
        let manager = Manager::open(options.clone()).await.unwrap();
        let mut manifest = format::fixture_manifest();
        manifest.publisher.clear();
        let mut item = installed(manifest);
        item.enabled = true;
        let legacy = serde_json::to_value(&item).unwrap();
        assert!(legacy.get("critical").is_none());
        assert!(
            serde_json::from_value::<Installed>(legacy)
                .unwrap()
                .critical
        );
        manager.0.registry.write().unwrap().installed.push(item);
        manager.enable_supervision().unwrap();
        assert!(!manager.store_ready().unwrap());
        assert!(!manager.runtime_ready().unwrap());
        let before = planning::fingerprint(&manager.registry().unwrap()).unwrap();
        manager.set_critical("fixture", false).await.unwrap();
        assert!(manager.store_ready().unwrap());
        assert!(!manager.runtime_ready().unwrap());
        assert_ne!(
            before,
            planning::fingerprint(&manager.registry().unwrap()).unwrap()
        );
        manager
            .0
            .update_preparing
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(!manager.store_ready().unwrap());
        assert!(manager.set_critical("fixture", true).await.is_err());
        manager
            .0
            .update_preparing
            .store(false, std::sync::atomic::Ordering::Release);
        drop(manager);
        let manager = Manager::open(options).await.unwrap();
        assert!(!manager.inventory().unwrap()[0].critical);
        assert!(manager.inventory().unwrap()[0].enabled);
        manager.enable_supervision().unwrap();
        assert!(manager.store_ready().unwrap());
        assert!(!manager.runtime_ready().unwrap());
        manager.set_critical("fixture", true).await.unwrap();
        assert!(!manager.store_ready().unwrap());
        assert!(manager.set_critical("missing", false).await.is_err());
        drop(manager);
        fs::remove_dir_all(root).unwrap();
    }
    fn installed(manifest: Manifest) -> Installed {
        Installed {
            critical: true,
            raw: serde_json::to_vec(&manifest).unwrap(),
            manifest,
            enabled: false,
            signature: vec![],
            previous: None,
            update: None,
            damaged: false,
        }
    }
    #[tokio::test]
    async fn updates_require_newer_version_and_additive_migrations_without_publisher_identity() {
        let config = config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let current = format::fixture_manifest();
        manager
            .0
            .registry
            .write()
            .unwrap()
            .installed
            .push(installed(current.clone()));
        let mut next = current.clone();
        next.version = "1.0.1".into();
        next.migrations.push(super::super::Migration {
            version: current.migrations.last().unwrap().version + 1,
            checksum: "cd".repeat(48),
        });
        let item = manager.inventory().unwrap().remove(0);
        manager.upgradable(&item, &next).unwrap();
        let mut other = next.clone();
        other.publisher = "99".repeat(32);
        manager.upgradable(&item, &other).unwrap();
        for version in [current.version.as_str(), "0.9.9"] {
            let mut same = next.clone();
            same.version = version.into();
            assert!(matches!(
                manager.upgradable(&item, &same),
                Err(Error::Version)
            ));
        }
        let mut changed = next.clone();
        changed.migrations[0].checksum = "ef".repeat(48);
        assert!(matches!(
            manager.upgradable(&item, &changed),
            Err(Error::Schema)
        ));
        let mut removed = next.clone();
        removed.migrations.remove(0);
        assert!(matches!(
            manager.upgradable(&item, &removed),
            Err(Error::Schema)
        ));
        let mut ledger = next.clone();
        ledger.ledger = "_other_migrations".into();
        assert!(matches!(
            manager.upgradable(&item, &ledger),
            Err(Error::Conflict)
        ));
        let preview = Preview {
            token: String::new(),
            manifest: next,
            current: Some(current),
        };
        let changes = preview.changes().unwrap();
        assert_eq!((changes.from.as_str(), changes.migrations), ("1.0.0", 1));
        drop(manager);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ledger_prefix_is_exact_on_version_and_checksum() {
        let migrations = format::fixture_manifest().migrations;
        let applied: Vec<_> = migrations
            .iter()
            .map(|m| (m.version, format::unhex(&m.checksum, 48).unwrap()))
            .collect();
        assert!(prefix(&[], &migrations).unwrap());
        assert!(prefix(&applied, &migrations).unwrap());
        assert!(prefix(&applied[..applied.len() - 1], &migrations).unwrap());
        assert!(!prefix(&applied, &migrations[..migrations.len() - 1]).unwrap());
        let mut changed = applied.clone();
        changed[0].1[0] ^= 1;
        assert!(!prefix(&changed, &migrations).unwrap());
        let mut renumbered = applied;
        renumbered[0].0 += 100;
        assert!(!prefix(&renumbered, &migrations).unwrap());
    }
    #[test]
    fn registry_written_before_updates_is_still_readable_and_states_are_derived() {
        let mut item = installed(format::fixture_manifest());
        let mut legacy = serde_json::to_value(&item).unwrap();
        assert!(legacy.get("previous").is_none() && legacy.get("update").is_none());
        legacy["enabled"] = true.into();
        item = serde_json::from_value(legacy).unwrap();
        assert!(item.available() && item.previous_version().is_none());
        item.update = Some(item.release());
        assert!(item.incomplete() && !item.available());
        assert_eq!(item.pending_version(), Some("1.0.0"));
        item.update = None;
        item.manifest.sdk_version = "0.0.1".into();
        assert!(!item.compatible() && !item.available());
        assert!(item.manifest.validate_structure().is_ok());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn paths_and_symlinks_cannot_escape_the_package_root() {
        let mut invalid = config();
        invalid.root = invalid.media_root.join("packages");
        assert!(Manager::open(invalid).await.is_err());
        let config = config();
        let parent = config.root.clone();
        fs::create_dir(&parent).unwrap();
        let target = parent.join("real");
        fs::create_dir(&target).unwrap();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut linked = config;
        linked.root = link;
        assert!(Manager::open(linked).await.is_err());
        fs::remove_dir_all(parent).unwrap();
    }
}
