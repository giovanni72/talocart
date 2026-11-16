//! Command syntax; no runtime or database initialization.
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "talocart",
    version = talocart_platform::version::DISPLAY,
    about = "Talocart — modular Rust e-commerce platform"
)]
pub(crate) struct Cli {
    /// TOML file; defaults to talocart.toml; TALOCART_* environment variables take precedence.
    #[arg(long, global = true)]
    pub(crate) config: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Build identity as JSON; no configuration or database needed.
    BuildInfo,
    /// Create a keyless binary-only .talopkg, without database access.
    PackageBuild {
        #[arg(long)]
        executable: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate and install a package while the host is stopped; preserves modules and data.
    PackageInstall {
        #[arg(long)]
        package: PathBuf,
        #[arg(long)]
        enable: bool,
    },
    /// Update an existing package while the host is stopped, preserving activation and data.
    PackageUpdate {
        #[arg(long)]
        package: PathBuf,
    },
    /// Retry an interrupted package update while the host is stopped.
    PackageRetry {
        #[arg(long)]
        id: String,
    },
    /// Deactivate an installed package while the host is stopped; pending work still blocks a verifiable module.
    PackageDisable {
        #[arg(long)]
        id: String,
    },
    /// Remove an inactive package's registry entry and executables while the host is stopped; private data, schema and settings are preserved.
    PackageRemove {
        #[arg(long)]
        id: String,
    },
    /// Read-only target update plan; optionally bind actual database ledgers, never starts workers.
    PackageUpdatePlan {
        #[arg(long = "package")]
        packages: Vec<PathBuf>,
        #[arg(long)]
        check_database: bool,
    },
    /// Persistently suspend modules after backup; requires the exclusive host lease.
    PackageUpdateSuspend {
        #[arg(long)]
        inventory_sha256: String,
        /// Read the live host's quiesced snapshot from bounded JSON stdin.
        #[arg(long)]
        quiesced: bool,
    },
    /// Apply base migrations under the exclusive host lease with modules inactive.
    PackageUpdateMigrate,
    /// Apply validated replacements to installed, inactive modules; never installs new IDs.
    PackageUpdateApply {
        #[arg(long = "package")]
        packages: Vec<PathBuf>,
    },
    /// Validate database/handshakes without HTTP or background jobs.
    PackageUpdateVerify,
    /// Restore original activation from a bounded JSON snapshot on stdin.
    PackageUpdateRestore,
    /// Start the local application; requires an up-to-date schema.
    Serve,
    /// Start a read-only HTTP verification host, without job executors or native serving.
    ServeVerify,
    /// Create a single-use token for assisted administrator password recovery.
    ResetToken {
        #[arg(long)]
        email: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Create a single-use token for assisted customer password recovery.
    CustomerResetToken {
        #[arg(long)]
        email: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Apply migrations embedded in the executable.
    Migrate,
    /// Load the demo catalog after setup, only when explicitly requested.
    SeedDemo {
        /// Confirm that the catalog is for development.
        #[arg(long, required = true)]
        development: bool,
        /// Preview the package without database or filesystem writes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check configuration, connection and schema without changes.
    Doctor,
    /// Generate missing variants of existing images.
    MediaVariants,
    /// Index private avatars in their account media libraries.
    MediaIndex,
    /// Acquire server images without moving or deleting source files.
    MediaAcquire {
        #[arg(long, default_value = "library")]
        directory: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Regenerate the persistent image cache; preserve all sources.
    MediaCache {
        #[arg(long, conflicts_with = "repair")]
        clear: bool,
        /// Repair only missing/invalid copies and retry failures.
        #[arg(long)]
        repair: bool,
    },
    /// Remove media unreferenced for at least 24 hours.
    MediaPrune {
        #[arg(long)]
        dry_run: bool,
    },
}
