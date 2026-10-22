//! Retained execution facts for v2. Construction uses the same consuming backend
//! checks as v1; neither a stream cursor nor these facts authorize settlement.

use std::sync::Arc;

use alloy_consensus::Header;
use alloy_primitives::B256;

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
        cancellation: &CancellationToken,
    ) -> Result<ValidatedStreamBlock, ValidationError> {
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
        let validated = self.validate_cold_window(vec![admitted], cancellation)?;
        if validated.settling_block.replay_input.pre_state_root != parent.state_root {
            return Err(ValidationError::Rejected(
                "stream execution state is discontinuous".into(),
            ));
        }
        Ok(ValidatedStreamBlock {
            header: block.header,
            hash,
            settling: validated.settling_block,
        })
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
