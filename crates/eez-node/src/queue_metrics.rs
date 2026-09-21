//! Composer-owned queue gauges on Reth's existing recorder and HTTP endpoint.
//! Reth adds its `reth_` prefix to these names. Only the held pool feeds them.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use eez_composer::{Composer, HeldPool, held_pool::QueueSnapshot, ingress::Direction};
use metrics::{Gauge, Unit};
use reth_tasks::TaskExecutor;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

struct QueueMetrics {
    counts: [(Direction, Gauge, Gauge); 2],
    readiness: [Gauge; 3],
    blocked: [(&'static str, Gauge); 4],
    oldest_pending_at: Gauge,
    evaluated_at: Gauge,
    last_completion_at: Gauge,
    completions: [metrics::Counter; 2],
    sampled_at: Gauge,
}

impl QueueMetrics {
    fn new(rollup_id: u64) -> Self {
        metrics::describe_gauge!(
            "eez_composer_queue_requests",
            Unit::Count,
            "Held cross-chain requests by direction and reservation state; in_flight includes awaiting settlement evidence"
        );
        metrics::describe_gauge!(
            "eez_composer_queue_sample_timestamp_seconds",
            Unit::Seconds,
            "Unix timestamp of the last successful held-pool sample, not a readiness evaluation or settlement timestamp"
        );
        metrics::describe_gauge!(
            "eez_composer_queue_readiness_requests",
            Unit::Count,
            "Queued requests classified against the last evaluated source state; consult evaluation timestamp"
        );
        metrics::describe_gauge!(
            "eez_composer_queue_blocked_requests",
            Unit::Count,
            "Queued requests blocked by source condition, including affected nonce successors"
        );
        metrics::describe_gauge!(
            "eez_composer_queue_oldest_pending_timestamp_seconds",
            Unit::Seconds,
            "Earliest retained admission time among queued and in-flight requests; NaN when absent"
        );
        metrics::describe_gauge!(
            "eez_composer_queue_evaluation_timestamp_seconds",
            Unit::Seconds,
            "Start time of the last source-state evaluation; NaN before evaluation or after invalidation"
        );
        let rollup = rollup_id.to_string();
        metrics::describe_counter!(
            "eez_composer_queue_completion_observations_total",
            Unit::Count,
            "Canonical user completion observations since restart; reobservation after reorg counts again"
        );
        metrics::describe_gauge!(
            "eez_composer_queue_last_completion_timestamp_seconds",
            Unit::Seconds,
            "Local observation time of the latest retained canonical cross-chain completion; NaN when unknown or invalidated"
        );
        Self {
            counts: [(Direction::Inbound, "inbound"), (Direction::Outbound, "outbound")]
                .map(|(direction, name)| (direction,
                    metrics::gauge!("eez_composer_queue_requests", "rollup_id" => rollup.clone(), "direction" => name, "state" => "queued"),
                    metrics::gauge!("eez_composer_queue_requests", "rollup_id" => rollup.clone(), "direction" => name, "state" => "in_flight"))),
            readiness: ["ready", "blocked", "unknown"].map(|status|
                metrics::gauge!("eez_composer_queue_readiness_requests", "rollup_id" => rollup.clone(), "status" => status)),
            blocked: ["nonce", "fee", "balance", "escrow"].map(|reason|
                (reason, metrics::gauge!("eez_composer_queue_blocked_requests", "rollup_id" => rollup.clone(), "reason" => reason))),
            oldest_pending_at: metrics::gauge!("eez_composer_queue_oldest_pending_timestamp_seconds", "rollup_id" => rollup.clone()),
            evaluated_at: metrics::gauge!("eez_composer_queue_evaluation_timestamp_seconds", "rollup_id" => rollup.clone()),
            last_completion_at: metrics::gauge!("eez_composer_queue_last_completion_timestamp_seconds", "rollup_id" => rollup.clone()),
            completions: ["inbound", "outbound"].map(|direction|
                metrics::counter!("eez_composer_queue_completion_observations_total", "rollup_id" => rollup.clone(), "direction" => direction)),
            sampled_at: metrics::gauge!("eez_composer_queue_sample_timestamp_seconds", "rollup_id" => rollup),
        }
    }

    #[allow(clippy::cast_precision_loss)] // Counts cannot practically approach f64's integer limit.
    fn record(&self, snapshot: QueueSnapshot, sampled_at: f64) {
        for (counter, count) in self
            .completions
            .iter()
            .zip(snapshot.completion_observations)
        {
            counter.absolute(count);
        }
        for (direction, counts) in snapshot.counts {
            let (_, queued, in_flight) = self
                .counts
                .iter()
                .find(|(d, _, _)| *d == direction)
                .expect("both directions registered");
            queued.set(counts.queued as f64);
            in_flight.set(counts.in_flight as f64);
        }
        for (gauge, count) in
            self.readiness
                .iter()
                .zip([snapshot.ready, snapshot.blocked, snapshot.unknown])
        {
            gauge.set(count as f64);
        }
        for (reason, gauge) in &self.blocked {
            gauge.set(
                snapshot
                    .blocked_reasons
                    .get(reason)
                    .copied()
                    .unwrap_or_default() as f64,
            );
        }
        let seconds = |time: Option<u64>| time.map_or(f64::NAN, |ms| ms as f64 / 1000.0);
        self.oldest_pending_at
            .set(seconds(snapshot.oldest_pending_at_ms));
        self.evaluated_at.set(seconds(snapshot.evaluated_at_ms));
        self.last_completion_at
            .set(seconds(snapshot.last_completion_at_ms));
        self.sampled_at.set(sampled_at);
    }
}

/// Register after L2 launch has installed Reth's recorder; start only when the
/// existing `--metrics` endpoint is enabled. Reth cancels this task on shutdown.
pub(crate) fn spawn<L2>(
    pool: Arc<HeldPool>,
    composer: Composer<L2>,
    rollup_id: u64,
    executor: &TaskExecutor,
) where
    L2: reth_storage_api::BlockReader + Clone + Send + Sync + 'static,
{
    let metrics = QueueMetrics::new(rollup_id);
    let settlement_metrics = crate::settlement_metrics::SettlementMetrics::new(rollup_id);
    let worker = executor.clone();
    executor.spawn_task(async move {
        let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let pool = Arc::clone(&pool);
            let composer = composer.clone();
            // Waiting on the pool mutex and scanning membership must not block
            // a Tokio worker. At most one sample is outstanding at a time.
            let sample = worker
                .spawn_blocking(move || {
                    // Age the sample from before lock waits: a slow policy read
                    // must not make the earlier queue snapshot appear fresh.
                    let sampled_at = SystemTime::now().duration_since(UNIX_EPOCH)?;
                    let counts = pool.queue_snapshot();
                    // Queue lock is released before reading policy or attempt state.
                    let settlement = composer.settlement_status(rollup_id);
                    Ok::<_, std::time::SystemTimeError>((
                        counts,
                        settlement,
                        sampled_at.as_secs_f64(),
                    ))
                })
                .await;
            match sample {
                Ok(Ok((counts, settlement, timestamp))) => {
                    settlement_metrics.record(settlement.as_ref());
                    metrics.record(counts, timestamp);
                }
                error => {
                    tracing::warn!(
                        ?error,
                        "queue metrics sampling stopped; last sample will become stale"
                    );
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, Bytes};
    use eez_composer::held_pool::HeldTx;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use metrics_util::layers::{PrefixLayer, Stack};

    #[test]
    fn exported_observations_reset_reasons_and_preserve_unknown_timestamps() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let recorder = Stack::new(recorder).push(PrefixLayer::new("reth"));
        let metrics = metrics::with_local_recorder(&recorder, || QueueMetrics::new(7));
        let pool = HeldPool::new(Default::default());
        let mut snapshot = pool.queue_snapshot();
        snapshot.counts[0].1.queued = 7;
        snapshot.ready = 1;
        snapshot.blocked = 5;
        snapshot.unknown = 1;
        snapshot.blocked_reasons = [("fee", 2), ("nonce", 1), ("balance", 1), ("escrow", 1)].into();
        snapshot.oldest_pending_at_ms = Some(1_250);
        snapshot.evaluated_at_ms = Some(2_500);
        snapshot.last_completion_at_ms = Some(1_500);
        snapshot.completion_observations = [2, 3];
        metrics.record(snapshot.clone(), 3.0);
        metrics.record(snapshot, 3.0);
        let output = handle.render();
        for (direction, count) in [("inbound", 2), ("outbound", 3)] {
            assert!(
                output.lines().any(|line| {
                    line.starts_with("reth_eez_composer_queue_completion_observations_total{")
                        && line.contains(&format!("direction=\"{direction}\""))
                        && line.ends_with(&format!(" {count}"))
                }),
                "repeated sampling must not double-count completions"
            );
        }
        assert!(output.contains(
            "reth_eez_composer_queue_last_completion_timestamp_seconds{rollup_id=\"7\"} 1.5"
        ));
        assert!(output.contains(
            "reth_eez_composer_queue_oldest_pending_timestamp_seconds{rollup_id=\"7\"} 1.25"
        ));
        assert!(
            output.contains(
                "reth_eez_composer_queue_evaluation_timestamp_seconds{rollup_id=\"7\"} 2.5"
            )
        );
        let reason_values = |output: &str| {
            output
                .lines()
                .filter(|line| line.starts_with("reth_eez_composer_queue_blocked_requests{"))
                .map(|line| line.rsplit_once(' ').unwrap().1.parse::<usize>().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(reason_values(&output).iter().sum::<usize>(), 5);
        metrics.record(pool.queue_snapshot(), 4.0);
        let empty = handle.render();
        assert_eq!(reason_values(&empty), vec![0; 4]);
        for family in ["evaluation", "oldest_pending", "last_completion"] {
            assert!(empty.contains(&format!(
                "reth_eez_composer_queue_{family}_timestamp_seconds{{rollup_id=\"7\"}} NaN"
            )));
        }
        assert!(
            empty.contains("reth_eez_composer_queue_sample_timestamp_seconds{rollup_id=\"7\"} 4")
        );
        assert_eq!(
            empty
                .lines()
                .filter(|line| line.starts_with("reth_eez_composer_queue_"))
                .count(),
            17
        );
    }

    #[test]
    #[allow(clippy::float_cmp)] // Exported small integer counts must match exactly.
    fn exported_gauges_follow_pool_membership_and_keep_rollups_separate() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let recorder = Stack::new(recorder).push(PrefixLayer::new("reth"));
        let (first, second) = metrics::with_local_recorder(&recorder, || {
            (QueueMetrics::new(1), QueueMetrics::new(2))
        });
        let pool = HeldPool::new(Default::default());
        let sample = |at| first.record(pool.queue_snapshot(), at);
        let value = |rollup, direction: &str, state: &str| {
            let rendered = handle.render();
            let line = rendered
                .lines()
                .find(|line| {
                    line.starts_with("reth_eez_composer_queue_requests{")
                        && line.contains(&format!("rollup_id=\"{rollup}\""))
                        && line.contains(&format!("direction=\"{direction}\""))
                        && line.contains(&format!("state=\"{state}\""))
                })
                .unwrap()
                .to_owned();
            line.rsplit_once(' ').unwrap().1.parse::<f64>().unwrap()
        };
        sample(100.0);
        second.record(pool.queue_snapshot(), 100.0);
        assert_eq!(value(1, "inbound", "queued"), 0.0);
        for (byte, direction) in [(1, Direction::Inbound), (2, Direction::Outbound)] {
            pool.push_contiguous(
                HeldTx {
                    raw_tx: Bytes::from(vec![byte]),
                    hash: B256::repeat_byte(byte),
                    attempts: 0,
                    sender: Address::repeat_byte(byte),
                    nonce: 0,
                    direction,
                    max_fee_per_gas: 1,
                    priority_fee_per_gas: 1,
                },
                0,
            )
            .unwrap();
        }
        sample(105.0);
        assert_eq!(value(1, "inbound", "queued"), 1.0);
        assert_eq!(value(1, "outbound", "queued"), 1.0);
        assert_eq!(value(2, "inbound", "queued"), 0.0);
        let drained = pool.pop_all();
        sample(110.0);
        assert_eq!(value(1, "inbound", "queued"), 0.0);
        assert_eq!(value(1, "outbound", "in_flight"), 1.0);
        pool.release_in_flight_batch(&drained);
        sample(115.0);
        assert_eq!(value(1, "outbound", "in_flight"), 0.0);
        assert!(
            handle
                .render()
                .contains("reth_eez_composer_queue_sample_timestamp_seconds{rollup_id=\"1\"} 115")
        );
        assert_eq!(
            handle
                .render()
                .lines()
                .filter(|line| line.starts_with("reth_eez_composer_queue_requests{"))
                .count(),
            8
        );
    }
}
