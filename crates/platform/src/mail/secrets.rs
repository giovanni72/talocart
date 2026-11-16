//! Private key outside PostgreSQL and the public media tree; never serialized or Debug.
use super::{MailError, Result};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct MailKey(LessSafeKey);
const AAD: &[u8] = b"talocart:smtp-password:v1";
impl MailKey {
    /// Deterministic sibling of media, included by offline backup/restore.
    pub fn directory(media: &Path) -> PathBuf {
        let mut name = media.file_name().unwrap_or_default().to_os_string();
        name.push(".secrets");
        media.with_file_name(name)
    }
    pub async fn load(directory: PathBuf, allow_create: bool) -> Result<Arc<Self>> {
        tokio::task::spawn_blocking(move || Self::load_sync(&directory, allow_create))
            .await
            .map_err(|_| MailError::Key)?
            .map(Arc::new)
    }
    fn load_sync(directory: &Path, allow_create: bool) -> Result<Self> {
        // Resolve the parent (e.g. macOS /var), but refuse a symlink key directory.
        let parent = directory.parent().ok_or(MailError::Key)?;
        if allow_create && !parent.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(parent).map_err(|_| MailError::Key)?;
        }
        let parent = fs::canonicalize(parent).map_err(|_| MailError::Key)?;
        let resolved = parent.join(directory.file_name().ok_or(MailError::Key)?);
        let directory = resolved.as_path();
        if fs::symlink_metadata(directory).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(MailError::Key);
        }
        if !directory.exists() && allow_create {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(directory).map_err(|_| MailError::Key)?;
        }
        let meta = fs::symlink_metadata(directory).map_err(|_| MailError::Key)?;
        if !meta.is_dir() {
            return Err(MailError::Key);
        }
        #[cfg(unix)]
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(MailError::Key);
        }
        let path = directory.join("smtp.key");
        if !path.exists() && allow_create {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|_| MailError::Key)?;
            let temp = directory.join(format!(".smtp-{}", super::token()?));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let result = (|| {
                let mut file = options.open(&temp).map_err(|_| MailError::Key)?;
                file.write_all(&bytes).map_err(|_| MailError::Key)?;
                file.sync_all().map_err(|_| MailError::Key)?;
                // Publish complete bytes without replacing an existing/concurrent key.
                match fs::hard_link(&temp, &path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(MailError::Key),
                }
                fs::File::open(directory)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| MailError::Key)
            })();
            let _ = fs::remove_file(temp);
            result?;
        }
        let meta = fs::symlink_metadata(&path).map_err(|_| MailError::Key)?;
        if !meta.is_file() || meta.len() != 32 {
            return Err(MailError::Key);
        }
        #[cfg(unix)]
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(MailError::Key);
        }
        Self::from_bytes(&fs::read(path).map_err(|_| MailError::Key)?)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(LessSafeKey::new(
            UnboundKey::new(&CHACHA20_POLY1305, bytes).map_err(|_| MailError::Key)?,
        )))
    }
    pub(super) fn encrypt(&self, value: &str) -> Result<Vec<u8>> {
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|_| MailError::Key)?;
        let mut sealed = value.as_bytes().to_vec();
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(AAD),
                &mut sealed,
            )
            .map_err(|_| MailError::Key)?;
        let mut result = vec![1];
        result.extend(nonce);
        result.extend(sealed);
        Ok(result)
    }
    pub(super) fn decrypt(&self, bytes: &[u8]) -> Result<String> {
        if bytes.len() < 30 || bytes[0] != 1 {
            return Err(MailError::Key);
        }
        let nonce: [u8; 12] = bytes[1..13].try_into().map_err(|_| MailError::Key)?;
        let mut sealed = bytes[13..].to_vec();
        let plain = self
            .0
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(AAD),
                &mut sealed,
            )
            .map_err(|_| MailError::Key)?;
        String::from_utf8(plain.to_vec()).map_err(|_| MailError::Key)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encryption_authenticates_key_nonce_and_content() {
        let key = MailKey::from_bytes(&[7; 32]).unwrap();
        let sealed = key.encrypt("synthetic-password").unwrap();
        assert_eq!(key.decrypt(&sealed).unwrap(), "synthetic-password");
        assert_ne!(sealed, key.encrypt("synthetic-password").unwrap());
        assert!(!sealed.windows(18).any(|w| w == b"synthetic-password"));
        assert!(
            MailKey::from_bytes(&[8; 32])
                .unwrap()
                .decrypt(&sealed)
                .is_err()
        );
        for index in [0, 1, 13, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[index] ^= 1;
            assert!(key.decrypt(&bad).is_err());
        }
        assert!(key.decrypt(&sealed[..15]).is_err());
    }
    #[tokio::test]
    async fn key_persists_and_is_never_replaced_when_lost() {
        let root = std::env::temp_dir().join(format!(
            "talocart-mail-key-{}",
            super::super::token().unwrap()
        ));
        let key = MailKey::load(root.clone(), true).await.unwrap();
        let sealed = key.encrypt("secret").unwrap();
        assert_eq!(
            MailKey::load(root.clone(), false)
                .await
                .unwrap()
                .decrypt(&sealed)
                .unwrap(),
            "secret"
        );
        fs::remove_file(root.join("smtp.key")).unwrap();
        assert!(MailKey::load(root.clone(), false).await.is_err());
        assert!(!root.join("smtp.key").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn concurrent_key_initialization_and_unsafe_files() {
        let root = std::env::temp_dir().join(format!(
            "talocart-mail-race-{}",
            super::super::token().unwrap()
        ));
        let (a, b) = tokio::join!(
            MailKey::load(root.clone(), true),
            MailKey::load(root.clone(), true)
        );
        let a = a.unwrap();
        let b = b.unwrap();
        let sealed = a.encrypt("race-secret").unwrap();
        assert_eq!(b.decrypt(&sealed).unwrap(), "race-secret");
        fs::write(root.join("smtp.key"), b"broken").unwrap();
        assert!(MailKey::load(root.clone(), true).await.is_err());
        fs::write(root.join("smtp.key"), [9; 32]).unwrap();
        #[cfg(unix)]
        {
            fs::set_permissions(root.join("smtp.key"), fs::Permissions::from_mode(0o644)).unwrap();
            assert!(MailKey::load(root.clone(), true).await.is_err());
            fs::remove_file(root.join("smtp.key")).unwrap();
            std::os::unix::fs::symlink("missing", root.join("smtp.key")).unwrap();
            assert!(MailKey::load(root.clone(), true).await.is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
