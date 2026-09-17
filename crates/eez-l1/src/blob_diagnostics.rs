//! A bounded diagnostic of the currently failing native blob dependency.
//! This state does not participate in retry, decoding, or execution decisions.
use alloy_primitives::B256;
use std::{sync::Mutex, time::Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredBlob {
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub rollup_id: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobUnavailableReason {
    BeaconResponse,
    ExpectedHashMissing,
}
impl BlobUnavailableReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::BeaconResponse => "beacon_unavailable",
            Self::ExpectedHashMissing => "expected_hash_missing",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobConsumer {
    Deriver,
    Watcher,
}
#[derive(Debug, Clone, Copy)]
pub struct RequiredBlobFailure {
    pub reference: RequiredBlob,
    pub reason: BlobUnavailableReason,
    pub attempts: u32,
    pub age_ms: u64,
    pub consumer: BlobConsumer,
}
#[derive(Debug, Default)]
pub struct RequiredBlobState(
    Mutex<
        Option<(
            RequiredBlob,
            BlobUnavailableReason,
            u32,
            Instant,
            BlobConsumer,
        )>,
    >,
);
impl RequiredBlobState {
    pub fn observe(&self, consumer: BlobConsumer, error: Option<&crate::L1Error>) {
        let Ok(mut state) = self.0.lock() else { return };
        let next = error.and_then(crate::L1Error::required_blob);
        if next.is_none()
            && state
                .as_ref()
                .is_some_and(|(_, _, _, _, owner)| *owner != consumer)
        {
            return;
        }
        *state = next.map(|(reference, reason)| {
            let attempts = state
                .as_ref()
                .filter(|(old, _, _, _, _)| *old == reference)
                .map_or(1, |(_, _, count, _, _)| count.saturating_add(1));
            (reference, reason, attempts, Instant::now(), consumer)
        });
    }
    /// Completion cannot erase a newer dependency discovered concurrently.
    pub fn completed(&self, consumer: Option<BlobConsumer>, through: u64) {
        if let Ok(mut state) = self.0.lock()
            && state.as_ref().is_some_and(|(reference, _, _, _, owner)| {
                reference.block_number <= through
                    && consumer.is_none_or(|consumer| consumer == *owner)
            })
        {
            *state = None;
        }
    }
    pub fn clear(&self) {
        if let Ok(mut state) = self.0.lock() {
            *state = None;
        }
    }
    pub fn snapshot(&self) -> Option<RequiredBlobFailure> {
        self.0
            .lock()
            .ok()?
            .as_ref()
            .map(
                |(reference, reason, attempts, at, consumer)| RequiredBlobFailure {
                    reference: *reference,
                    reason: *reason,
                    attempts: *attempts,
                    consumer: *consumer,
                    age_ms: at.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn required() -> RequiredBlob {
        RequiredBlob {
            block_number: 7,
            block_hash: B256::repeat_byte(1),
            transaction_hash: B256::repeat_byte(2),
            transaction_index: 3,
            rollup_id: 1,
        }
    }
    fn missing(reference: RequiredBlob) -> crate::L1Error {
        crate::L1Error::SourceIncomplete {
            block: reference.block_number,
            tx_hash: reference.transaction_hash,
            detail: "fixture missing body".into(),
        }
        .with_required_blob(reference, BlobUnavailableReason::BeaconResponse)
    }
    #[test]
    fn exact_reference_retries_are_bounded_and_clear_on_recovery_other_error_and_reorg() {
        let state = RequiredBlobState::default();
        assert!(state.snapshot().is_none());
        state.observe(BlobConsumer::Deriver, Some(&missing(required())));
        state.observe(BlobConsumer::Deriver, Some(&missing(required())));
        assert_eq!(state.snapshot().unwrap().attempts, 2);
        state.observe(BlobConsumer::Watcher, None);
        assert_eq!(
            state.snapshot().unwrap().attempts,
            2,
            "idle sibling must not erase active catch-up blocker"
        );
        state.observe(
            BlobConsumer::Deriver,
            Some(&missing(RequiredBlob {
                block_hash: B256::repeat_byte(3),
                ..required()
            })),
        );
        assert_eq!(state.snapshot().unwrap().attempts, 1);
        state.observe(
            BlobConsumer::Deriver,
            Some(&crate::L1Error::Provider("offline".into())),
        );
        assert!(state.snapshot().is_none());
        state.observe(BlobConsumer::Deriver, Some(&missing(required())));
        state.completed(None, 6);
        assert!(
            state.snapshot().is_some(),
            "older scan cannot erase newer blocker"
        );
        state.completed(Some(BlobConsumer::Watcher), 7);
        assert!(
            state.snapshot().is_some(),
            "sibling scan does not establish backfill availability"
        );
        state.completed(None, 7);
        assert!(state.snapshot().is_none());
        state.observe(BlobConsumer::Deriver, Some(&missing(required())));
        state.clear();
        assert!(state.snapshot().is_none());
    }
    #[test]
    fn classification_preserves_retry_and_display_without_parsing_error_text() {
        let plain = crate::L1Error::SourceIncomplete {
            block: 7,
            tx_hash: required().transaction_hash,
            detail: "body unavailable".into(),
        };
        let message = plain.to_string();
        let typed =
            plain.with_required_blob(required(), BlobUnavailableReason::ExpectedHashMissing);
        assert!(typed.is_source_incomplete());
        assert!(!typed.is_transport());
        assert!(!typed.is_terminal());
        assert_eq!(typed.to_string(), message);
        assert!(
            crate::L1Error::Provider("blob unavailable".into())
                .with_required_blob(required(), BlobUnavailableReason::BeaconResponse)
                .required_blob()
                .is_none()
        );
    }
}
