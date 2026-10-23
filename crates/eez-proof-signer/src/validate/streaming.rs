//! Retained execution facts for v2. Construction consumes checked backend output;
//! neither a stream cursor nor these facts authorize settlement.

use std::sync::Arc;

use alloy_consensus::Header;
use alloy_primitives::B256;
use eez_protocol::l1_history::{EvidenceAnchor, HeaderChain, HeaderRules};

use super::{ValidatedSettlingBlock, ValidatedWindow, ValidationError, Validator};
use crate::{cancel::CancellationToken, window::AdmittedBlock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockAnchor {
    pub(crate) number: u64,
    pub(crate) hash: B256,
    pub(crate) state_root: B256,
}

/// Exact header and checked execution, including the original replay witness
/// needed for target-dependent static/lookup observation checks at Finalize.
#[derive(Debug)]
pub(crate) struct ValidatedStreamBlock {
    header: Header,
    hash: B256,
    settling: ValidatedSettlingBlock,
    l1_segment: Vec<alloy_primitives::Bytes>,
    l1_checkpoint_len: usize,
    // Both inherited and newly selected sources were checked against the
    // authenticated execution state. Later headers cannot rewrite either.
    l1_selections: [(u64, B256); 2],
}

impl ValidatedStreamBlock {
    pub(crate) fn as_anchor(&self) -> BlockAnchor {
        BlockAnchor {
            number: self.number(),
            hash: self.hash,
            state_root: self.header.state_root,
        }
    }

    pub(crate) fn number(&self) -> u64 {
        self.header.number
    }

    pub(crate) fn hash(&self) -> B256 {
        self.hash
    }

    pub(crate) fn parent_hash(&self) -> B256 {
        self.header.parent_hash
    }

    pub(crate) fn post_state_root(&self) -> B256 {
        self.header.state_root
    }

    pub(crate) fn matches_witness(&self, admitted: &AdmittedBlock) -> bool {
        self.settling.replay_input.witness == admitted.witness
            && self.l1_segment == admitted.l1_headers_rlp
    }

    pub(crate) const fn l1_checkpoint_len(&self) -> usize {
        self.l1_checkpoint_len
    }

    pub(crate) fn verify_l1_anchor(
        &self,
        chain: &HeaderChain,
        anchor: EvidenceAnchor,
    ) -> Result<(), ValidationError> {
        for (cutoff, hash) in self.l1_selections {
            chain
                .verify_cutoff(cutoff, hash, anchor)
                .map_err(|error| ValidationError::Rejected(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn parent_header(&self) -> Result<Header, ValidationError> {
        let raw = self
            .settling
            .replay_input
            .witness
            .headers
            .last()
            .ok_or_else(|| ValidationError::Rejected("missing execution parent header".into()))?;
        alloy_rlp::decode_exact(raw)
            .map_err(|_| ValidationError::Rejected("invalid execution parent header".into()))
    }

    pub(crate) fn rlp(&self) -> &[u8] {
        self.settling.block.rlp()
    }
}

impl Validator {
    pub(crate) fn validate_stream_block(
        &self,
        parent: BlockAnchor,
        admitted: AdmittedBlock,
        mut history: Arc<HeaderChain>,
        checkpoint_len: usize,
        previous: Option<&ValidatedStreamBlock>,
        rules: &HeaderRules,
        max_headers: usize,
        cancellation: &CancellationToken,
    ) -> Result<(ValidatedStreamBlock, Arc<HeaderChain>), ValidationError> {
        if parent.number.checked_add(1) != Some(admitted.declared_number)
            || admitted.claimed_parent_hash != parent.hash
        {
            return Err(ValidationError::Rejected("stream parent mismatch".into()));
        }
        let block: eez_primitives::Block = alloy_rlp::decode_exact(&admitted.rlp)
            .map_err(|error| ValidationError::Rejected(format!("block RLP: {error}")))?;
        let hash = block.header.hash_slow();
        if block.header.number != admitted.declared_number
            || hash != admitted.claimed_hash
            || block.header.parent_hash != parent.hash
        {
            return Err(ValidationError::Rejected(
                "stream block identity mismatch".into(),
            ));
        }
        if history.headers().len() != checkpoint_len {
            history = Arc::new(
                history
                    .prefix(checkpoint_len)
                    .map_err(|error| ValidationError::Rejected(error.to_string()))?,
            );
        }
        let segment = admitted.l1_headers_rlp.clone();
        // Most L2 blocks retain the same source. Preserve the shared immutable
        // chain for empty segments instead of copying its entire retained prefix.
        if !segment.is_empty() {
            Arc::make_mut(&mut history)
                .append(&segment, rules, max_headers)
                .map_err(|error| ValidationError::Rejected(error.to_string()))?;
        }
        if let Some(previous) = previous {
            // Earlier unresolved cutoffs are no greater than the most recent
            // one. Checked linkage makes resolved selections immutable.
            let (cutoff, hash) = previous.l1_selections[1];
            if history
                .select(cutoff)
                .map_err(|error| ValidationError::Rejected(error.to_string()))?
                .hash()
                != hash
            {
                return Err(ValidationError::Rejected(
                    "L1 evidence contradicts earlier execution".into(),
                ));
            }
        }
        let schedule = self
            .schedule()
            .map_err(|error| ValidationError::Rejected(error.to_string()))?;
        let timing = schedule
            .history()
            .map_err(|error| ValidationError::Rejected(error.to_string()))?;
        let parent_timestamp = block
            .header
            .timestamp
            .checked_sub(schedule.config().block_interval_seconds)
            .ok_or_else(|| {
                ValidationError::Rejected("execution parent timestamp underflow".into())
            })?;
        let mut selections = [(0, B256::ZERO); 2];
        for (slot, timestamp) in selections
            .iter_mut()
            .zip([parent_timestamp, block.header.timestamp])
        {
            let cutoff = timing
                .source_cutoff(timestamp)
                .map_err(|error| ValidationError::Rejected(error.to_string()))?;
            *slot = (
                cutoff,
                history
                    .select(cutoff)
                    .map_err(|error| ValidationError::Rejected(error.to_string()))?
                    .hash(),
            );
        }
        let validated =
            self.validate_cold_window(vec![admitted], cancellation, Arc::clone(&history))?;
        if validated.settling_block.replay_input.pre_state_root != parent.state_root {
            return Err(ValidationError::Rejected(
                "stream execution state is discontinuous".into(),
            ));
        }
        Ok((
            ValidatedStreamBlock {
                header: block.header,
                hash,
                settling: validated.settling_block,
                l1_segment: segment,
                l1_checkpoint_len: history.headers().len(),
                l1_selections: selections,
            },
            history,
        ))
    }
}

impl ValidatedWindow {
    /// Reassemble immutable, fully checked facts. Run in the bounded blocking
    /// worker: terminal observation witnesses and intermediate RLP can be large.
    pub(crate) fn from_streamed_prefix(
        anchor: BlockAnchor,
        blocks: &[Arc<ValidatedStreamBlock>],
    ) -> Result<Self, ValidationError> {
        let (terminal, preceding) = blocks
            .split_last()
            .ok_or_else(|| ValidationError::Rejected("empty streamed range".into()))?;
        let mut parent = anchor;
        for block in blocks {
            if parent.number.checked_add(1) != Some(block.number())
                || block.parent_hash() != parent.hash
                || block.settling.replay_input.pre_state_root != parent.state_root
            {
                return Err(ValidationError::InternalInvariant(
                    "retained stream is discontinuous".into(),
                ));
            }
            parent = block.as_anchor();
        }
        Ok(Self {
            window_pre_block_hash: anchor.hash,
            settling_pre_block_hash: terminal.parent_hash(),
            window_post_block_hash: terminal.hash(),
            preceding_blocks: preceding
                .iter()
                .map(|block| block.settling.block.clone())
                .collect(),
            settling_block: terminal.settling.clone(),
        })
    }
}
