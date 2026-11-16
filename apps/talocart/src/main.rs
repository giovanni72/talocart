mod cli;
mod commands;
mod commands_update;
mod diagnostics;
mod logging;
mod preflight;
mod runtime;
mod schema;
mod update_control;
mod verification;
mod workers;

use clap::Parser;
use cli::Cli;
use commands::run;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let filter = match talocart_platform::config::environment("TALOCART_LOG") {
        Ok(Some(value)) => match EnvFilter::try_new(value) {
            Ok(filter) => filter,
            Err(_) => {
                eprintln!("Invalid TALOCART_LOG");
                return ExitCode::FAILURE;
            }
        },
        Ok(None) => EnvFilter::new("info,tower_http=info,sqlx=warn"),
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let cli = Cli::parse();
    let config = if matches!(cli.command, cli::Command::Serve | cli::Command::ServeVerify) {
        match talocart_platform::config::Config::load(cli.config.as_deref()) {
            Ok(config) => Some(config),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let _logging = match logging::initialize(
        filter,
        config
            .as_ref()
            .filter(|_| matches!(cli.command, cli::Command::Serve))
            .map(|c| &c.logging),
    ) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match run(cli, config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Errors exposed here are intentionally sanitized by the platform layer.
            tracing::error!(error = %error, "Operation failed");
            ExitCode::FAILURE
        }
    }
}
