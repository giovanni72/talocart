//! Owner-authorized update status and atomic requests to a separate OS updater.
//! This service never downloads executables, invokes sudo or stops the host.
use crate::identity::{Identity, IdentityError, Session};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::{fs, io::AsyncReadExt, sync::Semaphore};

#[derive(Clone)]
pub struct Updates {
    root: Option<PathBuf>,
    access: Arc<Semaphore>,
}
impl Default for Updates {
    fn default() -> Self {
        Self::new(None)
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub version: String,
    pub commit: String,
    pub digest: String,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Current,
    Available,
    Installing,
    Succeeded,
    Failed,
    CheckFailed,
    Blocked,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Keep,
    Update,
    Blocked,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocker {
    Incomplete,
    Incompatible,
    Publisher,
    Version,
    Schema,
    Conflict,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Prepared,
    UpdaterReady,
    Quiescing,
    Fenced,
    BackedUp,
    Applying,
    Verifying,
    Resuming,
    Succeeded,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    PlanChanged,
    PreparationFailed,
    EngineFailed,
    ControlBusy,
    ControlConflict,
    ControlConfiguration,
    ControlUnavailable,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulePlan {
    pub id: String,
    #[serde(rename = "from")]
    pub current: String,
    pub to: String,
    pub enabled: bool,
    pub action: Action,
    pub blocker: Option<Blocker>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub release: String,
    pub sequence: u64,
    pub base_from: String,
    pub base_to: String,
    pub base_action: Action,
    pub blocked: bool,
    pub modules: Vec<ModulePlan>,
}
#[derive(Deserialize)]
struct Status {
    format: u32,
    phase: Phase,
    checked_at: String,
    candidate: Option<Candidate>,
    plan: Option<Plan>,
    stage: Option<Stage>,
    #[serde(default)]
    recovery_required: bool,
    backup_id: Option<String>,
    notice: Option<Notice>,
}
pub struct Snapshot {
    pub configured: bool,
    pub phase: Option<Phase>,
    pub checked_at: String,
    pub candidate: Option<Candidate>,
    pub queued: bool,
    pub check_queued: bool,
    pub plan: Option<Plan>,
    pub stage: Option<Stage>,
    pub recovery_required: bool,
    pub backup_id: Option<String>,
    pub notice: Option<Notice>,
}
impl Snapshot {
    /// A manual check only asks the root updater to skip its hourly cache; it never
    /// competes with an install request, a running attempt or pending recovery.
    pub fn can_check(&self) -> bool {
        self.configured
            && !self.queued
            && !self.check_queued
            && !self.recovery_required
            && self.phase != Some(Phase::Installing)
    }
    pub fn can_request(&self) -> bool {
        !self.queued
            && !self.recovery_required
            && self.candidate.is_some()
            && (self.phase == Some(Phase::Available)
                || self.phase == Some(Phase::Failed)
                    && self.plan.as_ref().is_some_and(|p| !p.blocked))
    }
}
impl Status {
    fn valid(&self) -> bool {
        if self.checked_at.len() > 64 || self.candidate.as_ref().is_some_and(|c| !valid(c)) {
            return false;
        }
        if self.format == 1 {
            return self.plan.is_none()
                && self.stage.is_none()
                && !self.recovery_required
                && self.backup_id.is_none()
                && self.notice.is_none()
                && self.phase != Phase::Blocked;
        }
        if self.format != 2 || self.backup_id.as_ref().is_some_and(|v| !hex(v, 32)) {
            return false;
        }
        let Some(plan) = &self.plan else {
            return self.candidate.is_none()
                && matches!(self.phase, Phase::Failed | Phase::CheckFailed);
        };
        let Some(release) = plan.release.strip_prefix('v') else {
            return false;
        };
        if !version(release)
            || plan.sequence == 0
            || !version(&plan.base_from)
            || !version(&plan.base_to)
            || plan.base_action == Action::Blocked
            || plan.modules.len() > 32
            || self
                .candidate
                .as_ref()
                .is_some_and(|c| c.version != release)
            || self.phase == Phase::Available && (plan.blocked || self.candidate.is_none())
            || self.phase == Phase::Blocked && !plan.blocked
        {
            return false;
        }
        if (plan.base_action == Action::Keep) != (plan.base_from == plan.base_to) {
            return false;
        }
        let mut ids = std::collections::BTreeSet::new();
        plan.modules.iter().all(|m| {
            let mut bytes = m.id.bytes();
            let first = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
            first
                && m.id.len() <= 48
                && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && ids.insert(&m.id)
                && version(&m.current)
                && version(&m.to)
                && (m.action == Action::Blocked) == m.blocker.is_some()
                && (m.action != Action::Blocked || plan.blocked)
        })
    }
}
impl Updates {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            access: Arc::new(Semaphore::new(2)),
        }
    }
    pub async fn read(
        &self,
        identity: &Identity,
        session: &Session,
    ) -> Result<Snapshot, IdentityError> {
        identity.authorize_owner(session).await?;
        let _permit = self.access.try_acquire().map_err(|_| IdentityError::Busy)?;
        self.snapshot().await
    }
    async fn snapshot(&self) -> Result<Snapshot, IdentityError> {
        let mut snapshot = Snapshot {
            configured: self.root.is_some(),
            phase: None,
            checked_at: String::new(),
            candidate: None,
            queued: false,
            check_queued: false,
            plan: None,
            stage: None,
            recovery_required: false,
            backup_id: None,
            notice: None,
        };
        let Some(root) = &self.root else {
            return Ok(snapshot);
        };
        let path = root.join("status.json");
        match fs::symlink_metadata(&path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
            Ok(info) if info.is_file() && info.len() <= 65536 => {}
            _ => return Err(IdentityError::Unavailable),
        }
        let mut data = Vec::new();
        fs::File::open(path)
            .await
            .map_err(storage)?
            .take(65537)
            .read_to_end(&mut data)
            .await
            .map_err(storage)?;
        if data.len() > 65536 {
            return Err(IdentityError::Unavailable);
        }
        let status: Status =
            serde_json::from_slice(&data).map_err(|_| IdentityError::Unavailable)?;
        if !status.valid() {
            return Err(IdentityError::Unavailable);
        }
        snapshot.phase = Some(status.phase);
        snapshot.checked_at = status.checked_at;
        snapshot.candidate = status.candidate;
        snapshot.plan = status.plan;
        snapshot.stage = status.stage;
        snapshot.recovery_required = status.recovery_required;
        snapshot.backup_id = status.backup_id;
        snapshot.notice = status.notice;
        snapshot.queued = fs::try_exists(root.join("requests/install.json"))
            .await
            .map_err(storage)?;
        snapshot.check_queued = fs::try_exists(root.join("requests/check.json"))
            .await
            .map_err(storage)?;
        Ok(snapshot)
    }
    /// Publishes an empty marker; the updater consumes it without reading content
    /// and enforces its own minimum interval between network checks.
    pub async fn request_check(
        &self,
        identity: &Identity,
        session: &Session,
    ) -> Result<(), IdentityError> {
        identity.authorize_owner(session).await?;
        let _permit = self.access.try_acquire().map_err(|_| IdentityError::Busy)?;
        let status = self.snapshot().await?;
        if status.check_queued {
            return Ok(());
        }
        if !status.can_check() {
            return Err(IdentityError::Invalid);
        }
        let root = self.requests().await?;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(root.join("check.json")).await {
            Ok(file) => file.sync_all().await.map_err(storage)?,
            // Another owner click already queued the same check.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(error) => return Err(storage(error)),
        }
        fs::File::open(&root)
            .await
            .map_err(storage)?
            .sync_all()
            .await
            .map_err(storage)
    }
    async fn requests(&self) -> Result<PathBuf, IdentityError> {
        let root = self
            .root
            .as_ref()
            .ok_or(IdentityError::Unavailable)?
            .join("requests");
        if !fs::symlink_metadata(&root).await.map_err(storage)?.is_dir() {
            return Err(IdentityError::Unavailable);
        }
        Ok(root)
    }
    pub async fn request(
        &self,
        identity: &Identity,
        session: &Session,
        candidate: Candidate,
    ) -> Result<(), IdentityError> {
        identity.authorize_owner(session).await?;
        if !valid(&candidate) {
            return Err(IdentityError::Invalid);
        }
        let _permit = self.access.try_acquire().map_err(|_| IdentityError::Busy)?;
        let status = self.snapshot().await?;
        if status.queued {
            return Err(IdentityError::Busy);
        }
        if !status.can_request() || status.candidate.as_ref() != Some(&candidate) {
            return Err(IdentityError::Invalid);
        }
        let root = self.requests().await?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| IdentityError::Unavailable)?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let temporary = root.join(format!(".{name}.pending"));
        let bytes = serde_json::to_vec(&candidate).map_err(|_| IdentityError::Unavailable)?;
        let outcome = async {
            use tokio::io::AsyncWriteExt;
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary).await.map_err(storage)?;
            file.write_all(&bytes).await.map_err(storage)?;
            file.sync_all().await.map_err(storage)?;
            drop(file);
            // Publish a complete request without overwriting another owner's click.
            fs::hard_link(&temporary, root.join("install.json"))
                .await
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        IdentityError::Busy
                    } else {
                        storage(error)
                    }
                })?;
            fs::File::open(&root)
                .await
                .map_err(storage)?
                .sync_all()
                .await
                .map_err(storage)
        }
        .await;
        let _ = fs::remove_file(temporary).await;
        outcome
    }
}
fn storage(_: std::io::Error) -> IdentityError {
    IdentityError::Unavailable
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
                && part.parse::<u64>().is_ok()
        })
}
fn valid(candidate: &Candidate) -> bool {
    version(&candidate.version) && hex(&candidate.commit, 40) && hex(&candidate.digest, 64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unified_status_validates_public_contract_and_rejects_unknown_operations() {
        let value = serde_json::json!({"format":2,"phase":"available","checked_at":"2026-11-01T12:00:00+00:00",
            "candidate":{"version":"11.12.0","commit":"a".repeat(40),"digest":"b".repeat(64)},
            "plan":{"release":"v11.12.0","sequence":1,"base_from":"11.11.0","base_to":"11.12.0","base_action":"update","blocked":false,
                "modules":[{"id":"koala-import","from":"1.0.0","to":"1.0.0","enabled":false,"action":"keep","blocker":null}]},
            "recovery_required":false});
        let parse = |v: serde_json::Value| serde_json::from_value::<Status>(v);
        assert!(parse(value.clone()).unwrap().valid());
        for invalid in [
            {
                let mut v = value.clone();
                v["plan"]["modules"][0]["id"] = "../koala".into();
                v
            },
            {
                let mut v = value.clone();
                v["plan"]["modules"][0]["blocker"] = "incomplete".into();
                v
            },
            {
                let mut v = value.clone();
                v["plan"]["blocked"] = true.into();
                v
            },
            {
                let mut v = value.clone();
                let duplicate = v["plan"]["modules"][0].clone();
                v["plan"]["modules"].as_array_mut().unwrap().push(duplicate);
                v
            },
            {
                let mut v = value.clone();
                v["candidate"]["version"] = "12.0.0".into();
                v
            },
            {
                let mut v = value.clone();
                v["backup_id"] = "/private/backups".into();
                v
            },
        ] {
            assert!(!parse(invalid).unwrap().valid());
        }
        let mut injected = value;
        injected["plan"]["modules"][0]["action"] = "run-command".into();
        assert!(parse(injected).is_err());
    }

    #[test]
    fn manual_check_never_competes_with_installs_or_recovery() {
        let idle = || Snapshot {
            configured: true,
            phase: Some(Phase::Current),
            checked_at: String::new(),
            candidate: None,
            queued: false,
            check_queued: false,
            plan: None,
            stage: None,
            recovery_required: false,
            backup_id: None,
            notice: None,
        };
        assert!(idle().can_check());
        assert!(
            Snapshot {
                phase: None,
                ..idle()
            }
            .can_check()
        );
        assert!(
            Snapshot {
                phase: Some(Phase::CheckFailed),
                ..idle()
            }
            .can_check()
        );
        for blocked in [
            Snapshot {
                configured: false,
                ..idle()
            },
            Snapshot {
                queued: true,
                ..idle()
            },
            Snapshot {
                check_queued: true,
                ..idle()
            },
            Snapshot {
                recovery_required: true,
                ..idle()
            },
            Snapshot {
                phase: Some(Phase::Installing),
                ..idle()
            },
        ] {
            assert!(!blocked.can_check());
        }
    }

    #[test]
    fn rejects_ambiguous_or_injected_release_requests() {
        let mut c = Candidate {
            version: "1.2.3".into(),
            commit: "a".repeat(40),
            digest: "b".repeat(64),
        };
        assert!(valid(&c));
        for version in ["01.2.3", "1.2", "../1.2.3", "1.2.3-rc1", "1.2.+3"] {
            c.version = version.into();
            assert!(!valid(&c), "{version}");
        }
    }
}
