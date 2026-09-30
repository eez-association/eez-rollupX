//! Canonical keyless reconstruction from independently authorized effects.

use std::num::NonZeroU64;

use eez_protocol::entries::InboundSidecar;
use eez_protocol::system_tx::{OutboundSyncGroup, SyncPair, SystemTxContext};

/// Fixed native protocol parameters bound to the same chain and rollup identities
/// as execution validation. Code identity at EEZL2 remains a genesis invariant.
#[derive(Debug)]
pub(crate) struct SystemTransactionReconstructor {
    context: SystemTxContext,
}

impl SystemTransactionReconstructor {
    pub(crate) fn new(l2_chain_id: u64, expected_rollup_id: NonZeroU64) -> Self {
        Self {
            context: SystemTxContext {
                eezl2_address: eez_primitives::EEZL2_ADDRESS,
                l2_chain_id,
                this_rollup_id: expected_rollup_id.get(),
            },
        }
    }

    /// Checked shape is necessary but insufficient: the caller must supply the
    /// effects admitted by independent settlement authorization before comparing
    /// these bytes against replayed block transactions.
    pub(super) fn reconstruct_sync_pairs(
        &self,
        outbound: &[OutboundSyncGroup],
        inbound: &[InboundSidecar],
        starting_nonce: u64,
    ) -> Result<Vec<SyncPair>, String> {
        eez_protocol::system_tx::build_cross_chain_sync_groups(
            outbound,
            inbound,
            &self.context,
            starting_nonce,
        )
    }
}
