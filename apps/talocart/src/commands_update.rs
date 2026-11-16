//! Administrative update CLI composition; package rules remain in platform.
use crate::{
    cli::Command,
    schema::{check_schema, migrate_schema},
};
use talocart_platform::{
    config::Config,
    database,
    packages::{Manager, RuntimeConfig, UpdateSnapshot},
};

fn runtime(config: &Config) -> RuntimeConfig {
    RuntimeConfig {
        root: config.packages.root.clone(),
        database_url: config.database.url.clone(),
        media_root: config.media.root.clone(),
        store_name: config.store.name.clone(),
        public_origin: config.public_origin(),
        secure_cookies: config.server.secure_cookies,
        compiled: super::commands::compiled_modules(),
        serve: false,
    }
}
pub(crate) async fn run(
    command: &Command,
    config: &Config,
) -> Result<bool, Box<dyn std::error::Error>> {
    if let Command::PackageInstall { package, .. } | Command::PackageUpdate { package } = command {
        let mut options = runtime(config);
        options.serve = true;
        if !super::commands::compiled_modules().is_empty() {
            return Err(talocart_platform::packages::Error::Incompatible.into());
        }
        let manager = Manager::open(options).await?;
        let pool = database::connect(&config.database).await?;
        check_schema(&pool).await?;
        let package = package.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(package)?
                .take((talocart_platform::packages::LIMIT + 40001) as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        })
        .await??;
        let preview = manager.preview("administrative-cli".into(), bytes).await?;
        if matches!(command, Command::PackageUpdate { .. }) {
            manager
                .update("administrative-cli", &preview.token, &pool)
                .await?;
        } else {
            manager
                .install("administrative-cli", &preview.token, &pool)
                .await?;
        }
        if matches!(command, Command::PackageInstall { enable: true, .. }) {
            manager.set_enabled(&preview.manifest.id, true).await?;
        }
        println!(
            "Package applied: {} {}",
            preview.manifest.id, preview.manifest.version
        );
        pool.close().await;
        return Ok(true);
    }
    if let Command::PackageRetry { id } = command {
        let mut options = runtime(config);
        options.serve = true;
        let manager = Manager::open(options).await?;
        let pool = database::connect(&config.database).await?;
        check_schema(&pool).await?;
        manager.retry(id, &pool).await?;
        pool.close().await;
        println!("Package update completed: {id}");
        return Ok(true);
    }
    if let Command::PackageDisable { id } | Command::PackageRemove { id } = command {
        let mut options = runtime(config);
        options.serve = true;
        if !super::commands::compiled_modules().is_empty() {
            return Err(talocart_platform::packages::Error::Incompatible.into());
        }
        // No pool here: a healthy module's stop probe connects on its own, and a damaged
        // executable is never run.
        let manager = Manager::open(options).await?;
        if matches!(command, Command::PackageDisable { .. }) {
            manager.set_enabled(id, false).await?;
            println!("Package deactivated: {id}");
        } else {
            manager.uninstall(id).await?;
            println!("Package removed: {id}; private data preserved");
        }
        return Ok(true);
    }
    if let Command::PackageUpdatePlan {
        packages,
        check_database,
    } = command
    {
        let manager = Manager::open(runtime(config)).await?;
        let mut plan = manager.plan_update(packages.clone()).await?;
        if *check_database && !plan.blocked() {
            let pool = database::connect_read_only(&config.database).await?;
            plan.database_sha256 = Some(manager.update_database_fingerprint(&pool).await?);
            pool.close().await;
            plan.database_checked = true;
        }
        println!("{}", serde_json::to_string(&plan)?);
        if plan.blocked() {
            return Err(talocart_platform::packages::Error::Incompatible.into());
        }
        return Ok(true);
    }
    if !matches!(
        command,
        Command::PackageUpdateSuspend { .. }
            | Command::PackageUpdateApply { .. }
            | Command::PackageUpdateMigrate
            | Command::PackageUpdateVerify
            | Command::PackageUpdateRestore
    ) {
        return Ok(false);
    }
    // Read bounded metadata before taking a lease; credentials never come from stdin.
    let original = if matches!(
        command,
        Command::PackageUpdateRestore | Command::PackageUpdateSuspend { quiesced: true, .. }
    ) {
        Some(
            tokio::task::spawn_blocking(|| {
                use std::io::Read;
                let mut raw = Vec::new();
                std::io::stdin().take(16385).read_to_end(&mut raw)?;
                if raw.len() > 16384 {
                    return Err(std::io::Error::other("Update snapshot exceeds limit"));
                }
                serde_json::from_slice::<UpdateSnapshot>(&raw)
                    .map_err(|_| std::io::Error::other("Invalid update snapshot"))
            })
            .await??,
        )
    } else {
        None
    };
    let read_only = matches!(
        command,
        Command::PackageUpdateVerify | Command::PackageUpdateRestore
    );
    let package_config = runtime(config);
    let manager = Manager::open_offline(package_config).await?;
    if let Command::PackageUpdateSuspend {
        inventory_sha256, ..
    } = command
    {
        println!(
            "{}",
            serde_json::to_string(
                &manager
                    .suspend_offline(inventory_sha256.clone(), original)
                    .await?
            )?
        );
        return Ok(true);
    }

    let pool = if read_only {
        database::connect_read_only(&config.database).await?
    } else {
        database::connect(&config.database).await?
    };
    if !matches!(command, Command::PackageUpdateMigrate) {
        check_schema(&pool).await?;
    }
    match command {
        Command::PackageUpdateMigrate => {
            manager.require_inactive()?;
            manager.check_database(&pool).await?;
            migrate_schema(&pool).await?;
            println!("{}", serde_json::json!({"migrated":true}));
        }
        Command::PackageUpdateApply { packages } => {
            println!(
                "{}",
                serde_json::to_string(&manager.apply_offline(packages.clone(), &pool).await?)?
            );
        }
        Command::PackageUpdateVerify => {
            println!(
                "{}",
                serde_json::to_string(&manager.verify_offline(&pool).await?)?
            );
        }
        Command::PackageUpdateRestore => {
            manager.verify_offline(&pool).await?;
            let snapshot =
                original.ok_or_else(|| std::io::Error::other("Missing original activation"))?;
            println!(
                "{}",
                serde_json::to_string(&manager.restore_activation_offline(snapshot).await?)?
            );
        }
        _ => return Err(std::io::Error::other("Invalid offline update command").into()),
    }
    pool.close().await;
    Ok(true)
}
