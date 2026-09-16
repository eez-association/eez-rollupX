use clap::{Parser, Subcommand};
use eez_observer::config::Config;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "eez-observer",
    version,
    about = "Read-only EEZ network telemetry"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Exporter {
        #[arg(long)]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter("info")
        .init();
    let Command::Exporter { config } = Cli::parse().command;
    let result = match Config::load(&config) {
        Ok(config) => eez_observer::serve(config).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(reason = error, "observer stopped");
            std::process::ExitCode::FAILURE
        }
    }
}
