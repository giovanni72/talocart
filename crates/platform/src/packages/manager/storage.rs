//! Private filesystem persistence and atomic publication.
use super::*;

pub(super) fn directory(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => Err(Error::Storage),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(path)
                    .map_err(|_| Error::Storage)?;
            }
            #[cfg(not(unix))]
            {
                return Err(Error::Incompatible);
            }
            Ok(())
        }
        Err(_) => Err(Error::Storage),
    }
}
pub(super) fn private_file(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    let mut file = options.open(path).map_err(|_| Error::Storage)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::Storage)
}
/// Publish only a complete, synced executable, without overwriting any installed version.
/// The injected writer lets tests exercise a real partial I/O failure before publication.
pub(super) fn publish_executable_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<()> {
    let parent = path.parent().ok_or(Error::Storage)?;
    let temporary = parent.join(format!(
        ".module-{}.tmp",
        crate::identity::token().map_err(|_| Error::Storage)?
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(&temporary).map_err(|_| Error::Storage)?;
    let result = (|| {
        write(&mut file)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Storage)?;
        // Same-directory hard link publishes atomically and refuses an existing destination.
        fs::hard_link(&temporary, path).map_err(|_| Error::Storage)?;
        Ok(())
    })();
    drop(file);
    let cleanup = fs::remove_file(&temporary).map_err(|_| Error::Storage);
    result?;
    cleanup?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| Error::Storage)
}
pub(super) fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|_| Error::Storage)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit as u64 {
        return Err(Error::Storage);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| Error::Storage)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > limit {
        return Err(Error::Storage);
    }
    Ok(bytes)
}
pub(super) fn load(root: &Path) -> Result<Registry> {
    if !root.exists() {
        return Ok(Registry::default());
    }
    // Validate every ancestor without creating directories during a read.
    let mut cursor = Some(root);
    while let Some(path) = cursor {
        let meta = fs::symlink_metadata(path).map_err(|_| Error::Storage)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(Error::Storage);
        }
        cursor = path.parent();
    }
    let path = root.join("registry.json");
    if !path.exists() {
        return Ok(Registry::default());
    }
    let mut registry: Registry =
        serde_json::from_slice(&read_file(&path, 16 * 1024 * 1024)?).map_err(|_| Error::Storage)?;
    if registry.installed.len() > 32 {
        return Err(Error::Storage);
    }
    for item in &mut registry.installed {
        // Another SDK is not an integrity failure: such packages are kept but not executed.
        let current = item.release();
        verify_release(&current)?;
        for release in [&item.previous, &item.update].into_iter().flatten() {
            verify_release(release)?;
            if release.manifest.id != item.manifest.id {
                return Err(Error::Signature);
            }
        }
        // A bad executable only marks its entry, so the host can still start and the
        // operator can disable, remove or replace it. Every execution verifies it again.
        // A previous executable is verified when used; an interrupted update needs its own now.
        item.damaged = verify_executable(root, &item.manifest).is_err()
            || item
                .update
                .as_ref()
                .is_some_and(|update| verify_executable(root, &update.manifest).is_err());
    }
    for (i, item) in registry.installed.iter().enumerate() {
        if registry.installed[..i]
            .iter()
            .any(|other| conflict(&other.manifest, &item.manifest))
        {
            return Err(Error::Conflict);
        }
    }
    Ok(registry)
}
pub(super) fn executable(root: &Path, manifest: &Manifest) -> PathBuf {
    root.join("installed")
        .join(&manifest.id)
        .join(&manifest.version)
        .join("module")
}
/// Remove executables of versions other than `keep`; module data live elsewhere.
pub(super) fn prune(root: &Path, id: &str, keep: &[&str]) -> Result<()> {
    let folder = root.join("installed").join(id);
    for entry in fs::read_dir(&folder).map_err(|_| Error::Storage)? {
        let entry = entry.map_err(|_| Error::Storage)?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| Error::Storage)?;
        if meta.is_dir()
            && !meta.file_type().is_symlink()
            && !keep
                .iter()
                .any(|v| entry.file_name() == std::ffi::OsStr::new(v))
        {
            fs::remove_dir_all(entry.path()).map_err(|_| Error::Storage)?;
        }
    }
    File::open(&folder)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Storage)
}
pub(super) fn save(root: &Path, registry: &Registry) -> Result<()> {
    let path = root.join(format!(
        "registry-{}.tmp",
        crate::identity::token().map_err(|_| Error::Storage)?
    ));
    private_file(
        &path,
        &serde_json::to_vec(registry).map_err(|_| Error::Storage)?,
        false,
    )?;
    fs::rename(&path, root.join("registry.json")).map_err(|_| Error::Storage)?;
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Storage)
}
pub(super) fn location(path: &Path) -> Result<PathBuf> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::Storage);
    }
    let mut cursor = path;
    let mut tail = Vec::new();
    while !cursor.exists() {
        tail.push(cursor.file_name().ok_or(Error::Storage)?.to_owned());
        cursor = cursor.parent().ok_or(Error::Storage)?;
    }
    let mut resolved = fs::canonicalize(cursor).map_err(|_| Error::Storage)?;
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

impl Manager {
    pub fn inventory(&self) -> Result<Vec<Installed>> {
        Ok(self
            .0
            .registry
            .read()
            .map_err(|_| Error::Storage)?
            .installed
            .clone())
    }
    pub fn publishers(&self) -> Result<Vec<String>> {
        Ok(self
            .0
            .registry
            .read()
            .map_err(|_| Error::Storage)?
            .publishers
            .clone())
    }
    pub(super) fn registry(&self) -> Result<Registry> {
        Ok(self.0.registry.read().map_err(|_| Error::Storage)?.clone())
    }
    pub(super) async fn lock_disk(&self) -> Result<File> {
        if !self.0.config.serve {
            return Err(Error::Conflict);
        }
        let root = self.0.config.root.clone();
        blocking(move || {
            directory(&root)?;
            let path = root.join(".lock");
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
        .await
    }
    pub(super) async fn persist(&self, registry: Registry) -> Result<()> {
        let root = self.0.config.root.clone();
        let copy = registry.clone();
        blocking(move || save(&root, &copy)).await?;
        *self.0.registry.write().map_err(|_| Error::Storage)? = registry;
        Ok(())
    }
    /// Publish the executable for `manifest` unless the identical file already exists.
    /// A different file is replaced only when no registry release references that version
    /// (left by an attempt refused before its intent was recorded).
    pub(super) async fn publish(&self, manifest: &Manifest, payload: Vec<u8>) -> Result<PathBuf> {
        let root = self.0.config.root.clone();
        let referenced = self.inventory()?.iter().any(|i| {
            i.manifest.id == manifest.id
                && [
                    Some(&i.manifest),
                    i.previous.as_ref().map(|r| &r.manifest),
                    i.update.as_ref().map(|r| &r.manifest),
                ]
                .into_iter()
                .flatten()
                .any(|m| m.version == manifest.version)
        });
        let manifest = manifest.clone();
        blocking(move || {
            let destination = executable(&root, &manifest);
            let dir = destination.parent().ok_or(Error::Storage)?;
            directory(dir)?;
            if destination.exists() && read_file(&destination, format::LIMIT)? != payload {
                if referenced {
                    return Err(Error::Signature);
                }
                fs::remove_file(&destination).map_err(|_| Error::Storage)?;
            }
            if !destination.exists() {
                publish_executable_with(&destination, |file| file.write_all(&payload))?;
            }
            directory(&root.join("data").join(&manifest.id))?;
            Ok(destination)
        })
        .await
    }
    pub(super) async fn prune_logged(&self, id: &str, keep: &[&str]) {
        let root = self.0.config.root.clone();
        let id = id.to_owned();
        let keep: Vec<String> = keep.iter().map(|v| (*v).to_owned()).collect();
        let result = blocking(move || {
            prune(
                &root,
                &id,
                &keep.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        })
        .await;
        if result.is_err() {
            // Stale executables are harmless: never referenced by the registry.
            tracing::warn!("Failed to remove unused package executables");
        }
    }
}
