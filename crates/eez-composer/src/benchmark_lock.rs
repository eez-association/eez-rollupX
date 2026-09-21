//! Non-shipping HeldPool lock measurement. This module is absent from default
//! builds; samples use the existing private JSON log only after unlocking.
use std::{
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        Arc, LockResult, MutexGuard as StdGuard, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const SAMPLE_LIMIT: u64 = 65_536;
static CONFIG: OnceLock<Option<Arc<Probe>>> = OnceLock::new();
static POOL_IDS: AtomicU64 = AtomicU64::new(1);

/// Configure the opt-in probe before protocol startup. The flag is intentionally
/// consumed only by the explicitly identified benchmark build, never shipping.
pub fn configure_from_env() -> Result<(), &'static str> {
    let configured = match std::env::var("EEZ_BENCH_HELD_POOL_TIMING") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return Err("invalid benchmark lock timing flag"),
    };
    let probe = parse_enabled(configured.as_deref())?
        .then(Probe::new)
        .transpose()?
        .map(Arc::new);
    CONFIG
        .set(probe)
        .map_err(|_| "benchmark lock timing initialized twice")
}

fn parse_enabled(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err("benchmark lock timing requires true or false"),
    }
}

struct Probe {
    origin: Instant,
    unix_before_ns: u64,
    unix_after_ns: u64,
    sequence: AtomicU64,
    #[cfg(test)]
    hook: Option<
        Box<dyn Fn(Sample) + Send + Sync + std::panic::RefUnwindSafe + std::panic::UnwindSafe>,
    >,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    pool: u64,
    callsite: &'static std::panic::Location<'static>,
    started_ns: u64,
    acquired_ns: u64,
    released_ns: u64,
}

fn unix_ns() -> Result<u64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "benchmark wall clock predates epoch")?
        .as_nanos()
        .try_into()
        .map_err(|_| "benchmark wall clock exceeds u64 nanoseconds")
}

impl Probe {
    fn new() -> Result<Self, &'static str> {
        let unix_before_ns = unix_ns()?;
        let origin = Instant::now();
        let unix_after_ns = unix_ns()?;
        if unix_after_ns < unix_before_ns {
            return Err("benchmark wall clock moved backwards during initialization");
        }
        Ok(Self {
            origin,
            unix_before_ns,
            unix_after_ns,
            sequence: AtomicU64::new(0),
            #[cfg(test)]
            hook: None,
        })
    }

    fn now_ns(&self) -> u64 {
        self.origin
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn record(&self, sample: Sample) {
        // This function must run only after dropping the actual protocol guard.
        let Ok(previous) =
            self.sequence
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |sequence| {
                    (sequence <= SAMPLE_LIMIT).then_some(sequence + 1)
                })
        else {
            return;
        };
        let sequence = previous + 1;
        if sequence > SAMPLE_LIMIT {
            if sequence == SAMPLE_LIMIT + 1 {
                tracing::info!(
                    event_name = "eez.benchmark.held_pool_overflow",
                    sample_limit = SAMPLE_LIMIT,
                    "bounded HeldPool benchmark sample limit exceeded"
                );
            }
            return;
        }
        #[cfg(test)]
        if let Some(hook) = &self.hook {
            hook(sample);
        }
        let clock_before_ns = self.now_ns();
        let wall = unix_ns();
        let clock_after_ns = self.now_ns();
        let Ok(observed_unix_ns) = wall else {
            tracing::info!(
                event_name = "eez.benchmark.held_pool_clock_error",
                sequence,
                "HeldPool benchmark wall clock unavailable"
            );
            return;
        };
        tracing::info!(
            event_name = "eez.benchmark.held_pool_sample",
            sequence,
            pool = sample.pool,
            callsite_file = sample.callsite.file(),
            callsite_line = sample.callsite.line(),
            callsite_column = sample.callsite.column(),
            origin_unix_before_ns = self.unix_before_ns,
            origin_unix_after_ns = self.unix_after_ns,
            started_ns = sample.started_ns,
            acquired_ns = sample.acquired_ns,
            released_ns = sample.released_ns,
            wait_ns = sample.acquired_ns.saturating_sub(sample.started_ns),
            hold_ns = sample.released_ns.saturating_sub(sample.acquired_ns),
            clock_before_ns,
            observed_unix_ns,
            clock_after_ns,
            "bounded HeldPool benchmark lock sample"
        );
    }
}

/// Only HeldPool's private state mutex selects this wrapper in a benchmark build.
pub(crate) struct Mutex<T> {
    inner: std::sync::Mutex<T>,
    probe: Option<Arc<Probe>>,
    pool: u64,
}
impl<T: fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}
impl<T> Mutex<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            inner: std::sync::Mutex::new(value),
            probe: CONFIG.get().cloned().flatten(),
            pool: POOL_IDS.fetch_add(1, Ordering::Relaxed),
        }
    }
    #[track_caller]
    pub(crate) fn lock(&self) -> LockResult<Guard<'_, T>> {
        let callsite = std::panic::Location::caller();
        let started = self.probe.as_ref().map(|probe| (probe, probe.now_ns()));
        let result = self.inner.lock();
        let wrap = |inner| Guard {
            inner: Some(inner),
            owner: self,
            callsite,
            timing: started.map(|(probe, started_ns)| (started_ns, probe.now_ns())),
        };
        match result {
            Ok(inner) => Ok(wrap(inner)),
            Err(error) => Err(PoisonError::new(wrap(error.into_inner()))),
        }
    }
}

pub(crate) struct Guard<'a, T> {
    inner: Option<StdGuard<'a, T>>,
    owner: &'a Mutex<T>,
    callsite: &'static std::panic::Location<'static>,
    timing: Option<(u64, u64)>,
}
impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // Only Drop removes this guard; callers cannot observe that state.
        self.inner.as_ref().expect("live benchmark guard").deref()
    }
}
impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner
            .as_mut()
            .expect("live benchmark guard")
            .deref_mut()
    }
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        // Timestamp the boundary before unlock, so scheduler preemption after
        // unlock cannot be attributed to time holding the protocol guard.
        let release_begin_ns = self.owner.probe.as_ref().map(|probe| probe.now_ns());
        drop(self.inner.take());
        if let (Some(probe), Some((started_ns, acquired_ns)), Some(released_ns)) =
            (&self.owner.probe, self.timing, release_begin_ns)
        {
            probe.record(Sample {
                pool: self.owner.pool,
                callsite: self.callsite,
                started_ns,
                acquired_ns,
                released_ns,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn probe_flag_is_explicit_and_disabled_by_default() {
        assert_eq!(parse_enabled(None), Ok(false));
        assert_eq!(parse_enabled(Some("false")), Ok(false));
        assert_eq!(parse_enabled(Some("true")), Ok(true));
        for invalid in ["", "1", "TRUE", " false ", "yes"] {
            assert!(parse_enabled(Some(invalid)).is_err());
        }
    }

    #[test]
    fn data_remains_locked_until_guard_drop_and_recording_happens_after_unlock() {
        let reference = Arc::new(OnceLock::<std::sync::Weak<Mutex<u64>>>::new());
        let observed = reference.clone();
        let (send, received) = mpsc::channel();
        let mut probe = Probe::new().unwrap();
        probe.hook = Some(Box::new(move |sample| {
            let mutex = observed.get().unwrap().upgrade().unwrap();
            assert!(
                mutex.inner.try_lock().is_ok(),
                "recording still owns protocol lock"
            );
            send.send(sample).unwrap();
        }));
        let mutex = Arc::new(Mutex {
            inner: std::sync::Mutex::new(7),
            probe: Some(Arc::new(probe)),
            pool: 4,
        });
        reference.set(Arc::downgrade(&mutex)).unwrap();
        let mut guard = mutex.lock().unwrap();
        *guard = 9;
        assert!(mutex.inner.try_lock().is_err());
        assert!(received.try_recv().is_err());
        drop(guard);
        let sample = received.recv().unwrap();
        assert_eq!(sample.pool, 4);
        assert_eq!(sample.callsite.file(), file!());
        assert!(sample.callsite.line() > 0);
        assert!(
            sample.started_ns <= sample.acquired_ns && sample.acquired_ns <= sample.released_ns
        );
        assert_eq!(*mutex.inner.lock().unwrap(), 9);
    }

    #[test]
    fn poisoning_and_recovery_preserve_standard_mutex_semantics() {
        let mutex = Mutex {
            inner: std::sync::Mutex::new(7),
            probe: Some(Arc::new(Probe::new().unwrap())),
            pool: 1,
        };
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut guard = mutex.lock().unwrap();
            *guard = 9;
            panic!("fixture panic while holding guard");
        }));
        assert!(failure.is_err());
        let poisoned = mutex.lock().err().expect("std poisoning preserved");
        assert_eq!(*poisoned.into_inner(), 9);
        assert!(mutex.inner.is_poisoned());
    }

    #[test]
    fn sample_limit_stops_recording_and_retains_explicit_overflow_state() {
        let sampled = Arc::new(AtomicU64::new(0));
        let counter = sampled.clone();
        let mut probe = Probe::new().unwrap();
        probe.sequence.store(SAMPLE_LIMIT - 1, Ordering::Relaxed);
        probe.hook = Some(Box::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        for _ in 0..8 {
            probe.record(Sample {
                pool: 1,
                callsite: std::panic::Location::caller(),
                started_ns: 0,
                acquired_ns: 1,
                released_ns: 2,
            });
        }
        assert_eq!(sampled.load(Ordering::Relaxed), 1);
        assert_eq!(probe.sequence.load(Ordering::Relaxed), SAMPLE_LIMIT + 1);
    }

    #[test]
    fn disabled_probe_leaves_no_measurement_state_on_the_guard() {
        let mutex = Mutex {
            inner: std::sync::Mutex::new(7),
            probe: None,
            pool: 1,
        };
        let guard = mutex.lock().unwrap();
        assert!(guard.timing.is_none());
        assert_eq!(*guard, 7);
    }
}
