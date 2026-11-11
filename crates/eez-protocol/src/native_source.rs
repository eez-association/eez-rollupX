//! Live Sync-block execution identities shared by production and replay.
//!
//! Identity is not authorization or a global deduplication key. Callers must
//! independently authenticate the live target, complete original settlement
//! mapping, exact native bytes, effects and execution checkpoints.

use alloy_primitives::{B256, U256, keccak256};
use alloy_sol_types::SolValue as _;

use crate::l2::ProfileError;

/// The identity inputs fixed before any group in a live candidate executes.
/// Deployment, timestamp, accepted-root and proof authorization remain bound by
/// the full offline context; they are not alternate source-hash domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSourceContext {
    chain_id: u64,
    parent_hash: B256,
}

impl LiveSourceContext {
    /// Construct from the intended live L1 parent, never the eventual log block
    /// hash or the currently accepted L2 root. Construction does not authenticate it.
    pub fn new(chain_id: u64, parent_hash: B256) -> Result<Self, ProfileError> {
        if chain_id == 0 || parent_hash.is_zero() {
            return Err(ProfileError("invalid native live source context"));
        }
        Ok(Self {
            chain_id,
            parent_hash,
        })
    }

    /// Bind a zero-based complete-group ordinal in the full Live Sync block.
    /// A later posting or a consumed-prefix selection must not reset this index.
    pub const fn entry(self, entry_index: u64) -> SourceEntry {
        SourceEntry {
            context: self,
            entry_index,
        }
    }
}

/// One complete native group's position in the full Live Sync block. Derive
/// these before selecting a consumed suffix. A load and its signed user count
/// once, including an included reverted user; lookups without an L2 tx do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEntry {
    context: LiveSourceContext,
    entry_index: u64,
}

impl SourceEntry {
    /// Zero-based whole-group position, before consumption filtering.
    pub const fn index(self) -> u64 {
        self.entry_index
    }

    /// Hash the execution position using the literal dynamic ABI string,
    /// uint256 EVM chain ID, bytes32 L1 parent and uint64 group index.
    /// Exact prefix/deployment authorization remains a separate proof obligation.
    pub fn hash(self) -> B256 {
        keccak256(
            (
                "SOURCE_ENTRY_V1",
                U256::from(self.context.chain_id),
                self.context.parent_hash,
                self.entry_index,
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
            assert_eq!(args.len(), 4);
            let live = LiveSourceContext::new(
                args[1].as_str().unwrap().parse().unwrap(),
                args[2].as_str().unwrap().parse().unwrap(),
            )
            .unwrap();
            let source = live.entry(args[3].as_str().unwrap().parse().unwrap());
            assert_eq!(
                source.hash().to_string(),
                vector["expectedKeccak256"].as_str().unwrap()
            );
            checked += 1;
        }
        assert!(checked >= 7);
    }

    #[test]
    fn independent_execution_position_encoding_and_boundaries() {
        let live = LiveSourceContext::new(916969, B256::repeat_byte(0x88)).unwrap();
        // Standard ABI: four head words, followed by the literal dynamic string.
        let mut encoded = vec![0; 192];
        encoded[31] = 128;
        encoded[56..64].copy_from_slice(&916969u64.to_be_bytes());
        encoded[64..96].fill(0x88);
        encoded[159] = 15;
        encoded[160..175].copy_from_slice(b"SOURCE_ENTRY_V1");
        for index in [0, 1, u64::MAX] {
            encoded[120..128].copy_from_slice(&index.to_be_bytes());
            assert_eq!(live.entry(index).hash(), keccak256(&encoded));
        }
        let first = live.entry(0).hash();
        assert_ne!(first, live.entry(1).hash());
        assert_ne!(
            first,
            LiveSourceContext::new(916970, B256::repeat_byte(0x88))
                .unwrap()
                .entry(0)
                .hash()
        );
        assert_ne!(
            first,
            LiveSourceContext::new(916969, B256::repeat_byte(0x89))
                .unwrap()
                .entry(0)
                .hash()
        );
        assert!(LiveSourceContext::new(0, B256::repeat_byte(0x88)).is_err());
        assert!(LiveSourceContext::new(916969, B256::ZERO).is_err());
    }
}
