//! Persisted package policy and preview state.
use super::*;

#[derive(Clone)]
pub struct RuntimeConfig {
    pub root: PathBuf,
    pub database_url: String,
    pub media_root: PathBuf,
    pub store_name: String,
    pub public_origin: String,
    pub secure_cookies: bool,
    /// IDs present in the statically composed executable cannot also be installed.
    pub compiled: Vec<String>,
    pub serve: bool,
}
/// A manifest kept verbatim; legacy signatures remain verifiable after a restart.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    pub(super) manifest: Manifest,
    pub(super) raw: Vec<u8>,
    pub(super) signature: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub manifest: Manifest,
    pub enabled: bool,
    /// Whether an enabled module is required for ordinary store readiness.
    #[serde(
        default = "critical_default",
        skip_serializing_if = "is_critical_default"
    )]
    pub critical: bool,
    pub(super) raw: Vec<u8>,
    pub(super) signature: Vec<u8>,
    /// Release replaced by the last completed update, kept for an executable rollback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) previous: Option<Release>,
    /// Write-ahead update intent: persisted before module migrations run.
    /// While present the module is not served and its ledger may lie between both releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) update: Option<Release>,
    /// Runtime only, never persisted: an executable this entry needs is missing or
    /// differs from its manifest. Recomputed whenever the registry is loaded.
    #[serde(skip)]
    pub(super) damaged: bool,
}
impl Installed {
    /// Listed and removable, never executed; every execution verifies the file again.
    pub fn damaged(&self) -> bool {
        self.damaged
    }
    /// Built for another base SDK or host: listed and updatable, never executed for serving.
    pub fn compatible(&self) -> bool {
        self.manifest.compatible()
    }
    /// An update stopped after its intent was recorded; only retry or a newer package proceed.
    pub fn incomplete(&self) -> bool {
        self.update.is_some()
    }
    pub fn available(&self) -> bool {
        self.compatible() && !self.incomplete() && !self.damaged
    }
    pub fn pending_version(&self) -> Option<&str> {
        self.update.as_ref().map(|r| r.manifest.version.as_str())
    }
    pub fn previous_version(&self) -> Option<&str> {
        self.previous.as_ref().map(|r| r.manifest.version.as_str())
    }
    pub(super) fn release(&self) -> Release {
        Release {
            manifest: self.manifest.clone(),
            raw: self.raw.clone(),
            signature: self.signature.clone(),
        }
    }
    pub(super) fn replace(&mut self, release: Release) {
        self.manifest = release.manifest;
        self.raw = release.raw;
        self.signature = release.signature;
        // Callers verify the replacing executable before switching to it.
        self.damaged = false;
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registry {
    /// Read legacy registries without making their keys an installation requirement.
    #[serde(default)]
    pub(super) publishers: Vec<String>,
    pub(super) installed: Vec<Installed>,
}
#[derive(Clone)]
pub struct Preview {
    pub token: String,
    pub manifest: Manifest,
    /// Installed manifest when the package updates an existing ID.
    pub current: Option<Manifest>,
}
/// Differences shown before an update is confirmed.
pub struct Changes {
    pub from: String,
    pub migrations: usize,
    pub added_permissions: Vec<String>,
    pub removed_permissions: Vec<String>,
    pub added_paths: Vec<String>,
    pub removed_paths: Vec<String>,
}
impl Preview {
    pub fn changes(&self) -> Option<Changes> {
        let current = self.current.as_ref()?;
        let missing = |a: &[String], b: &[String]| {
            a.iter()
                .filter(|x| !b.contains(x))
                .cloned()
                .collect::<Vec<_>>()
        };
        Some(Changes {
            from: current.version.clone(),
            migrations: self
                .manifest
                .migrations
                .len()
                .saturating_sub(current.migrations.len()),
            added_permissions: missing(&self.manifest.permissions, &current.permissions),
            removed_permissions: missing(&current.permissions, &self.manifest.permissions),
            added_paths: missing(&self.manifest.paths, &current.paths),
            removed_paths: missing(&current.paths, &self.manifest.paths),
        })
    }
}
pub(super) struct Pending {
    pub(super) owner: String,
    /// Verified at preview creation; immutable together with the signed bytes.
    pub(super) module_id: String,
    pub(super) expires: std::time::Instant,
    pub(super) bytes: Vec<u8>,
}

fn critical_default() -> bool {
    true
}
fn is_critical_default(value: &bool) -> bool {
    *value
}
