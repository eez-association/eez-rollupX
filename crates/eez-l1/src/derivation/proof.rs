//! Independent canonical L1 facts needed to authorize an L2 candidate.
//! No blob hydration or historical L2 execution is delegated to the source:
//! accepted endpoint headers are hash-bound to actual registry settlement.

use alloy_consensus::Header;
use alloy_eips::BlockId;
use alloy_primitives::{Address, B256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_sol_types::SolCall;
use eez_protocol::{
    abi::rollupsCall,
    l2::{AnchorKind, validate_header},
    public_inputs::L1PostingContext,
};

use super::{AuthenticatedL1Header, DerivationError, DerivationSource, fetch};
use crate::{L1BlockRef, scan::scan_batch_logs_range};

mod retrieval;

pub use eez_protocol::l2::ParentProvenance;

#[derive(Debug, thiserror::Error)]
pub enum ProofContextError {
    #[error(transparent)]
    Source(#[from] DerivationError),
    #[error("L1 proof target is no longer canonical")]
    StaleTarget,
    #[error("invalid candidate provenance: {0}")]
    Invalid(&'static str),
}

/// Authenticated starting facts, not authorization to sign. The signer must
/// still independently execute every candidate block, bind its seed to this
/// value, and prove that `registry_root` is its parent or a consumed terminal
/// prefix. A decoded registry root alone never supplies parent provenance.
#[derive(Debug, Clone)]
pub struct AuthenticatedProofContext {
    target: L1PostingContext,
    parent: Header,
    registry_root: B256,
    descendant_seed: B256,
    schedule: eez_protocol::l2::Schedule,
}

impl AuthenticatedProofContext {
    /// Validate the whole candidate against authenticated timing, independently
    /// of execution and of the registry's possibly consumed terminal prefix.
    pub fn check_candidate(
        &self,
        blocks: u32,
        has_actions: bool,
    ) -> Result<(), eez_protocol::l2::ProfileError> {
        self.schedule
            .candidate(
                &self.parent,
                blocks,
                self.target.target_timestamp,
                has_actions,
            )
            .map(|_| ())
    }

    pub const fn target(&self) -> L1PostingContext {
        self.target
    }
    pub const fn parent(&self) -> &Header {
        &self.parent
    }
    pub const fn registry_root(&self) -> B256 {
        self.registry_root
    }
    pub const fn descendant_seed(&self) -> B256 {
        self.descendant_seed
    }
}

impl<P: Provider> DerivationSource<P> {
    /// Authenticate a target, the accepted L2 parent and its descendant seed.
    /// The caller owns the request deadline around these reads. The target is
    /// checked again after reads for a coherent canonical snapshot. Later head
    /// changes are handled by the on-chain parent/timestamp signature binding.
    pub async fn proof_context(
        &self,
        registry: Address,
        rollup_id: u64,
        target: L1PostingContext,
        supplied: ParentProvenance,
    ) -> Result<AuthenticatedProofContext, ProofContextError> {
        target
            .validate()
            .map_err(|_| ProofContextError::Invalid("L1 target shape"))?;
        if registry.is_zero()
            || rollup_id == 0
            || target.chain_id != self.schedule.config().l1_reference.chain_id
        {
            return Err(ProofContextError::Invalid(
                "configured registry or chain identity",
            ));
        }
        let l1_parent = self.canonical_target(target).await?;
        if target.target_timestamp <= l1_parent.header.timestamp
            || target
                .target_timestamp
                .checked_sub(self.schedule.config().l1_reference.timestamp)
                .is_none_or(|elapsed| !elapsed.is_multiple_of(self.schedule.l1_slot_seconds()))
        {
            return Err(ProofContextError::Invalid("target child slot"));
        }
        self.schedule
            .validate_position(&supplied.parent)
            .map_err(DerivationError::from)?;
        if supplied.parent.number != 0 {
            validate_header(&supplied.parent).map_err(DerivationError::from)?;
        }
        let parent_hash = supplied.parent.hash_slow();
        if supplied.parent.number == 0 {
            if parent_hash != self.schedule.genesis_hash() || supplied.accepted_l1_number != 0 {
                return Err(ProofContextError::Invalid("configured genesis parent"));
            }
        } else {
            let number = supplied.accepted_l1_number;
            if number < self.schedule.config().l1_reference.number || number > target.parent_number
            {
                return Err(ProofContextError::Invalid("parent acceptance height"));
            }
            let canonical = self.header_at(number).await?;
            let accepted = self
                .accepted_endpoints(registry, rollup_id, &canonical)
                .await?;
            if !accepted.contains(&parent_hash) {
                return Err(ProofContextError::Invalid(
                    "parent is not an accepted endpoint",
                ));
            }
            self.schedule
                .anchor_kind(
                    supplied.parent.number,
                    supplied.parent.timestamp,
                    canonical.header.timestamp,
                )
                .map_err(DerivationError::from)?;
        }

        // Timestamp placement proves a live refresh is possible in exactly one
        // canonical L1 block. Later catch-up variants cannot replace that seed.
        let mut seed = supplied.parent.mix_hash;
        let slot = if supplied.parent.number == 0 {
            None
        } else {
            self.slot_at_timestamp(supplied.parent.timestamp, &l1_parent)
                .await?
        };
        if let Some(slot) = slot {
            let accepted = self.accepted_endpoints(registry, rollup_id, &slot).await?;
            if accepted.len() != supplied.live_slot_endpoints.len() {
                return Err(ProofContextError::Invalid(
                    "incomplete live-slot endpoint set",
                ));
            }
            for (hash, header) in accepted.iter().zip(&supplied.live_slot_endpoints) {
                validate_header(header).map_err(DerivationError::from)?;
                let kind = self
                    .schedule
                    .anchor_kind(header.number, header.timestamp, slot.header.timestamp)
                    .map_err(DerivationError::from)?;
                if *hash != header.hash_slow() {
                    return Err(ProofContextError::Invalid("live-slot endpoint hash"));
                }
                if header.number == supplied.parent.number && kind == AnchorKind::Live {
                    seed = slot.header.mix_hash;
                }
            }
        } else if !supplied.live_slot_endpoints.is_empty() {
            return Err(ProofContextError::Invalid("unexpected live-slot endpoints"));
        }

        let call = rollupsCall {
            rollupId: rollup_id,
        };
        let result = self
            .provider
            .call(
                TransactionRequest::default()
                    .to(registry)
                    .input(call.abi_encode().into()),
            )
            .block(BlockId::hash_canonical(target.parent_hash))
            .await
            .map_err(|_| DerivationError::Unavailable("registry state at target parent"))?;
        if result.len() != 96 {
            return Err(ProofContextError::Invalid("registry getter response"));
        }
        let state = rollupsCall::abi_decode_returns(&result)
            .map_err(|_| ProofContextError::Invalid("registry getter response"))?;
        if state.rollupContract.is_zero() || state.root.is_zero() {
            return Err(ProofContextError::Invalid("rollup is not registered"));
        }
        self.canonical_target(target).await?;
        Ok(AuthenticatedProofContext {
            target,
            parent: supplied.parent,
            registry_root: state.root,
            descendant_seed: seed,
            schedule: self.schedule,
        })
    }

    /// Pin the named canonical block, without requiring it to remain the tip.
    /// Extending this block does not change its authenticated history. Replacing
    /// it during the reads must still invalidate the snapshot.
    async fn canonical_target(
        &self,
        target: L1PostingContext,
    ) -> Result<AuthenticatedL1Header, ProofContextError> {
        if target.parent_number < self.schedule.config().l1_reference.number {
            return Err(ProofContextError::Invalid(
                "L1 target predates genesis reference",
            ));
        }
        let parent = self.header_at(target.parent_number).await?;
        if parent.reference()
            != (L1BlockRef {
                number: target.parent_number,
                hash: target.parent_hash,
            })
        {
            return Err(ProofContextError::StaleTarget);
        }
        Ok(parent)
    }

    async fn header_at(&self, number: u64) -> Result<AuthenticatedL1Header, DerivationError> {
        fetch(
            &self.provider,
            self.schedule.config().l1_reference.chain_id,
            number.into(),
        )
        .await
    }

    /// Canonical L1 timestamps are strictly increasing. Binary search avoids
    /// traversing history or allocating by a caller-supplied L2 block count.
    pub async fn slot_at_timestamp(
        &self,
        timestamp: u64,
        through: &AuthenticatedL1Header,
    ) -> Result<Option<AuthenticatedL1Header>, DerivationError> {
        let reference = self.schedule.config().l1_reference;
        if timestamp < reference.timestamp || timestamp > through.header.timestamp {
            return Ok(None);
        }
        let (mut low, mut high) = (reference.number, through.header.number);
        while low <= high {
            let middle = low + (high - low) / 2;
            let header = self.header_at(middle).await?;
            match header.header.timestamp.cmp(&timestamp) {
                std::cmp::Ordering::Equal => return Ok(Some(header)),
                std::cmp::Ordering::Less => {
                    let Some(next) = middle.checked_add(1) else {
                        return Ok(None);
                    };
                    low = next;
                }
                std::cmp::Ordering::Greater => {
                    let Some(previous) = middle.checked_sub(1) else {
                        return Ok(None);
                    };
                    high = previous;
                }
            }
        }
        Ok(None)
    }

    async fn accepted_endpoints(
        &self,
        registry: Address,
        rollup_id: u64,
        containing: &AuthenticatedL1Header,
    ) -> Result<Vec<B256>, ProofContextError> {
        // Use the same complete same-block attribution as the deriver, including
        // partial consumption, foreign batches and skipped leading entries.
        let batches = scan_batch_logs_range(
            &self.provider,
            registry,
            rollup_id,
            containing.header.number,
            containing.header.number,
        )
        .await
        .map_err(|_| DerivationError::Unavailable("canonical settlement attribution"))?;
        let mut endpoints = Vec::new();
        for batch in batches {
            if batch.l1_block_hash != containing.hash {
                return Err(ProofContextError::StaleTarget);
            }
            if !batch.settlement.is_empty() {
                endpoints.push(
                    batch
                        .settlement
                        .final_state
                        .ok_or(ProofContextError::Invalid("settled endpoint missing"))?,
                );
            }
        }
        Ok(endpoints)
    }
}

#[cfg(test)]
mod tests;
