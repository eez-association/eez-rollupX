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
        sources: &[alloy_primitives::B256],
    ) -> Result<Vec<SyncPair>, String> {
        eez_protocol::system_tx::build_cross_chain_sync_groups(
            outbound,
            inbound,
            &self.context,
            sources,
        )
    }

    pub(super) fn source_hashes(
        &self,
        transactions: &[eez_protocol::blob_semantics::SemanticTransaction],
        context: &eez_protocol::l1_history::OfflineContext,
    ) -> Result<Vec<alloy_primitives::B256>, String> {
        let live = context
            .live_source_context()
            .map_err(|error| error.to_string())?;
        if context.rollup_id() != self.context.this_rollup_id {
            return Err("native source context belongs to another rollup".into());
        }
        let actions = eez_protocol::blob_semantics::ReconstructionActions::from_semantics(
            transactions,
            eez_protocol::RollupId(self.context.this_rollup_id),
            self.context.l2_chain_id,
        )
        .map_err(|error| error.to_string())?;
        Ok(actions.source_hashes(&live))
    }
}
