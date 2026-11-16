//! Live-process supervision. Desired activation stays in the package registry.
use super::*;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAX_FAILURES: u8 = 3;
/// After the budget is spent, keep retrying slowly so a transient outage recovers unattended.
const SLOW_RETRY: Duration = Duration::from_secs(60);
const STABLE_SECONDS: u64 = 60;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_MAX_AGE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Stopped,
    Starting,
    Running,
    Waiting,
    Failed,
}
impl RuntimeState {
    pub fn key(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePhase {
    Start,
    Exit,
    Request,
}
impl RuntimePhase {
    pub fn key(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Exit => "exit",
            Self::Request => "request",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCause {
    Storage,
    Integrity,
    Compatibility,
    Execution,
}
impl RuntimeCause {
    pub fn key(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Integrity => "integrity",
            Self::Compatibility => "compatibility",
            Self::Execution => "execution",
        }
    }
    fn from_error(error: &Error) -> Self {
        match error {
            Error::Storage => Self::Storage,
            Error::Signature | Error::Publisher => Self::Integrity,
            Error::Incompatible | Error::Schema => Self::Compatibility,
            _ => Self::Execution,
        }
    }
}
#[derive(Clone, Debug)]
pub struct RuntimeFailure {
    pub phase: RuntimePhase,
    pub cause: RuntimeCause,
    pub at_unix_seconds: u64,
}
#[derive(Clone, Debug)]
pub struct RuntimeStatus {
    pub state: RuntimeState,
    pub failures: u8,
    pub last_error: Option<RuntimeFailure>,
}
pub(super) struct Health {
    pub(super) status: RuntimeStatus,
    started: Option<Instant>,
    next_attempt: Instant,
    last_probe: Option<(Instant, bool)>,
}
impl Default for Health {
    fn default() -> Self {
        Self {
            status: RuntimeStatus {
                state: RuntimeState::Stopped,
                failures: 0,
                last_error: None,
            },
            started: None,
            next_attempt: Instant::now(),
            last_probe: None,
        }
    }
}
impl Health {
    fn ready(&self) -> bool {
        self.status.state == RuntimeState::Running
            && self
                .last_probe
                .is_some_and(|(at, responsive)| responsive && at.elapsed() <= PROBE_MAX_AGE)
    }
    pub(super) fn can_start(&self) -> bool {
        Instant::now() >= self.next_attempt
    }
    pub(super) fn running(&mut self) {
        let started = *self.started.get_or_insert_with(Instant::now);
        if started.elapsed() >= Duration::from_secs(STABLE_SECONDS) {
            self.status.failures = 0;
        }
        self.status.state = RuntimeState::Running;
    }
    fn failure(&mut self, phase: RuntimePhase, cause: RuntimeCause) {
        if phase != RuntimePhase::Request {
            self.last_probe = None;
            if self
                .started
                .take()
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(STABLE_SECONDS))
            {
                self.status.failures = 0;
            }
            self.status.failures = self.status.failures.saturating_add(1).min(MAX_FAILURES);
            self.status.state = if self.status.failures >= MAX_FAILURES {
                RuntimeState::Failed
            } else {
                RuntimeState::Waiting
            };
            self.next_attempt = Instant::now()
                + if self.status.failures >= MAX_FAILURES {
                    SLOW_RETRY
                } else {
                    Duration::from_secs(2_u64.pow(u32::from(self.status.failures)))
                };
        }
        self.status.last_error = Some(RuntimeFailure {
            phase,
            cause,
            at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
    }
}

impl Manager {
    /// Only the ordinary serving host enables supervision. Doctor/offline/serve-verify never do.
    pub fn enable_supervision(&self) -> Result<()> {
        if !self.0.config.serve || self.0.offline {
            return Err(Error::Conflict);
        }
        self.0.supervised.store(true, Ordering::Release);
        Ok(())
    }
    pub fn runtime_status(&self, id: &str) -> Result<RuntimeStatus> {
        Ok(self
            .runtime(id)?
            .health
            .read()
            .map_err(|_| Error::Storage)?
            .status
            .clone())
    }
    /// Strict readiness for every enabled module, regardless of owner policy.
    pub fn runtime_ready(&self) -> Result<bool> {
        self.readiness(true)
    }
    pub fn store_ready(&self) -> Result<bool> {
        self.readiness(false)
    }
    fn readiness(&self, all_modules: bool) -> Result<bool> {
        if !self.0.supervised.load(Ordering::Acquire) {
            return Ok(true);
        }
        if self.0.stopping.load(Ordering::Acquire)
            || self.0.update_preparing.load(Ordering::Acquire)
        {
            return Ok(false);
        }
        for item in self
            .inventory()?
            .into_iter()
            .filter(|p| p.enabled && (all_modules || p.critical))
        {
            if !item.available()
                || !self
                    .runtime(&item.manifest.id)?
                    .health
                    .read()
                    .map_err(|_| Error::Storage)?
                    .ready()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// At most one task per installed ID. Slow startup does not delay another ID's next tick.
    pub async fn supervise(&self) -> Result<()> {
        if !self.0.supervised.load(Ordering::Acquire) {
            return Err(Error::Conflict);
        }
        let mut tasks = tokio::task::JoinSet::new();
        let mut active = std::collections::HashSet::new();
        let mut ticks = tokio::time::interval(Duration::from_secs(1));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                result = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(result) = result {
                        let id = result.map_err(|_| Error::Execution)??;
                        active.remove(&id);
                    }
                }
                _ = ticks.tick() => {
                    if self.0.stopping.load(Ordering::Acquire) { return Ok(()) }
                    if self.ensure_mutable().is_err() { continue }
                    for item in self.inventory()?.into_iter().filter(|p| p.enabled && p.available()) {
                        if !active.insert(item.manifest.id.clone()) { continue }
                        let manager = self.clone();
                        tasks.spawn(async move {
                            let id = item.manifest.id;
                            let runtime = manager.runtime(&id)?;
                            // Shared with requests; skipped while a transition owns or awaits the gate.
                            if let Ok(_gate) = runtime.gate.clone().try_read_owned()
                                && manager.ensure_mutable().is_ok()
                            {
                                // Failures are recorded; further attempts obey the same retry schedule as HTTP.
                                let _ = manager.ensure_running(&id).await;
                            }
                            // A hung request must not prevent the independent health probe.
                            manager.probe(&id, &runtime).await?;
                            Ok::<_, Error>(id)
                        });
                    }
                }
            }
        }
    }
    async fn probe(&self, id: &str, runtime: &ModuleRuntime) -> Result<()> {
        // Never retain the process mutex across network I/O; startup, shutdown and
        // lifecycle operations retain their own ownership and deadlines.
        let (port, token) = {
            let Ok(process) = runtime.process.try_lock() else {
                return Ok(());
            };
            let Some(process) = process.as_ref() else {
                return Ok(());
            };
            (process.port, process.token.clone())
        };
        let mut authentication =
            reqwest::header::HeaderValue::from_str(&token).map_err(|_| Error::Execution)?;
        authentication.set_sensitive(true);
        let responsive = self
            .0
            .client
            .get(format!("http://127.0.0.1:{port}{}", protocol::HEALTH_PATH))
            .header("x-talocart-module", authentication)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .is_ok_and(|response| response.status() == reqwest::StatusCode::NO_CONTENT);
        let Ok(process) = runtime.process.try_lock() else {
            return Ok(());
        };
        if !process
            .as_ref()
            .is_some_and(|process| process.port == port && process.token == token)
        {
            return Ok(()); // A replaced generation cannot inherit an old probe result.
        }
        let report = {
            let mut health = runtime.health.write().map_err(|_| Error::Storage)?;
            let report = !responsive && health.last_probe.is_none_or(|(_, healthy)| healthy);
            health.last_probe = Some((Instant::now(), responsive));
            report
        };
        if report {
            // A failed probe affects readiness only. It never spends the restart
            // budget or kills a live process with possibly uncertain operations.
            self.record_request_failure(id);
        }
        Ok(())
    }
    pub(super) fn runtime_failure(&self, item: &Installed, phase: RuntimePhase, error: &Error) {
        let cause = RuntimeCause::from_error(error);
        if let Ok(runtime) = self.runtime(&item.manifest.id)
            && let Ok(mut health) = runtime.health.write()
        {
            if phase == RuntimePhase::Request && health.status.state != RuntimeState::Running {
                return;
            }
            // Slow retries repeat the same failure every minute: journal each problem once.
            // The budget, not the state, tells: a slow attempt is `Starting` or `Running` here.
            let repeated = health.status.failures >= MAX_FAILURES
                && health
                    .status
                    .last_error
                    .as_ref()
                    .is_some_and(|last| last.phase == phase && last.cause == cause);
            health.failure(phase, cause);
            if repeated {
                return;
            }
        }
        // Never include child stderr, SQL, URLs, headers, credentials or provider payloads.
        tracing::error!(target: "talocart::packages", package = %item.manifest.id,
            version = %item.manifest.version, phase = phase.key(), error = cause.key(),
            "Native package operation failed");
    }
    pub fn record_request_failure(&self, id: &str) {
        if let Ok(items) = self.inventory()
            && let Some(item) = items.into_iter().find(|p| p.manifest.id == id)
        {
            self.runtime_failure(&item, RuntimePhase::Request, &Error::Execution);
        }
    }
    /// Explicit operator retry. A running module must first confirm that no durable work is busy.
    pub async fn restart(&self, id: &str) -> Result<()> {
        if !self.0.config.serve || self.0.offline {
            return Err(Error::Conflict);
        }
        self.ensure_mutable()?;
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _gate = manager.module_guard(&id).await?;
            manager.ensure_mutable()?;
            let item = manager
                .inventory()?
                .into_iter()
                .find(|p| p.manifest.id == id && p.enabled)
                .ok_or(Error::Invalid)?;
            if !item.available() {
                return Err(Error::Incompatible);
            }
            let runtime = manager.runtime(&id)?;
            let alive = {
                let mut process = runtime.process.lock().await;
                match process.as_mut() {
                    Some(process) => process
                        .child
                        .try_wait()
                        .map_err(|_| Error::Execution)?
                        .is_none(),
                    None => false,
                }
            };
            if alive {
                manager.quiesce(&item).await?;
            }
            manager.stop(&id).await;
            manager.ensure_running(&id).await.map(|_| ())
        })
        .await
        .map_err(|_| Error::Execution)?
    }
    pub fn begin_shutdown(&self) {
        self.0.stopping.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn readiness_probes_a_live_process_outside_its_request_gate_without_restarting_it() {
        use std::sync::atomic::AtomicBool;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let config = super::super::tests::config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        manager
            .0
            .registry
            .write()
            .unwrap()
            .installed
            .push(Installed {
                critical: true,
                manifest: format::fixture_manifest(),
                enabled: true,
                raw: vec![],
                signature: vec![],
                previous: None,
                update: None,
                damaged: false,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let token = "ab".repeat(32);
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id();
        let stdin = child.stdin.take().unwrap();
        let runtime = manager.runtime("fixture").unwrap();
        *runtime.process.lock().await = Some(process::Process {
            child,
            _stdin: stdin,
            port: listener.local_addr().unwrap().port(),
            token: token.clone(),
        });
        manager.ensure_running("fixture").await.unwrap();
        manager.enable_supervision().unwrap();
        let gate = manager.module_guard("fixture").await.unwrap();
        let responsive = Arc::new(AtomicBool::new(true));
        let (seen, mut probes) = tokio::sync::mpsc::unbounded_channel();
        let responder = tokio::spawn({
            let responsive = responsive.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        let count = socket.read(&mut chunk).await.unwrap();
                        if count == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..count]);
                        assert!(request.len() <= 4096);
                    }
                    let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                    assert!(request.starts_with("get /__talocart_package/health http/1.1\r\n"));
                    assert!(request.contains(&format!("x-talocart-module: {token}\r\n")));
                    let healthy = responsive.load(Ordering::Acquire);
                    let status = if healthy {
                        "204 No Content"
                    } else {
                        "503 Service Unavailable"
                    };
                    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                    seen.send(healthy).unwrap();
                }
            }
        });
        let supervisor = tokio::spawn({
            let manager = manager.clone();
            async move { manager.supervise().await }
        });
        let observed = tokio::time::timeout(Duration::from_secs(3), probes.recv()).await;
        let transitions = async {
            for expected in [true, false, true] {
                responsive.store(expected, Ordering::Release);
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if manager.runtime_ready().unwrap() == expected {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await?;
            }
            Ok::<_, tokio::time::error::Elapsed>(())
        }
        .await;
        let status = manager.runtime_status("fixture").unwrap();
        let same_pid = runtime.process.lock().await.as_ref().unwrap().child.id() == pid;
        drop(gate);
        supervisor.abort();
        let _ = supervisor.await;
        responder.abort();
        let _ = responder.await;
        runtime.health.write().unwrap().last_probe = Some((
            Instant::now() - PROBE_MAX_AGE - Duration::from_secs(1),
            true,
        ));
        let expired_is_ready = manager.runtime_ready().unwrap();
        runtime.health.write().unwrap().last_probe = Some((Instant::now(), true));
        manager.stop("fixture").await;
        // A new process cannot inherit a recent result from the stopped generation.
        runtime.health.write().unwrap().running();
        let replacement_is_ready = manager.runtime_ready().unwrap();
        manager.shutdown().await;
        fs::remove_dir_all(root).unwrap();
        assert!(
            observed.is_ok(),
            "Probe must run even while a request owns the module gate"
        );
        assert!(
            transitions.is_ok(),
            "Readiness must follow probe failure and recovery"
        );
        assert_eq!(status.state, RuntimeState::Running);
        assert_eq!(
            status.failures, 0,
            "Probe failure must not spend the process restart budget"
        );
        assert!(
            same_pid,
            "Probe recovery must preserve the existing process"
        );
        assert!(
            !expired_is_ready,
            "Stale probe evidence must fail readiness"
        );
        assert!(
            !replacement_is_ready,
            "A replacement needs its own successful probe"
        );
    }
    #[test]
    fn repeated_crashes_exhaust_the_budget_and_stable_service_resets_it() {
        let mut health = Health::default();
        for expected in 1..=MAX_FAILURES {
            health.failure(RuntimePhase::Exit, RuntimeCause::Execution);
            assert_eq!(health.status.failures, expected);
            assert!(!health.can_start());
            if expected < MAX_FAILURES {
                health.next_attempt = Instant::now();
                assert!(health.can_start());
            }
        }
        // The exhausted budget switches to slow unattended retries instead of stopping.
        assert_eq!(health.status.state, RuntimeState::Failed);
        assert!(!health.can_start());
        let wait = health
            .next_attempt
            .saturating_duration_since(Instant::now());
        assert!(wait > SLOW_RETRY - Duration::from_secs(5) && wait <= SLOW_RETRY);
        health.next_attempt = Instant::now();
        assert!(health.can_start());
        assert_eq!(health.status.state, RuntimeState::Failed);
        health.failure(RuntimePhase::Start, RuntimeCause::Execution);
        assert_eq!(health.status.failures, MAX_FAILURES);
        assert_eq!(health.status.state, RuntimeState::Failed);
        assert!(!health.can_start());
        assert!(
            health
                .next_attempt
                .saturating_duration_since(Instant::now())
                > SLOW_RETRY - Duration::from_secs(5)
        );
        health.started = Some(Instant::now() - Duration::from_secs(STABLE_SECONDS));
        health.running();
        assert_eq!(health.status.failures, 0);
        health.failure(RuntimePhase::Request, RuntimeCause::Execution);
        assert_eq!(health.status.state, RuntimeState::Running);
        assert_eq!(
            health.status.failures, 0,
            "An RPC error must not trigger a process restart"
        );
        health.last_probe = Some((Instant::now(), true));
        assert!(health.ready());
        health.failure(RuntimePhase::Exit, RuntimeCause::Execution);
        health.running();
        assert!(
            !health.ready(),
            "A crash discards the previous process's probe"
        );
    }
}
