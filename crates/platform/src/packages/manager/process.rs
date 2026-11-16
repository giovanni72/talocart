//! Child-process handshake, quiesce, serving and shutdown.
use super::*;

/// Longest wait behind a lifecycle transition before an ordinary request is refused.
const REQUEST_GATE_WAIT: Duration = Duration::from_secs(10);

/// One module's admission, independent of other IDs. Reads and payment calls share the gate;
/// administrative mutations and lifecycle transitions own it exclusively. The lock is fair:
/// requests arriving after a waiting transition queue behind it, so in-flight requests drain and none can start
/// between a work check and an executable swap.
pub struct ModuleGuard {
    _shared: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    _exclusive: Option<tokio::sync::OwnedRwLockWriteGuard<()>>,
    _runtime: Arc<ModuleRuntime>,
    manager: Manager,
    id: String,
}
impl ModuleGuard {
    pub(crate) fn record_request_failure(&self) {
        self.manager.record_request_failure(&self.id);
    }
    /// Use the endpoint from the same generation protected by this guard.
    pub async fn endpoint(&self) -> Result<(u16, String)> {
        self.manager.ensure_mutable()?;
        self.manager.ensure_running(&self.id).await
    }
}
pub(super) struct ModuleRuntime {
    pub(super) gate: Arc<tokio::sync::RwLock<()>>,
    pub(super) process: Mutex<Option<Process>>,
    pub(super) health: RwLock<supervision::Health>,
    /// Presentation-only provider availability, discarded with its process generation.
    pub(super) payment_status: RwLock<Option<(std::time::Instant, bool)>>,
}
impl ModuleRuntime {
    pub(super) fn forget_payment_status(&self) {
        if let Ok(mut status) = self.payment_status.write() {
            *status = None;
        }
    }
}

pub(super) struct Process {
    pub(super) child: Child,
    pub(super) _stdin: ChildStdin,
    pub(super) port: u16,
    pub(super) token: String,
}

impl Manager {
    /// Refuse while work is pending or running. A serving process drains its handlers and
    /// accepts no new work afterwards; otherwise the installed executable answers `can_stop`.
    /// Never launch an incompatible executable. With no managed process, queued
    /// rows are retained for a compatible release rather than sent to old native code.
    pub(super) async fn quiesce(&self, item: &Installed) -> Result<()> {
        let id = item.manifest.id.as_str();
        let runtime = self.runtime(id)?;
        let mut running = runtime.process.lock().await;
        if let Some(process) = running.as_mut()
            && process
                .child
                .try_wait()
                .map_err(|_| Error::Execution)?
                .is_none()
        {
            let mut authentication = reqwest::header::HeaderValue::from_str(&process.token)
                .map_err(|_| Error::Execution)?;
            authentication.set_sensitive(true);
            // Quiesce inside the serving process: timed-out HTTP handlers may still be running.
            // It drains those handlers before checking pending jobs, and accepts no new work afterward.
            let response = self
                .0
                .client
                .post(format!(
                    "http://127.0.0.1:{}/__talocart_package/control",
                    process.port
                ))
                .timeout(Duration::from_secs(120))
                .header("x-talocart-module", authentication.clone())
                .body("quiesce")
                .send()
                .await;
            return match response {
                Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => Err(Error::Busy),
                Ok(r) if r.status() == reqwest::StatusCode::OK => Ok(()),
                _ => {
                    let _ = self
                        .0
                        .client
                        .post(format!(
                            "http://127.0.0.1:{}/__talocart_package/control",
                            process.port
                        ))
                        .timeout(Duration::from_secs(120))
                        .header("x-talocart-module", authentication.clone())
                        .body("resume")
                        .send()
                        .await;
                    Err(Error::Execution)
                }
            };
        }
        drop(running);
        // A damaged executable cannot be asked. It has not served since the registry was
        // loaded; its queued rows are retained for a repaired release.
        if item.damaged() {
            return Ok(());
        }
        let root = self.0.config.root.clone();
        let manifest = item.manifest.clone();
        let path = blocking(move || verify_executable(&root, &manifest)).await?;
        if !item.compatible() {
            return Ok(());
        }
        self.command(&path, &item.manifest, protocol::Action::CanStop)
            .await
    }

    pub(super) async fn start(
        &self,
        path: &Path,
        manifest: &Manifest,
        action: protocol::Action,
    ) -> Result<(Process, protocol::Reply)> {
        if self.0.offline && action == protocol::Action::Serve {
            return Err(Error::Conflict);
        }
        manifest.validate()?;
        let token = crate::identity::token().map_err(|_| Error::Execution)?;
        let start = protocol::Start {
            protocol: protocol::VERSION,
            sdk_version: crate::packages::sdk::VERSION.into(),
            application: crate::version::ApplicationIdentity::current(),
            database: crate::database::contract(),
            action,
            database_url: if action == protocol::Action::CanStop {
                crate::database::read_only_url(&self.0.config.database_url)
                    .map_err(|_| Error::Schema)?
            } else {
                self.0.config.database_url.clone()
            },
            media_root: self.0.config.media_root.clone(),
            data_root: self.0.config.root.join("data").join(&manifest.id),
            store_name: self.0.config.store_name.clone(),
            public_origin: self.0.config.public_origin.clone(),
            secure_cookies: self.0.config.secure_cookies,
            token: token.clone(),
        };
        let mut child = Command::new(path)
            .arg("--module-protocol")
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| Error::Execution)?;
        let mut stdin = child.stdin.take().ok_or(Error::Execution)?;
        let mut message = serde_json::to_vec(&start).map_err(|_| Error::Execution)?;
        message.push(b'\n');
        if message.len() > 32768 {
            return Err(Error::Execution);
        }
        stdin
            .write_all(&message)
            .await
            .map_err(|_| Error::Execution)?;
        let stdout = child.stdout.take().ok_or(Error::Execution)?;
        let reply = async {
            let raw = protocol::read_line(&mut BufReader::new(stdout)).await?;
            serde_json::from_slice::<protocol::Reply>(&raw).map_err(|_| Error::Execution)
        }
        .await;
        let reply = match reply {
            Ok(r) if r.matches_manifest(manifest) => r,
            _ => {
                let _ = child.kill().await;
                return Err(Error::Execution);
            }
        };
        let port = reply.port.unwrap_or(0);
        if action == protocol::Action::Serve && (port == 0 || reply.busy)
            || action != protocol::Action::Serve && reply.port.is_some()
        {
            let _ = child.kill().await;
            return Err(Error::Execution);
        }
        Ok((
            Process {
                child,
                _stdin: stdin,
                port,
                token,
            },
            reply,
        ))
    }
    pub(super) async fn command(
        &self,
        path: &Path,
        manifest: &Manifest,
        action: protocol::Action,
    ) -> Result<()> {
        if self.inspect_command(path, manifest, action).await?.busy {
            return Err(Error::Busy);
        }
        Ok(())
    }
    /// Read-only handshake probes may report queued work without claiming it.
    pub(super) async fn inspect_command(
        &self,
        path: &Path,
        manifest: &Manifest,
        action: protocol::Action,
    ) -> Result<protocol::Reply> {
        let (mut process, reply) = self.start(path, manifest, action).await?;
        let status = tokio::time::timeout(Duration::from_secs(120), process.child.wait()).await;
        if !matches!(status,Ok(Ok(s)) if s.success()) {
            let _ = process.child.kill().await;
            return Err(Error::Execution);
        }
        Ok(reply)
    }
    /// Read-only provider settings/schema probe; never starts serving or external calls.
    pub async fn check_payment(&self) -> Result<()> {
        for item in self.payments()? {
            let root = self.0.config.root.clone();
            let manifest = item.manifest.clone();
            let path = blocking(move || verify_executable(&root, &manifest)).await?;
            self.inspect_command(&path, &item.manifest, protocol::Action::CanStop)
                .await?;
        }
        Ok(())
    }
    pub async fn endpoint(&self, id: &str) -> Result<(u16, String)> {
        self.ensure_mutable()?;
        // Shared with other requests; startup itself is exclusive per ID.
        let runtime = self.runtime(id)?;
        let _module = runtime.gate.read().await;
        self.ensure_mutable()?;
        self.ensure_running(id).await
    }
    /// Caller holds the module gate, shared or exclusive. The process mutex makes
    /// startup exclusive per ID, so concurrent requests never start two children.
    pub(super) async fn ensure_running(&self, id: &str) -> Result<(u16, String)> {
        // Admission is checked by callers before acquiring lifecycle ownership.
        // An update already in flight must finish even if maintenance begins meanwhile.
        let item = self
            .inventory()?
            .into_iter()
            .find(|p| p.manifest.id == id && p.enabled)
            .ok_or(Error::Invalid)?;
        if item.incomplete() {
            return Err(Error::Incomplete);
        }
        if !item.compatible() {
            return Err(Error::Incompatible);
        }
        if item.damaged() {
            return Err(Error::Signature);
        }
        let runtime = self.runtime(id)?;
        let mut running = runtime.process.lock().await;
        if let Some(p) = running.as_mut() {
            if p.child.try_wait().map_err(|_| Error::Execution)?.is_none() {
                runtime
                    .health
                    .write()
                    .map_err(|_| Error::Storage)?
                    .running();
                return Ok((p.port, p.token.clone()));
            }
            *running = None;
            runtime.forget_payment_status();
            self.runtime_failure(&item, RuntimePhase::Exit, &Error::Execution);
            return Err(Error::Execution);
        }
        if self.0.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Busy);
        }
        {
            let mut health = runtime.health.write().map_err(|_| Error::Storage)?;
            if !health.can_start() {
                return Err(Error::Execution);
            }
            health.status.state = RuntimeState::Starting;
        }
        let root = self.0.config.root.clone();
        let manifest = item.manifest.clone();
        let started = async {
            let path = blocking(move || verify_executable(&root, &manifest)).await?;
            self.start(&path, &item.manifest, protocol::Action::Serve)
                .await
        }
        .await;
        let (process, _) = match started {
            Ok(process) => process,
            Err(error) => {
                self.runtime_failure(&item, RuntimePhase::Start, &error);
                return Err(error);
            }
        };
        let result = (process.port, process.token.clone());
        *running = Some(process);
        runtime.forget_payment_status();
        runtime
            .health
            .write()
            .map_err(|_| Error::Storage)?
            .running();
        Ok(result)
    }
    /// Exclusive ownership for a lifecycle transition: waits for in-flight requests.
    pub async fn module_guard(&self, id: &str) -> Result<ModuleGuard> {
        self.ensure_mutable()?;
        let runtime = self.runtime(id)?;
        let module = runtime.gate.clone().write_owned().await;
        self.ensure_mutable()?;
        Ok(ModuleGuard {
            _shared: None,
            _exclusive: Some(module),
            _runtime: runtime,
            manager: self.clone(),
            id: id.into(),
        })
    }
    /// Shared admission for one ordinary request, kept until its response is complete.
    /// The wait is bounded: a request never queues indefinitely behind a transition.
    /// Administrative mutations use `mutation_guard` to exclude host payment transactions too.
    pub async fn request_guard(&self, id: &str) -> Result<ModuleGuard> {
        self.ensure_mutable()?;
        let runtime = self.runtime(id)?;
        let module = tokio::time::timeout(REQUEST_GATE_WAIT, runtime.gate.clone().read_owned())
            .await
            .map_err(|_| Error::Busy)?;
        self.ensure_mutable()?;
        Ok(ModuleGuard {
            _shared: Some(module),
            _exclusive: None,
            _runtime: runtime,
            manager: self.clone(),
            id: id.into(),
        })
    }
    /// Exclusive administrative request, bounded like shared request admission.
    /// A module's own handler lock cannot protect payment transactions in the host.
    pub async fn mutation_guard(&self, id: &str) -> Result<ModuleGuard> {
        tokio::time::timeout(REQUEST_GATE_WAIT, self.module_guard(id))
            .await
            .map_err(|_| Error::Busy)?
    }
    pub(super) fn runtime(&self, id: &str) -> Result<Arc<ModuleRuntime>> {
        let installed = self.inventory()?;
        let mut runtimes = self.0.runtimes.write().map_err(|_| Error::Storage)?;
        // Keep live processes and referenced slots; reclaim idle historical IDs.
        // A ModuleGuard owns its slot, so waiters can never split onto a new gate.
        runtimes.retain(|id, runtime| {
            installed.iter().any(|item| item.manifest.id == *id)
                || Arc::strong_count(runtime) > 1
                || runtime
                    .process
                    .try_lock()
                    .map_or(true, |process| process.is_some())
        });
        Ok(runtimes
            .entry(id.into())
            .or_insert_with(|| {
                Arc::new(ModuleRuntime {
                    gate: Arc::new(tokio::sync::RwLock::new(())),
                    process: Mutex::new(None),
                    health: RwLock::new(supervision::Health::default()),
                    payment_status: RwLock::new(None),
                })
            })
            .clone())
    }
    pub fn client(&self) -> &reqwest::Client {
        &self.0.client
    }
    pub async fn stop(&self, id: &str) {
        if let Ok(runtime) = self.runtime(id) {
            if let Some(mut process) = runtime.process.lock().await.take() {
                let _ = process.child.kill().await;
            }
            runtime.forget_payment_status();
            if let Ok(mut health) = runtime.health.write() {
                *health = supervision::Health::default();
            }
        }
    }
    /// A module asked for its own restart, for example after saving settings. The caller
    /// has released its request guard: wait for the other in-flight requests, then stop
    /// only the generation that answered. A replaced process is left untouched.
    pub async fn stop_generation(&self, id: &str, port: u16, token: &str) {
        let Ok(_module) = self.module_guard(id).await else {
            return;
        };
        let Ok(runtime) = self.runtime(id) else {
            return;
        };
        let current = runtime
            .process
            .lock()
            .await
            .as_ref()
            .is_some_and(|process| process.port == port && process.token == token);
        if current {
            self.stop(id).await;
        }
    }
    pub(in crate::packages) fn cached_payment_status(
        &self,
        id: &str,
        max_age: Duration,
    ) -> Option<bool> {
        let runtime = self.runtime(id).ok()?;
        let status = *runtime.payment_status.read().ok()?;
        status
            .filter(|(at, _)| at.elapsed() <= max_age)
            .map(|(_, accepting)| accepting)
    }
    /// Call while holding the request guard that produced the answer.
    pub(in crate::packages) fn remember_payment_status(&self, id: &str, accepting: bool) {
        if let Ok(runtime) = self.runtime(id)
            && let Ok(mut status) = runtime.payment_status.write()
        {
            *status = Some((std::time::Instant::now(), accepting));
        }
    }
    /// A forwarded mutation may have changed provider settings inside the module.
    pub fn forget_payment_status(&self, id: &str) {
        if let Ok(runtime) = self.runtime(id) {
            runtime.forget_payment_status();
        }
    }
    pub async fn shutdown(&self) {
        self.begin_shutdown();
        let ids: Vec<_> = match self.0.runtimes.read() {
            Ok(runtimes) => runtimes.keys().cloned().collect(),
            Err(_) => {
                tracing::error!("Package runtime registry unavailable during shutdown");
                return;
            }
        };
        for id in ids {
            self.stop(&id).await;
        }
    }
}
