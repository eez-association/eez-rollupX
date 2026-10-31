//! Authorization for mutable L2 calls represented by outbound settlement entries.
//!
//! For each claimed call, the gate recomputes the gas-aware key used by
//! `EEZL2.executeCrossChainCall` and consumes one matching event observation
//! from the re-executed Sync block. The hash binds the immediate L2 caller,
//! source and target rollups, target, value, calldata, and manager-entry gas.
//! Treating observations as a multiset preserves duplicate calls while
//! preventing one event from authorizing more than one entry.

use std::collections::HashMap;

use crate::RollupId;
use alloy_primitives::B256;

use crate::abi::ExecutionEntrySol;
use crate::action::{CallHashInput, CallMode, l2_outbound_call_hash};

/// `RollupId(0)` — MAINNET. An L2->L1 outbound's L1 target lives on mainnet, so
/// the `targetRollupId` field of its call hash is 0 (the L2 proxy's
/// `originalRollupId`) — the value `EEZL2.executeCrossChainCall` recomputes.
const MAINNET_ROLLUP_ID: RollupId = RollupId(0);

/// The supported EEZL2 deployment disables `USE_GAS_LEFT`.
const SUPPORTED_CALL_GAS: u64 = 0;

/// Decode the outbound-call events `eez_l2` emitted in `logs`.
///
/// Shared so the composer (raw execution logs) and the deriver (receipt logs)
/// decide from the same decoder.
#[must_use]
pub fn observations_from_logs(
    logs: &[alloy_primitives::Log],
    eez_l2: alloy_primitives::Address,
) -> Vec<OutboundCallObservation> {
    use alloy_sol_types::SolEvent as _;

    logs.iter()
        .filter(|log| log.address == eez_l2)
        .filter_map(|log| {
            crate::abi::eez_l2_events::CrossChainCallExecuted::decode_log_validate(log).ok()
        })
        .map(|event| {
            OutboundCallObservation::new(event.data.crossChainCallHash, event.data.callGas)
        })
        .collect()
}

/// Canonically decoded evidence from an `EEZL2.CrossChainCallExecuted` log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutboundCallObservation {
    call_hash: B256,
    call_gas: u64,
}

impl OutboundCallObservation {
    /// Construct an observation from a canonically decoded `EEZL2` event.
    #[must_use]
    pub const fn new(call_hash: B256, call_gas: u64) -> Self {
        Self {
            call_hash,
            call_gas,
        }
    }

    /// Hash emitted by `EEZL2` for the executed call.
    #[must_use]
    pub const fn call_hash(&self) -> B256 {
        self.call_hash
    }

    /// Manager-entry gas value folded into the emitted hash.
    ///
    /// This is not the destination-call forwarding gas limit.
    #[must_use]
    pub const fn call_gas(&self) -> u64 {
        self.call_gas
    }
}

/// Verify every L2-origin call in the supplied L1 settlement entries by
/// matching its recomputed cross-chain call hash against the
/// `CrossChainCallExecuted` events the re-executed Sync block emitted.
///
/// `settlement_entries` may contain outbound hosts (`proxyEntryHash == 0`) or
/// inbound roots (`proxyEntryHash != 0`) with callbacks to L1. Every flat call
/// in either shape executes on L1 and therefore must originate on this L2.
/// `observed_calls` contains the canonically decoded
/// `CrossChainCallExecuted` logs from the Sync block. `l2_rollup_id` is this
/// L2's own id, which each entry's `sourceRollupId` must equal.
///
/// # Errors
/// The first outbound entry/call that is malformed or whose recomputed call
/// hash has no matching emitted event — or any event left without a settlement
/// call. Nested row sub-arrays are checked in the same unified table.
pub fn verify_outbound_authorized(
    settlement_entries: &[ExecutionEntrySol],
    observed_calls: &[OutboundCallObservation],
    l2_rollup_id: u64,
) -> Result<(), String> {
    let mut expected_hashes = Vec::new();

    for (i, entry) in settlement_entries.iter().enumerate() {
        crate::system_tx::check_entry_shape(entry, &format!("outbound entry {i}"))?;
        if entry.proxyEntryHash == B256::ZERO
            && entry.l2ToL1Calls.is_empty()
            && entry
                .expectedL1ToL2Calls
                .iter()
                .all(|row| row.l2ToL1Calls.is_empty())
        {
            return Err(format!(
                "outbound host entry {i} contains no L2-to-L1 calls"
            ));
        }
        let arrays = std::iter::once(entry.l2ToL1Calls.as_slice()).chain(
            entry
                .expectedL1ToL2Calls
                .iter()
                .map(|row| row.l2ToL1Calls.as_slice()),
        );
        let mut call_index = 0usize;
        for calls in arrays {
            let mut array_index = 0usize;
            while array_index < calls.len() {
                let span = usize::from(calls[array_index].revertNextNCalls);
                let end = array_index + span.max(1);
                for call in &calls[array_index..end] {
                    if call.sourceRollupId != l2_rollup_id {
                        return Err(format!(
                            "outbound entry {i} call {call_index}: sourceRollupId {} != this L2 {l2_rollup_id}",
                            call.sourceRollupId,
                        ));
                    }

                    let expected = l2_outbound_call_hash(
                        CallHashInput {
                            call_mode: CallMode::from_is_static(call.isStatic),
                            source_address: call.sourceAddress,
                            source_rollup_id: RollupId(l2_rollup_id),
                            target_address: call.targetAddress,
                            target_rollup_id: MAINNET_ROLLUP_ID,
                            value: call.value,
                            data: &call.data,
                        },
                        SUPPORTED_CALL_GAS,
                    );
                    // Events emitted by the source L2 inside its reverted EVM
                    // frame are rolled back. Their destination calls remain in
                    // the settlement table and are authorized by the marker,
                    // not by a receipt log that cannot exist.
                    if span == 0 {
                        expected_hashes.push(expected);
                    }
                    call_index += 1;
                }
                array_index = end;
            }
        }
    }
    verify_expected_outbound_hashes(&expected_hashes, observed_calls)
}

/// Match a verified ordered set of committed L2-origin call hashes against the
/// `CrossChainCallExecuted` logs emitted by re-execution.
///
/// Callers that have both the L1 settlement table and its L2 sidecars should
/// derive `expected_hashes` with [`crate::entries::verify_nested_group`]. That
/// verifier omits calls whose proxy frame naturally reverted, as well as calls
/// rolled back by `revertNextNCalls`; neither can retain an L2 receipt log.
/// Duplicate hashes retain multiset semantics.
pub fn verify_expected_outbound_hashes(
    expected_hashes: &[B256],
    observed_calls: &[OutboundCallObservation],
) -> Result<(), String> {
    let mut available: HashMap<B256, usize> = HashMap::new();
    for observation in observed_calls {
        if observation.call_gas() != SUPPORTED_CALL_GAS {
            return Err(format!(
                "outbound event uses callGas {}; this deployment requires callGas 0",
                observation.call_gas(),
            ));
        }
        *available.entry(observation.call_hash()).or_insert(0) += 1;
    }

    for (index, expected) in expected_hashes.iter().enumerate() {
        match available.get_mut(expected) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                return Err(format!(
                    "verified outbound call {index} has NO matching CrossChainCallExecuted event \
                     in the re-executed Sync block (hash {expected:#x}) — phantom / tampered / \
                     redirected / non-mainnet-target withdrawal",
                ));
            }
        }
    }

    let unmatched = available.values().copied().sum::<usize>();
    if unmatched != 0 {
        return Err(format!(
            "re-executed Sync block emitted {unmatched} outbound call(s) with no L1 settlement call"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{ExpectedL1ToL2CallSol, L2ToL1CallSol};
    use alloy_primitives::{Address, Bytes, U256, address};

    /// A real `CrossChainCallExecuted` log as EEZL2 emits it.
    fn cc_log(eez_l2: Address, call_hash: B256, call_gas: u64) -> alloy_primitives::Log {
        use alloy_sol_types::SolEvent as _;
        let event = crate::abi::eez_l2_events::CrossChainCallExecuted {
            crossChainCallHash: call_hash,
            proxy: Address::ZERO,
            sourceAddress: Address::ZERO,
            callData: Bytes::new(),
            value: U256::ZERO,
            callGas: call_gas,
        };
        alloy_primitives::Log {
            address: eez_l2,
            data: alloy_primitives::LogData::new_unchecked(
                event.encode_topics().iter().map(|t| t.0).collect(),
                event.encode_data().into(),
            ),
        }
    }

    /// Only the configured manager's logs count. A look-alike event from any
    /// other address must not authorize an entry.
    #[test]
    fn observations_come_only_from_the_configured_manager() {
        let eez_l2 = address!("0x4B2760F84B83d6b3829E30A7573B0172B6d6402C");
        let impostor = address!("0x00000000000000000000000000000000deadbeef");
        let hash = B256::repeat_byte(0xab);

        let mine = observations_from_logs(&[cc_log(eez_l2, hash, 0)], eez_l2);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0], OutboundCallObservation::new(hash, 0));

        assert!(
            observations_from_logs(&[cc_log(impostor, hash, 0)], eez_l2).is_empty(),
            "an event from another address must not be observed",
        );
        assert!(observations_from_logs(&[], eez_l2).is_empty());
    }

    /// Duplicate calls are a multiset, so two identical events must both survive
    /// decoding — one event may not authorize two entries.
    #[test]
    fn duplicate_events_are_both_observed() {
        let eez_l2 = address!("0x4B2760F84B83d6b3829E30A7573B0172B6d6402C");
        let hash = B256::repeat_byte(0xcd);
        let logs = [cc_log(eez_l2, hash, 0), cc_log(eez_l2, hash, 0)];
        assert_eq!(observations_from_logs(&logs, eez_l2).len(), 2);
    }

    fn entry(calls: Vec<L2ToL1CallSol>) -> ExecutionEntrySol {
        ExecutionEntrySol {
            rootUpdates: Vec::new(),
            proxyEntryHash: B256::ZERO, // outbound immediate
            l2ToL1Calls: calls,
            expectedL1ToL2Calls: Vec::new(),
            rollingHash: B256::ZERO,
            destinationRollupId: 1,
            success: true,
            returnData: Bytes::new(),
        }
    }

    /// The observed `CrossChainCallExecuted` topic1 EEZL2 would emit for `call`
    /// on this L2 — `sourceRollupId` forced to `l2_rollup_id`, `targetRollupId`
    /// = MAINNET(0). The gate recomputes the identical hash.
    fn observed(call: &L2ToL1CallSol, l2_rollup_id: u64) -> OutboundCallObservation {
        OutboundCallObservation::new(
            l2_outbound_call_hash(
                CallHashInput {
                    call_mode: CallMode::Mutable,
                    source_address: call.sourceAddress,
                    source_rollup_id: RollupId(l2_rollup_id),
                    target_address: call.targetAddress,
                    target_rollup_id: RollupId::MAINNET,
                    value: call.value,
                    data: &call.data,
                },
                SUPPORTED_CALL_GAS,
            ),
            SUPPORTED_CALL_GAS,
        )
    }

    fn call(source: Address, target: Address, value: u64, data: &[u8]) -> L2ToL1CallSol {
        L2ToL1CallSol {
            revertNextNCalls: 0,
            isStatic: false,
            gas: 0,
            sourceAddress: source,
            sourceRollupId: 1,
            targetAddress: target,
            value: U256::from(value),
            data: Bytes::from(data.to_vec()),
        }
    }

    #[test]
    fn gate_accepts_authorized_and_rejects_phantom() {
        let source = address!("00000000000000000000000000000000000000aa");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(source, target, 7, &[0x12, 0x34]);
        let entries = vec![entry(vec![c.clone()])];

        // Authorized — a matching event was observed.
        assert!(verify_outbound_authorized(&entries, &[observed(&c, 1)], 1).is_ok());

        // Phantom — the settlement entry has no matching observed event.
        assert!(verify_outbound_authorized(&entries, &[], 1).is_err());
    }

    #[test]
    fn gate_accounts_for_l2_calls_nested_under_an_inbound_root() {
        let c = call(
            Address::with_last_byte(0x31),
            Address::with_last_byte(0x32),
            3,
            &[0x33],
        );
        let mut inbound = entry(vec![c.clone()]);
        inbound.proxyEntryHash = B256::with_last_byte(0x34);
        assert!(
            verify_outbound_authorized(&[inbound], &[observed(&c, 1)], 1).is_ok(),
            "an inbound root can emit nested L2-to-L1 events"
        );

        let mut inbound_without_callback = entry(Vec::new());
        inbound_without_callback.proxyEntryHash = B256::with_last_byte(0x35);
        assert!(
            verify_outbound_authorized(&[inbound_without_callback], &[], 1).is_ok(),
            "a simple inbound root has no L2-origin event to authorize"
        );
    }

    /// THE outbound-via-wrapper property: the `sourceAddress` is a CONTRACT
    /// (a wrapper that internally called the proxy), not an EOA. The gate accepts
    /// it as long as a matching `CrossChainCallExecuted` event was emitted —
    /// there is no EOA / `tx.to == proxy` assumption. The prior gate rejected
    /// this by design.
    #[test]
    fn gate_accepts_contract_source_wrapper() {
        let wrapper = address!("cccccccccccccccccccccccccccccccccccccccc");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(wrapper, target, 42, &[0xab]);
        let entries = vec![entry(vec![c.clone()])];
        assert!(
            verify_outbound_authorized(&entries, &[observed(&c, 1)], 1).is_ok(),
            "a contract-initiated (wrapper) outbound must be accepted"
        );
    }

    #[test]
    fn gate_rejects_tampered_value_data_or_target() {
        let source = address!("00000000000000000000000000000000000000aa");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(source, target, 7, &[0x12, 0x34]);
        let good = observed(&c, 1); // the real event

        // Tampered value: the settlement entry claims value 999 → different hash.
        let mut tv = c.clone();
        tv.value = U256::from(999u64);
        assert!(verify_outbound_authorized(&[entry(vec![tv])], &[good], 1).is_err());

        // Tampered data.
        let mut td = c.clone();
        td.data = Bytes::from(vec![0xff]);
        assert!(verify_outbound_authorized(&[entry(vec![td])], &[good], 1).is_err());

        // Redirected target.
        let mut tt = c.clone();
        tt.targetAddress = address!("00000000000000000000000000000000000000bb");
        assert!(verify_outbound_authorized(&[entry(vec![tt])], &[good], 1).is_err());
    }

    /// The MAINNET-target bind survives: the gate recomputes with
    /// `targetRollupId = MAINNET(0)`, so an event produced by a NON-mainnet-target
    /// proxy (an L2->L2 outbound, `originalRollupId != 0`) hashes differently and
    /// is rejected even though target/value/data/source match.
    #[test]
    fn gate_enforces_mainnet_target_rollup() {
        let source = address!("00000000000000000000000000000000000000aa");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(source, target, 7, &[0x12, 0x34]);

        // An event whose hash used targetRollupId = 5 (a non-mainnet target).
        let non_mainnet_event = OutboundCallObservation::new(
            l2_outbound_call_hash(
                CallHashInput {
                    call_mode: CallMode::Mutable,
                    source_address: c.sourceAddress,
                    source_rollup_id: RollupId(1),
                    target_address: c.targetAddress,
                    target_rollup_id: RollupId(5),
                    value: c.value,
                    data: &c.data,
                },
                SUPPORTED_CALL_GAS,
            ),
            SUPPORTED_CALL_GAS,
        );
        assert_ne!(non_mainnet_event, observed(&c, 1));
        assert!(
            verify_outbound_authorized(&[entry(vec![c])], &[non_mainnet_event], 1).is_err(),
            "a withdrawal whose only event targets a non-mainnet proxy must be rejected"
        );
    }

    #[test]
    fn gate_rejects_wrong_source_rollup_id() {
        let source = address!("00000000000000000000000000000000000000aa");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(source, target, 7, &[0x12, 0x34]);
        // Gate run for L2 rollup 2, but the entry's call claims sourceRollupId 1.
        assert!(
            verify_outbound_authorized(&[entry(vec![c.clone()])], &[observed(&c, 2)], 2).is_err()
        );
    }

    #[test]
    fn gate_rejects_nonzero_call_gas() {
        let c = call(
            address!("00000000000000000000000000000000000000aa"),
            address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9"),
            7,
            &[0x12, 0x34],
        );
        let call_gas = 1;
        let call_hash = l2_outbound_call_hash(
            CallHashInput {
                call_mode: CallMode::Mutable,
                source_address: c.sourceAddress,
                source_rollup_id: RollupId(1),
                target_address: c.targetAddress,
                target_rollup_id: RollupId::MAINNET,
                value: c.value,
                data: &c.data,
            },
            call_gas,
        );
        let observation = OutboundCallObservation::new(call_hash, call_gas);

        assert!(verify_outbound_authorized(&[entry(vec![c])], &[observation], 1).is_err());
    }

    /// Multiset semantics: two IDENTICAL outbound entries need two matching
    /// events. One event authorizes only the first; the second is a phantom.
    #[test]
    fn gate_requires_one_event_per_entry() {
        let source = address!("00000000000000000000000000000000000000aa");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(source, target, 7, &[0x12, 0x34]);
        let two = vec![entry(vec![c.clone()]), entry(vec![c.clone()])];

        // Two events → both authorized.
        assert!(verify_outbound_authorized(&two, &[observed(&c, 1), observed(&c, 1)], 1).is_ok());
        // One event → the second entry is unmatched.
        assert!(verify_outbound_authorized(&two, &[observed(&c, 1)], 1).is_err());
    }

    #[test]
    fn gate_rejects_malformed_entries() {
        let source = address!("00000000000000000000000000000000000000aa");
        let target = address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9");
        let c = call(source, target, 7, &[0x12, 0x34]);

        // Empty l2ToL1Calls = not an outbound immediate.
        assert!(verify_outbound_authorized(&[entry(vec![])], &[], 1).is_err());

        let mut failed = entry(vec![c.clone()]);
        failed.success = false;
        assert!(verify_outbound_authorized(&[failed.clone()], &[observed(&c, 1)], 1).is_ok());
        failed.expectedL1ToL2Calls.push(ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::with_last_byte(1),
            l2ToL1Calls: Vec::new(),
            revertedOrStaticRollingHash: B256::with_last_byte(2),
            success: false,
            returnData: Bytes::new(),
        });
        // Callback rows are supported; this gate binds receipt authorization,
        // while the paired-table verifier checks their execution commitments.
        assert!(verify_outbound_authorized(&[failed.clone()], &[observed(&c, 1)], 1).is_ok());
        assert!(verify_outbound_authorized(&[failed], &[], 1).is_err());

        for unsupported in [
            L2ToL1CallSol {
                isStatic: true,
                ..c.clone()
            },
            L2ToL1CallSol {
                gas: 1,
                ..c.clone()
            },
        ] {
            assert!(verify_outbound_authorized(&[entry(vec![unsupported])], &[], 1).is_err());
        }

        let mut reverted = c.clone();
        reverted.revertNextNCalls = 1;
        assert!(verify_outbound_authorized(&[entry(vec![reverted])], &[], 1).is_ok());

        let mut out_of_bounds = c;
        out_of_bounds.revertNextNCalls = 2;
        assert!(verify_outbound_authorized(&[entry(vec![out_of_bounds])], &[], 1).is_err());
    }

    #[test]
    fn gate_accepts_committed_empty_failed_nested_row() {
        let c = call(
            address!("00000000000000000000000000000000000000aa"),
            address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9"),
            7,
            &[0x12, 0x34],
        );
        let mut settlement = entry(vec![c.clone()]);
        settlement.expectedL1ToL2Calls = vec![crate::abi::ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x41),
            l2ToL1Calls: Vec::new(),
            revertedOrStaticRollingHash: B256::repeat_byte(0x42),
            success: false,
            returnData: Bytes::from_static(&[0xde, 0xad]),
        }];

        verify_outbound_authorized(&[settlement], &[observed(&c, 1)], 1)
            .expect("a failed empty nested row emits no extra outbound event");
    }
}
