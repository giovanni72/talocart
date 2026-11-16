//! Explicit update operations under the exclusive host lease, without workers.
use super::*;

impl Manager {
    fn require_offline(&self) -> Result<()> {
        if self.0.offline && self.0.config.serve {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }
    pub fn require_inactive(&self) -> Result<()> {
        self.require_offline()?;
        let registry = self.registry()?;
        if registry.installed.iter().any(|item| item.incomplete()) {
            return Err(Error::Incomplete);
        }
        if registry.installed.iter().any(|item| item.enabled) {
            return Err(Error::Active);
        }
        Ok(())
    }
    /// Call only AFTER the coordinated backup. Original activation is captured in
    /// the updater journal/backup before invoking this command, never inferred later.
    pub async fn suspend_offline(
        &self,
        expected: String,
        quiesced: Option<UpdateSnapshot>,
    ) -> Result<UpdateSnapshot> {
        self.require_offline()?;
        format::unhex(&expected, 32)?;
        let _exclusive = self.0.offline_operation.lock().await;
        let _op = self.0.operation.lock().await;
        let _disk = self.lock_disk().await?;
        let mut registry = self.registry()?;
        let root = self.0.config.root.clone();
        let actual = blocking(move || load(&root)).await?;
        intact(&actual)?;
        if planning::fingerprint(&registry)? != expected
            || planning::fingerprint(&actual)? != expected
        {
            return Err(Error::Conflict);
        }
        if registry.installed.iter().any(|item| item.incomplete()) {
            return Err(Error::Incomplete);
        }
        let snapshot = self.update_snapshot()?;
        if let Some(proof) = quiesced {
            if !proof.suspended
                || proof.inventory_sha256 != expected
                || proof.modules != snapshot.modules
            {
                return Err(Error::Conflict);
            }
        } else if registry.installed.iter().any(|item| item.enabled) {
            return Err(Error::Conflict);
        }
        // The live host already quiesced these exact modules before fencing/backup.
        // Do not execute old modules against the target's not-yet-migrated schema.
        for item in &mut registry.installed {
            item.enabled = false;
        }
        self.persist(registry).await?;
        Ok(snapshot)
    }
    /// Installs no new ID. The same trusted manager rules verify each package,
    /// contracts, migration prefix and executable integrity before applying an intent.
    pub async fn apply_offline(
        &self,
        paths: Vec<PathBuf>,
        pool: &sqlx::PgPool,
    ) -> Result<UpdatePlan> {
        self.require_offline()?;
        let _exclusive = self.0.offline_operation.lock().await;
        self.require_inactive()?;
        let plan = self.plan_update(paths.clone()).await?;
        if plan.blocked() {
            return Err(Error::Incompatible);
        }
        // Validate all replacements and the live ledgers before the first update.
        self.check_database(pool).await.map_err(|_| Error::Schema)?;
        let mut replacements = std::collections::HashMap::new();
        for path in paths {
            let source = path.clone();
            let (manifest, digest) = blocking(move || {
                let bytes = read_file(&source, format::LIMIT + 32768 + 77)?;
                let digest = format::hex(&Sha256::digest(&bytes));
                Ok((format::inspect(&bytes)?.manifest, digest))
            })
            .await?;
            if let Some(item) = plan.modules.iter().find(|item| item.id == manifest.id) {
                match item.action {
                    planning::PlanAction::Update => {
                        if item.package_sha256.as_ref() != Some(&digest) {
                            return Err(Error::Signature);
                        }
                        replacements.insert(item.id.clone(), path);
                    }
                    planning::PlanAction::Keep => {
                        let current = self
                            .inventory()?
                            .into_iter()
                            .find(|m| m.manifest.id == item.id)
                            .ok_or(Error::Conflict)?;
                        if manifest != current.manifest {
                            return Err(Error::Signature);
                        }
                    }
                    planning::PlanAction::Blocked => return Err(Error::Incompatible),
                }
            } else if !plan.ignored.contains(&manifest.id) {
                return Err(Error::Signature);
            }
        }
        let required = plan
            .modules
            .iter()
            .filter(|item| item.action == planning::PlanAction::Update)
            .count();
        if replacements.len() != required {
            return Err(Error::Signature);
        }
        // Plan order is stable by module ID. Re-read and re-hash immediately before
        // each preview so an input change can never become a silently skipped update.
        for item in plan
            .modules
            .iter()
            .filter(|item| item.action == planning::PlanAction::Update)
        {
            let path = replacements.remove(&item.id).ok_or(Error::Signature)?;
            let bytes = blocking(move || read_file(&path, format::LIMIT + 32768 + 77)).await?;
            if item.package_sha256.as_ref() != Some(&format::hex(&Sha256::digest(&bytes))) {
                return Err(Error::Signature);
            }
            let preview = self.preview("system-update".into(), bytes).await?;
            if preview.manifest.id != item.id {
                return Err(Error::Signature);
            }
            self.update("system-update", &preview.token, pool).await?;
        }
        self.check_database(pool).await.map_err(|_| Error::Schema)?;
        Ok(plan)
    }
    /// Handshake and read-only module checks use the existing CanStop action;
    /// they never start Serve or acquire background jobs, including disabled IDs.
    pub async fn verify_offline(&self, pool: &sqlx::PgPool) -> Result<UpdateSnapshot> {
        self.require_offline()?;
        let _exclusive = self.0.offline_operation.lock().await;
        self.check_database(pool).await.map_err(|_| Error::Schema)?;
        for item in self.inventory()? {
            if item.incomplete() {
                return Err(Error::Incomplete);
            }
            if !item.compatible() {
                return Err(Error::Incompatible);
            }
            let root = self.0.config.root.clone();
            let manifest = item.manifest.clone();
            let path = blocking(move || verify_executable(&root, &manifest)).await?;
            // Queued work is preserved; this is a compatibility/ledger probe, not
            // permission to run jobs. Active modules were quiesced before backup.
            self.inspect_command(&path, &item.manifest, protocol::Action::CanStop)
                .await?;
        }
        self.update_snapshot()
    }
    pub async fn restore_activation_offline(
        &self,
        original: UpdateSnapshot,
    ) -> Result<UpdateSnapshot> {
        self.require_offline()?;
        let _exclusive = self.0.offline_operation.lock().await;
        let _op = self.0.operation.lock().await;
        let _disk = self.lock_disk().await?;
        let mut registry = self.registry()?;
        let root = self.0.config.root.clone();
        let actual = blocking(move || load(&root)).await?;
        intact(&actual)?;
        if planning::fingerprint(&actual)? != planning::fingerprint(&registry)? {
            return Err(Error::Conflict);
        }
        let mut ids = std::collections::HashSet::new();
        if original.modules.len() != registry.installed.len()
            || original.modules.iter().any(|item| !ids.insert(&item.id))
            || original
                .modules
                .iter()
                .any(|old| !registry.installed.iter().any(|i| i.manifest.id == old.id))
        {
            return Err(Error::Conflict);
        }
        for item in &mut registry.installed {
            if item.incomplete() {
                return Err(Error::Incomplete);
            }
            if !item.compatible() {
                return Err(Error::Incompatible);
            }
            let enabled = original
                .modules
                .iter()
                .find(|old| old.id == item.manifest.id)
                .ok_or(Error::Conflict)?
                .enabled;
            item.enabled = enabled;
        }
        self.persist(registry).await?;
        self.update_snapshot()
    }
}
