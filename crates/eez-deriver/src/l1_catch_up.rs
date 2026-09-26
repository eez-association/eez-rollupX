//! Best-effort L1 catch-up before validating persistent metadata.

use std::time::Duration;

const POLL: Duration = Duration::from_secs(2);
const WAIT: Duration = Duration::from_mins(1);

pub(crate) async fn wait_for_head<F, Probe>(block: u64, mut probe: F) -> bool
where
    F: FnMut() -> Probe,
    Probe: Future<Output = Option<u64>>,
{
    // Include the complete RPC probe in the deadline. A poll-count limit alone
    // neither bounds a stalled request nor accounts for time spent in slow RPCs.
    tokio::time::timeout(WAIT, async {
        loop {
            if probe().await.is_some_and(|head| head >= block) {
                return;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .is_ok()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use tokio::time::{Instant, sleep, timeout};

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn stalled_rpc_cannot_exceed_the_catch_up_deadline() {
        let start = Instant::now();
        let result = timeout(
            WAIT + POLL,
            wait_for_head(100, std::future::pending::<Option<u64>>),
        )
        .await
        .expect("a stalled RPC must not bypass the catch-up deadline");
        assert!(!result);
        assert_eq!(start.elapsed(), WAIT);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_rpc_time_counts_towards_the_same_deadline() {
        let start = Instant::now();
        let result = timeout(
            WAIT + POLL,
            wait_for_head(100, || async {
                sleep(Duration::from_secs(9)).await;
                Some(99)
            }),
        )
        .await
        .expect("RPC latency and sleeps must share one catch-up deadline");
        assert!(!result);
        assert_eq!(start.elapsed(), WAIT);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_error_and_lag_are_retried_until_the_head_catches_up() {
        let calls = Cell::new(0);
        let start = Instant::now();
        assert!(
            wait_for_head(100, || {
                calls.set(calls.get() + 1);
                std::future::ready(match calls.get() {
                    1 => None,
                    2 => Some(99),
                    _ => Some(100),
                })
            })
            .await
        );
        assert_eq!(calls.get(), 3);
        assert_eq!(start.elapsed(), 2 * POLL);
    }

    #[tokio::test(start_paused = true)]
    async fn current_head_needs_no_delay() {
        let start = Instant::now();
        assert!(wait_for_head(100, || std::future::ready(Some(101))).await);
        assert_eq!(start.elapsed(), Duration::ZERO);
    }
}
