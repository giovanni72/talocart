//! Identity of the running executable, fixed at compile time; no runtime Git or database access.
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub display: &'static str,
    pub commit: &'static str,
    pub source_dirty: bool,
    pub tagged: bool,
    pub source_fingerprint: &'static str,
}

pub const DISPLAY: &str = env!("TALOCART_BUILD_DISPLAY");
pub const INFO: BuildInfo = BuildInfo {
    version: env!("CARGO_PKG_VERSION"),
    display: DISPLAY,
    commit: env!("TALOCART_BUILD_COMMIT"),
    source_dirty: matches!(env!("TALOCART_BUILD_DIRTY").as_bytes(), b"true"),
    tagged: matches!(env!("TALOCART_BUILD_TAGGED").as_bytes(), b"true"),
    source_fingerprint: env!("TALOCART_BUILD_FINGERPRINT"),
};

/// Display identity of the application host, independent of module compatibility.
/// Carried over the private startup channel; never inferred from HTTP headers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApplicationIdentity {
    pub version: String,
    pub display: String,
}
impl ApplicationIdentity {
    pub fn current() -> Self {
        Self {
            version: INFO.version.into(),
            display: INFO.display.into(),
        }
    }
    pub fn valid(&self) -> bool {
        [&self.version, &self.display].into_iter().all(|value| {
            !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        })
    }
}
