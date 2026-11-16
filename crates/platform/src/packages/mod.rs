//! Versioned executable packages. The base never links private module code.
pub mod build;
mod format;
mod manager;
pub mod payments;
pub mod protocol;
pub mod sdk;
pub use format::{
    LIMIT, Labels, Manifest, Migration, hex, inspect, matches_path, pack, sign, unhex, verify,
    version,
};
pub use manager::{
    Activation, Changes, Installed, Manager, ModuleGuard, ModulePlan, PlanAction, PlanBlocker,
    Preview, RuntimeCause, RuntimeConfig, RuntimeFailure, RuntimePhase, RuntimeState,
    RuntimeStatus, UpdatePlan, UpdateSnapshot,
};
/// Reject URL-parser normalization before forwarding, including encoded dot-segment escapes.
pub fn proxy_url(port: u16, path: &str) -> Result<url::Url> {
    let parsed =
        url::Url::parse(&format!("http://127.0.0.1:{port}{path}")).map_err(|_| Error::Invalid)?;
    if parsed.path() != path.split('?').next().ok_or(Error::Invalid)? || parsed.fragment().is_some()
    {
        return Err(Error::Invalid);
    }
    Ok(parsed)
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid package or request")]
    Invalid,
    #[error("Incompatible package")]
    Incompatible,
    #[error("Publisher is not trusted")]
    Publisher,
    #[error("Package integrity check failed")]
    Signature,
    #[error("Package storage unavailable")]
    Storage,
    #[error("Package conflicts with installed modules or an active operation")]
    Conflict,
    #[error("Module has active work; try again after completion")]
    Busy,
    #[error("Module execution failed")]
    Execution,
    #[error("Preview expired or already consumed")]
    Preview,
    #[error("Update requires a strictly newer version")]
    Version,
    #[error("Module schema does not match the package migrations")]
    Schema,
    #[error("Module update incomplete; retry from the panel")]
    Incomplete,
    #[error("Updated module did not start")]
    Started,
    #[error("Updated module did not start; previous version restored")]
    RolledBack,
    #[error("Deactivate the module first")]
    Active,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_paths_cannot_be_normalized_out_of_the_declared_namespace() {
        for path in [
            "/admin/extensions/a/../users",
            "/admin/extensions/a/%2e%2e/users",
            "/admin/extensions/a/%2E./users",
            "/admin/extensions/a#fragment",
        ] {
            assert!(proxy_url(1234, path).is_err());
        }
        let url = proxy_url(1234, "/admin/extensions/a/jobs?page=2").unwrap();
        assert_eq!(url.path(), "/admin/extensions/a/jobs");
        assert_eq!(url.query(), Some("page=2"));
    }
}
