use std::{
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use sqlx::postgres::PgConnectOptions;
use thiserror::Error;

#[derive(Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Fingerprint of the exact startup TOML, exposed only to the root control peer.
    #[serde(skip)]
    pub source_sha256: Option<String>,
    pub updates: UpdatesConfig,
    pub logging: LoggingConfig,
    pub packages: PackagesConfig,
    pub server: ServerConfig,
    pub store: StoreConfig,
    pub database: DatabaseConfig,
    pub media: MediaConfig,
}

/// Optional private status/request directory owned by the installed OS updater.
#[derive(Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    pub root: Option<PathBuf>,
}

/// Private error journal. Rotation dates use UTC; retention counts daily files.
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub root: PathBuf,
    pub max_files: usize,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("storage/logs"),
            max_files: 30,
        }
    }
}

/// Installed executable packages use a private directory distinct from media.
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PackagesConfig {
    pub root: PathBuf,
    pub install: PackageInstall,
}
impl Default for PackagesConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("storage/packages"),
            install: PackageInstall::Cli,
        }
    }
}

/// Bootstrap default, overridden by a persisted owner choice in settings.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageInstall {
    #[default]
    Cli,
    Panel,
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    /// Explicit HTTPS origin behind a local reverse proxy. Never derived from forwarded headers.
    pub public_origin: Option<String>,
    /// Enable only when accessed through HTTPS; loopback restriction remains.
    pub secure_cookies: bool,
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StoreConfig {
    pub name: String,
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    pub root: PathBuf,
    pub discovery_interval_seconds: u64,
}
impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("storage/media"),
            discovery_interval_seconds: 30,
        }
    }
}

// Deliberately no Debug: this structure contains a connection secret.
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub timeout_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 3000)),
            public_origin: None,
            secure_cookies: false,
        }
    }
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            name: "Talocart".into(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 5,
            timeout_seconds: 3,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Unable to read the requested configuration file")]
    Read,
    // Do not display TOML parser diagnostics: they may include the secret-bearing source line.
    #[error("Invalid TOML configuration: check syntax and field names")]
    Parse,
    #[error("Invalid value for {0}")]
    Invalid(&'static str),
}

impl Config {
    pub fn public_origin(&self) -> String {
        self.server.public_origin.as_deref().map_or_else(
            || format!("http://{}", self.server.listen),
            |origin| origin.trim_end_matches('/').to_owned(),
        )
    }

    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let selected = Self::configuration_path(path, Path::new("."));
        let path = selected.as_deref();
        let source = match path {
            Some(path) => fs::read_to_string(path).map_err(|_| ConfigError::Read)?,
            None => String::new(),
        };
        let mut config = Self::parse(&source)?;
        if path.is_some() {
            use sha2::{Digest, Sha256};
            config.source_sha256 = Some(format!("{:x}", Sha256::digest(source.as_bytes())));
        }
        config.apply_overrides(environment)?;
        config.validate()?;
        config.resolve_media_root(path)?;
        if config.packages.root.is_relative() {
            let base = path
                .and_then(Path::parent)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            config.packages.root = fs::canonicalize(base)
                .map_err(|_| ConfigError::Read)?
                .join(&config.packages.root);
        }
        if config.packages.root.starts_with(&config.media.root)
            || config.media.root.starts_with(&config.packages.root)
            || config
                .packages
                .root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ConfigError::Invalid("packages.root"));
        }
        config.resolve_logging_root(path)?;
        let secrets = resolved_path(&crate::mail::MailKey::directory(&config.media.root))?;
        for root in [
            &config.media.root,
            &config.packages.root,
            &config.logging.root,
        ] {
            let root = resolved_path(root)?;
            if secrets.starts_with(&root) || root.starts_with(&secrets) {
                return Err(ConfigError::Invalid(
                    "mail secrets directory overlaps storage",
                ));
            }
        }

        if let Some(root) = &config.updates.root
            && (!root.is_absolute()
                || root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)))
        {
            return Err(ConfigError::Invalid("updates.root"));
        }
        Ok(config)
    }

    fn resolve_logging_root(&mut self, path: Option<&Path>) -> Result<(), ConfigError> {
        if self.logging.root.is_relative() {
            let base = path
                .and_then(Path::parent)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            self.logging.root = fs::canonicalize(base)
                .map_err(|_| ConfigError::Read)?
                .join(&self.logging.root);
        }
        let log_root = resolved_path(&self.logging.root)?;
        let media_root = resolved_path(&self.media.root)?;
        let packages_root = resolved_path(&self.packages.root)?;
        if !(2..=365).contains(&self.logging.max_files)
            || self
                .logging
                .root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || log_root.starts_with(&media_root)
            || media_root.starts_with(&log_root)
            || log_root.starts_with(&packages_root)
            || packages_root.starts_with(&log_root)
        {
            return Err(ConfigError::Invalid("logging"));
        }
        Ok(())
    }

    // Explicit paths are authoritative; only talocart.toml is discovered automatically.
    fn configuration_path(path: Option<&Path>, directory: &Path) -> Option<PathBuf> {
        path.map(Path::to_path_buf)
            .or_else(|| Some(directory.join("talocart.toml")).filter(|path| path.exists()))
    }

    fn resolve_media_root(&mut self, path: Option<&Path>) -> Result<(), ConfigError> {
        if self.media.root.is_relative() {
            let base = path
                .and_then(Path::parent)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let base = fs::canonicalize(base).map_err(|_| ConfigError::Read)?;
            self.media.root = base.join(&self.media.root);
        }
        Ok(())
    }

    fn parse(source: &str) -> Result<Self, ConfigError> {
        toml::from_str(source).map_err(|_| ConfigError::Parse)
    }

    fn apply_overrides(
        &mut self,
        get: impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
    ) -> Result<(), ConfigError> {
        if let Some(value) = get("TALOCART_DATABASE_URL")? {
            self.database.url = value;
        }
        if let Some(value) = get("TALOCART_STORE_NAME")? {
            self.store.name = value;
        }
        if let Some(value) = get("TALOCART_LISTEN")? {
            self.server.listen = value
                .parse()
                .map_err(|_| ConfigError::Invalid("TALOCART_LISTEN"))?;
        }
        if let Some(value) = get("TALOCART_PUBLIC_ORIGIN")? {
            self.server.public_origin = Some(value);
        }
        if let Some(value) = get("TALOCART_MEDIA_ROOT")? {
            self.media.root = value.into();
        }
        if let Some(value) = get("TALOCART_MEDIA_DISCOVERY_INTERVAL_SECONDS")? {
            self.media.discovery_interval_seconds = value
                .parse()
                .map_err(|_| ConfigError::Invalid("TALOCART_MEDIA_DISCOVERY_INTERVAL_SECONDS"))?;
        }
        if let Some(value) = get("TALOCART_PACKAGES_ROOT")? {
            self.packages.root = value.into();
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(origin) = &self.server.public_origin {
            let valid = url::Url::parse(origin).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.as_str().trim_end_matches('/') == origin.trim_end_matches('/')
            });
            if !valid || !self.server.secure_cookies {
                return Err(ConfigError::Invalid(
                    "server.public_origin / server.secure_cookies",
                ));
            }
        }
        if self.packages.root.as_os_str().is_empty() {
            return Err(ConfigError::Invalid("packages.root"));
        }
        if self.media.root.file_name().is_none()
            || self
                .media
                .root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ConfigError::Invalid("media.root"));
        }
        if !(1..=3600).contains(&self.media.discovery_interval_seconds) {
            return Err(ConfigError::Invalid("media.discovery_interval_seconds"));
        }
        if !self.server.listen.ip().is_loopback() || self.server.listen.port() == 0 {
            return Err(ConfigError::Invalid(
                "server.listen: use a loopback address and a nonzero port for local preview",
            ));
        }
        if self.store.name.trim().is_empty()
            || self.store.name.chars().count() > 80
            || self.store.name.chars().any(char::is_control)
        {
            return Err(ConfigError::Invalid("store.name"));
        }
        if !(1..=50).contains(&self.database.max_connections) {
            return Err(ConfigError::Invalid("database.max_connections"));
        }
        if !(1..=30).contains(&self.database.timeout_seconds) {
            return Err(ConfigError::Invalid("database.timeout_seconds"));
        }
        if !(self.database.url.starts_with("postgres://")
            || self.database.url.starts_with("postgresql://"))
            || PgConnectOptions::from_str(&self.database.url).is_err()
        {
            return Err(ConfigError::Invalid(
                "TALOCART_DATABASE_URL / database.url: PostgreSQL URL required",
            ));
        }
        Ok(())
    }
}

/// Read a Talocart environment setting, rejecting non-Unicode values.
pub fn environment(key: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid(key)),
    }
}

// Resolve existing ancestors too: a symlinked parent must not hide public storage.
fn resolved_path(path: &Path) -> Result<PathBuf, ConfigError> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(ConfigError::Invalid("logging.root"))?;
            let name = path
                .file_name()
                .ok_or(ConfigError::Invalid("logging.root"))?;
            Ok(resolved_path(parent)?.join(name))
        }
        Err(_) => Err(ConfigError::Read),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_installation_is_cli_only_unless_explicitly_enabled() {
        assert_eq!(PackagesConfig::default().install, PackageInstall::Cli);
        assert_eq!(
            toml::from_str::<PackagesConfig>("install='panel'")
                .unwrap()
                .install,
            PackageInstall::Panel
        );
        assert!(toml::from_str::<PackagesConfig>("install='anything'").is_err());
    }

    #[test]
    fn optional_settings_belong_only_to_native_packages() {
        assert!(Config::parse("[modules]\nkoala_import=true").is_err());
        assert!(Config::parse("[modules]\nstripe=true").is_err());
        assert!(Config::parse("[stripe]\nenabled=false").is_err());
        assert!(Config::parse("[extensions.stripe]\nenabled=false").is_err());
    }

    #[test]
    fn public_https_origin_requires_secure_cookies_and_keeps_loopback_listener() {
        let mut config = valid();
        config.server.public_origin = Some("https://shop.example.test/".into());
        assert!(config.validate().is_err());
        config.server.secure_cookies = true;
        assert!(config.validate().is_ok());
        assert_eq!(config.public_origin(), "https://shop.example.test");
        for origin in [
            "http://shop.example.test",
            "https://user:secret@shop.test",
            "https://shop.test/path",
            "https://shop.test/?a=1",
            "https://shop.test/#fragment",
        ] {
            config.server.public_origin = Some(origin.into());
            assert!(config.validate().is_err(), "{origin}");
        }
        config.server.public_origin = Some("https://shop.example.test".into());
        config.server.listen = "0.0.0.0:3000".parse().unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn talocart_overrides_every_setting() {
        let mut config = valid();
        config
            .apply_overrides(|key| {
                Ok(match key {
                    "TALOCART_DATABASE_URL" => Some("postgres://new@localhost/new".into()),
                    "TALOCART_STORE_NAME" => Some("Custom store".into()),
                    "TALOCART_LISTEN" => Some("127.0.0.1:3200".into()),
                    "TALOCART_MEDIA_ROOT" => Some("/new/media".into()),
                    _ => None,
                })
            })
            .expect("new overrides");
        assert_eq!(config.database.url, "postgres://new@localhost/new");
        assert_eq!(config.store.name, "Custom store");
        assert_eq!(config.server.listen.port(), 3200);
        assert_eq!(config.media.root, PathBuf::from("/new/media"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn legacy_environment_settings_are_not_read() {
        let mut config = valid();
        let original = config.database.url.clone();
        config
            .apply_overrides(|key| {
                assert!(key.starts_with("TALOCART_"));
                Ok(None)
            })
            .expect("no current overrides");
        assert_eq!(config.database.url, original);
        assert_eq!(config.store.name, "Talocart");
        assert!(
            config
                .apply_overrides(|key| { Ok((key == "TALOCART_LISTEN").then(|| "invalid".into())) })
                .is_err()
        );
    }

    #[test]
    fn startup_source_fingerprint_is_bound_to_loaded_bytes_and_cannot_come_from_toml() {
        use sha2::{Digest, Sha256};
        let folder = fs::canonicalize(env::temp_dir())
            .unwrap()
            .join(format!("talocart-source-config-{}", std::process::id()));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("talocart.toml");
        let source = "[database]\nurl='postgres://fixture@127.0.0.1:1/config_test'\n";
        fs::write(&path, source).unwrap();
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(
            config.source_sha256,
            Some(format!("{:x}", Sha256::digest(source.as_bytes())))
        );
        fs::write(&path, format!("{source}\n# modified after startup\n")).unwrap();
        let changed = Config::load(Some(&path)).unwrap();
        assert_ne!(changed.source_sha256, config.source_sha256);
        assert!(Config::parse("source_sha256='forged'").is_err());
        fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn configuration_file_selection_ignores_legacy_and_respects_explicit_path() {
        let folder = env::temp_dir().join(format!("talocart-config-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("isolated folder");
        let new = folder.join("talocart.toml");
        let legacy = folder.join("koala.toml");
        assert_eq!(Config::configuration_path(None, &folder), None);
        fs::write(&legacy, "[store]\nname = 'Custom store'").expect("legacy file");
        assert_eq!(Config::configuration_path(None, &folder), None);
        fs::write(&new, "invalid TOML!").expect("invalid new file");
        assert_eq!(Config::configuration_path(None, &folder), Some(new.clone()));
        assert!(Config::parse(&fs::read_to_string(&new).expect("new file")).is_err());
        let explicit = folder.join("missing.toml");
        assert_eq!(
            Config::configuration_path(Some(&explicit), &folder),
            Some(explicit.clone())
        );
        assert!(matches!(
            Config::load(Some(&explicit)),
            Err(ConfigError::Read)
        ));
        let parsed = Config::parse(&fs::read_to_string(&legacy).expect("legacy file"))
            .expect("legacy config");
        assert_eq!(parsed.store.name, "Custom store");
        fs::remove_dir_all(folder).expect("cleanup");
    }

    #[test]
    fn journal_configuration_is_private_and_bounded() {
        let root = env::temp_dir().join(format!(
            "talocart-log-config-{}",
            crate::identity::token().unwrap()
        ));
        fs::create_dir_all(root.join("media")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut config = valid();
        config.media.root = root.join("media");
        config.packages.root = root.join("packages");
        config
            .resolve_logging_root(Some(&root.join("talocart.toml")))
            .unwrap();
        assert_eq!(config.logging.root, root.join("storage/logs"));
        for maximum in [0, 1, 366] {
            config.logging.max_files = maximum;
            assert!(config.resolve_logging_root(None).is_err());
        }
        config.logging.max_files = 30;
        for bad in [
            root.join("media/logs"),
            root.join("packages/logs"),
            root.join("../logs"),
            root.clone(),
        ] {
            config.logging.root = bad;
            assert!(config.resolve_logging_root(None).is_err());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("media"), root.join("alias")).unwrap();
            config.logging.root = root.join("alias/logs");
            assert!(config.resolve_logging_root(None).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    fn valid() -> Config {
        let mut config = Config::default();
        config.database.url = "postgres://fixture:local@localhost/config_test".into();
        config
    }

    #[test]
    fn missing_database_and_unknown_keys_fail() {
        assert!(Config::default().validate().is_err());
        assert!(Config::parse("[server]\nlsiten = 'localhost'").is_err());
    }

    #[test]
    fn config_error_does_not_expose_source() {
        let error = Config::parse("[database]\nurl = \"secret_password\"\nbroken!")
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret_password"));
    }

    #[test]
    fn rejects_public_listeners_in_local_preview() {
        let mut config = valid();
        config.server.listen = "0.0.0.0:3000".parse().unwrap();
        assert!(config.validate().is_err());
        config.server.listen = "[::1]:3000".parse().unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn environment_overrides_file_values_without_global_mutation() {
        let mut config = valid();
        config
            .apply_overrides(|key| {
                Ok(match key {
                    "TALOCART_STORE_NAME" => Some("Il mio negozio".into()),
                    "TALOCART_LISTEN" => Some("127.0.0.1:3100".into()),
                    _ => None,
                })
            })
            .unwrap();
        assert_eq!(config.store.name, "Il mio negozio");
        assert_eq!(config.server.listen.port(), 3100);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn media_configuration_has_predictable_paths_and_environment_override() {
        let mut config = valid();
        let base = fs::canonicalize(env::temp_dir()).unwrap();
        config
            .resolve_media_root(Some(&base.join("talocart-test.toml")))
            .unwrap();
        assert_eq!(config.media.root, base.join("storage/media"));
        config
            .apply_overrides(|key| {
                Ok((key == "TALOCART_MEDIA_ROOT").then(|| "/srv/talocart-media".to_owned()))
            })
            .unwrap();
        config
            .resolve_media_root(Some(Path::new("/nonexistent/talocart.toml")))
            .unwrap();
        assert_eq!(config.media.root, PathBuf::from("/srv/talocart-media"));
        config.media.root = PathBuf::new();
        assert!(config.validate().is_err());
    }

    #[test]
    fn discovery_interval_is_bounded_and_overridable() {
        let mut config = valid();
        assert_eq!(config.media.discovery_interval_seconds, 30);
        for seconds in [0, 3601] {
            config.media.discovery_interval_seconds = seconds;
            assert!(config.validate().is_err());
        }
        for value in ["1", "3600"] {
            config
                .apply_overrides(|key| {
                    Ok((key == "TALOCART_MEDIA_DISCOVERY_INTERVAL_SECONDS").then(|| value.into()))
                })
                .unwrap();
            assert!(config.validate().is_ok());
        }
        assert!(
            config
                .apply_overrides(|key| Ok(
                    (key == "TALOCART_MEDIA_DISCOVERY_INTERVAL_SECONDS").then(|| "invalid".into())
                ))
                .is_err()
        );
    }
    #[test]
    fn bounds_are_validated() {
        let mut config = valid();
        config.database.max_connections = 0;
        assert!(config.validate().is_err());
        config.database.max_connections = 5;
        config.store.name = " ".into();
        assert!(config.validate().is_err());
    }
    #[test]
    fn smtp_file_configuration_is_no_longer_accepted() {
        assert!(Config::parse("[mail]\nenabled=true\n").is_err());
    }
}
