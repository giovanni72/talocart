//! Read-only planning against the target host's compiled compatibility contracts.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    Keep,
    Update,
    Blocked,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanBlocker {
    Incomplete,
    Incompatible,
    Publisher,
    Version,
    Schema,
    Conflict,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulePlan {
    pub id: String,
    pub from: String,
    pub to: String,
    pub enabled: bool,
    pub publisher: String,
    /// Hash of the target executable, including unchanged third-party modules.
    pub sha256: String,
    pub action: PlanAction,
    pub blocker: Option<PlanBlocker>,
    /// Hash of the complete .talopkg, absent when keeping an executable.
    pub package_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlan {
    pub format: u32,
    pub inventory_sha256: String,
    pub host_version: String,
    pub host_commit: String,
    pub sdk_version: String,
    pub protocol: u32,
    pub database: crate::database::SchemaContract,
    /// Inspection defaults to false; administrative CLI may bind actual read-only ledgers.
    pub database_checked: bool,
    pub database_sha256: Option<String>,
    pub modules: Vec<ModulePlan>,
    pub ignored: Vec<String>,
}
impl UpdatePlan {
    pub fn blocked(&self) -> bool {
        self.modules.iter().any(|m| m.action == PlanAction::Blocked)
    }
}
pub(super) fn fingerprint(registry: &Registry) -> Result<String> {
    Ok(format::hex(&Sha256::digest(
        serde_json::to_vec(registry).map_err(|_| Error::Storage)?,
    )))
}
fn blocker(error: Error) -> PlanBlocker {
    match error {
        Error::Publisher => PlanBlocker::Publisher,
        Error::Version => PlanBlocker::Version,
        Error::Schema => PlanBlocker::Schema,
        _ => PlanBlocker::Conflict,
    }
}
impl Manager {
    /// Bind real base/private ledgers to a plan without acquiring jobs or running code.
    /// Ordinary business rows may change while preparing; migration identities may not.
    pub async fn update_database_fingerprint(&self, pool: &sqlx::PgPool) -> Result<String> {
        let _op = self.0.operation.lock().await;
        let registry = self.registry()?;
        if registry.installed.iter().any(Installed::incomplete) {
            return Err(Error::Incomplete);
        }
        let mut names = vec![crate::database::MIGRATOR.table_name.to_string()];
        names.extend(registry.installed.iter().map(|i| i.manifest.ledger.clone()));
        let ledgers = crate::database::migration_snapshot(pool, &names)
            .await
            .map_err(|_| Error::Schema)?;
        let base = ledgers
            .iter()
            .find(|l| l.name == names[0])
            .ok_or(Error::Schema)?;
        if base.applied.is_empty()
            || base.applied.len() > crate::database::MIGRATOR.iter().count()
            || base
                .applied
                .iter()
                .zip(crate::database::MIGRATOR.iter())
                .any(|((v, c), expected)| {
                    *v != expected.version || c.as_slice() != expected.checksum.as_ref()
                })
        {
            return Err(Error::Schema);
        }
        for item in &registry.installed {
            let ledger = ledgers
                .iter()
                .find(|l| l.name == item.manifest.ledger)
                .ok_or(Error::Schema)?;
            if ledger.applied.len() != item.manifest.migrations.len()
                || !prefix(&ledger.applied, &item.manifest.migrations)?
            {
                return Err(Error::Schema);
            }
        }
        let root = self.0.config.root.clone();
        let actual = blocking(move || load(&root)).await?;
        if fingerprint(&actual)? != fingerprint(&registry)? {
            return Err(Error::Conflict);
        }
        let raw = serde_json::to_vec(&ledgers).map_err(|_| Error::Schema)?;
        Ok(format::hex(&Sha256::digest(&raw)))
    }

    /// Inspect only. No preview token, process, migration, directory creation or registry write.
    /// Run with the *target* host binary; every supplied package must verify for that host.
    pub async fn plan_update(&self, packages: Vec<PathBuf>) -> Result<UpdatePlan> {
        if packages.len() > 32 || !self.0.config.compiled.is_empty() {
            return Err(Error::Incompatible);
        }
        let _op = self.0.operation.lock().await;
        let root = self.0.config.root.clone();
        let initial = blocking(move || load(&root)).await?;
        intact(&initial)?;
        // upgradable() must see precisely the same inventory that was validated from disk.
        if fingerprint(&initial)? != fingerprint(&self.registry()?)? {
            return Err(Error::Conflict);
        }
        let supplied = blocking(move || {
            let mut verified = Vec::new();
            for path in packages {
                let bytes = read_file(&path, format::LIMIT + 32768 + 77)?;
                let package = format::inspect(&bytes)?;
                if verified
                    .iter()
                    .any(|(m, _): &(Manifest, String)| m.id == package.manifest.id)
                {
                    return Err(Error::Conflict);
                }
                verified.push((package.manifest, format::hex(&Sha256::digest(&bytes))));
            }
            Ok(verified)
        })
        .await?;
        let mut modules = Vec::new();
        for item in &initial.installed {
            let replacement = supplied.iter().find(|(m, _)| m.id == item.manifest.id);
            let (target, digest) = match replacement {
                Some((m, hash)) => (m, Some(hash.clone())),
                None => (&item.manifest, None),
            };
            let mut installed_contract = item.manifest.clone();
            // Repackaging the same executable without a legacy key is not a module upgrade.
            installed_contract.publisher = target.publisher.clone();
            let changed = replacement.is_some() && target != &installed_contract;
            let reason = if item.incomplete() {
                Some(PlanBlocker::Incomplete)
            } else if !target.compatible() {
                Some(PlanBlocker::Incompatible)
            } else if changed {
                self.upgradable(item, target).err().map(blocker)
            } else {
                None
            };
            modules.push(ModulePlan {
                id: item.manifest.id.clone(),
                from: item.manifest.version.clone(),
                to: target.version.clone(),
                enabled: item.enabled,
                publisher: target.publisher.clone(),
                sha256: target.sha256.clone(),
                action: if reason.is_some() {
                    PlanAction::Blocked
                } else if changed {
                    PlanAction::Update
                } else {
                    PlanAction::Keep
                },
                blocker: reason,
                package_sha256: if changed { digest } else { None },
            });
        }
        // Replacements may introduce conflicts with each other, not just with the old inventory.
        for a in 0..modules.len() {
            for b in 0..a {
                let final_manifest = |i: usize| {
                    supplied
                        .iter()
                        .find(|(m, _)| m.id == modules[i].id)
                        .map_or(&initial.installed[i].manifest, |(m, _)| m)
                };
                if conflict(final_manifest(a), final_manifest(b)) {
                    for i in [a, b] {
                        modules[i].action = PlanAction::Blocked;
                        modules[i].blocker = Some(PlanBlocker::Conflict);
                    }
                }
            }
        }
        modules.sort_by(|a, b| a.id.cmp(&b.id));
        let mut ignored: Vec<_> = supplied
            .iter()
            .filter(|(m, _)| !initial.installed.iter().any(|i| i.manifest.id == m.id))
            .map(|(m, _)| m.id.clone())
            .collect();
        ignored.sort();
        let root = self.0.config.root.clone();
        let final_state = blocking(move || load(&root)).await?;
        let inventory_sha256 = fingerprint(&initial)?;
        if inventory_sha256 != fingerprint(&final_state)? {
            return Err(Error::Conflict);
        }
        Ok(UpdatePlan {
            format: 1,
            inventory_sha256,
            host_version: crate::version::INFO.version.into(),
            host_commit: crate::version::INFO.commit.into(),
            database: crate::database::contract(),
            database_checked: false,
            database_sha256: None,
            sdk_version: crate::packages::sdk::VERSION.into(),
            protocol: protocol::VERSION,
            modules,
            ignored,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    struct Fixture {
        root: PathBuf,
        manager: Manager,
        pair: Ed25519KeyPair,
        registry: Registry,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    impl Fixture {
        async fn new(enabled: bool) -> Self {
            let root = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "talocart-plan-{}",
                    crate::identity::token().unwrap()
                ));
            let key = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
            let pair = Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
            let mut manifest = format::fixture_manifest();
            manifest.publisher = format::hex(pair.public_key().as_ref());
            manifest.size = format::fixture_payload().len();
            manifest.sha256 = format::hex(&Sha256::digest(format::fixture_payload()));
            let registry = Registry {
                publishers: vec![manifest.publisher.clone()],
                installed: vec![],
            };
            let config = RuntimeConfig {
                root: root.join("packages"),
                database_url: "postgres://unused/unused_test".into(),
                media_root: root.join("media"),
                store_name: "fixture".into(),
                public_origin: "http://127.0.0.1:3000".into(),
                secure_cookies: false,
                compiled: vec![],
                serve: false,
            };
            directory(&config.root).unwrap();
            let raw = serde_json::to_vec(&manifest).unwrap();
            let signature = pair
                .sign(&[b"talocart-package-v1\0".as_slice(), &raw].concat())
                .as_ref()
                .to_vec();
            let mut registry = registry;
            registry.installed.push(Installed {
                critical: true,
                manifest,
                enabled,
                raw,
                signature,
                previous: None,
                update: None,
                damaged: false,
            });
            let path = executable(&config.root, &registry.installed[0].manifest);
            directory(path.parent().unwrap()).unwrap();
            private_file(&path, &format::fixture_payload(), true).unwrap();
            save(&config.root, &registry).unwrap();
            let manager = Manager::open(config).await.unwrap();
            Self {
                root,
                manager,
                pair,
                registry,
            }
        }
        fn package(&self, version: &str) -> PathBuf {
            let mut manifest = self.registry.installed[0].manifest.clone();
            manifest.version = version.into();
            self.write_package(&manifest)
        }
        fn write_package(&self, manifest: &Manifest) -> PathBuf {
            let raw = serde_json::to_vec(manifest).unwrap();
            let signature = self
                .pair
                .sign(&[b"talocart-package-v1\0".as_slice(), &raw].concat());
            let data = [
                b"TALOPKG1\n".as_slice(),
                &(raw.len() as u32).to_be_bytes(),
                signature.as_ref(),
                &raw,
                &format::fixture_payload(),
            ]
            .concat();
            let path = self
                .root
                .join(format!("{}.talopkg", crate::identity::token().unwrap()));
            fs::write(&path, data).unwrap();
            path
        }
        async fn reload(&mut self) {
            save(&self.manager.0.config.root, &self.registry).unwrap();
            self.manager = Manager::open(self.manager.0.config.clone()).await.unwrap();
        }
    }
    #[tokio::test]
    async fn keeps_identical_executable_and_enabled_without_any_writes_or_database() {
        for enabled in [true, false] {
            let f = Fixture::new(enabled).await;
            let registry = f.manager.0.config.root.join("registry.json");
            let before = fs::read(&registry).unwrap();
            let plan = f.manager.plan_update(vec![]).await.unwrap();
            assert!(!plan.blocked());
            assert!(!plan.database_checked);
            assert_eq!(plan.modules[0].action, PlanAction::Keep);
            assert_eq!(plan.modules[0].enabled, enabled);
            assert_eq!(fs::read(&registry).unwrap(), before);
            assert!(!f.manager.0.config.root.join("host.lock").exists());
            let same = f
                .manager
                .plan_update(vec![f.package(&plan.modules[0].from)])
                .await
                .unwrap();
            assert_eq!(same.modules[0].action, PlanAction::Keep);
            assert_eq!(plan.inventory_sha256, same.inventory_sha256);
        }
    }
    #[tokio::test]
    async fn repacking_a_legacy_release_without_keys_preserves_identity_and_inventory() {
        let mut f = Fixture::new(true).await;
        f.registry.publishers.clear();
        f.reload().await;
        let registry = f.manager.0.config.root.join("registry.json");
        let before = fs::read(&registry).unwrap();
        let path = f.root.join("keyless.talopkg");
        fs::write(
            &path,
            format::pack(
                f.registry.installed[0].manifest.clone(),
                &format::fixture_payload(),
            )
            .unwrap(),
        )
        .unwrap();
        let plan = f.manager.plan_update(vec![path]).await.unwrap();
        assert_eq!(plan.modules[0].action, PlanAction::Keep);
        assert!(!plan.blocked());
        assert_eq!(fs::read(registry).unwrap(), before);
        assert!(f.manager.publishers().unwrap().is_empty());
    }
    #[tokio::test]
    async fn plans_signed_update_preserving_disabled_state_and_rejects_downgrade() {
        let f = Fixture::new(false).await;
        let package = f.package("99.0.0");
        let hash = format::hex(&Sha256::digest(fs::read(&package).unwrap()));
        let plan = f.manager.plan_update(vec![package]).await.unwrap();
        assert_eq!(plan.modules[0].action, PlanAction::Update);
        assert!(!plan.modules[0].enabled);
        assert_eq!(plan.modules[0].package_sha256.as_ref(), Some(&hash));
        let plan = f
            .manager
            .plan_update(vec![f.package("0.0.0")])
            .await
            .unwrap();
        assert_eq!(plan.modules[0].blocker, Some(PlanBlocker::Version));
    }
    #[tokio::test]
    async fn rejects_tampering_duplicate_catalogue_and_inconsistent_legacy_signatures() {
        let f = Fixture::new(true).await;
        let package = f.package("99.0.0");
        assert!(matches!(
            f.manager
                .plan_update(vec![package.clone(), package.clone()])
                .await,
            Err(Error::Conflict)
        ));
        let mut bytes = fs::read(&package).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&package, bytes).unwrap();
        assert!(matches!(
            f.manager.plan_update(vec![package]).await,
            Err(Error::Signature)
        ));
        let mut m = f.registry.installed[0].manifest.clone();
        m.publisher = "ff".repeat(32);
        assert!(
            f.manager
                .plan_update(vec![f.write_package(&m)])
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn alternate_publisher_is_allowed_but_changed_migration_prefix_is_blocked() {
        let mut f = Fixture::new(true).await;
        let mut m = f.registry.installed[0].manifest.clone();
        m.version = "99.0.0".into();
        m.migrations[0].checksum = "ff".repeat(48);
        let plan = f
            .manager
            .plan_update(vec![f.write_package(&m)])
            .await
            .unwrap();
        assert_eq!(plan.modules[0].blocker, Some(PlanBlocker::Schema));
        let key = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        f.pair = Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
        m = f.registry.installed[0].manifest.clone();
        m.version = "99.0.0".into();
        m.publisher = format::hex(f.pair.public_key().as_ref());
        f.registry.publishers.push(m.publisher.clone());
        f.reload().await;
        let plan = f
            .manager
            .plan_update(vec![f.write_package(&m)])
            .await
            .unwrap();
        assert_eq!(plan.modules[0].blocker, None);
        assert_eq!(plan.modules[0].action, PlanAction::Update);
    }
    #[tokio::test]
    async fn incomplete_or_incompatible_disabled_module_blocks_the_plan() {
        let mut f = Fixture::new(false).await;
        f.registry.installed[0].update = Some(f.registry.installed[0].release());
        f.reload().await;
        assert_eq!(
            f.manager.plan_update(vec![]).await.unwrap().modules[0].blocker,
            Some(PlanBlocker::Incomplete)
        );
        f.registry.installed[0].update = None;
        let m = &mut f.registry.installed[0];
        m.manifest.sdk_version = "99.0.0".into();
        m.raw = serde_json::to_vec(&m.manifest).unwrap();
        m.signature = f
            .pair
            .sign(&[b"talocart-package-v1\0".as_slice(), &m.raw].concat())
            .as_ref()
            .to_vec();
        f.reload().await;
        assert_eq!(
            f.manager.plan_update(vec![]).await.unwrap().modules[0].blocker,
            Some(PlanBlocker::Incompatible)
        );
    }
    #[tokio::test]
    async fn new_modules_are_ignored_and_inventory_changes_invalidate_the_plan() {
        let mut f = Fixture::new(true).await;
        let mut m = f.registry.installed[0].manifest.clone();
        m.id = "other".into();
        m.ledger = "_other_migrations".into();
        m.menu = "/admin/extensions/other".into();
        m.paths = vec![m.menu.clone()];
        let plan = f
            .manager
            .plan_update(vec![f.write_package(&m)])
            .await
            .unwrap();
        assert_eq!(plan.ignored, vec!["other"]);
        assert_eq!(plan.modules.len(), 1);
        f.registry.installed[0].enabled = false;
        save(&f.manager.0.config.root, &f.registry).unwrap();
        assert!(matches!(
            f.manager.plan_update(vec![]).await,
            Err(Error::Conflict)
        ));
        f.reload().await;
        let after = f.manager.plan_update(vec![]).await.unwrap();
        assert_ne!(after.inventory_sha256, plan.inventory_sha256);
    }
    #[tokio::test]
    async fn damaged_executable_is_flagged_without_blocking_the_host() {
        for modified in [false, true] {
            let mut f = Fixture::new(true).await;
            let mut other = f.registry.installed[0].clone();
            other.manifest.id = "other".into();
            other.manifest.ledger = "_other_migrations".into();
            other.manifest.menu = "/admin/extensions/other".into();
            other.manifest.paths = vec![other.manifest.menu.clone()];
            other.enabled = false;
            other.raw = serde_json::to_vec(&other.manifest).unwrap();
            other.signature = f
                .pair
                .sign(&[b"talocart-package-v1\0".as_slice(), &other.raw].concat())
                .as_ref()
                .to_vec();
            let root = f.manager.0.config.root.clone();
            let path = executable(&root, &other.manifest);
            directory(path.parent().unwrap()).unwrap();
            private_file(&path, &format::fixture_payload(), true).unwrap();
            f.registry.installed.push(other);
            f.reload().await;
            let damaged = executable(&root, &f.registry.installed[0].manifest);
            if modified {
                fs::write(&damaged, b"modified").unwrap();
            } else {
                fs::remove_file(&damaged).unwrap();
            }
            let mut config = f.manager.0.config.clone();
            config.serve = true;
            let manager = Manager::open(config).await.unwrap();
            let items = manager.inventory().unwrap();
            assert!(items[0].damaged() && !items[0].available());
            assert!(!items[1].damaged() && items[1].available());
            assert!(matches!(
                manager.plan_update(vec![]).await,
                Err(Error::Signature)
            ));
            // Disabling never runs the damaged executable; enabling it again is refused.
            manager.set_enabled("fixture", false).await.unwrap();
            assert!(matches!(
                manager.set_enabled("fixture", true).await,
                Err(Error::Signature)
            ));
            manager.uninstall("fixture").await.unwrap();
            let remaining = manager.inventory().unwrap();
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].manifest.id, "other");
            manager.plan_update(vec![]).await.unwrap();
            // Reinstalling replaces an unreferenced leftover file with the verified payload.
            directory(damaged.parent().unwrap()).unwrap();
            fs::write(&damaged, b"leftover").unwrap();
            let manifest = f.registry.installed[0].manifest.clone();
            manager
                .publish(&manifest, format::fixture_payload())
                .await
                .unwrap();
            verify_executable(&root, &manifest).unwrap();
        }
    }
    #[tokio::test]
    async fn tampered_registry_release_remains_fatal() {
        let mut f = Fixture::new(false).await;
        let root = f.manager.0.config.root.clone();
        let last = f.registry.installed[0].raw.len() - 2;
        f.registry.installed[0].raw[last] ^= 1;
        save(&root, &f.registry).unwrap();
        assert!(Manager::open(f.manager.0.config.clone()).await.is_err());
        f.registry.installed[0].raw[last] ^= 1;
        f.registry.installed[0].manifest.version = "1.0.1".into();
        save(&root, &f.registry).unwrap();
        assert!(matches!(
            Manager::open(f.manager.0.config.clone()).await,
            Err(Error::Signature)
        ));
    }
    #[tokio::test]
    async fn warm_update_rejects_damaged_disabled_executables_without_registry_changes() {
        for modified in [false, true] {
            let f = Fixture::new(false).await;
            let expected = f.manager.update_snapshot().unwrap().inventory_sha256;
            let mut options = f.manager.0.config.clone();
            options.serve = true;
            let manager = Manager::open(options).await.unwrap();
            let root = &manager.0.config.root;
            let before = fs::read(root.join("registry.json")).unwrap();
            let path = executable(root, &f.registry.installed[0].manifest);
            if modified {
                fs::write(&path, b"modified").unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            assert!(matches!(
                manager.warm_update(expected.clone()).await,
                Err(Error::Signature)
            ));
            assert_eq!(fs::read(root.join("registry.json")).unwrap(), before);
            // A repaired file passes without changing the inventory or starting a disabled child.
            fs::write(path, format::fixture_payload()).unwrap();
            assert!(manager.warm_update(expected).await.is_ok());
        }
    }
    #[tokio::test]
    async fn restore_activation_rejects_damaged_executables_without_registry_changes() {
        for modified in [false, true] {
            let mut f = Fixture::new(true).await;
            let original = f.manager.update_snapshot().unwrap();
            f.registry.installed[0].enabled = false;
            f.reload().await;
            let manager = Manager::open_offline(f.manager.0.config.clone())
                .await
                .unwrap();
            let root = &manager.0.config.root;
            let before = fs::read(root.join("registry.json")).unwrap();
            let path = executable(root, &f.registry.installed[0].manifest);
            if modified {
                fs::write(&path, b"modified").unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            assert!(matches!(
                manager.restore_activation_offline(original.clone()).await,
                Err(Error::Signature)
            ));
            assert_eq!(fs::read(root.join("registry.json")).unwrap(), before);
            assert!(!manager.inventory().unwrap()[0].enabled);
            fs::write(path, format::fixture_payload()).unwrap();
            let restored = manager.restore_activation_offline(original).await.unwrap();
            assert!(restored.modules[0].enabled);
        }
    }
}
