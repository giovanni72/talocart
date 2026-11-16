//! Secrets use a bounded stdin message, never arguments, environment or diagnostics.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
pub const VERSION: u32 = 3;
/// SDK 6 serving processes answer authenticated GET with 204, without taking
/// their request/lifecycle gate or performing provider/database work.
pub const HEALTH_PATH: &str = "/__talocart_package/health";
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Install,
    Serve,
    CanStop,
}
// Intentionally no Debug: includes credentials and an authentication secret.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub protocol: u32,
    pub sdk_version: String,
    pub application: crate::version::ApplicationIdentity,
    pub database: crate::database::SchemaContract,
    pub action: Action,
    pub database_url: String,
    pub media_root: PathBuf,
    pub data_root: PathBuf,
    pub store_name: String,
    pub public_origin: String,
    pub secure_cookies: bool,
    pub token: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub protocol: u32,
    pub sdk_version: String,
    pub database_contract: u32,
    pub id: String,
    pub version: String,
    pub port: Option<u16>,
    pub busy: bool,
}
impl Reply {
    pub fn matches_manifest(&self, manifest: &super::Manifest) -> bool {
        self.protocol == VERSION
            && self.id == manifest.id
            && self.version == manifest.version
            && self.sdk_version == manifest.sdk_version
            && manifest.database.version == self.database_contract
    }
}
impl Start {
    /// The authenticated host supplies its compiled checkpoint, never an observed
    /// database digest. The module checks the actual ledger before any action.
    pub fn validate_for(&self, manifest: &super::Manifest) -> Result<()> {
        let required = &manifest.database;
        if self.protocol != VERSION
            || manifest.protocol != VERSION
            || manifest.format != super::sdk::MANIFEST_FORMAT
            || !super::sdk::accepts(&manifest.sdk_version, &self.sdk_version)
            || !required.valid()
            || !self.database.valid()
            || required.version != self.database.version
            || self.database.version != crate::database::CONTRACT_VERSION
            || required.migrations > self.database.migrations
            || required.migrations == self.database.migrations
                && required.sha256 != self.database.sha256
        {
            return Err(Error::Incompatible);
        }
        if !self.application.valid()
            || !crate::identity::valid_token(&self.token)
            || !self.data_root.is_absolute()
            || !self.media_root.is_absolute()
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
pub async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let count = tokio::time::timeout(
        Duration::from_secs(120),
        reader.take(32769).read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| Error::Execution)?
    .map_err(|_| Error::Execution)?;
    if count == 0 || bytes.len() > 32768 || bytes.last() != Some(&b'\n') {
        return Err(Error::Execution);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_checks_sdk_protocol_schema_and_the_signed_reply() {
        let manifest = crate::packages::format::fixture_manifest();
        let mut start = Start {
            protocol: VERSION,
            sdk_version: super::super::sdk::VERSION.into(),
            application: crate::version::ApplicationIdentity::current(),
            database: crate::database::contract(),
            action: Action::Serve,
            database_url: "postgres://unused@127.0.0.1:1/test".into(),
            media_root: std::env::temp_dir(),
            data_root: std::env::temp_dir(),
            store_name: "Synthetic".into(),
            public_origin: "http://127.0.0.1".into(),
            secure_cookies: false,
            token: "ab".repeat(32),
        };
        start.validate_for(&manifest).unwrap();
        start.application = crate::version::ApplicationIdentity {
            version: "99.1.0".into(),
            display: "99.1.0+future-host".into(),
        };
        start.validate_for(&manifest).unwrap();
        for invalid in ["", "bad\nidentity"] {
            start.application.display = invalid.into();
            assert!(start.validate_for(&manifest).is_err());
        }
        start.application = crate::version::ApplicationIdentity::current();
        let [major, minor, _] = super::super::version(super::super::sdk::VERSION).unwrap();
        let future_sdk = format!("{major}.{}.0", minor + 1);
        start.sdk_version = future_sdk.clone();
        start.validate_for(&manifest).unwrap();
        for version in [
            "0.9.0", "1.2.0", "2.0.0", "3.0.0", "4.0.0", "5.0.0", "5.2.0", "invalid",
        ] {
            start.sdk_version = version.into();
            assert!(start.validate_for(&manifest).is_err());
        }
        start.sdk_version = super::super::sdk::VERSION.into();
        start.protocol += 1;
        assert!(start.validate_for(&manifest).is_err());
        start.protocol = VERSION;
        start.database.version += 1;
        assert!(start.validate_for(&manifest).is_err());
        start.database = crate::database::contract();
        start.database.sha256 = "00".repeat(32);
        assert!(start.validate_for(&manifest).is_err());
        let mut reply = Reply {
            protocol: VERSION,
            sdk_version: manifest.sdk_version.clone(),
            database_contract: crate::database::CONTRACT_VERSION,
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            port: Some(1234),
            busy: false,
        };
        assert!(reply.matches_manifest(&manifest));
        reply.sdk_version = future_sdk;
        assert!(
            !reply.matches_manifest(&manifest),
            "Reply cannot claim an SDK different from its signed manifest"
        );
        reply.sdk_version = manifest.sdk_version.clone();
        reply.database_contract += 1;
        assert!(!reply.matches_manifest(&manifest));
    }
    #[tokio::test]
    async fn protocol_input_is_bounded_and_requires_a_complete_line() {
        for raw in [
            b"".as_slice(),
            b"{\"protocol\":1}".as_slice(),
            &vec![b'a'; 32769],
        ] {
            assert!(
                read_line(&mut tokio::io::BufReader::new(raw))
                    .await
                    .is_err()
            );
        }
        let bytes = read_line(&mut tokio::io::BufReader::new(b"{}\ntrailing".as_slice()))
            .await
            .unwrap();
        assert_eq!(bytes, b"{}\n");
    }
}
