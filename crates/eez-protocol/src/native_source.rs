//! Native-v1 occurrence identities shared by production and independent replay.
//!
//! These commitments identify inputs, not their authorization. Callers must
//! authenticate the live context, original semantic positions, exact origin
//! transaction and execution separately before accepting or signing a block.

use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_sol_types::SolValue as _;
use eez_primitives::SYSTEM_TX_VERSION;

use crate::{RollupId, l2::ProfileError};

/// Original position in the complete authenticated semantic stream, before
/// filtering failures/static calls or taking a partially consumed suffix.
/// Neither field is a mutable-table, queue or L2 transaction index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePosition {
    /// Zero-based top-level InitiateCrossChainTransaction bracket ordinal.
    pub manifest_index: u64,
    /// Zero-based root Call/StaticCall ordinal within that bracket.
    pub top_level_call_index: u64,
}

/// Structurally checked cross-chain deployment and intended live L1 context.
/// The manager and replay checks, not construction of this value, establish
/// that the context matches canonical settlement. Historical mode cannot use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSourceContext {
    inbound_domain: B256,
    outbound_domain: B256,
    target_timestamp: u64,
    parent_hash: B256,
}

impl LiveSourceContext {
    /// Reject absent deployment/live identities without substituting defaults.
    /// Use the intended L1 child timestamp and known parent, never eventual log
    /// coordinates, acceptedRoot, or a same-height L2 prefix commitment.
    pub fn new(
        l1_chain_id: u64,
        registry: Address,
        rollup_id: RollupId,
        l2_chain_id: u64,
        target_timestamp: u64,
        parent_hash: B256,
    ) -> Result<Self, ProfileError> {
        if l1_chain_id == 0
            || l2_chain_id == 0
            || registry.is_zero()
            || rollup_id.is_mainnet()
            || target_timestamp == 0
            || parent_hash.is_zero()
        {
            return Err(ProfileError("invalid native live source context"));
        }
        let domain = |tag: &str| {
            keccak256(
                (
                    keccak256(tag),
                    U256::from(SYSTEM_TX_VERSION),
                    U256::from(l1_chain_id),
                    registry,
                    U256::from(rollup_id.0),
                    U256::from(l2_chain_id),
                )
                    .abi_encode_params(),
            )
        };
        Ok(Self {
            inbound_domain: domain("EEZ_ROLLUP0_INBOUND_SOURCE"),
            outbound_domain: domain("EEZ_ROLLUPX_OUTBOUND_SOURCE"),
            target_timestamp,
            parent_hash,
        })
    }

    /// Identify an admitted inbound root using its pinned-core call hash.
    /// Callback and value/table authorization remain separate checks.
    pub fn inbound(&self, position: SourcePosition, call_hash: B256) -> B256 {
        keccak256(
            (
                self.inbound_domain,
                U256::from(self.target_timestamp),
                self.parent_hash,
                U256::from(position.manifest_index),
                U256::from(position.top_level_call_index),
                call_hash,
            )
                .abi_encode_params(),
        )
    }

    /// Identify the single load for an L2-origin bracket. origin_transaction_hash
    /// must be Keccak of the exact admitted signed transaction immediately after
    /// this load; a digest alone does not validate that origin or its tables.
    pub fn outbound(&self, manifest_index: u64, origin_transaction_hash: B256) -> B256 {
        keccak256(
            (
                self.outbound_domain,
                U256::from(self.target_timestamp),
                self.parent_hash,
                U256::from(manifest_index),
                origin_transaction_hash,
            )
                .abi_encode_params(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    fn reference(id: &str) -> B256 {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../eez-control-rpc/conformance/native-profile-v1.json"
        ))
        .unwrap();
        corpus["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .unwrap()["expectedKeccak256"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn live(timestamp: u64, parent: B256) -> LiveSourceContext {
        LiveSourceContext::new(
            1,
            eez_primitives::EEZL2_ADDRESS,
            RollupId(7),
            901,
            timestamp,
            parent,
        )
        .unwrap()
    }

    #[test]
    fn independent_cross_chain_source_vectors() {
        let ctx = live(144, B256::repeat_byte(0x88));
        assert_eq!(ctx.inbound_domain, reference("inbound-domain"));
        assert_eq!(ctx.outbound_domain, reference("outbound-domain"));
        let call = crate::common_cross_chain_call_hash(crate::CallHashInput {
            call_mode: crate::CallMode::Mutable,
            source_address: address!("1111111111111111111111111111111111111111"),
            source_rollup_id: RollupId(0),
            target_address: address!("2222222222222222222222222222222222222222"),
            target_rollup_id: RollupId(7),
            value: U256::from(5),
            data: &[0xaa, 0xbb],
        });
        assert_eq!(call, reference("core-call-hash"));
        let p = SourcePosition {
            manifest_index: 1,
            top_level_call_index: 2,
        };
        assert_eq!(ctx.inbound(p, call), reference("inbound-base"));
        assert_eq!(
            ctx.inbound(p, call),
            reference("inbound-after-prefix-consumed")
        );
        assert_eq!(
            ctx.inbound(
                SourcePosition {
                    top_level_call_index: 3,
                    ..p
                },
                call
            ),
            reference("inbound-next-root")
        );
        assert_eq!(
            ctx.inbound(
                SourcePosition {
                    manifest_index: 2,
                    ..p
                },
                call
            ),
            reference("inbound-next-bracket")
        );
        assert_eq!(
            live(156, ctx.parent_hash).inbound(p, call),
            reference("inbound-new-target")
        );
        assert_eq!(
            live(144, B256::repeat_byte(0xaa)).inbound(p, call),
            reference("inbound-new-parent")
        );
        assert_eq!(
            ctx.outbound(0, B256::repeat_byte(0x66)),
            reference("outbound-base")
        );
        assert_eq!(
            ctx.outbound(1, B256::repeat_byte(0x66)),
            reference("outbound-next-bracket")
        );
        assert_eq!(
            ctx.outbound(0, B256::repeat_byte(0xab)),
            reference("outbound-different-origin")
        );
    }
}
