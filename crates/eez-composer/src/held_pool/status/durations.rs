//! Optional bounded monotonic observations, never queue admission/ownership state.
use super::Direction;
use alloy_primitives::Address;
use eez_telemetry::operational::{EVALUATION_MAX_AGE, ReadinessDurations};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Blocked,
    Serviceable,
    Unknown,
}
impl Kind {
    fn index(self) -> usize {
        match self {
            Self::Blocked => 0,
            Self::Serviceable => 1,
            Self::Unknown => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::held_pool) struct ReadinessClock {
    pub(super) chain: (Address, Direction, u64),
    origin: Instant,
    cursor: Instant,
    observed_at: Option<Instant>,
    kind: Kind,
    parts: [Duration; 3],
    valid: bool,
}
impl ReadinessClock {
    pub(super) fn new(origin: Instant, chain: (Address, Direction, u64)) -> Self {
        Self {
            chain,
            origin,
            cursor: origin,
            observed_at: None,
            kind: Kind::Unknown,
            parts: [Duration::ZERO; 3],
            valid: true,
        }
    }
    fn advance(&mut self, now: Instant) -> Option<()> {
        now.checked_duration_since(self.cursor)?;
        let known_end = if self.kind == Kind::Unknown {
            self.cursor
        } else {
            now.min(self.observed_at?.checked_add(EVALUATION_MAX_AGE)?)
                .max(self.cursor)
        };
        let known = known_end.checked_duration_since(self.cursor)?;
        let unknown = now.checked_duration_since(known_end)?;
        self.parts[self.kind.index()] = self.parts[self.kind.index()].checked_add(known)?;
        self.parts[2] = self.parts[2].checked_add(unknown)?;
        self.cursor = now;
        Some(())
    }
    pub(super) fn transition(&mut self, kind: Kind, now: Instant) {
        self.valid &= self.advance(now).is_some();
        self.kind = kind;
        self.observed_at = (kind != Kind::Unknown).then_some(now);
    }
    pub(super) fn blocked(&self) -> bool {
        self.kind == Kind::Blocked
    }
    pub(in crate::held_pool) fn finish(
        mut self,
        origin: Instant,
        now: Instant,
    ) -> Option<ReadinessDurations> {
        if !self.valid || self.origin != origin {
            return None;
        }
        self.advance(now)?;
        let elapsed = now.checked_duration_since(origin)?;
        (self
            .parts
            .iter()
            .try_fold(Duration::ZERO, |sum, part| sum.checked_add(*part))?
            == elapsed)
            .then_some(ReadinessDurations { parts: self.parts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clock(at: Instant) -> ReadinessClock {
        ReadinessClock::new(at, (Address::ZERO, Direction::Inbound, 0))
    }
    #[test]
    fn transitions_partition_total_and_expiry_is_unknown_without_a_sampler_tick() {
        let start = Instant::now();
        let mut c = clock(start);
        c.transition(Kind::Blocked, start + Duration::from_secs(5));
        c.transition(Kind::Serviceable, start + Duration::from_secs(12));
        c.transition(Kind::Serviceable, start + Duration::from_secs(20));
        let parts = c
            .finish(start, start + Duration::from_secs(90))
            .unwrap()
            .parts;
        assert_eq!(
            parts,
            [
                Duration::from_secs(7),
                Duration::from_secs(68),
                Duration::from_secs(15)
            ]
        );
        assert_eq!(parts.iter().sum::<Duration>(), Duration::from_secs(90));
    }
    #[test]
    fn explicit_invalidation_closes_known_time_and_future_clock_is_missing() {
        let start = Instant::now();
        let mut c = clock(start);
        c.transition(Kind::Serviceable, start + Duration::from_secs(2));
        c.transition(Kind::Unknown, start + Duration::from_secs(7));
        assert_eq!(
            c.finish(start, start + Duration::from_secs(10))
                .unwrap()
                .parts,
            [
                Duration::ZERO,
                Duration::from_secs(5),
                Duration::from_secs(5)
            ]
        );
        assert!(c.finish(start, start).is_none());
        assert!(
            c.finish(
                start + Duration::from_secs(1),
                start + Duration::from_secs(10)
            )
            .is_none()
        );
    }
}
