//! A single executable envelope. No archive paths, extraction or customer-side compilation.
use super::{Error, Result};
use ring::signature::{self, Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LIMIT: usize = 128 * 1024 * 1024;
const MAGIC: &[u8] = b"TALOPKG1\n";
const UNSIGNED_MAGIC: &[u8] = b"TALOPKG2\n";
const MANIFEST_LIMIT: usize = 32768;
const DOMAIN: &[u8] = b"talocart-package-v1\0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Labels {
    pub en: String,
    pub it: String,
}
impl Labels {
    pub fn get(&self, locale: &str) -> &str {
        if locale == "it" { &self.it } else { &self.en }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    pub version: i64,
    pub checksum: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub protocol: u32,
    pub id: String,
    pub version: String,
    pub sdk_version: String,
    pub database: crate::database::SchemaContract,
    pub os: String,
    pub arch: String,
    pub title: Labels,
    pub description: Labels,
    pub notices: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<super::payments::Descriptor>,
    pub menu: String,
    /// All prefixes are confined to this module's admin namespace.
    pub paths: Vec<String>,
    pub permissions: Vec<String>,
    pub ledger: String,
    pub migrations: Vec<Migration>,
    /// Legacy envelope metadata; new packages have no publisher key.
    #[serde(default)]
    pub publisher: String,
    pub size: usize,
    pub sha256: String,
}
pub fn version(value: &str) -> Result<[u32; 3]> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || p.len() > 1 && p.starts_with('0'))
    {
        return Err(Error::Invalid);
    }
    let mut result = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Invalid);
        }
        result[i] = part.parse().map_err(|_| Error::Invalid)?;
    }
    Ok(result)
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex(value: &str, size: usize) -> Result<Vec<u8>> {
    if value.len() != size * 2
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let s = std::str::from_utf8(p).map_err(|_| Error::Invalid)?;
            u8::from_str_radix(s, 16).map_err(|_| Error::Invalid)
        })
        .collect()
}
pub fn matches_path(prefix: &str, path: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|s| s.starts_with('/'))
}
impl Manifest {
    /// Structure, content metadata and compatibility with this base and host.
    pub fn validate(&self) -> Result<()> {
        self.validate_structure()?;
        if !self.compatible() {
            return Err(Error::Incompatible);
        }
        Ok(())
    }
    /// Application version is deliberately absent from the compatibility decision.
    pub fn compatible(&self) -> bool {
        self.format == super::sdk::MANIFEST_FORMAT
            && self.protocol == super::protocol::VERSION
            && super::sdk::accepts(&self.sdk_version, super::sdk::VERSION)
            && self
                .payment
                .as_ref()
                .is_none_or(|p| p.protocol == super::payments::VERSION)
            && crate::database::supports_contract(&self.database)
            && self.os == std::env::consts::OS
            && self.arch == std::env::consts::ARCH
    }
    /// Checks independent of the running base, so an installed package of another SDK
    /// can still be listed, diagnosed and updated without being executed.
    pub fn validate_structure(&self) -> Result<()> {
        let id = &self.id;
        if id == crate::modules::BASE.id
            || id.is_empty()
            || id.len() > 48
            || !id.as_bytes()[0].is_ascii_lowercase()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !matches!(self.arch.as_str(), "aarch64" | "x86_64")
            || self.size == 0
            || self.size > LIMIT
            || self.paths.is_empty()
            || self.paths.len() > 8
            || self.permissions.is_empty()
            || self.permissions.len() > 24
            || self
                .permissions
                .iter()
                .any(|p| crate::identity::Permission::parse(p).is_err())
            || self.notices.is_empty()
            || self.notices.len() > 16384
            || self.migrations.is_empty()
            || self.migrations.len() > 1000
            || self.ledger != format!("_{}_migrations", id.replace('-', "_"))
        {
            return Err(Error::Incompatible);
        }
        if self.format != super::sdk::MANIFEST_FORMAT {
            return Err(Error::Incompatible);
        }
        version(&self.sdk_version)?;
        if let Some(payment) = &self.payment {
            payment.validate_structure(&self.id)?;
            if payment.amount_limits.is_some() && version(&self.sdk_version)? < [5, 2, 0] {
                return Err(Error::Incompatible);
            }
            if version(&self.sdk_version)?
                < if payment.protocol == 1 {
                    [3, 0, 0]
                } else {
                    [4, 0, 0]
                }
            {
                return Err(Error::Incompatible);
            }
        }
        if !self.database.valid() {
            return Err(Error::Invalid);
        }
        version(&self.version)?;
        if !self.publisher.is_empty() {
            unhex(&self.publisher, 32)?;
        }
        unhex(&self.sha256, 32)?;
        for label in [
            &self.title.en,
            &self.title.it,
            &self.description.en,
            &self.description.it,
        ] {
            if label.trim().is_empty() || label.len() > 512 || label.chars().any(char::is_control) {
                return Err(Error::Invalid);
            }
        }
        let namespace = format!("/admin/extensions/{id}");
        for path in &self.paths {
            if !matches_path(&namespace, path)
                || path.len() > 160
                || path.contains("..")
                || !path
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
                || path.ends_with('/')
            {
                return Err(Error::Invalid);
            }
        }
        if !self.paths.iter().any(|p| matches_path(p, &self.menu))
            || self.menu.contains('?')
            || self.menu.contains('#')
            || self.menu.contains("..")
            || !self
                .menu
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
        {
            return Err(Error::Invalid);
        }
        let mut previous = 0;
        for migration in &self.migrations {
            if migration.version <= previous {
                return Err(Error::Invalid);
            }
            unhex(&migration.checksum, 48)?;
            previous = migration.version;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Verified {
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub signature: Vec<u8>,
    pub executable: Vec<u8>,
}
fn message(raw: &[u8]) -> Vec<u8> {
    [DOMAIN, raw].concat()
}
/// Validate contracts and content without a publisher allowlist. Legacy envelopes
/// remain readable using their embedded signature, without registering any key.
pub fn inspect(bytes: &[u8]) -> Result<Verified> {
    let unsigned = bytes.starts_with(UNSIGNED_MAGIC);
    if bytes.len() > LIMIT + MANIFEST_LIMIT + 77 || !(unsigned || bytes.starts_with(MAGIC)) {
        return Err(Error::Invalid);
    }
    let mut cursor = MAGIC.len();
    let n = bytes.get(cursor..cursor + 4).ok_or(Error::Invalid)?;
    let length = u32::from_be_bytes(n.try_into().map_err(|_| Error::Invalid)?) as usize;
    if length == 0 || length > MANIFEST_LIMIT {
        return Err(Error::Invalid);
    }
    cursor += 4;
    let signature = if unsigned {
        &[][..]
    } else {
        let signature = bytes.get(cursor..cursor + 64).ok_or(Error::Invalid)?;
        cursor += 64;
        signature
    };
    let raw = bytes.get(cursor..cursor + length).ok_or(Error::Invalid)?;
    cursor += length;
    let manifest: Manifest = serde_json::from_slice(raw).map_err(|_| Error::Invalid)?;
    manifest.validate()?;
    if unsigned && !manifest.publisher.is_empty() {
        return Err(Error::Invalid);
    }
    verify_manifest(raw, signature, &manifest)?;
    let executable = bytes.get(cursor..).ok_or(Error::Invalid)?;
    native_executable(executable)?;
    if executable.len() != manifest.size || hex(&Sha256::digest(executable)) != manifest.sha256 {
        return Err(Error::Signature);
    }
    Ok(Verified {
        manifest,
        manifest_bytes: raw.to_vec(),
        signature: signature.to_vec(),
        executable: executable.to_vec(),
    })
}
pub fn verify_manifest(raw: &[u8], signature: &[u8], manifest: &Manifest) -> Result<()> {
    if signature.is_empty() && manifest.publisher.is_empty() {
        return Ok(());
    }
    signature::UnparsedPublicKey::new(&signature::ED25519, unhex(&manifest.publisher, 32)?)
        .verify(&message(raw), signature)
        .map_err(|_| Error::Signature)
}
/// Legacy codec API. The host installation policy uses `inspect`, not this allowlist.
pub fn verify(bytes: &[u8], publishers: &[String]) -> Result<Verified> {
    let verified = inspect(bytes)?;
    if !verified.manifest.publisher.is_empty() && !publishers.contains(&verified.manifest.publisher)
    {
        return Err(Error::Publisher);
    }
    Ok(verified)
}
/// Current keyless envelope; hashes detect corruption, not publisher authenticity.
pub fn pack(mut manifest: Manifest, executable: &[u8]) -> Result<Vec<u8>> {
    native_executable(executable)?;
    manifest.publisher.clear();
    manifest.size = executable.len();
    manifest.sha256 = hex(&Sha256::digest(executable));
    manifest.validate()?;
    let raw = serde_json::to_vec(&manifest).map_err(|_| Error::Invalid)?;
    if raw.len() > MANIFEST_LIMIT {
        return Err(Error::Invalid);
    }
    Ok([
        UNSIGNED_MAGIC,
        &(raw.len() as u32).to_be_bytes(),
        &raw,
        executable,
    ]
    .concat())
}
pub fn sign(mut manifest: Manifest, executable: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    native_executable(executable)?;
    let pair = Ed25519KeyPair::from_pkcs8(key).map_err(|_| Error::Invalid)?;
    manifest.publisher = hex(pair.public_key().as_ref());
    manifest.size = executable.len();
    manifest.sha256 = hex(&Sha256::digest(executable));
    manifest.validate()?;
    let raw = serde_json::to_vec(&manifest).map_err(|_| Error::Invalid)?;
    if raw.len() > MANIFEST_LIMIT {
        return Err(Error::Invalid);
    }
    let signature = pair.sign(&message(&raw));
    Ok([
        MAGIC,
        &(raw.len() as u32).to_be_bytes(),
        signature.as_ref(),
        &raw,
        executable,
    ]
    .concat())
}

fn native_executable(bytes: &[u8]) -> Result<()> {
    let u16_at = |offset| {
        bytes
            .get(offset..offset + 2)
            .and_then(|b| b.try_into().ok())
            .map(u16::from_le_bytes)
    };
    let u32_at = |offset| {
        bytes
            .get(offset..offset + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let valid = match std::env::consts::OS {
        "macos" => {
            bytes.len() >= 32
                && u32_at(0) == Some(0xfeedfacf)
                && u32_at(12) == Some(2)
                && u32_at(4)
                    == Some(if std::env::consts::ARCH == "aarch64" {
                        0x100000c
                    } else {
                        0x1000007
                    })
        }
        "linux" => {
            bytes.len() >= 64
                && bytes.starts_with(b"\x7fELF\x02\x01")
                && matches!(u16_at(16), Some(2 | 3))
                && u16_at(18)
                    == Some(if std::env::consts::ARCH == "aarch64" {
                        183
                    } else {
                        62
                    })
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::Incompatible)
    }
}
#[cfg(test)]
pub(super) fn fixture_payload() -> Vec<u8> {
    let mut bytes = vec![0; 64];
    if std::env::consts::OS == "macos" {
        bytes[..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
        bytes[4..8].copy_from_slice(
            &(if std::env::consts::ARCH == "aarch64" {
                0x100000cu32
            } else {
                0x1000007u32
            })
            .to_le_bytes(),
        );
        bytes[12..16].copy_from_slice(&2u32.to_le_bytes());
    } else {
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(
            &(if std::env::consts::ARCH == "aarch64" {
                183u16
            } else {
                62u16
            })
            .to_le_bytes(),
        );
    }
    bytes
}
#[cfg(test)]
pub(super) fn fixture_manifest() -> Manifest {
    Manifest {
        format: super::sdk::MANIFEST_FORMAT,
        protocol: super::protocol::VERSION,
        id: "fixture".into(),
        version: "1.0.0".into(),

        sdk_version: super::sdk::VERSION.into(),
        database: crate::database::contract(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        title: Labels {
            en: "Fixture".into(),
            it: "Prova".into(),
        },
        description: Labels {
            en: "Test only".into(),
            it: "Solo test".into(),
        },
        notices: "Synthetic license notice".into(),
        payment: None,
        menu: "/admin/extensions/fixture".into(),
        paths: vec!["/admin/extensions/fixture".into()],
        permissions: vec!["overview.read".into()],
        ledger: "_fixture_migrations".into(),
        migrations: vec![Migration {
            version: 1,
            checksum: "00".repeat(48),
        }],
        publisher: "00".repeat(32),
        size: 64,
        sha256: "00".repeat(32),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyless_envelopes_validate_contracts_and_detect_corrupt_payloads() {
        let bytes = pack(fixture_manifest(), &fixture_payload()).unwrap();
        assert!(bytes.starts_with(UNSIGNED_MAGIC));
        let inspected = inspect(&bytes).unwrap();
        assert!(inspected.manifest.publisher.is_empty());
        assert!(inspected.signature.is_empty());
        assert_eq!(inspected.executable, fixture_payload());
        for offset in [0, 9, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[offset] ^= 1;
            assert!(inspect(&bad).is_err());
        }
        let mut manifest = fixture_manifest();
        manifest.permissions = vec!["unknown.permission".into()];
        assert!(pack(manifest, &fixture_payload()).is_err());
    }
    #[test]
    fn sdk_contract_is_independent_of_the_application_release() {
        let mut manifest = fixture_manifest();
        manifest.format = super::super::sdk::MANIFEST_FORMAT;
        manifest.protocol = super::super::protocol::VERSION;
        manifest.sdk_version = super::super::sdk::VERSION.into();
        assert_ne!(manifest.sdk_version, env!("CARGO_PKG_VERSION"));
        assert!(
            manifest.compatible(),
            "An independent SDK must not track the application version"
        );
    }

    #[test]
    fn obsolete_manifests_are_rejected() {
        let mut manifest = fixture_manifest();
        manifest.format = 1;
        assert!(manifest.validate_structure().is_err());
        let mut raw = serde_json::to_value(fixture_manifest()).unwrap();
        raw["base_major"] = 9.into();
        assert!(serde_json::from_value::<Manifest>(raw).is_err());
        let mut raw = serde_json::to_value(fixture_manifest()).unwrap();
        raw.as_object_mut().unwrap().remove("database");
        assert!(serde_json::from_value::<Manifest>(raw).is_err());
    }

    fn key() -> ring::pkcs8::Document {
        Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .expect("Synthetic signing key")
    }
    #[test]
    fn signed_native_payload_roundtrip_and_all_bytes_are_authenticated() {
        let key = key();
        let pair = Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
        let keys = vec![hex(pair.public_key().as_ref())];
        let bytes = sign(fixture_manifest(), &fixture_payload(), key.as_ref()).unwrap();
        let verified = verify(&bytes, &keys).unwrap();
        assert_eq!(verified.executable, fixture_payload());
        assert_eq!(verified.manifest.version, "1.0.0");
        assert!(matches!(verify(&bytes, &[]), Err(Error::Publisher)));
        for offset in [0, 9, 13, 77, bytes.len() - 1] {
            let mut corrupted = bytes.clone();
            corrupted[offset] ^= 1;
            assert!(verify(&corrupted, &keys).is_err(), "Byte {offset}");
        }
        for end in [0, 9, 12, 70, bytes.len() - 1] {
            assert!(verify(&bytes[..end], &keys).is_err());
        }
        let mut extra = bytes;
        extra.push(1);
        assert!(verify(&extra, &keys).is_err());
        assert!(sign(fixture_manifest(), b"#!/bin/sh\n", key.as_ref()).is_err());
    }
    #[test]
    fn manifest_boundaries_are_validated_before_execution() {
        let base = fixture_manifest();
        base.validate().unwrap();
        for value in ["../fixture", "/fixture", "Fixture", "a/b", ""] {
            let mut m = base.clone();
            m.id = value.into();
            assert!(m.validate().is_err());
        }
        for path in [
            "/admin",
            "/admin/modules",
            "/admin/extensions/fixture/../users",
            "/admin/extensions/fixture?x",
            "/admin/extensions/fixture/",
            "https://example.test",
        ] {
            let mut m = base.clone();
            m.paths = vec![path.into()];
            assert!(m.validate().is_err());
        }
        let mut m = base.clone();
        m.protocol = crate::packages::protocol::VERSION + 1;
        assert!(m.validate().is_err());
        m = base.clone();
        m.format = 1;
        assert!(m.validate().is_err());
        m = base.clone();
        m.sdk_version = "0.0.0".into();
        assert!(m.validate().is_err());
        m = base.clone();
        m.arch = "wrong".into();
        assert!(m.validate().is_err());
        m = base.clone();
        m.os = "wrong".into();
        assert!(m.validate().is_err());
        m = base.clone();
        m.ledger = "_sqlx_migrations".into();
        assert!(m.validate().is_err());
        m = base.clone();
        m.permissions = vec!["unknown".into()];
        assert!(m.validate().is_err());
        m = base.clone();
        m.migrations.push(m.migrations[0].clone());
        assert!(m.validate().is_err());
        m = base.clone();
        m.size = LIMIT + 1;
        assert!(m.validate().is_err());
        m = base.clone();
        m.menu = "/admin/extensions/fixture/../modules".into();
        assert!(m.validate().is_err());
        let mut value = serde_json::to_value(base).unwrap();
        value["unknown"] = true.into();
        assert!(serde_json::from_value::<Manifest>(value).is_err());
    }
    #[test]
    fn base_id_cannot_be_claimed_by_an_installable_package() {
        let mut manifest = fixture_manifest();
        manifest.id = "base".into();
        manifest.ledger = "_base_migrations".into();
        manifest.menu = "/admin/extensions/base".into();
        manifest.paths = vec![manifest.menu.clone()];
        assert!(
            manifest.validate().is_err(),
            "The required base ID must remain reserved"
        );
    }
    #[test]
    fn every_package_uses_its_own_admin_namespace() {
        for id in ["fixture", "koala-import"] {
            let mut manifest = fixture_manifest();
            manifest.id = id.into();
            manifest.ledger = format!("_{}_migrations", id.replace('-', "_"));
            manifest.menu = format!("/admin/extensions/{id}/imports");
            manifest.paths = vec![format!("/admin/extensions/{id}")];
            assert!(manifest.validate().is_ok());
            for path in [
                "/admin/catalog/imports",
                "/admin/koala-import.css",
                "/admin/extensions/other",
                "/admin/extensions/fixture-extra",
            ] {
                manifest.menu = path.into();
                manifest.paths = vec![path.into()];
                assert!(manifest.validate().is_err(), "{id} accepted {path}");
            }
        }
    }

    #[test]
    fn versions_hex_and_route_boundaries_are_strict() {
        assert_eq!(version("12.3.45").unwrap(), [12, 3, 45]);
        for v in [
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-beta",
            "1.-1.3",
            "1.2.4294967296",
        ] {
            assert!(version(v).is_err());
        }
        assert!(unhex("FF", 1).is_err());
        assert!(unhex("ff", 1).is_ok());
        assert!(unhex("f", 1).is_err());
        assert!(matches_path(
            "/admin/extensions/a",
            "/admin/extensions/a/jobs"
        ));
        assert!(!matches_path("/admin/extensions/a", "/admin/extensions/ab"));
    }
}
