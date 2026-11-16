//! Immutable publication shared by uploads, imports and generated representations.
use super::*;

impl MediaStore {
    /// Write and sync a sibling temporary file before linking the complete bytes into place.
    /// Callers hold the media publication lock through database registration. Existing targets
    /// are never followed or overwritten; numbered collisions preserve human-readable names.
    pub(crate) async fn publish_bytes(
        &self,
        key: &str,
        bytes: &[u8],
        numbered: bool,
    ) -> Result<String, MediaError> {
        if bytes.is_empty() || bytes.len() > STORED_LIMIT {
            return Err(MediaError::Invalid);
        }
        let path = self.object_path(key)?;
        self.check_parents(&path).await?;
        let directory = path.parent().ok_or(MediaError::Storage)?;
        tokio::fs::create_dir_all(directory).await?;
        self.check_parents(&path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(self.root.as_path(), std::fs::Permissions::from_mode(0o700))
                .await?;
        }
        let token = crate::identity::token().map_err(|_| MediaError::Storage)?;
        let temporary = directory.join(format!(".upload-{token}.pending"));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        let result = async {
            file.write_all(bytes).await?;
            file.sync_all().await?;
            drop(file);
            let (stem, extension) = key.rsplit_once('.').ok_or(MediaError::Invalid)?;
            for number in 1..=if numbered { 10000 } else { 1 } {
                let candidate = if number == 1 {
                    key.to_owned()
                } else {
                    format!("{stem}-{number}.{extension}")
                };
                let target = self.object_path(&candidate)?;
                self.check_parents(&target).await?;
                match tokio::fs::hard_link(&temporary, target).await {
                    Ok(()) => {
                        tokio::fs::File::open(directory).await?.sync_all().await?;
                        return Ok(candidate);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            Err(MediaError::Storage)
        }
        .await;
        // Cancellation may leave a recognizable pending file, reclaimed after the normal grace.
        let cleanup = tokio::fs::remove_file(&temporary).await;
        match result {
            Ok(key) => {
                cleanup?;
                tokio::fs::File::open(directory).await?.sync_all().await?;
                Ok(key)
            }
            Err(error) => Err(error),
        }
    }
}
