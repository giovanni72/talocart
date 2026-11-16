//! In-memory maintenance barrier owned by the live host, never a registry rewrite.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Before any child can have been quiesced, every error must reopen admission.
/// Once control I/O starts, only explicit recovery may release the barrier.
struct Preparation<'a> {
    barrier: &'a AtomicBool,
    release: bool,
}
impl Preparation<'_> {
    fn retain(&mut self) {
        self.release = false;
    }
}
impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        if self.release {
            self.barrier.store(false, Ordering::Release);
        }
    }
}

pub(super) struct PausedModule {
    _guard: tokio::sync::OwnedRwLockWriteGuard<()>,
    _runtime: Arc<ModuleRuntime>,
}
pub(super) struct Pause {
    _modules: Vec<PausedModule>,
    quiesced: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub id: String,
    pub version: String,
    pub enabled: bool,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSnapshot {
    pub inventory_sha256: String,
    pub modules: Vec<Activation>,
    pub suspended: bool,
}
impl Manager {
    pub fn update_snapshot(&self) -> Result<UpdateSnapshot> {
        let registry = self.registry()?;
        let mut modules: Vec<_> = registry
            .installed
            .iter()
            .map(|item| Activation {
                id: item.manifest.id.clone(),
                version: item.manifest.version.clone(),
                enabled: item.enabled,
                sha256: item.manifest.sha256.clone(),
            })
            .collect();
        modules.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(UpdateSnapshot {
            inventory_sha256: planning::fingerprint(&registry)?,
            modules,
            suspended: self.0.update_preparing.load(Ordering::Acquire),
        })
    }
    /// Controlled ordinary activation, after workerless verification and flag restore.
    /// Starts only originally enabled modules; never rewrites desired activation.
    pub async fn warm_update(&self, expected: String) -> Result<UpdateSnapshot> {
        format::unhex(&expected, 32)?;
        self.ensure_mutable()?;
        if !self.0.config.serve || self.0.offline || !self.0.config.compiled.is_empty() {
            return Err(Error::Conflict);
        }
        let original = self.verified_update_snapshot().await?;
        if original.inventory_sha256 != expected {
            return Err(Error::Conflict);
        }
        for item in original.modules.iter().filter(|item| item.enabled) {
            let guard = self.module_guard(&item.id).await?;
            tokio::time::timeout(Duration::from_secs(125), async {
                let (port, authentication) = guard.endpoint().await?;
                // Resume restores quiescence state; independent authenticated
                // health probes establish readiness for the serving process.
                let response = self
                    .0
                    .client
                    .post(format!(
                        "http://127.0.0.1:{port}/__talocart_package/control"
                    ))
                    .timeout(Duration::from_secs(120))
                    .header("x-talocart-module", authentication)
                    .body("resume")
                    .send()
                    .await
                    .map_err(|_| Error::Execution)?;
                if response.status() != reqwest::StatusCode::OK {
                    return Err(Error::Execution);
                }
                Ok(())
            })
            .await
            .map_err(|_| Error::Execution)??;
        }
        let actual = self.verified_update_snapshot().await?;
        if actual.suspended || actual.inventory_sha256 != expected {
            return Err(Error::Conflict);
        }
        Ok(actual)
    }
    async fn verified_update_snapshot(&self) -> Result<UpdateSnapshot> {
        let snapshot = self.update_snapshot()?;
        let root = self.0.config.root.clone();
        let actual = blocking(move || load(&root)).await?;
        intact(&actual)?;
        if planning::fingerprint(&actual)? != snapshot.inventory_sha256
            || self.update_snapshot()?.inventory_sha256 != snapshot.inventory_sha256
        {
            return Err(Error::Conflict);
        }
        Ok(snapshot)
    }
    pub(super) fn ensure_mutable(&self) -> Result<()> {
        if self.0.update_preparing.load(Ordering::Acquire) {
            Err(Error::Busy)
        } else {
            Ok(())
        }
    }
    /// Task lifetime is independent of the controlling socket/browser disconnect.
    pub async fn prepare_update(&self, expected: String) -> Result<UpdateSnapshot> {
        let manager = self.clone();
        tokio::spawn(async move { manager.prepare_update_inner(expected).await })
            .await
            .map_err(|_| Error::Execution)?
    }
    async fn prepare_update_inner(&self, expected: String) -> Result<UpdateSnapshot> {
        format::unhex(&expected, 32)?;
        if !self.0.config.serve || self.0.offline || !self.0.config.compiled.is_empty() {
            return Err(Error::Conflict);
        }
        let mut paused = self.0.update_pause.lock().await;
        if paused.is_some() || self.0.update_preparing.swap(true, Ordering::AcqRel) {
            return Err(Error::Busy);
        }
        let mut preparation = Preparation {
            barrier: &self.0.update_preparing,
            release: true,
        };
        // Stop admission of lifecycle changes first, then acquire the same module
        // gates in deterministic order. Existing requests are allowed to drain.
        let initial = self.registry()?;
        let mut items = initial.installed.clone();
        items.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        let mut guards = Vec::new();
        for item in &items {
            let runtime = self.runtime(&item.manifest.id)?;
            let guard = match tokio::time::timeout(
                Duration::from_secs(120),
                runtime.gate.clone().write_owned(),
            )
            .await
            {
                Ok(guard) => guard,
                Err(_) => {
                    return Err(Error::Busy);
                }
            };
            guards.push(PausedModule {
                _guard: guard,
                _runtime: runtime,
            });
        }
        let _op = self.0.operation.lock().await;
        let _disk = self.lock_disk().await?;
        let root = self.0.config.root.clone();
        let verified = blocking(move || load(&root)).await?;
        intact(&verified)?;
        if planning::fingerprint(&initial)? != expected
            || planning::fingerprint(&verified)? != expected
            || planning::fingerprint(&self.registry()?)? != expected
        {
            return Err(Error::Conflict);
        }
        let mut state = Pause {
            _modules: guards,
            quiesced: Vec::new(),
        };
        for item in &verified.installed {
            if !item.enabled {
                continue;
            }
            // Include this ID before sending: a lost response can still have paused it.
            state.quiesced.push(item.manifest.id.clone());
            preparation.retain();
            let result = tokio::time::timeout(Duration::from_secs(125), self.quiesce(item)).await;
            if !matches!(result, Ok(Ok(()))) {
                let reason = match result {
                    Ok(Err(error)) => error,
                    _ => Error::Execution,
                };
                if self.resume_processes(&state.quiesced).await.is_ok() {
                    self.0.update_preparing.store(false, Ordering::Release);
                } else {
                    // Keep gates and barrier on uncertain resume; operator must recover.
                    *paused = Some(state);
                }
                return Err(reason);
            }
        }
        *paused = Some(state);
        preparation.retain();
        self.update_snapshot()
    }
    async fn resume_processes(&self, ids: &[String]) -> Result<()> {
        let mut failed = false;
        for id in ids {
            let result = async {
                let runtime = self.runtime(id)?;
                let mut running = runtime.process.lock().await;
                let Some(process) = running.as_mut() else {
                    return Ok(());
                };
                if process
                    .child
                    .try_wait()
                    .map_err(|_| Error::Execution)?
                    .is_some()
                {
                    return Err(Error::Execution);
                }
                let mut authentication = reqwest::header::HeaderValue::from_str(&process.token)
                    .map_err(|_| Error::Execution)?;
                authentication.set_sensitive(true);
                let response = self
                    .0
                    .client
                    .post(format!(
                        "http://127.0.0.1:{}/__talocart_package/control",
                        process.port
                    ))
                    .header("x-talocart-module", authentication)
                    .body("resume")
                    .timeout(Duration::from_secs(120))
                    .send()
                    .await
                    .map_err(|_| Error::Execution)?;
                if response.status() != reqwest::StatusCode::OK {
                    return Err(Error::Execution);
                }
                Ok(())
            }
            .await;
            failed |= result.is_err();
        }
        if failed {
            Err(Error::Execution)
        } else {
            Ok(())
        }
    }
    pub async fn resume_update(&self) -> Result<UpdateSnapshot> {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut paused = manager.0.update_pause.lock().await;
            if let Some(state) = paused.as_ref() {
                manager.resume_processes(&state.quiesced).await?;
                paused.take();
                manager.0.update_preparing.store(false, Ordering::Release);
            } else if manager.0.update_preparing.load(Ordering::Acquire) {
                return Err(Error::Execution);
            }
            manager.update_snapshot()
        })
        .await
        .map_err(|_| Error::Execution)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn preparation_storage_failure_reopens_admission_before_any_quiesce() {
        let config = super::super::tests::config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let initial = manager.update_snapshot().unwrap();
        // The live registry is valid; only the independent disk verification fails.
        fs::write(root.join("registry.json"), b"invalid registry").unwrap();
        let prepared = manager
            .prepare_update(initial.inventory_sha256.clone())
            .await;
        fs::remove_file(root.join("registry.json")).unwrap();
        let suspended = manager.update_snapshot().unwrap().suspended;
        let resumed = manager.resume_update().await;
        let retry = manager.prepare_update(initial.inventory_sha256).await;
        if retry.is_ok() {
            manager.resume_update().await.unwrap();
        }
        fs::remove_dir_all(root).unwrap();
        assert!(matches!(prepared, Err(Error::Storage)));
        assert!(!suspended, "A pre-quiesce failure must release admission");
        assert!(
            resumed.is_ok(),
            "Recovery must remain idempotent: {resumed:?}"
        );
        assert!(
            retry.is_ok(),
            "A corrected storage fault must be retryable: {retry:?}"
        );
    }
    #[tokio::test]
    async fn live_barrier_preserves_registry_blocks_lifecycle_and_resumes() {
        let config = super::super::tests::config();
        let root = config.root.clone();
        let manager = Manager::open(config).await.unwrap();
        let initial = manager.update_snapshot().unwrap();
        let snapshot = manager
            .prepare_update(initial.inventory_sha256.clone())
            .await
            .unwrap();
        assert!(snapshot.suspended);
        assert_eq!(snapshot.inventory_sha256, initial.inventory_sha256);
        assert!(matches!(
            manager.trust("aa".repeat(32)).await,
            Err(Error::Busy)
        ));
        assert!(!root.join("registry.json").exists());
        assert!(
            manager
                .prepare_update(initial.inventory_sha256.clone())
                .await
                .is_err()
        );
        assert!(!manager.resume_update().await.unwrap().suspended);
        manager.trust("aa".repeat(32)).await.unwrap();
        assert_ne!(
            manager.update_snapshot().unwrap().inventory_sha256,
            initial.inventory_sha256
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn changed_inventory_and_readonly_managers_cannot_quiesce() {
        let mut config = super::super::tests::config();
        let root = config.root.clone();
        let manager = Manager::open(config.clone()).await.unwrap();
        assert!(matches!(
            manager.prepare_update("ff".repeat(32)).await,
            Err(Error::Conflict)
        ));
        assert!(!manager.update_snapshot().unwrap().suspended);
        drop(manager);
        config.serve = false;
        let readonly = Manager::open(config).await.unwrap();
        assert!(
            readonly
                .prepare_update(readonly.update_snapshot().unwrap().inventory_sha256)
                .await
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
