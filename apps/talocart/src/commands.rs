//! CLI orchestration and explicit administrative operations.
use crate::{
    cli::{Cli, Command},
    preflight, runtime,
    schema::{check_schema, migrate_schema},
};
#[cfg(test)]
use clap::Parser;
use talocart_platform::{config::Config, database, identity::Identity, modules};

pub(crate) fn compiled_modules() -> Vec<String> {
    Vec::new()
}

pub(crate) async fn run(
    cli: Cli,
    config: Option<Config>,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(cli.command, Command::BuildInfo) {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "version": talocart_platform::version::INFO.version,
                "display": talocart_platform::version::INFO.display,
                "commit": talocart_platform::version::INFO.commit,
                "source_dirty": talocart_platform::version::INFO.source_dirty,
                "tagged": talocart_platform::version::INFO.tagged,
                "source_fingerprint": talocart_platform::version::INFO.source_fingerprint,
                "sdk_version": talocart_platform::packages::sdk::VERSION,
                "protocol": talocart_platform::packages::protocol::VERSION,
                "database": talocart_platform::database::contract(),
                "compiled_modules": compiled_modules(),
            }))?
        );
        return Ok(());
    }
    if let Command::PackageBuild { executable, output } = &cli.command {
        talocart_platform::packages::build::bundle(executable, output).await?;
        return Ok(());
    }
    modules::validate(modules::MODULES)?;
    let config = match config {
        Some(config) => config,
        None => Config::load(cli.config.as_deref())?,
    };
    if crate::commands_update::run(&cli.command, &config).await? {
        return Ok(());
    }
    let verification = matches!(cli.command, Command::ServeVerify);
    if verification && !compiled_modules().is_empty() {
        return Err(talocart_platform::packages::Error::Incompatible.into());
    }
    let public_origin = config.public_origin();
    let pool = if verification {
        database::connect_read_only(&config.database).await?
    } else {
        database::connect(&config.database).await?
    };
    let media = talocart_platform::media::MediaStore::new(config.media.root.clone());
    // Migrate opens the registry read-only: base schema changes require inactive packages.
    let packages = if matches!(
        cli.command,
        Command::Doctor | Command::Serve | Command::ServeVerify | Command::Migrate
    ) {
        let compiled = compiled_modules();
        let package_config = talocart_platform::packages::RuntimeConfig {
            root: config.packages.root.clone(),
            database_url: config.database.url.clone(),
            media_root: config.media.root.clone(),
            store_name: config.store.name.clone(),
            public_origin: public_origin.clone(),
            secure_cookies: config.server.secure_cookies,
            compiled,
            serve: matches!(cli.command, Command::Serve),
        };
        let manager = if verification {
            talocart_platform::packages::Manager::open_offline(package_config).await?
        } else {
            talocart_platform::packages::Manager::open(package_config).await?
        };
        if !matches!(cli.command, Command::Migrate) {
            manager.check_database(&pool).await?;
        }
        if verification {
            manager.verify_offline(&pool).await?;
        }
        Some(manager)
    } else {
        None
    };
    match cli.command {
        Command::BuildInfo
        | Command::PackageBuild { .. }
        | Command::PackageInstall { .. }
        | Command::PackageUpdate { .. }
        | Command::PackageRetry { .. }
        | Command::PackageDisable { .. }
        | Command::PackageRemove { .. }
        | Command::PackageUpdatePlan { .. }
        | Command::PackageUpdateSuspend { .. }
        | Command::PackageUpdateMigrate
        | Command::PackageUpdateApply { .. }
        | Command::PackageUpdateVerify
        | Command::PackageUpdateRestore => {
            return Ok(());
        }
        Command::ResetToken { email, output } => {
            check_schema(&pool).await?;
            let mut file = token_file(&output)?;
            let raw = Identity::new(pool.clone()).issue_reset(&email).await?;
            write_token(&mut file, &raw)?;
            println!(
                "Token created in the requested file. Open /admin/reset; valid for 30 minutes."
            );
        }
        Command::CustomerResetToken { email, output } => {
            check_schema(&pool).await?;
            let mut file = token_file(&output)?;
            let raw = talocart_platform::identity::Customers::new(
                pool.clone(),
                Identity::new(pool.clone()),
            )
            .issue_reset(&email)
            .await?;
            write_token(&mut file, &raw)?;
            println!(
                "Customer token created in the requested file. Open /account/reset in the store language; valid for 30 minutes."
            );
        }
        Command::Migrate => {
            if let Some(manager) = &packages
                && database::pending(&pool).await?
                && manager.inventory()?.iter().any(|p| p.enabled)
            {
                return Err(
                    "Deactivate installed packages before migrating the base schema".into(),
                );
            }
            migrate_schema(&pool).await?;
            check_schema(&pool).await?;
            println!(
                "Migrations applied. Schema compatible with Talocart {}.",
                talocart_platform::version::DISPLAY
            );
        }
        Command::SeedDemo {
            development: _,
            dry_run,
        } => {
            check_schema(&pool).await?;
            let seeder = talocart_commerce::demo::DemoSeeder::new(pool.clone(), media);
            let report = if dry_run {
                seeder.preview().await?
            } else {
                seeder.install().await?
            };
            println!(
                "Catalog-demo package v{}: {:?}. Price mode: {}.\nCategories: {}, brands: {}, rates: {}, classes: {}, options: {}, values: {}, products: {}, variants: {}, images: {}.",
                talocart_commerce::demo::VERSION,
                report.outcome,
                report.price_mode,
                report.categories,
                report.brands,
                report.rates,
                report.classes,
                report.options,
                report.values,
                report.products,
                report.variants,
                report.images
            );
        }
        Command::Doctor => {
            preflight::database(
                &pool,
                Some(
                    packages
                        .as_ref()
                        .map(|m| m.payments())
                        .transpose()?
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| p.manifest.id)
                        .collect(),
                ),
            )
            .await?;
            preflight::mail_key(&pool, &config.media.root, preflight::MailPolicy::Inspect).await?;
            media.check(&pool).await?;
            if let Some(manager) = &packages {
                manager.check_payment().await?;
                let mut blocking = false;
                for item in manager.inventory()?.iter().filter(|p| !p.available()) {
                    blocking |= item.enabled;
                    if item.damaged() {
                        println!(
                            "Package {} {}: executable missing or modified; deactivate and reinstall the same package, install a newer version, or remove it with package-remove.",
                            item.manifest.id, item.manifest.version
                        );
                    } else if item.incomplete() {
                        println!(
                            "Package {} {}: update to {} incomplete; retry from /admin/modules.",
                            item.manifest.id,
                            item.manifest.version,
                            item.pending_version().unwrap_or_default()
                        );
                    } else {
                        println!(
                            "Package {} {}: SDK {}, protocol {}, database contract {} ({}/{}); host SDK {}, protocol {}, database contract {}.",
                            item.manifest.id,
                            item.manifest.version,
                            item.manifest.sdk_version,
                            item.manifest.protocol,
                            item.manifest.database.version,
                            item.manifest.os,
                            item.manifest.arch,
                            talocart_platform::packages::sdk::VERSION,
                            talocart_platform::packages::protocol::VERSION,
                            talocart_platform::database::CONTRACT_VERSION
                        );
                    }
                }
                if blocking {
                    return Err("Active packages unavailable".into());
                }
            }
            println!("Talocart {}", talocart_platform::version::DISPLAY);
            println!("Configuration valid. PostgreSQL reachable. Schema up to date.");
        }
        Command::MediaVariants => {
            check_schema(&pool).await?;
            println!("Images processed: {}", media.rebuild_variants(&pool).await?);
        }
        Command::MediaIndex => {
            check_schema(&pool).await?;
            println!(
                "Private avatars indexed: {}",
                media.index_private_avatars(&pool).await?
            );
        }
        Command::MediaAcquire { directory, dry_run } => {
            check_schema(&pool).await?;
            println!(
                "Media acquisition: {:?}",
                media.acquire(&pool, &directory, dry_run).await?
            );
        }
        Command::MediaCache { clear, repair } => {
            check_schema(&pool).await?;
            use talocart_platform::media::CacheAction;
            media
                .queue_all_cache_action(
                    &pool,
                    if repair {
                        CacheAction::Repair
                    } else if clear {
                        CacheAction::Clear
                    } else {
                        CacheAction::Rebuild
                    },
                )
                .await?;
            while media.process_cache_job(&pool).await? {}
            println!("Image cache: {:?}", media.cache_report(&pool).await?);
        }
        Command::MediaPrune { dry_run } => {
            check_schema(&pool).await?;
            println!("Media pruning: {:?}", media.prune(&pool, dry_run).await?);
        }
        Command::Serve | Command::ServeVerify => {
            runtime::serve(
                config,
                pool.clone(),
                media,
                packages,
                public_origin,
                verification,
                runtime::RuntimeExtensions {},
            )
            .await?;
        }
    }
    pool.close().await;
    Ok(())
}

fn token_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    {
        return Err(std::io::Error::other(
            "Secure token creation is supported only on Unix",
        ));
    }
    options.open(path)
}
fn write_token(file: &mut std::fs::File, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    file.write_all(token.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn build_info_needs_neither_configuration_nor_database() {
        let cli = Cli::try_parse_from([
            "talocart",
            "--config",
            "/missing/version-fixture.toml",
            "build-info",
        ])
        .expect("Build identity command");
        run(cli, None)
            .await
            .expect("Identity is available without configuration");
        let info =
            serde_json::to_value(&talocart_platform::version::INFO).expect("Serializable identity");
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["display"], talocart_platform::version::DISPLAY);
        let version = Cli::try_parse_from(["talocart", "--version"])
            .err()
            .expect("Clap prints the version");
        assert_eq!(version.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(
            version
                .to_string()
                .contains(talocart_platform::version::DISPLAY)
        );
    }
    #[tokio::test]
    async fn package_plan_needs_no_database_and_creates_no_storage() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("talocart-plan-cli-{}", std::process::id()));
        assert!(!root.exists());
        let mut config = Config::default();
        config.database.url = "postgres://synthetic:synthetic@127.0.0.1:1/plan_test".into();
        config.packages.root = root.join("packages");
        config.media.root = root.join("media");
        let cli = Cli::try_parse_from(["talocart", "package-update-plan"]).unwrap();
        let result = run(cli, Some(config)).await;
        if !compiled_modules().is_empty() {
            assert!(
                result.is_err(),
                "Statically composed hosts are outside the planner scope"
            );
        } else {
            result.expect("No database access or writes");
        }
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn offline_suspension_does_not_require_the_target_database_before_migration() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("talocart-suspend-cli-{}", std::process::id()));
        assert!(!root.exists());
        let mut config = Config::default();
        config.database.url = "postgres://synthetic:synthetic@127.0.0.1:1/suspend_test".into();
        config.packages.root = root.join("packages");
        config.media.root = root.join("media");
        let manager = talocart_platform::packages::Manager::open(
            talocart_platform::packages::RuntimeConfig {
                root: config.packages.root.clone(),
                media_root: config.media.root.clone(),
                database_url: config.database.url.clone(),
                store_name: config.store.name.clone(),
                public_origin: config.public_origin(),
                secure_cookies: false,
                compiled: vec![],
                serve: false,
            },
        )
        .await
        .unwrap();
        let expected = manager.update_snapshot().unwrap().inventory_sha256;
        drop(manager);
        let cli = Cli::try_parse_from([
            "talocart",
            "package-update-suspend",
            "--inventory-sha256",
            &expected,
        ])
        .unwrap();
        let result = run(cli, Some(config)).await;
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
        result.expect(
            "Suspension precedes base migration and must not connect/check the target schema",
        );
    }
    #[tokio::test]
    async fn offline_disable_and_remove_need_no_database_and_reject_unknown_ids() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("talocart-remove-cli-{}", std::process::id()));
        let mut config = Config::default();
        config.database.url = "postgres://synthetic:synthetic@127.0.0.1:1/remove_test".into();
        config.packages.root = root.join("packages");
        config.media.root = root.join("media");
        for command in ["package-disable", "package-remove"] {
            assert!(Cli::try_parse_from(["talocart", command]).is_err());
            let cli = Cli::try_parse_from(["talocart", command, "--id", "missing"]).unwrap();
            let error = run(cli, Some(config.clone())).await.unwrap_err();
            assert!(matches!(
                error.downcast_ref::<talocart_platform::packages::Error>(),
                Some(talocart_platform::packages::Error::Invalid)
            ));
        }
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn installation_has_no_token_command_and_recovery_still_requires_one() {
        assert!(Cli::try_parse_from(["talocart", "setup-token", "--output", "unused"]).is_err());
        assert!(matches!(
            Cli::try_parse_from([
                "talocart",
                "reset-token",
                "--email",
                "owner@example.test",
                "--output",
                "reset"
            ])
            .expect("Recovery command exists")
            .command,
            Command::ResetToken { .. }
        ));
    }
    #[test]
    fn demo_cli_requires_explicit_development_and_accepts_preview() {
        assert!(Cli::try_parse_from(["talocart", "seed-demo"]).is_err());
        assert!(
            Cli::try_parse_from(["talocart", "seed-demo", "--development", "--expand"]).is_err()
        );
        assert!(matches!(
            Cli::try_parse_from(["talocart", "seed-demo", "--development", "--dry-run"])
                .expect("CLI demo valido")
                .command,
            Command::SeedDemo {
                development: true,
                dry_run: true
            }
        ));
    }
}
