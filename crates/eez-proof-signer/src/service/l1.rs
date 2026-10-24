//! Conditional offline authorization. Settlement authenticates these exact
//! public inputs through the deployed manager; no chain provider exists here.

use crate::validate::{
    ValidatedWindow,
    streaming::{BlockAnchor, ValidatedStreamBlock},
};
use alloy_consensus::Header;
use alloy_primitives::Address;
use eez_protocol::{
    EvmBatch,
    l1_history::{EvidenceAnchor, HeaderChain, OfflineContext, OfflineProfile},
    l2::Schedule,
    public_inputs::L1PostingContext,
};
use std::sync::Arc;

/// Operator-configured deployment, derived from the same local genesis and L1
/// rules as execution. Request claims never choose these identities.
#[derive(Debug)]
pub(crate) struct OfflineDeployment {
    pub(crate) registry: Address,
    pub(crate) manager: Address,
    pub(crate) profile: OfflineProfile,
    schedule: Schedule,
}

impl OfflineDeployment {
    pub(crate) fn new(
        registry: Address,
        manager: Address,
        profile: OfflineProfile,
        schedule: Schedule,
    ) -> eyre::Result<Self> {
        eyre::ensure!(
            !registry.is_zero() && !manager.is_zero(),
            "offline deployment addresses must be nonzero"
        );
        eyre::ensure!(
            profile.genesisHash == schedule.genesis_hash(),
            "offline profile genesis mismatch"
        );
        Ok(Self {
            registry,
            manager,
            profile,
            schedule,
        })
    }

    /// Execute in the bounded validation worker. Truncating a session prefix
    /// can copy a large bridge and may never run on an async executor thread.
    pub(super) fn candidate(
        &self,
        rollup_id: u64,
        batch: &EvmBatch,
        posting: Option<L1PostingContext>,
        parent_anchor: BlockAnchor,
        blocks: &[Arc<ValidatedStreamBlock>],
        history: &HeaderChain,
    ) -> Result<CandidateContext, &'static str> {
        let first = blocks.first().ok_or("empty offline range")?;
        let terminal = blocks.last().ok_or("empty offline range")?;
        let parent = first
            .parent_header()
            .map_err(|_| "missing exact execution parent")?;
        if parent.number != parent_anchor.number
            || parent.hash_slow() != parent_anchor.hash
            || parent.state_root != parent_anchor.state_root
        {
            return Err("Begin/subrange parent differs from replayed parent header");
        }
        self.schedule.validate_position(&parent).map_err(|e| e.0)?;
        let history = Arc::new(
            history
                .prefix(terminal.l1_checkpoint_len())
                .map_err(|e| e.0)?,
        );
        let end = history.endpoint().map_err(|e| e.0)?;
        let (anchor, eligibility_timestamp) = match (batch.blockNumber, posting) {
            (u64::MAX, Some(posting)) => {
                posting.validate().map_err(|_| "invalid posting context")?;
                if alloy_primitives::U256::from(posting.chain_id) != self.profile.l1ChainId {
                    return Err("posting chain differs from configured L1");
                }
                (
                    EvidenceAnchor::ActualParent {
                        number: posting.parent_number,
                        hash: posting.parent_hash,
                        target_timestamp: posting.target_timestamp,
                    },
                    posting.target_timestamp,
                )
            }
            (0, _) => return Err("zero is a reserved anchor selector"),
            (u64::MAX, None) => return Err("actual-parent mode requires posting_context"),
            (_, Some(_)) => return Err("historical mode forbids posting_context"),
            (number, None) => (
                EvidenceAnchor::Historical {
                    number,
                    hash: end.hash(),
                },
                end.header().timestamp,
            ),
        };
        history.verify_anchor(anchor).map_err(|e| e.0)?;
        for block in blocks {
            block
                .verify_l1_anchor(&history, anchor)
                .map_err(|_| "L1 source maximality or anchor termination failed")?;
        }
        let [pin] = batch.expectedRootPerRollup.as_slice() else {
            return Err("offline settlement requires exactly one accepted-root pin");
        };
        if pin.rollupId != rollup_id || pin.root.is_zero() {
            return Err("accepted-root pin identifies another rollup or zero root");
        }
        let context = OfflineContext::new(
            &self.profile,
            self.registry,
            self.manager,
            rollup_id,
            pin.root,
            anchor,
            end.header().timestamp,
        )
        .map_err(|e| e.0)?;
        let candidate = CandidateContext {
            context,
            parent,
            schedule: self.schedule,
            history,
            eligibility_timestamp,
            historical: matches!(anchor, EvidenceAnchor::Historical { .. }),
        };
        candidate.check_candidate(
            u32::try_from(blocks.len()).map_err(|_| "candidate block count exceeds V0")?,
            false,
        )?;
        Ok(candidate)
    }
}

/// Independently checked evidence and conditional settlement context. Its root
/// pin is authorized only after whole-group execution binding below.
pub(super) struct CandidateContext {
    context: OfflineContext,
    parent: Header,
    schedule: Schedule,
    history: Arc<HeaderChain>,
    eligibility_timestamp: u64,
    historical: bool,
}

impl CandidateContext {
    pub(super) const fn public_inputs(&self) -> OfflineContext {
        self.context
    }
    pub(super) fn history(&self) -> Arc<HeaderChain> {
        Arc::clone(&self.history)
    }
    pub(super) const fn rules_timestamp(&self) -> u64 {
        self.eligibility_timestamp
    }

    fn check_candidate(&self, count: u32, has_actions: bool) -> Result<(), &'static str> {
        if self.historical && has_actions {
            return Err("historical mode forbids every cross-chain action");
        }
        self.schedule
            .candidate(&self.parent, count, self.eligibility_timestamp, has_actions)
            .map_err(|e| e.0)?;
        Ok(())
    }

    pub(super) fn check_execution(&self, window: &ValidatedWindow) -> Result<(), &'static str> {
        if window.window_pre_block_hash() != self.parent.hash_slow() {
            return Err("window does not extend its exact execution parent");
        }
        let count = u32::try_from(window.preceding_blocks().len() + 1)
            .map_err(|_| "candidate block count exceeds V0")?;
        self.check_candidate(count, false)
    }

    /// Bind the fully decoded and replayed action stream to this mode's timing
    /// rules. Root-pin coverage is checked against bound settlement effects.
    pub(super) fn check_actions(
        &self,
        window: &ValidatedWindow,
        has_actions: bool,
    ) -> Result<(), &'static str> {
        let count = u32::try_from(window.preceding_blocks().len() + 1)
            .map_err(|_| "candidate block count exceeds V0")?;
        self.check_candidate(count, has_actions)
    }
}
