//! Authenticated reconstruction facts, created only after Stateless succeeds.
//!
//! A fact says that this exact block can be reproduced by the shared follower
//! assembler. It confers no intermediate/terminal or settlement authorization.

use alloy_consensus::Header;
use alloy_primitives::{Address, B256, Bytes};
use eez_driver::derivation::{reassemble_full_from_replay, require_exact_block};
use reth_primitives_traits::SealedHeader;

use super::{Block, EezEvmConfig, StatelessValidationOutput};

/// No caller-supplied environment, claimed output root, or profile-only decoder
/// can construct this fact. Retained independently of the settlement evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconstructedBlock {
    block_hash: B256,
    beneficiary: Address,
    extra_data: Bytes,
}

impl ReconstructedBlock {
    /// Explicitly synthetic reconstruction evidence for settlement/lifecycle unit
    /// tests. Process and captured-witness tests construct facts by real replay.
    #[cfg(test)]
    pub(crate) fn for_test(block_hash: B256, header: &Header) -> Self {
        Self {
            block_hash,
            beneficiary: header.beneficiary,
            extra_data: header.extra_data.clone(),
        }
    }

    pub(crate) const fn block_hash(&self) -> B256 {
        self.block_hash
    }

    /// Only these two execution inputs are published by V0. The omitted fields
    /// are fixed by the profile, parent and independently authenticated L1 seed.
    pub(crate) fn matches_metadata(&self, beneficiary: Address, extra_data: &[u8]) -> bool {
        self.beneficiary == beneficiary && self.extra_data.as_ref() == extra_data
    }
}

/// Called with the exact block and ancestor consumed by SUCCESSFUL Stateless
/// replay and its still-live outputs. Failure means this block is ineligible for
/// the supported operations profile and the DA gate must reject it.
pub(super) fn reconstruct(
    config: &EezEvmConfig,
    block: &Block,
    original_rlp: &[u8],
    parent_rlp: Option<&[u8]>,
    replay: &StatelessValidationOutput<eez_primitives::Receipt>,
) -> eyre::Result<ReconstructedBlock> {
    let parent_rlp = parent_rlp.ok_or_else(|| eyre::eyre!("missing reconstruction parent"))?;
    let header: Header = alloy_rlp::decode_exact(parent_rlp)?;
    eyre::ensure!(
        alloy_rlp::encode(&header) == parent_rlp,
        "noncanonical reconstruction parent"
    );
    let parent = SealedHeader::seal_slow(header);
    eyre::ensure!(
        parent.hash() == block.header.parent_hash,
        "reconstruction parent hash mismatch"
    );
    eyre::ensure!(
        parent.state_root == replay.pre_state_root,
        "reconstruction parent state mismatch"
    );
    eyre::ensure!(
        block.header.hash_slow() == replay.block_hash,
        "reconstruction block hash mismatch"
    );
    let rebuilt = reassemble_full_from_replay(
        config,
        &parent,
        block,
        &replay.execution_output,
        replay.post_state_root,
    )?;
    require_exact_block(&rebuilt, original_rlp)?;
    Ok(ReconstructedBlock {
        block_hash: replay.block_hash,
        beneficiary: block.header.beneficiary,
        extra_data: block.header.extra_data.clone(),
    })
}
