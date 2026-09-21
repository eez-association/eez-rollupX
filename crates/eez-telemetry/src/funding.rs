//! Exact candidate funding decisions with approximate diagnostic display values.
//! No account identity, signed transaction or provider call enters telemetry.
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static GENERATION: AtomicU64 = AtomicU64::new(0);
static PUBLICATION: Mutex<()> = Mutex::new(());

/// Process-local ownership fence for overlapping preparation/repricing futures.
/// It never locks a protocol owner or waits for another telemetry publisher.
#[derive(Debug)]
pub struct Attempt(Option<u64>);

impl Attempt {
    pub fn start() -> Self {
        Self::with_enabled(crate::operational::enabled())
    }

    fn with_enabled(enabled: bool) -> Self {
        if !enabled {
            return Self(None);
        }
        for reason in ["contention", "superseded", "exhausted"] {
            metrics::counter!("eez_poster_funding_sample_drops_total", "reason" => reason)
                .increment(0);
        }
        let next = GENERATION.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value.checked_add(1)
        });
        // This invalidation intentionally does not acquire PUBLICATION: a newer
        // attempt must not wait for an old publisher. Its generation change is
        // checked again after that publisher's final validity write.
        invalidate();
        match next {
            Ok(previous) => Self(Some(previous + 1)),
            Err(_) => {
                metrics::counter!("eez_poster_funding_sample_drops_total", "reason" => "exhausted")
                    .increment(1);
                Self(None)
            }
        }
    }

    /// Compare full-width values exactly; floats are display-only.
    pub fn candidate(&self, required: [u8; 32], balance: Option<[u8; 32]>) {
        self.publish(|| {
            record_candidate(required, balance);
            metrics::gauge!("eez_poster_funding_minimum_only").set(0.);
        });
    }

    /// A necessary lower bound establishes insufficiency, not affordability.
    pub fn minimum_unaffordable(&self, required: [u8; 32], balance: [u8; 32]) {
        if balance < required {
            self.publish(|| {
                record_candidate(required, Some(balance));
                metrics::gauge!("eez_poster_funding_minimum_only").set(1.);
            });
        }
    }

    fn publish(&self, emit: impl FnOnce()) {
        let Some(generation) = self.0 else {
            return;
        };
        let Ok(_publication) = PUBLICATION.try_lock() else {
            metrics::counter!("eez_poster_funding_sample_drops_total", "reason" => "contention")
                .increment(1);
            return;
        };
        if GENERATION.load(Ordering::SeqCst) != generation {
            metrics::counter!("eez_poster_funding_sample_drops_total", "reason" => "superseded")
                .increment(1);
            return; // Do not erase a newer successfully published snapshot.
        }
        emit();
        if GENERATION.load(Ordering::SeqCst) != generation {
            invalidate();
            metrics::counter!("eez_poster_funding_sample_drops_total", "reason" => "superseded")
                .increment(1);
        }
    }
}

/// Invalidate the previous candidate before fetching a new fee/balance snapshot.
fn invalidate() {
    metrics::gauge!("eez_poster_funding_estimate_valid").set(0.);
    metrics::gauge!("eez_poster_funding_balance_valid").set(0.);
    metrics::gauge!("eez_poster_can_fund_candidate").set(f64::NAN);
}

fn record_candidate(required: [u8; 32], balance: Option<[u8; 32]>) {
    let Some(timestamp) = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs_f64())
    else {
        invalidate();
        return;
    };
    metrics::gauge!("eez_poster_funding_required_native_units").set(native_units(required));
    metrics::gauge!("eez_poster_funding_balance_native_units")
        .set(balance.map_or(f64::NAN, native_units));
    metrics::gauge!("eez_poster_can_fund_candidate")
        .set(balance.map_or(f64::NAN, |value| f64::from(u8::from(value >= required))));
    metrics::gauge!("eez_poster_funding_balance_valid").set(f64::from(u8::from(balance.is_some())));
    metrics::gauge!("eez_poster_funding_estimate_timestamp_seconds").set(timestamp);
    metrics::gauge!("eez_poster_funding_estimate_valid").set(1.);
}

fn native_units(value: [u8; 32]) -> f64 {
    value
        .into_iter()
        .fold(0., |value, byte| value * 256. + f64::from(byte))
        / 1e18
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "native")]
    #[test]
    fn overlapping_attempts_cannot_restore_superseded_validity() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            Attempt::with_enabled(false).candidate([1; 32], Some([2; 32]));
            assert!(!handle.render().contains("eez_poster_"));
            let old = Attempt::with_enabled(true);
            let newer = Attempt::with_enabled(true);
            old.candidate([1; 32], Some([2; 32]));
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_estimate_valid 0\n")
            );
            newer.candidate([2; 32], Some([1; 32]));
            old.candidate([1; 32], Some([2; 32]));
            assert!(
                handle
                    .render()
                    .contains("eez_poster_can_fund_candidate 0\n")
            );
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_estimate_valid 1\n")
            );
            let publishing = Attempt::with_enabled(true);
            let mut during = None;
            publishing.publish(|| {
                during = Some(Attempt::with_enabled(true));
                record_candidate([1; 32], Some([2; 32]));
            });
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_estimate_valid 0\n")
            );
            during.unwrap().candidate([2; 32], Some([1; 32]));
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_estimate_valid 1\n")
            );
            assert!(
                handle
                    .render()
                    .contains("eez_poster_can_fund_candidate 0\n")
            );
            let held = PUBLICATION.lock().unwrap();
            let contended = Attempt::with_enabled(true);
            contended.candidate([1; 32], Some([2; 32]));
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_estimate_valid 0\n")
            );
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_sample_drops_total{reason=\"contention\"} 1\n")
            );
            drop(held);
            contended.candidate([1; 32], Some([2; 32]));
            assert!(
                handle
                    .render()
                    .contains("eez_poster_funding_estimate_valid 1\n")
            );
        });
    }

    #[test]
    fn candidate_funding_uses_exact_full_width_comparison_not_display_rounding() {
        let mut required = [255; 32];
        let mut insufficient = required;
        insufficient[31] = 254;
        assert!(insufficient < required);
        assert!((native_units(required) - native_units(insufficient)).abs() < f64::EPSILON);
        required[31] = 253;
        assert!(insufficient >= required);
        assert!(native_units([255; 32]).is_finite());
        assert!(native_units([0; 32]).abs() < f64::EPSILON);
    }

    #[cfg(feature = "native")]
    #[test]
    fn unknown_balance_is_not_reported_as_zero_or_affordable() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || record_candidate([1; 32], None));
        let rendered = handle.render();
        let gauge = |name: &str| {
            rendered
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name} ")))
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        assert!(gauge("eez_poster_can_fund_candidate").is_nan());
        assert!(gauge("eez_poster_funding_balance_native_units").is_nan());
        assert!(gauge("eez_poster_funding_balance_valid").abs() < f64::EPSILON);
        assert!((gauge("eez_poster_funding_estimate_valid") - 1.).abs() < f64::EPSILON);
        let required = [255; 32];
        let mut insufficient = required;
        insufficient[31] -= 1;
        metrics::with_local_recorder(&recorder, || record_candidate(required, Some(insufficient)));
        assert!(
            handle
                .render()
                .contains("eez_poster_can_fund_candidate 0\n")
        );
        metrics::with_local_recorder(&recorder, || record_candidate(required, Some(required)));
        assert!(
            handle
                .render()
                .contains("eez_poster_can_fund_candidate 1\n")
        );
    }
}
