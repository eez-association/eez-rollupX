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
    /// Persist a bounded canonical history; schema must already be migrated.
    Indexer {
        #[arg(long)]
        config: PathBuf,
        /// Scan at most one configured batch per chain, then exit (restart-safe backfill).
        #[arg(long)]
        once: bool,
    },
    /// Apply forward observer schema changes using administrative credentials.
    Migrate {
        #[arg(long)]
        database_config: PathBuf,
    },
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
    let result = match Cli::parse().command {
        Command::Exporter { config } => match Config::load(&config) {
            Ok(config) => eez_observer::serve(config).await,
            Err(error) => Err(error),
        },
        Command::Migrate { database_config } => migrate(&database_config).await,
        Command::Indexer { config, once } => Box::pin(index(&config, once)).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(reason = error, "observer stopped");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn index(path: &std::path::Path, once: bool) -> Result<(), &'static str> {
    let config = eez_observer::indexer::Config::load(path)?;
    if once {
        let rpc = eez_observer::rpc::Rpc::new()?;
        let status = std::sync::Arc::new(tokio::sync::RwLock::new(
            eez_observer::indexer::RuntimeStatus::default(),
        ));
        Box::pin(eez_observer::indexer::once(&config, &rpc, &status))
            .await
            .map_err(eez_observer::indexer::Error::label)
    } else {
        eez_observer::serve_indexer(config).await
    }
}

async fn migrate(path: &std::path::Path) -> Result<(), &'static str> {
    let config = eez_observer::database::DatabaseConfig::load(path)?;
    let mut database = eez_observer::database::Database::connect(&config).await?;
    database.migrate().await?;
    tracing::info!(
        version = eez_observer::database::SCHEMA_VERSION,
        "observer schema verified"
    );
    Ok(())
}
