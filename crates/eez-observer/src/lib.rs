//! Read-only chain observer with optional durable canonical receipt indexing.
pub mod accounting;
mod blobs;
pub mod config;
mod correlation;
pub mod costs;
pub mod database;
pub mod indexer;
pub mod metrics;
pub mod model;
pub mod network;
pub mod outcomes;
mod posting;
pub mod protocol_logs;
pub mod rpc;

use axum::{Router, extract::State, http::header, routing::get};
use config::Config;
use model::{Sample, Source};
use rpc::Rpc;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::RwLock, task::JoinSet};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub const fn retry_delay(failures: u32) -> u64 {
    match failures {
        0 | 1 => 15,
        2 => 30,
        _ => 60,
    }
}

type Samples = Arc<RwLock<Vec<Sample>>>;

#[derive(Clone)]
struct ScrapeState {
    samples: Samples,
    index: Option<indexer::SharedStatus>,
    network: network::SharedStatus,
}

async fn scrape(
    State(state): State<ScrapeState>,
) -> ([(header::HeaderName, &'static str); 1], String) {
    let snapshot = state.samples.read().await.clone();
    let now = now_ms();
    let mut rendered = metrics::render(&snapshot, now);
    rendered.push_str(&network::render(&state.network.read().await, now));
    if let Some(index) = state.index {
        let status = index.read().await;
        rendered.push_str(&indexer::render(&status.chains, now));
        rendered.push_str(&indexer::render_settlements(&status, now));
        rendered.push_str(&indexer::protocol_logs::render(
            status.protocol_logs.as_ref(),
            now,
        ));
        rendered.push_str(&indexer::lifecycle::render(&status.lifecycle, now));
        rendered.push_str(&indexer::source_receipts::render(
            status.source_receipts.as_ref(),
            now,
        ));
        rendered.push_str(&indexer::outcomes::render(status.outcomes.as_ref(), now));
        rendered.push_str(&indexer::outcomes::render_progress(&status, now));
        rendered.push_str(&indexer::costs::render(status.costs.as_ref(), now));
        rendered.push_str(&indexer::blobs::render(status.blobs.as_ref(), now));
        rendered.push_str(&indexer::follower::render(status.follower.as_ref(), now));
        rendered.push_str(&indexer::required_blobs::render(
            status.required_blobs.as_ref(),
            now,
        ));
        rendered.push_str(&indexer::funding::render(&status, now));
    }
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        rendered,
    )
}

/// Run only local telemetry services; this function never submits a transaction.
pub async fn serve(config: Config) -> Result<(), &'static str> {
    serve_runtime(config, None).await
}

pub async fn serve_indexer(config: indexer::Config) -> Result<(), &'static str> {
    config.validate()?;
    let database_config = database::DatabaseConfig::load(&config.database_config)?;
    database::Database::connect(&database_config)
        .await?
        .verify_writer()
        .await?;
    serve_runtime(config.exporter.clone(), Some(config)).await
}

async fn serve_runtime(config: Config, index: Option<indexer::Config>) -> Result<(), &'static str> {
    config.validate()?;
    let rpc = Rpc::new()?;
    let mut endpoints = vec![
        (Source::L1, config.l1.clone()),
        (Source::L2, config.l2.clone()),
        (Source::Queue, config.l2.clone()),
    ];
    let network_workers = network::workers(&config);
    let network_status = network::status(&network_workers);
    if let Some(url) = config.reference_l1_url {
        let mut reference = config.l1;
        reference.url = url;
        endpoints.push((Source::ReferenceL1, reference));
    }
    let samples = Arc::new(RwLock::new(
        endpoints
            .iter()
            .map(|(source, _)| Sample::new(*source))
            .collect::<Vec<_>>(),
    ));
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|_| "cannot bind observer metrics listener")?;
    let mut tasks = JoinSet::new();
    for (index, worker) in network_workers.into_iter().enumerate() {
        let rpc = rpc.clone();
        let status = network_status.clone();
        tasks.spawn(network::run(worker, rpc, status, index));
    }
    let index_status = index
        .as_ref()
        .map(|_| Arc::new(RwLock::new(indexer::RuntimeStatus::default())));
    if let Some(index) = index {
        let status = Arc::clone(index_status.as_ref().expect("index status initialized"));
        let rpc = rpc.clone();
        tasks.spawn(async move {
            Box::pin(indexer::run(index, rpc, status)).await;
        });
    }
    for (index, (source, endpoint)) in endpoints.into_iter().enumerate() {
        let rpc = rpc.clone();
        let samples = Arc::clone(&samples);
        tasks.spawn(async move {
            let mut failures = 0u32;
            loop {
                let result = rpc.collect(source, &endpoint).await;
                failures = if result.is_ok() {
                    0
                } else {
                    failures.saturating_add(1)
                };
                if let Err(error) = &result {
                    tracing::warn!(
                        source = source.label(),
                        reason = error.label(),
                        "RPC collection failed"
                    );
                }
                samples.write().await[index].update(result, now_ms());
                tokio::time::sleep(Duration::from_secs(retry_delay(failures))).await;
            }
        });
    }
    let router = Router::new()
        .route("/metrics", get(scrape))
        .route(
            "/healthz",
            get(|| async { "observer process is running; inspect metrics for source health\n" }),
        )
        .with_state(ScrapeState {
            samples,
            index: index_status,
            network: network_status,
        });
    tracing::info!(listen = %config.listen, "starting read-only EEZ observer");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .into_future();
    tokio::pin!(server);
    let result = tokio::select! {
        result = &mut server => result.map_err(|_| "observer HTTP server failed"),
        _ = tasks.join_next() => Err("observer collection task stopped unexpectedly"),
        _ = shutdown() => {
            // Stop readers/transactions promptly; a slow scrape client must not
            // hold shutdown or a database transaction open without a deadline.
            tasks.abort_all();
            let _ = stop.send(());
            match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
                Ok(result) => result.map_err(|_| "observer HTTP server failed"),
                Err(_) => {
                    tracing::warn!("observer HTTP shutdown drain deadline reached");
                    Ok(())
                }
            }
        }
    };
    tasks.abort_all();
    result
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests;
