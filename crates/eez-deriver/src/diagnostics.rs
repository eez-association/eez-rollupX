//! Bounded, process-local evidence of actual STF execution. This is diagnostic
//! state only: it never authorizes execution or upgrades reused correlation rows.
use std::{collections::VecDeque, sync::Mutex, time::Instant};

use alloy_consensus::Header;
use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

/// Maximum retained executed headers across all references. Eviction is unknown.
pub const CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub l1_block_number: u64,
    pub l1_block_hash: B256,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub l2_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct Evidence {
    pub reference: Reference,
    pub header: Header,
    /// True only after the existing native claimed-state check succeeded.
    pub settled: bool,
    /// Authenticated complete-template hash; candidate was rejected before commit.
    pub advertised_hash: Option<B256>,
    observed: Instant,
}
impl Evidence {
    pub fn age_ms(&self) -> u64 {
        self.observed
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Default)]
pub struct ReplayEvidence(Mutex<VecDeque<Evidence>>);
impl ReplayEvidence {
    /// Called only after reconciliation's execution and outbound gates return.
    /// A failed claimed-state check records actual computed candidates separately
    /// from successful settled evidence. A generic execution failure records none.
    pub fn record(
        &self,
        reference: Reference,
        headers: VecDeque<Header>,
        settled_end: Option<u64>,
    ) {
        self.record_inner(reference, headers, settled_end, None);
    }
    pub fn record_computed(&self, reference: Reference, header: Header, advertised: B256) {
        self.record_inner(reference, [header].into(), None, Some(advertised));
    }
    fn record_inner(
        &self,
        reference: Reference,
        headers: VecDeque<Header>,
        settled_end: Option<u64>,
        advertised_hash: Option<B256>,
    ) {
        let Ok(mut cache) = self.0.lock() else { return };
        for header in headers {
            let key = Reference {
                l2_block_number: header.number,
                ..reference
            };
            // Do not promote executions beyond a partially settled prefix.
            if settled_end.is_some_and(|end| header.number > end) {
                continue;
            }
            cache.retain(|entry| entry.reference != key);
            cache.push_back(Evidence {
                reference: key,
                header,
                settled: settled_end.is_some(),
                advertised_hash,
                observed: Instant::now(),
            });
            while cache.len() > CAPACITY {
                cache.pop_front();
            }
        }
    }
    pub fn get(&self, reference: Reference) -> Option<Evidence> {
        self.0
            .lock()
            .ok()?
            .iter()
            .find(|entry| entry.reference == reference)
            .cloned()
    }
    pub fn remove_after_l1(&self, ancestor: u64) {
        if let Ok(mut cache) = self.0.lock() {
            cache.retain(|entry| entry.reference.l1_block_number <= ancestor);
        }
    }
}

/// Bounds in-flight diagnostic memory even when a protocol batch is very large.
pub fn retain_header(headers: &mut VecDeque<Header>, header: Header) {
    if headers.len() == CAPACITY {
        headers.pop_front();
    }
    headers.push_back(header);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference() -> Reference {
        Reference {
            l1_block_number: 10,
            l1_block_hash: B256::repeat_byte(1),
            transaction_hash: B256::repeat_byte(2),
            transaction_index: 0,
            l2_block_number: 1,
        }
    }
    #[test]
    fn exact_reference_reuse_partial_prefix_reorg_and_restart_are_conservative() {
        let cache = ReplayEvidence::default();
        let key = reference();
        assert!(cache.get(key).is_none());
        cache.record(
            key,
            [Header {
                number: 2,
                ..Default::default()
            }]
            .into(),
            Some(1),
        );
        assert!(
            cache.get(key).is_none(),
            "replaying a suffix does not prove a skipped prefix"
        );
        cache.record(
            key,
            [Header {
                number: 1,
                ..Default::default()
            }]
            .into(),
            Some(1),
        );
        assert!(cache.get(key).unwrap().settled);
        assert!(
            cache
                .get(Reference {
                    transaction_index: 1,
                    ..key
                })
                .is_none()
        );
        assert!(
            cache
                .get(Reference {
                    l1_block_hash: B256::ZERO,
                    ..key
                })
                .is_none()
        );
        assert!(ReplayEvidence::default().get(key).is_none());
        cache.remove_after_l1(9);
        assert!(cache.get(key).is_none());
    }
    #[test]
    fn failure_is_distinct_and_both_buffers_are_bounded() {
        let cache = ReplayEvidence::default();
        let mut headers = VecDeque::new();
        for number in 0..=CAPACITY as u64 {
            retain_header(
                &mut headers,
                Header {
                    number,
                    ..Default::default()
                },
            );
        }
        assert_eq!(headers.len(), CAPACITY);
        cache.record(reference(), headers, None);
        assert!(
            cache
                .get(Reference {
                    l2_block_number: 0,
                    ..reference()
                })
                .is_none()
        );
        assert!(!cache.get(reference()).unwrap().settled);
    }
}
