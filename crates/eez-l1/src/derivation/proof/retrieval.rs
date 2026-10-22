//! Prover-owned evidence retrieval for the v2 settlement contract.

use alloy_consensus::Header;
use alloy_primitives::{Address, B256, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::Filter;
use alloy_sol_types::SolEvent;
use eez_protocol::{abi::L2ExecutionPerformed, public_inputs::L1PostingContext};

use super::{
    AuthenticatedProofContext, DerivationError, DerivationSource, ParentProvenance,
    ProofContextError,
};

const HISTORY_PAGE_BLOCKS: u64 = 2_048;

impl<P: Provider> DerivationSource<P> {
    /// Retrieve the selected predecessor and live-slot headers from an
    /// operator-selected L2 source, then authenticate the complete evidence
    /// through the same canonical L1 gates as v1. The caller must bound the
    /// whole operation with its request deadline. No scan result is persisted
    /// or treated as authorization without `proof_context`.
    #[allow(clippy::too_many_arguments)]
    pub async fn owned_proof_context(
        &self,
        registry: Address,
        rollup_id: u64,
        target: L1PostingContext,
        number: u64,
        hash: B256,
        state_root: B256,
        l2: &impl Provider,
        max_headers: usize,
    ) -> Result<AuthenticatedProofContext, ProofContextError> {
        target
            .validate()
            .map_err(|_| ProofContextError::Invalid("L1 target shape"))?;
        if registry.is_zero()
            || rollup_id == 0
            || max_headers == 0
            || target.chain_id != self.schedule.config().l1_reference.chain_id
            || target.parent_number < self.schedule.config().l1_reference.number
        {
            return Err(ProofContextError::Invalid(
                "configured provenance identity or bounds",
            ));
        }
        let parent = header_by_hash(l2, hash).await?;
        if parent.number != number || parent.state_root != state_root {
            return Err(ProofContextError::Invalid(
                "selected parent differs from L2 header",
            ));
        }
        let through = self.canonical_target(target).await?;
        let accepted_l1_number = if number == 0 {
            0
        } else {
            self.acceptance_height(registry, rollup_id, target.parent_number, hash)
                .await?
        };
        let mut live_slot_endpoints = Vec::new();
        if number != 0
            && let Some(slot) = self.slot_at_timestamp(parent.timestamp, &through).await?
        {
            let hashes = self.accepted_endpoints(registry, rollup_id, &slot).await?;
            if hashes.len().saturating_add(1) > max_headers {
                return Err(DerivationError::Unavailable(
                    "live-slot provenance exceeds header limit",
                )
                .into());
            }
            for endpoint in hashes {
                live_slot_endpoints.push(header_by_hash(l2, endpoint).await?);
            }
        }
        self.proof_context(
            registry,
            rollup_id,
            target,
            ParentProvenance {
                parent,
                accepted_l1_number,
                live_slot_endpoints,
            },
        )
        .await
    }

    /// Logs only locate candidate containing blocks. The complete same-block
    /// attribution scanner decides whether this root was an accepted endpoint;
    /// a partial execution event alone cannot authorize the predecessor.
    async fn acceptance_height(
        &self,
        registry: Address,
        rollup_id: u64,
        through: u64,
        hash: B256,
    ) -> Result<u64, ProofContextError> {
        let floor = self.schedule.config().l1_reference.number;
        let mut high = through;
        loop {
            let low = high.saturating_sub(HISTORY_PAGE_BLOCKS - 1).max(floor);
            let filter = Filter::new()
                .address(registry)
                .event_signature(L2ExecutionPerformed::SIGNATURE_HASH)
                .topic1(B256::from(U256::from(rollup_id)))
                .from_block(low)
                .to_block(high);
            let logs = self
                .provider
                .get_logs(&filter)
                .await
                .map_err(|_| DerivationError::Unavailable("parent acceptance history"))?;
            let mut candidates = std::collections::BTreeSet::new();
            for log in logs {
                if log.address() != registry
                    || log.topics().len() != 2
                    || log.data().data.len() != 64
                    || log.removed
                {
                    return Err(ProofContextError::Invalid("parent acceptance log shape"));
                }
                let event = L2ExecutionPerformed::decode_log(&log.inner)
                    .map_err(|_| ProofContextError::Invalid("parent acceptance log"))?;
                if event.rollupId != rollup_id {
                    return Err(ProofContextError::Invalid("parent acceptance rollup"));
                }
                if event.newRoot == hash {
                    let number = log
                        .block_number
                        .ok_or(DerivationError::Unavailable("acceptance log block number"))?;
                    if number < low || number > high {
                        return Err(ProofContextError::Invalid("acceptance log outside scan"));
                    }
                    candidates.insert((
                        number,
                        log.block_hash
                            .ok_or(DerivationError::Unavailable("acceptance log block hash"))?,
                    ));
                }
            }
            for (number, claimed_hash) in candidates.into_iter().rev() {
                let canonical = self.header_at(number).await?;
                if canonical.hash != claimed_hash {
                    return Err(ProofContextError::StaleTarget);
                }
                if self
                    .accepted_endpoints(registry, rollup_id, &canonical)
                    .await?
                    .contains(&hash)
                {
                    return Ok(number);
                }
            }
            if low == floor {
                return Err(ProofContextError::Invalid(
                    "parent has no canonical acceptance",
                ));
            }
            high = low - 1;
        }
    }
}

async fn header_by_hash(provider: &impl Provider, hash: B256) -> Result<Header, ProofContextError> {
    let block = provider
        .get_block_by_hash(hash)
        .await
        .map_err(|_| DerivationError::Unavailable("L2 provenance header"))?
        .ok_or(DerivationError::Unavailable("L2 provenance header missing"))?;
    let advertised_hash = block.header.hash;
    let header = block.header.inner;
    if advertised_hash != hash || header.hash_slow() != hash {
        return Err(ProofContextError::Invalid("L2 provenance header hash"));
    }
    Ok(header)
}
