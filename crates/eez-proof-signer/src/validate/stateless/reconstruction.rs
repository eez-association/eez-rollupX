//! Authenticated reconstruction facts, created only after Stateless succeeds.
//!
//! A fact says that this exact block can be reproduced by the shared follower
//! assembler. It confers no intermediate/terminal or settlement authorization.

use alloy_consensus::Header;
use alloy_primitives::B256;
use eez_driver::derivation::{
    ordinary_environment, reassemble_full_from_replay, require_exact_block,
};
use eez_payload_codec::derivable::Environment;
use reth_primitives_traits::SealedHeader;

use super::{Block, EthEvmConfig, StatelessValidationOutput};

/// No caller-supplied environment, claimed output root, or profile-only decoder
/// can construct this fact. Retained independently of the settlement evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconstructedBlock {
    block_hash: B256,
    environment: Option<Environment>,
    has_beacon_root: bool,
}

impl ReconstructedBlock {
    pub(crate) const fn block_hash(&self) -> B256 {
        self.block_hash
    }

    /// Before Cancun the published root is unused; all other input bytes must
    /// agree. Whole-block assembly already established every omitted output.
    pub(crate) fn matches_environment(&self, published: &Environment) -> bool {
        let Some(actual) = &self.environment else {
            return false;
        };
        let mut effective = published.clone();
        if !self.has_beacon_root {
            effective.cancun_parent_beacon_root = B256::ZERO;
        }
        effective == *actual
    }
}

/// Called with the exact block and ancestor consumed by SUCCESSFUL Stateless
/// replay and its still-live outputs. Failure means this block is ineligible for
/// the supported operations profile and the DA gate must reject it.
pub(super) fn reconstruct(
    config: &EthEvmConfig,
    block: &Block,
    original_rlp: &[u8],
    parent_rlp: Option<&[u8]>,
    replay: &StatelessValidationOutput,
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
    let has_beacon_root = block.header.parent_beacon_block_root.is_some();
    let environment = ordinary_environment(config, &parent, block)?;
    Ok(ReconstructedBlock {
        block_hash: replay.block_hash,
        environment,
        has_beacon_root,
    })
}
