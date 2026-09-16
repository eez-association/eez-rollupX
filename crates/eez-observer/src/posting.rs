//! Decode the existing protocol ABI without storing calldata or inferring effects.
use alloy_primitives::{B256, keccak256};
use alloy_sol_types::{SolCall, SolType};
use eez_protocol::{EvmBatch, abi::postAndVerifyBatchCall, entries::decode_postbatch};

mod abi_budget;

pub(crate) const SELECTOR: [u8; 4] = postAndVerifyBatchCall::SELECTOR;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Analysis {
    pub state: &'static str,
    pub calldata_hash: Option<B256>,
}

impl Analysis {
    pub const fn other() -> Self {
        Self {
            state: "other",
            calldata_hash: None,
        }
    }

    pub fn parse(calldata: &[u8]) -> Self {
        if !calldata.starts_with(&SELECTOR) {
            return Self::other();
        }
        let state = if calldata.len() < 36 {
            "invalid"
        } else if !abi_budget::admits(&calldata[4..]) {
            "unverified"
        } else {
            match decode_postbatch(calldata) {
                Err(_) => "invalid",
                Ok(batch) => {
                    // Match the signer's canonical argument check without
                    // cloning the decoded batch or storing any calldata.
                    let token = <EvmBatch as SolType>::tokenize(&batch);
                    if alloy_sol_types::abi::encode_sequence(&(token,)) == calldata[4..] {
                        "decoded"
                    } else {
                        "unverified"
                    }
                }
            }
        };
        Self {
            state,
            calldata_hash: Some(keccak256(calldata)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_protocol::entries::encode_postbatch;

    #[test]
    fn selector_alone_is_not_a_decoded_post() {
        assert_eq!(Analysis::parse(&[]), Analysis::other());
        assert_eq!(Analysis::parse(&[1, 2, 3, 4]), Analysis::other());
        assert_eq!(Analysis::parse(&SELECTOR).state, "invalid");
        let calldata = encode_postbatch(&EvmBatch::default());
        assert_eq!(
            format!("0x{}", alloy_primitives::hex::encode(&calldata)),
            include_str!("../../../observability/tests/fixtures/empty-post-calldata.hex").trim()
        );
        let analysis = Analysis::parse(&calldata);
        assert_eq!(analysis.state, "decoded");
        assert_eq!(analysis.calldata_hash, Some(keccak256(&calldata)));
        assert_eq!(
            Analysis::parse(&calldata[..calldata.len() - 1]).state,
            "unverified"
        );
    }
    #[test]
    fn legacy_selector_stays_separate_and_current_nested_arrays_decode() {
        let raw = include_str!(
            "../../eez-proof-signer/tests/fixtures/captured-anchor-40155/postbatch.hex"
        )
        .trim();
        let bytes = alloy_primitives::hex::decode(raw.strip_prefix("0x").unwrap_or(raw)).unwrap();
        // This captured protocol fixture deliberately predates the int192 ABI
        // migration. Its old selector is not silently reinterpreted as current.
        assert_eq!(Analysis::parse(&bytes).state, "other");
        for size in [0, 1, 31, 32, 33, 255, 4096] {
            let batch = EvmBatch {
                entries: vec![
                    eez_protocol::abi::ExecutionEntrySol {
                        returnData: vec![7; size].into(),
                        ..Default::default()
                    };
                    3
                ],
                proofs: vec![vec![0; size].into(), vec![1; size + 1].into()],
                callData: vec![2; size].into(),
                ..Default::default()
            };
            assert_eq!(Analysis::parse(&encode_postbatch(&batch)).state, "decoded");
        }
    }

    #[test]
    fn canonical_nested_call_and_static_root_arrays_fit_the_budget() {
        use eez_protocol::abi::*;
        let call = L2ToL1CallSol {
            revertNextNCalls: 0,
            isStatic: false,
            gas: 42,
            sourceAddress: Default::default(),
            sourceRollupId: 1,
            targetAddress: Default::default(),
            value: Default::default(),
            data: vec![1; 33].into(),
        };
        let batch = EvmBatch {
            entries: vec![ExecutionEntrySol {
                rootUpdates: vec![RootUpdateSol {
                    rollupId: 1,
                    etherDelta: Default::default(),
                    currentRoot: B256::ZERO,
                    newRoot: B256::repeat_byte(1),
                }],
                l2ToL1Calls: vec![call.clone()],
                expectedL1ToL2Calls: vec![ExpectedL1ToL2CallSol {
                    expectedL1toL2Hash: B256::ZERO,
                    l2ToL1Calls: vec![call],
                    revertedOrStaticRollingHash: B256::ZERO,
                    success: true,
                    returnData: vec![2; 65].into(),
                }],
                ..Default::default()
            }],
            staticEntries: vec![StaticExecutionEntrySol {
                expectedRoots: vec![ExpectedRootPerRollupSol {
                    rollupId: 1,
                    root: B256::ZERO,
                }],
                ..Default::default()
            }],
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: 1,
                proofSystemIndexes: vec![0, 1],
            }],
            proofSystems: vec![Default::default(); 2],
            blobIndices: vec![Default::default()],
            proofs: vec![vec![3; 64].into()],
            ..Default::default()
        };
        assert_eq!(Analysis::parse(&encode_postbatch(&batch)).state, "decoded");
    }

    #[test]
    fn forged_lengths_aliases_and_trailing_bytes_stay_unverified_without_decoding() {
        let original = encode_postbatch(&EvmBatch::default());
        let mut huge = original.clone();
        // The first outer tuple offset cannot be interpreted as an allocation.
        huge[4..36].fill(0xff);
        assert_eq!(Analysis::parse(&huge).state, "unverified");
        let offset =
            |word: &[u8]| usize::try_from(alloy_primitives::U256::from_be_slice(word)).unwrap();
        let root = 4 + offset(&original[4..36]);
        let first_array = root + offset(&original[root..root + 32]);
        huge = original.clone();
        huge[first_array..first_array + 32].fill(0xff);
        assert_eq!(Analysis::parse(&huge).state, "unverified");
        let mut aliased = original.clone();
        aliased[root + 32..root + 64].copy_from_slice(&original[root..root + 32]);
        assert_eq!(Analysis::parse(&aliased).state, "unverified");
        let mut trailing = original;
        trailing.extend_from_slice(&[0; 32]);
        assert_eq!(Analysis::parse(&trailing).state, "unverified");
    }
}
