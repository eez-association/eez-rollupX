//! Automatic FIFO selection is a preparation estimate, never a protocol limit.
//! Gas and encoded DA are still checked before accepting each complete group,
//! and the final encoded transaction/bundle remains the authoritative check.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use alloy_eips::Decodable2718;
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use eez_driver::RollupTiming;
use eez_l1::BundleTarget;

use crate::{
    prover_retry::{proof_start_cutoff_ms, unix_time_millis},
    settlement_policy::index::DaOverhead,
};

#[derive(Clone, Copy, Debug)]
struct Sample {
    groups: usize,
    preparation: Duration,
    tail: Duration,
}

#[derive(Default, Debug)]
pub(super) struct AutomaticAdmission {
    sample: Option<Sample>,
    retry_limit: Option<usize>,
}

impl AutomaticAdmission {
    /// Relay rejection, exclusion or partial consumption can expose a limit
    /// absent from the local gas/blob model. Canonical reconciliation decides
    /// which users retry; only shrink their next preparation selection here.
    pub fn submission_failed(&mut self, submitted: usize) -> usize {
        let limit = (submitted / 2).max(1);
        self.retry_limit = Some(self.retry_limit.map_or(limit, |old| old.min(limit)));
        self.retry_limit.unwrap()
    }

    fn limit(&self, queued: usize, available: Duration) -> usize {
        if available.is_zero() {
            return 0;
        }
        // Prefix commitments currently rebuild each effect prefix. Quadratic
        // scaling avoids assuming that twice as many groups takes twice as long.
        // Probe at most twice the last measured size, with no fixed count cap.
        let estimated = self.sample.map_or(1, |s| {
            let squared = (s.groups as u128).saturating_mul(s.groups as u128);
            let n = available.as_nanos().saturating_mul(squared) / s.preparation.as_nanos().max(1);
            usize::try_from(n.isqrt())
                .unwrap_or(usize::MAX)
                .max(1)
                .min(s.groups.saturating_mul(2))
        });
        estimated
            .min(self.retry_limit.unwrap_or(usize::MAX))
            .min(queued)
    }

    pub fn begin(&self, queued: usize, timing: RollupTiming, target: BundleTarget) -> Attempt {
        let available = Duration::from_millis(
            proof_start_cutoff_ms(timing, target)
                // A missing target has no safe preparation budget. The normal
                // minimal/history path resolves a target without draining users.
                .unwrap_or(0)
                .saturating_sub(unix_time_millis()),
        );
        Attempt {
            started: Instant::now(),
            available,
            selected: self.limit(queued, available),
            previous: self.sample,
            progress: Mutex::default(),
        }
    }

    pub fn finish(&mut self, attempt: &Attempt) {
        let progress = attempt.progress.lock().unwrap();
        if let Some(sample) = progress.prepared {
            self.sample = Some(sample);
        }
        // A resource/time cut must shrink the next FIFO selection. Otherwise
        // outbound-first execution could repeatedly consume the budget ahead
        // of an older inbound group. Eventually the FIFO head is tried alone.
        self.retry_limit = if progress.limited {
            Some(progress.accepted.max(1).min((attempt.selected / 2).max(1)))
        } else if !progress.completed
            || progress
                .prepared
                .is_none_or(|s| s.preparation > attempt.available)
        {
            Some((attempt.selected / 2).max(1))
        } else {
            None
        };
    }
}

#[derive(Default, Debug)]
struct Progress {
    phase: Duration,
    accepted: usize,
    limited: bool,
    completed: bool,
    prepared: Option<Sample>,
}

#[derive(Debug)]
pub(super) struct Attempt {
    started: Instant,
    available: Duration,
    pub selected: usize,
    previous: Option<Sample>,
    progress: Mutex<Progress>,
}

impl Attempt {
    pub fn can_start_group(&self, accepted: usize) -> bool {
        self.can_start_at(accepted, self.started.elapsed())
    }

    fn can_start_at(&self, accepted: usize, elapsed: Duration) -> bool {
        let remaining = self.available.saturating_sub(elapsed);
        if remaining.is_zero() {
            return false;
        }
        if accepted == 0 {
            return true;
        }
        // Reserve another group's observed work plus the measured build,
        // prefix-hash, encoding, KZG and witness tail before the proof cutoff.
        let next = accepted.saturating_add(1) as u128;
        let tail = self.previous.map_or(elapsed.as_nanos(), |s| {
            s.tail.as_nanos().saturating_mul(next.saturating_mul(next))
                / (s.groups as u128).saturating_mul(s.groups as u128).max(1)
        });
        remaining.as_nanos() > elapsed.as_nanos() / accepted as u128 + tail
    }

    pub fn phase_finished(&self, accepted: usize, limited: bool) {
        let mut progress = self.progress.lock().unwrap();
        progress.phase = self.started.elapsed();
        progress.accepted = accepted;
        progress.limited |= limited;
    }

    pub fn completed(&self) {
        self.progress.lock().unwrap().completed = true;
    }

    pub fn prepared(&self, rollup_id: u64, groups: usize) {
        if groups == 0 {
            return;
        }
        let elapsed = self.started.elapsed();
        let mut progress = self.progress.lock().unwrap();
        progress.prepared = Some(Sample {
            groups,
            preparation: elapsed,
            tail: elapsed.saturating_sub(progress.phase),
        });
        tracing::event!(name: "eez.composer.admission.prepared", tracing::Level::INFO,
            event_name = "eez.composer.admission.prepared", rollup_id,
            selected = self.selected, accepted = groups,
            preparation_ms = elapsed.as_millis() as u64,
            available_ms = self.available.as_millis() as u64,
            limited = progress.limited,
            "measured automatic admission through the start of proof generation");
    }
}

/// Incremental upper bound for the indivisible terminal Sync block. Earlier
/// history is handled by the existing exact encoding / historical-drain path.
/// Unknown execution header fields and RLP prefixes are rounded up. Transaction,
/// metadata and semantic bytes use their actual wire encodings, without padding.
#[derive(Clone, Copy, Debug)]
pub(super) struct BlobAdmission {
    header_and_body: usize,
    header_and_body_items: usize,
    transaction_items: usize,
    transaction_byte_items: usize,
    overhead: DaOverhead,
    pub companion_blobs: u64,
}

fn list_len(items: usize) -> Result<usize, String> {
    items
        .checked_add(
            alloy_rlp::Header {
                list: true,
                payload_length: items,
            }
            .length(),
        )
        .ok_or_else(|| "admission RLP size overflow".into())
}

impl BlobAdmission {
    pub fn new(mut empty: reth_ethereum_primitives::Block) -> Self {
        // These fields are only known after final execution. Maximal encodings
        // bound their contribution to Vec<u8>'s RLP INTEGER list, where every
        // byte >= 128 takes two bytes. The remaining header fields are fixed by
        // the same parent and next-block attributes as the canonical builder.
        empty.header.gas_used = u64::MAX;
        empty.header.state_root = alloy_primitives::B256::repeat_byte(0xff);
        empty.header.receipts_root = alloy_primitives::B256::repeat_byte(0xff);
        empty.header.transactions_root = alloy_primitives::B256::repeat_byte(0xff);
        empty.header.logs_bloom = alloy_primitives::Bloom::repeat_byte(0xff);
        if let Some(hash) = &mut empty.header.requests_hash {
            *hash = alloy_primitives::B256::repeat_byte(0xff);
        }
        if let Some(blob_gas) = &mut empty.header.blob_gas_used {
            *blob_gas = u64::MAX;
        }
        let header = alloy_rlp::encode(&empty.header);
        let body_lists = 1 + usize::from(empty.body.withdrawals.is_some());
        Self {
            // Empty ommers, plus an empty withdrawals list when present. The
            // synthetic Sync builder never adds ommers or withdrawals.
            header_and_body: header.len() + body_lists,
            // The header's own RLP prefix can change when gas-used changes.
            // Charge its maximum byte expansion even if this template's prefix
            // happened to contain low bytes.
            header_and_body_items: header.iter().map(Encodable::length).sum::<usize>()
                + 2 * body_lists
                + 2 * (1 + std::mem::size_of::<usize>()),
            transaction_items: 0,
            transaction_byte_items: 0,
            overhead: DaOverhead::default(),
            companion_blobs: 0,
        }
    }

    pub fn extend(
        &self,
        txs: &[Bytes],
        overhead: DaOverhead,
        companion: Option<&Bytes>,
    ) -> Result<Self, String> {
        let mut next = *self;
        for raw in txs {
            let tx = alloy_consensus::TxEnvelope::decode_2718(&mut raw.as_ref())
                .map_err(|e| format!("admission transaction decode: {e}"))?;
            next.transaction_items = next
                .transaction_items
                .checked_add(tx.length())
                .ok_or("admission transaction size overflow")?;
            let encoded = alloy_rlp::encode(tx);
            next.transaction_byte_items = next
                .transaction_byte_items
                .checked_add(encoded.iter().map(Encodable::length).sum::<usize>())
                .ok_or("admission encoded transaction size overflow")?;
        }
        if let Some(raw) = companion {
            use alloy_consensus::Transaction;
            let tx = alloy_consensus::TxEnvelope::decode_2718(&mut raw.as_ref())
                .map_err(|e| format!("admission companion decode: {e}"))?;
            next.companion_blobs = next
                .companion_blobs
                .checked_add(
                    tx.blob_versioned_hashes()
                        .map_or(0, |hashes| hashes.len() as u64),
                )
                .ok_or("admission companion blob count overflow")?;
        }
        next.overhead.entry_items = next
            .overhead
            .entry_items
            .checked_add(overhead.entry_items)
            .ok_or("admission entry size overflow")?;
        next.overhead.group_items = next
            .overhead
            .group_items
            .checked_add(overhead.group_items)
            .ok_or("admission group size overflow")?;
        next.overhead.semantic_bytes = next
            .overhead
            .semantic_bytes
            .checked_add(overhead.semantic_bytes)
            .ok_or("admission semantic size overflow")?;
        Ok(next)
    }

    pub fn stream_bytes(&self) -> Result<usize, String> {
        let block_payload = self
            .header_and_body
            .checked_add(list_len(self.transaction_items)?)
            .ok_or("admission block size overflow")?;
        let tx_prefix = list_len(self.transaction_items)? - self.transaction_items;
        let block_prefix = list_len(block_payload)? - block_payload;
        let block_byte_items = self
            .header_and_body_items
            .checked_add(self.transaction_byte_items)
            .and_then(|n| n.checked_add(2 * (tx_prefix + block_prefix)))
            .ok_or("admission block byte list size overflow")?;
        // Preserve the codec's Vec<u8> representation: an RLP integer list,
        // not Bytes. Prefix bytes may change as the final header gets shorter.
        let block_item = block_byte_items
            .checked_add(2 * (list_len(block_byte_items)? - block_byte_items))
            .ok_or("admission block item size overflow")?;
        let payload = eez_payload_codec::self_contained_payload_len(
            block_item,
            self.overhead.entry_items,
            self.overhead.group_items,
        )
        .map_err(|e| e.to_string())?;
        eez_blob_codec::native_stream_len(payload, self.overhead.semantic_bytes)
            .map_err(|e| e.to_string())
    }

    pub fn fits(&self, blobs: usize) -> Result<bool, String> {
        let required = self
            .stream_bytes()?
            .div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB);
        Ok(required <= blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEip4844, TxEnvelope, TxLegacy};
    use alloy_eips::Encodable2718;
    use alloy_primitives::{B256, Signature, U256};

    fn sample(groups: usize, preparation_ms: u64) -> Sample {
        Sample {
            groups,
            preparation: Duration::from_millis(preparation_ms),
            tail: Duration::from_millis(preparation_ms / 2),
        }
    }

    fn attempt(
        selected: usize,
        accepted: usize,
        limited: bool,
        prepared: Option<Sample>,
    ) -> Attempt {
        Attempt {
            started: Instant::now(),
            available: Duration::from_secs(1),
            selected,
            previous: prepared,
            progress: Mutex::new(Progress {
                accepted,
                limited,
                completed: prepared.is_some(),
                prepared,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn automatic_selection_starts_small_grows_past_fifty_and_respects_available_time() {
        let mut state = AutomaticAdmission::default();
        assert_eq!(state.limit(1000, Duration::ZERO), 0);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
        assert_eq!(state.limit(0, Duration::from_secs(1)), 0);
        let unknown_target = state.begin(
            1000,
            RollupTiming::new(5000, 1000, 1000, 100),
            BundleTarget::NextBlock,
        );
        assert_eq!(
            unknown_target.selected, 0,
            "no safe admission deadline without an exact target"
        );
        for groups in [1, 2, 4, 8, 16, 32, 64] {
            state.finish(&attempt(groups, groups, false, Some(sample(groups, 200))));
            assert_eq!(state.limit(1000, Duration::from_secs(1)), groups * 2);
        }
        assert_eq!(state.limit(1000, Duration::from_millis(50)), 32);
        assert_eq!(state.limit(3, Duration::from_secs(1)), 3);
        // Restart discards only performance estimates.
        assert_eq!(
            AutomaticAdmission::default().limit(1000, Duration::from_secs(1)),
            1
        );
    }

    #[test]
    fn capacity_cuts_reduce_fifo_lookahead_until_an_older_opposite_direction_is_alone() {
        let mut state = AutomaticAdmission {
            sample: Some(sample(64, 100)),
            retry_limit: None,
        };
        let mut selected = 64;
        while selected > 1 {
            // Outbound-first execution used the budget before the FIFO's oldest inbound.
            state.finish(&attempt(
                selected,
                selected - 1,
                true,
                Some(sample(selected - 1, 100)),
            ));
            let next = state.limit(1000, Duration::from_secs(1));
            assert!(next < selected);
            selected = next;
        }
        assert_eq!(selected, 1);
    }

    #[test]
    fn timeouts_and_incomplete_preparation_back_off_without_an_unbounded_retry() {
        let mut state = AutomaticAdmission::default();
        state.finish(&attempt(64, 64, false, Some(sample(64, 2000))));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 32);
        state.finish(&attempt(32, 0, false, None));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 16);
        state.finish(&attempt(16, 0, true, None));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
        let failed_proof = attempt(16, 16, false, Some(sample(16, 100)));
        failed_proof.progress.lock().unwrap().completed = false;
        state.finish(&failed_proof);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 8);
        state.submission_failed(8);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 4);
        state.submission_failed(4);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 2);
        state.submission_failed(2);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
    }

    #[test]
    fn group_start_reserves_final_preparation_and_stops_at_the_shared_proof_cutoff() {
        let a = attempt(64, 0, false, Some(sample(10, 500)));
        assert!(a.can_start_at(0, Duration::from_millis(1)));
        assert!(!a.can_start_at(0, Duration::from_secs(1)));
        assert!(a.can_start_at(5, Duration::from_millis(300)));
        assert!(!a.can_start_at(10, Duration::from_millis(700)));
    }

    fn signed(size: usize, typed: bool) -> TxEnvelope {
        let signature = Signature::new(U256::from(1), U256::from(2), false);
        if typed {
            TxEip1559 {
                gas_limit: 1_000_000,
                input: vec![0xffu8; size].into(),
                ..Default::default()
            }
            .into_signed(signature)
            .into()
        } else {
            TxLegacy {
                gas_limit: 1_000_000,
                input: vec![0u8; size].into(),
                ..Default::default()
            }
            .into_signed(signature)
            .into()
        }
    }

    #[test]
    fn incremental_da_bounds_real_encoding_across_rlp_boundaries_and_multiple_blobs() {
        for withdrawals in [false, true] {
            let mut block = reth_ethereum_primitives::Block::default();
            block.header.gas_limit = 30_000_000;
            block.header.gas_used = block.header.gas_limit;
            if withdrawals {
                block.body.withdrawals = Some(Default::default());
            }
            let mut budget = BlobAdmission::new(block.clone());
            let mut entries = Vec::<Vec<u8>>::new();
            let mut groups = Vec::<u16>::new();
            let mut messages = Vec::new();
            for (i, size) in [0, 55, 56, 128, 255, 256, 16_384, 126_976]
                .into_iter()
                .enumerate()
            {
                let tx = signed(size, i % 2 == 0);
                let raw = Bytes::from(tx.encoded_2718());
                block.body.transactions.push(tx.into());
                let entry = vec![0u8; size];
                let group = 1u16;
                let group_messages = vec![
                    eez_blob_codec::Message::InitiateCrossChainTransaction {
                        chain_id: 1,
                        tx_data: raw.clone(),
                    },
                    eez_blob_codec::Message::FinishCrossChainTransaction,
                ];
                let semantic =
                    eez_blob_codec::encode_native_stream(1, Bytes::new(), group_messages.clone())
                        .unwrap()
                        .blob_portion
                        .len()
                        - eez_blob_codec::native_stream_len(0, 0).unwrap();
                budget = budget
                    .extend(
                        &[raw],
                        DaOverhead {
                            entry_items: entry.length(),
                            group_items: group.length(),
                            semantic_bytes: semantic,
                        },
                        None,
                    )
                    .unwrap();
                entries.push(entry);
                groups.push(group);
                messages.extend(group_messages);
                let payload = eez_payload_codec::encode_self_contained(
                    &[alloy_rlp::encode(&block)],
                    &entries,
                    &groups,
                )
                .unwrap();
                let encoded =
                    eez_blob_codec::encode_native_stream(1, payload.into(), messages.clone())
                        .unwrap();
                let estimated = budget.stream_bytes().unwrap();
                let actual = encoded.blob_portion.len();
                assert!(estimated >= actual, "estimate {estimated}, actual {actual}");
                assert!(
                    estimated - actual < 600,
                    "only the unknown header and prefixes may be rounded up"
                );
                let required = estimated.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB);
                assert!(budget.fits(required).unwrap());
                assert!(!budget.fits(required - 1).unwrap());
            }
            assert!(budget.stream_bytes().unwrap() > 2 * eez_blob_codec::DATA_BYTES_PER_BLOB);
        }
    }

    #[test]
    fn refused_blob_candidate_does_not_spend_budget_and_companion_blobs_are_counted() {
        let budget = BlobAdmission::new(Default::default());
        let initial = budget.stream_bytes().unwrap();
        let raw = Bytes::from(signed(2 * eez_blob_codec::DATA_BYTES_PER_BLOB, true).encoded_2718());
        let next = budget.extend(&[raw], DaOverhead::default(), None).unwrap();
        assert!(!next.fits(2).unwrap());
        assert_eq!(budget.stream_bytes().unwrap(), initial);
        let companion = TxEip4844 {
            blob_versioned_hashes: vec![B256::ZERO; 2],
            ..Default::default()
        }
        .into_signed(Signature::new(U256::from(1), U256::from(2), false));
        let raw = Bytes::from(TxEnvelope::from(companion).encoded_2718());
        let next = budget
            .extend(&[], DaOverhead::default(), Some(&raw))
            .unwrap();
        assert_eq!(next.companion_blobs, 2);
        assert_eq!(next.stream_bytes().unwrap(), initial);
        assert!(
            budget
                .extend(&[Bytes::from_static(&[0xff])], DaOverhead::default(), None)
                .is_err()
        );
    }
}
