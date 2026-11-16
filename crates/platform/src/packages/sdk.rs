//! Native module API contract. This version is independent of Cargo/application releases.
use super::version;

// Stable native module contract: administrative and payment packages share one generation.
pub const VERSION: &str = "6.0.0";
pub const MANIFEST_FORMAT: u32 = 4;

/// A newer host may serve an older module within the same non-zero SDK major.
/// Incompatible API changes require a new SDK major; protocol/schema have their own gates.
pub fn accepts(required: &str, host: &str) -> bool {
    match (version(required), version(host)) {
        (Ok(required), Ok(host)) => required[0] != 0 && required[0] == host[0] && required <= host,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdk_compatibility_is_directional_and_rejects_unknown_contracts() {
        assert!(accepts(VERSION, "6.1.0"));
        assert!(!accepts("6.1.0", VERSION));
        assert!(
            !accepts("5.2.0", VERSION),
            "SDK 5 has no required health probe"
        );
        assert!(accepts("1.0.0", "1.2.3"));
        assert!(accepts("1.2.3", "1.2.3"));
        for (required, host) in [
            ("1.2.3", "1.2.2"),
            ("1.0.0", "2.0.0"),
            ("0.1.0", "0.1.0"),
            ("1.0", "1.0.0"),
            ("1.0.0", "latest"),
        ] {
            assert!(!accepts(required, host));
        }
    }
}
