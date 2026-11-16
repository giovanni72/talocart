//! Content integrity, compatibility and schema verification.
use super::*;

/// Verify the real executable before granting it database credentials or running migrations.
pub(super) async fn verify_descriptor(path: &Path, manifest: &Manifest) -> Result<()> {
    let mut described = crate::packages::build::describe(path).await?;
    // Envelope metadata belongs to the packager, not the executable's self-description.
    described.publisher = manifest.publisher.clone();
    described.size = manifest.size;
    described.sha256 = manifest.sha256.clone();
    if described != *manifest {
        return Err(Error::Incompatible);
    }
    Ok(())
}

pub(super) fn verify_release(release: &Release) -> Result<()> {
    release.manifest.validate_structure()?;
    let parsed: Manifest = serde_json::from_slice(&release.raw).map_err(|_| Error::Signature)?;
    if parsed != release.manifest {
        return Err(Error::Signature);
    }
    format::verify_manifest(&release.raw, &release.signature, &release.manifest)
}
/// Whether an applied ledger is an exact prefix (version and checksum) of the declared list.
pub(super) fn prefix(
    applied: &[(i64, Vec<u8>)],
    migrations: &[crate::packages::Migration],
) -> Result<bool> {
    if applied.len() > migrations.len() {
        return Ok(false);
    }
    for (actual, expected) in applied.iter().zip(migrations) {
        if actual.0 != expected.version || actual.1 != format::unhex(&expected.checksum, 48)? {
            return Ok(false);
        }
    }
    Ok(true)
}
pub(super) fn verify_executable(root: &Path, manifest: &Manifest) -> Result<PathBuf> {
    let path = executable(root, manifest);
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir == root {
            break;
        }
        let meta = fs::symlink_metadata(dir).map_err(|_| Error::Storage)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(Error::Storage);
        }
        ancestor = dir.parent();
    }
    let bytes = read_file(&path, format::LIMIT)?;
    if bytes.len() != manifest.size || format::hex(&Sha256::digest(&bytes)) != manifest.sha256 {
        return Err(Error::Signature);
    }
    Ok(path)
}
/// Coordinated updates require every installed executable intact, as before damage was tolerated.
pub(super) fn intact(registry: &Registry) -> Result<()> {
    if registry.installed.iter().any(Installed::damaged) {
        return Err(Error::Signature);
    }
    Ok(())
}
pub(super) fn conflict(a: &Manifest, b: &Manifest) -> bool {
    a.id == b.id
        || a.ledger == b.ledger
        || a.paths.iter().any(|p| {
            b.paths
                .iter()
                .any(|q| format::matches_path(p, q) || format::matches_path(q, p))
        })
}

impl Manager {
    pub(super) fn allowed(&self, manifest: &Manifest) -> Result<()> {
        if self.0.config.compiled.contains(&manifest.id)
            || self
                .inventory()?
                .iter()
                .any(|i| conflict(&i.manifest, manifest))
            || self.inventory()?.len() >= 32
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    /// Update rules against the installed release: strictly newer
    /// version, additive migrations and no conflict with the other installed modules.
    pub(super) fn upgradable(&self, installed: &Installed, manifest: &Manifest) -> Result<()> {
        if format::version(&manifest.version)? <= format::version(&installed.manifest.version)? {
            return Err(Error::Version);
        }
        if manifest.ledger != installed.manifest.ledger
            || self.0.config.compiled.contains(&manifest.id)
            || self
                .inventory()?
                .iter()
                .any(|i| i.manifest.id != manifest.id && conflict(&i.manifest, manifest))
        {
            return Err(Error::Conflict);
        }
        let installed_migrations: Vec<_> = installed
            .manifest
            .migrations
            .iter()
            .map(|m| Ok((m.version, format::unhex(&m.checksum, 48)?)))
            .collect::<Result<_>>()?;
        if !prefix(&installed_migrations, &manifest.migrations)? {
            return Err(Error::Schema);
        }
        Ok(())
    }
    /// The database ledger must be a prefix of the declared list: no unknown or changed
    /// migration is ever replaced, and an interrupted update can resume.
    pub(super) async fn ledger(
        &self,
        pool: &sqlx::PgPool,
        manifest: &Manifest,
    ) -> Result<Vec<(i64, Vec<u8>)>> {
        let applied = crate::database::applied_migrations(pool, &manifest.ledger)
            .await
            .map_err(|_| Error::Schema)?
            .unwrap_or_default();
        if !prefix(&applied, &manifest.migrations)? {
            return Err(Error::Schema);
        }
        Ok(applied)
    }
    /// Signed migration descriptors are sufficient for read-only diagnostics; no private SQL in the base.
    /// An interrupted update accepts any ledger between the installed and the intended release.
    pub async fn check_database(
        &self,
        pool: &sqlx::PgPool,
    ) -> std::result::Result<(), crate::database::DatabaseError> {
        use crate::database::DatabaseError;
        for item in self.inventory().map_err(|_| DatabaseError::Schema)? {
            let applied = crate::database::applied_migrations(pool, &item.manifest.ledger)
                .await?
                .ok_or(DatabaseError::Schema)?;
            let target = item.update.as_ref().map_or(&item.manifest, |u| &u.manifest);
            let installed = item.manifest.migrations.len();
            let valid = prefix(&applied, &target.migrations).map_err(|_| DatabaseError::Schema)?
                && if item.update.is_some() {
                    applied.len() >= installed
                } else {
                    applied.len() == installed
                };
            if !valid {
                return Err(DatabaseError::Schema);
            }
        }
        Ok(())
    }
}
