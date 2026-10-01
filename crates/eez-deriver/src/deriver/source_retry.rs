//! Startup authentication of the L1 derivation source.
//!
//! Startup first waits for the L1 source to serve the genesis reference
//! (`eez_node_common::wait_for_l1_ready`), but the source can still drop out
//! before the deriver authenticates it, for example while an embedded L1 is
//! restarted. Treat that window like the readiness wait: retry
//! [`DerivationError::Unavailable`] with the same poll interval, and fail on
//! any other error at once, because a mismatched or invalid reference never
//! becomes valid by waiting.

use std::time::Duration;

use eez_l1::derivation::DerivationError;
use tracing::{Level, event};

/// Same cadence as the L1 readiness wait.
pub(super) const POLL: Duration = Duration::from_secs(2);
/// Bounded like the readiness wait's no-progress window (15 minutes).
pub(super) const MAX_ATTEMPTS: u32 = 450;

/// Run `authenticate` until it succeeds, returns a non-transient error, or
/// `max_attempts` transient failures have occurred; the last transient error
/// is returned in that case.
pub(super) async fn retry_unavailable<T, F, Fut>(
    poll: Duration,
    max_attempts: u32,
    mut authenticate: F,
) -> Result<T, DerivationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, DerivationError>>,
{
    let mut attempt = 1;
    loop {
        match authenticate().await {
            Err(error @ DerivationError::Unavailable(_)) if attempt < max_attempts => {
                event!(
                    name: "eez.deriver.derivation_source_unavailable",
                    Level::WARN,
                    attempt,
                    %error,
                    "L1 derivation source unavailable at startup; retrying",
                );
                attempt += 1;
                tokio::time::sleep(poll).await;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test(start_paused = true)]
    async fn transient_unavailability_is_retried_until_the_source_answers() {
        let calls = AtomicU32::new(0);
        let result = retry_unavailable(POLL, MAX_ATTEMPTS, || async {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => Err(DerivationError::Unavailable("chain identity")),
                _ => Ok(7_u32),
            }
        })
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn mismatched_reference_fails_without_retrying() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, _> = retry_unavailable(POLL, MAX_ATTEMPTS, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(DerivationError::Mismatch(
                "genesis reference chain identity",
            ))
        })
        .await;
        assert!(matches!(result, Err(DerivationError::Mismatch(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_unavailability_is_bounded() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, _> = retry_unavailable(POLL, 3, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(DerivationError::Unavailable("chain identity"))
        })
        .await;
        assert!(matches!(
            result,
            Err(DerivationError::Unavailable("chain identity"))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
