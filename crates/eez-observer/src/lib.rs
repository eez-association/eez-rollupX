//! Read-only EEZ observer. This first implementation is an RPC exporter only.
pub mod accounting;
pub mod config;
pub mod metrics;
pub mod model;
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

async fn scrape(
    State(samples): State<Samples>,
) -> ([(header::HeaderName, &'static str); 1], String) {
    let snapshot = samples.read().await.clone();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        metrics::render(&snapshot, now_ms()),
    )
}

/// Run only local telemetry services; this function never submits a transaction.
pub async fn serve(config: Config) -> Result<(), &'static str> {
    config.validate()?;
    let rpc = Rpc::new()?;
    let mut endpoints = vec![
        (Source::L1, config.l1.clone()),
        (Source::L2, config.l2.clone()),
        (Source::Queue, config.l2.clone()),
    ];
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
        .with_state(samples);
    tracing::info!(listen = %config.listen, "starting read-only EEZ observer");
    let server = axum::serve(listener, router).with_graceful_shutdown(shutdown());
    tokio::select! {
        result = server => { result.map_err(|_| "observer HTTP server failed")?; }
        _ = tasks.join_next() => { return Err("observer collection task stopped unexpectedly"); }
    }
    tasks.abort_all();
    Ok(())
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
