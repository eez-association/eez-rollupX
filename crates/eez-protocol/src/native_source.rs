//! Prefix-bound native input identities shared by production and replay.
//!
//! Identity is not authorization or a global deduplication key. Callers must
//! independently authenticate the live target, complete original settlement
//! mapping, exact native bytes, effects and execution checkpoints.

use alloy_primitives::{B256, keccak256};
use alloy_sol_types::SolValue as _;

use crate::{RollupId, l2::ProfileError};

/// The identity inputs fixed before any group in a live candidate executes.
/// Deployment, timestamp, accepted-root and proof authorization remain bound by
/// the full offline context; they are not alternate source-hash domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSourceContext {
    rollup_id: RollupId,
    parent_hash: B256,
}

impl LiveSourceContext {
    /// Construct from the intended live L1 parent, never the eventual log block
    /// hash or the currently accepted L2 root. Construction does not authenticate it.
    pub fn new(rollup_id: RollupId, parent_hash: B256) -> Result<Self, ProfileError> {
        if rollup_id.is_mainnet() || parent_hash.is_zero() {
            return Err(ProfileError("invalid native live source context"));
        }
        Ok(Self {
            rollup_id,
            parent_hash,
        })
    }

    /// Bind an original mutable PostBatch.entries position to this live context.
    /// Index zero is the ordinary anchor and cannot produce a native group.
    pub fn entry(self, entry_index: u64) -> Result<SourceEntry, ProfileError> {
        if entry_index == 0 {
            return Err(ProfileError("ordinary anchor has no native source"));
        }
        Ok(SourceEntry {
            context: self,
            entry_index,
        })
    }
}

/// A native group's original mutable settlement-row identity. Build these from
/// the complete stream before selecting a consumed suffix, retaining gaps for
/// nonexecuting mutable rows. L1 static rows occupy a separate array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEntry {
    context: LiveSourceContext,
    entry_index: u64,
}

impl SourceEntry {
    /// Original mutable settlement-row position, before consumption filtering.
    pub const fn index(self) -> u64 {
        self.entry_index
    }

    /// Hash the independently reconstructed raw B[i] BEFORE this whole group.
    /// Never use its post-group hash, a folded carrier root or mutable acceptedRoot.
    /// The literal dynamic string and uint64 ABI types are part of the contract.
    pub fn hash(self, original_pre_group_prefix: B256) -> B256 {
        keccak256(
            (
                "SOURCE_ENTRY_V1",
                self.context.rollup_id.0,
                self.context.parent_hash,
                self.entry_index,
                original_pre_group_prefix,
            )
                .abi_encode_params(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_source_vectors_match_independent_abi_oracle() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../eez-control-rpc/conformance/native-profile-v1.json"
        ))
        .unwrap();
        let mut checked = 0;
        for vector in corpus["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["id"].as_str().unwrap().starts_with("source-"))
        {
            let args = vector["arguments"].as_array().unwrap();
            assert_eq!(args[0], "SOURCE_ENTRY_V1");
            let live = LiveSourceContext::new(
                RollupId(args[1].as_str().unwrap().parse().unwrap()),
                args[2].as_str().unwrap().parse().unwrap(),
            )
            .unwrap();
            let source = live
                .entry(args[3].as_str().unwrap().parse().unwrap())
                .unwrap();
            assert_eq!(
                source
                    .hash(args[4].as_str().unwrap().parse().unwrap())
                    .to_string(),
                vector["expectedKeccak256"].as_str().unwrap()
            );
            checked += 1;
        }
        assert_eq!(checked, 7);
    }

    #[test]
    fn identity_uses_original_row_and_raw_pre_group_prefix() {
        let live = LiveSourceContext::new(RollupId(7), B256::repeat_byte(0x88)).unwrap();
        let source = live.entry(3).unwrap();
        let prefix = B256::repeat_byte(0x44);
        // Independent manual standard-ABI layout: five head slots, followed by
        // the dynamic string's length and its padded bytes. No Solidity tuple
        // offset, prehashed domain or packed integer encoding is accepted.
        let mut encoded = vec![0u8; 7 * 32];
        encoded[31] = 5 * 32;
        encoded[63] = 7;
        encoded[64..96].fill(0x88);
        encoded[127] = 3;
        encoded[128..160].fill(0x44);
        encoded[191] = 15;
        encoded[192..207].copy_from_slice(b"SOURCE_ENTRY_V1");
        assert_eq!(source.hash(prefix), keccak256(encoded));
        assert_ne!(source.hash(prefix), live.entry(4).unwrap().hash(prefix));
        assert_ne!(source.hash(prefix), source.hash(B256::repeat_byte(0x45)));
        assert_ne!(
            source.hash(prefix),
            LiveSourceContext::new(RollupId(8), B256::repeat_byte(0x88))
                .unwrap()
                .entry(3)
                .unwrap()
                .hash(prefix)
        );
        assert_ne!(
            source.hash(prefix),
            LiveSourceContext::new(RollupId(7), B256::repeat_byte(0x89))
                .unwrap()
                .entry(3)
                .unwrap()
                .hash(prefix)
        );
        assert!(live.entry(0).is_err());
        assert!(LiveSourceContext::new(RollupId(0), prefix).is_err());
        assert!(LiveSourceContext::new(RollupId(7), B256::ZERO).is_err());
    }
}
