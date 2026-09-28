//! Checked L2 inbound transport. Shape checking does not authorize settlement:
//! the complementary L1 table must still pass independent nested verification.

use alloy_primitives::{B256, Bytes, U256};
use alloy_sol_types::SolValue;

use super::{IncomingEntry, build_l2_incoming_entry, encode_execute_incoming};
use crate::abi::{ExecutionEntrySol, L2ExecutionEntrySol};
use crate::system_tx::{
    check_entry_shape, committed_l2_table_value, lift_entry_from_l2, lower_entry_to_l2,
};
use crate::{
    CallHashInput, CallMode, ProtocolErrorKind, ProtocolResult, RollupId,
    common_cross_chain_call_hash,
};

/// A successful L1-to-L2 delivery with checked routing, root identity, supported
/// execution-table shape and bounded committed value. Unlike a settlement entry,
/// its transport carries no L1 root updates. Nested rows are retained exactly;
/// their position keys and rolling hashes require complementary L1 authorization.
#[derive(Clone, Debug)]
pub struct InboundSidecar {
    entry: ExecutionEntrySol,
    committed_value: U256,
}

impl InboundSidecar {
    /// Build a canonical flat delivery from semantic input.
    pub fn new(entry: IncomingEntry) -> ProtocolResult<Self> {
        let destination = entry.l2_rollup_id;
        let l2 = build_l2_incoming_entry(entry)?;
        Self::try_from(&lift_entry_from_l2(&l2, destination))
    }

    /// Check supplied transport against the configured destination. No supplied
    /// field is rewritten, including routing and L1-only root updates.
    pub fn for_rollup(entry: &ExecutionEntrySol, expected: RollupId) -> ProtocolResult<Self> {
        if entry.destinationRollupId != expected.0 {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "inbound sidecar targets rollup {}, expected {}",
                entry.destinationRollupId, expected.0,
            ))
            .into());
        }
        Self::try_from(entry)
    }

    /// Borrow for independent table authorization and canonical ABI projection.
    /// Reconstruction consumes the checked sidecar directly.
    pub fn as_abi_entry(&self) -> &ExecutionEntrySol {
        &self.entry
    }

    /// Canonical complete transport ABI used for DA equality checks.
    pub fn encoded(&self) -> Vec<u8> {
        self.entry.abi_encode()
    }

    /// Nonzero destination bound into the checked root call hash.
    pub fn destination_rollup_id(&self) -> RollupId {
        RollupId(self.entry.destinationRollupId)
    }

    /// Convert only at the final contract-ABI encoding boundary.
    pub fn l2_entry(&self) -> L2ExecutionEntrySol {
        lower_entry_to_l2(&self.entry)
    }

    /// Canonical `executeIncomingCrossChainCall` ABI preserving every nested row.
    pub fn calldata(&self) -> Bytes {
        encode_execute_incoming(self.l2_entry()).into()
    }

    /// Checked sum of all incoming table values, including nested incoming calls.
    pub const fn committed_value(&self) -> U256 {
        self.committed_value
    }
}

impl TryFrom<&ExecutionEntrySol> for InboundSidecar {
    type Error = crate::ProtocolError;

    fn try_from(entry: &ExecutionEntrySol) -> ProtocolResult<Self> {
        let invalid = |reason: &str| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                "invalid inbound sidecar: {reason}"
            )))
        };
        if !entry.rootUpdates.is_empty() {
            return Err(invalid("L1 settlement root updates are not L2 DA"));
        }
        if entry.destinationRollupId == RollupId::MAINNET.0 {
            return Err(invalid("destination must be an L2 rollup"));
        }
        if !entry.success {
            return Err(invalid("delivery entry must be successful"));
        }
        let [call] = entry.l2ToL1Calls.as_slice() else {
            return Err(invalid("expected exactly one incoming root call"));
        };
        if call.sourceRollupId != RollupId::MAINNET.0 {
            return Err(invalid("source must be L1 rollup 0"));
        }
        if call.isStatic || call.gas != 0 {
            return Err(invalid("root must be a mutable call without explicit gas"));
        }
        check_entry_shape(entry, "inbound").map_err(|reason| invalid(&reason))?;
        let call_hash = common_cross_chain_call_hash(CallHashInput {
            call_mode: CallMode::Mutable,
            source_address: call.sourceAddress,
            source_rollup_id: RollupId::MAINNET,
            target_address: call.targetAddress,
            target_rollup_id: RollupId(entry.destinationRollupId),
            value: call.value,
            data: &call.data,
        });
        if call_hash == B256::ZERO || entry.proxyEntryHash != call_hash {
            return Err(invalid("root call hash differs from its supplied identity"));
        }
        // A flat successful call has all inputs needed to validate its rolling
        // hash here. Nested/reverted execution instead needs the paired L1 table;
        // do not flatten it or invent outcomes at this conversion boundary.
        if entry.expectedL1ToL2Calls.is_empty() && call.revertNextNCalls == 0 {
            let mut rolling = crate::rolling_hash::EntryRollingHash::seed_for_l2(call_hash);
            rolling.call_begin(call_hash);
            rolling.call_end(true, &entry.returnData);
            if entry.rollingHash != rolling.current() {
                return Err(invalid(
                    "flat rolling hash differs from its supplied outcome",
                ));
            }
        }
        let committed_value = committed_l2_table_value(&[lower_entry_to_l2(entry)])
            .map_err(|reason| invalid(&reason))?;
        Ok(Self {
            entry: entry.clone(),
            committed_value,
        })
    }
}

impl PartialEq for InboundSidecar {
    fn eq(&self, other: &Self) -> bool {
        self.encoded() == other.encoded()
    }
}

impl Eq for InboundSidecar {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{ExpectedL1ToL2CallSol, RootUpdateSol};
    use alloy_primitives::Address;

    fn flat() -> InboundSidecar {
        InboundSidecar::new(IncomingEntry {
            target: Address::repeat_byte(1),
            source: Address::repeat_byte(2),
            value: U256::from(3),
            data: Bytes::from_static(&[4]),
            source_rollup_id: RollupId::MAINNET,
            l2_rollup_id: RollupId(7),
            return_data: Bytes::from_static(&[5]),
            success: true,
        })
        .unwrap()
    }

    #[test]
    fn canonical_sidecar_keeps_exact_abi_and_rejects_supplied_mutations() {
        let checked = flat();
        let original = checked.as_abi_entry();
        assert_eq!(InboundSidecar::try_from(original).unwrap(), checked);
        assert_eq!(checked.committed_value(), U256::from(3));
        assert_eq!(checked.encoded(), original.abi_encode());
        assert!(InboundSidecar::for_rollup(original, RollupId(8)).is_err());
        let mutate: &[fn(&mut ExecutionEntrySol)] = &[
            |e| {
                e.rootUpdates.push(RootUpdateSol {
                    rollupId: 7,
                    currentRoot: B256::ZERO,
                    newRoot: B256::ZERO,
                    etherDelta: crate::abi::EtherDeltaSol::ZERO,
                })
            },
            |e| e.destinationRollupId = 0,
            |e| e.destinationRollupId = 8,
            |e| e.l2ToL1Calls[0].sourceRollupId = 9,
            |e| e.l2ToL1Calls[0].isStatic = true,
            |e| e.l2ToL1Calls[0].gas = 1,
            |e| e.l2ToL1Calls[0].revertNextNCalls = 2,
            |e| e.l2ToL1Calls.clear(),
            |e| e.l2ToL1Calls.push(e.l2ToL1Calls[0].clone()),
            |e| e.proxyEntryHash = B256::ZERO,
            |e| e.proxyEntryHash = B256::repeat_byte(1),
            |e| e.rollingHash = B256::ZERO,
            |e| e.returnData = Bytes::new(),
            |e| e.success = false,
        ];
        for (index, mutation) in mutate.iter().enumerate() {
            let mut supplied = original.clone();
            mutation(&mut supplied);
            assert!(
                InboundSidecar::try_from(&supplied).is_err(),
                "accepted mutation {index}"
            );
        }
    }

    #[test]
    fn nested_sidecar_retains_rows_and_counts_all_incoming_value() {
        let mut entry = flat().as_abi_entry().clone();
        let mut nested = entry.l2ToL1Calls[0].clone();
        nested.value = U256::from(11);
        entry.expectedL1ToL2Calls.push(ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(6),
            l2ToL1Calls: vec![nested],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::new(),
        });
        let sidecar = InboundSidecar::try_from(&entry).unwrap();
        assert_eq!(sidecar.encoded(), entry.abi_encode());
        assert_eq!(sidecar.l2_entry().expectedOutgoingCalls.len(), 1);
        assert_eq!(sidecar.committed_value(), U256::from(14));
        entry.expectedL1ToL2Calls[0].l2ToL1Calls[0].value = U256::MAX;
        assert!(InboundSidecar::try_from(&entry).is_err());
    }
}
