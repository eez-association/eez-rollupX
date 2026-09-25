use anyhow::{Context, Result, ensure};
use eez_indexer::{
    config::Config,
    database::Database,
    rpc::{self, Rpc},
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("eez_indexer=info")
        .init();
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3 && args[1] == "--config",
        "usage: eez-indexer --config CONFIG.json (DATABASE_URL environment required)"
    );
    let config: Config = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    config.validate()?;
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let rpc = Rpc::new(&config.http_url)?;
    rpc.identity(&config).await?;
    let mut db = Database::connect(&database_url, &config).await?;
    let postings = config.posting_logs_dir.as_ref().map(|_| {
        tokio::spawn(eez_indexer::postings::run(
            config.clone(),
            database_url.clone(),
        ))
    });
    let heads = tokio::spawn(eez_indexer::heads::run(
        config.clone(),
        database_url.clone(),
    ));
    let details = config.details.as_ref().map(|_| {
        tokio::spawn(eez_indexer::details::run(
            config.clone(),
            database_url.clone(),
        ))
    });
    let (wake, mut changed) = tokio::sync::watch::channel(0);
    let listener = tokio::spawn(rpc::notifications(config.clone(), wake));
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let work = async {
        loop {
            tokio::select! { _ = tick.tick() => {}, _ = changed.changed() => {} }
            loop {
                match eez_indexer::sync(&config, &rpc, &mut db).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(error) => {
                        if db.client.is_closed() {
                            return Err(error);
                        }
                        tracing::warn!(%error, "indexing cycle failed; checkpoint retained for retry");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        break;
                    }
                }
            }
        }
    };
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let stop = async {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    };
    #[cfg(not(unix))]
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let result = tokio::select! { result = work => result, _ = stop => Ok(()) };
    listener.abort();
    if let Some(postings) = postings {
        postings.abort();
    }
    heads.abort();
    if let Some(details) = details {
        details.abort();
    }
    result
}
