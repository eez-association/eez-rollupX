//! Profile-1 execution inputs and canonical block reconstruction.
//!
//! This adapter does not validate execution or authorize settlement. The caller
//! must authenticate the parent, transactions and execution outputs, execute
//! every block (including empty ones), and retain all ordinary/terminal gates.
//! In particular, never supply a claimed header's state root as replay output.
//!
//! The follower can use [`next_attributes`] before execution. The signer can use
//! [`reassemble`] AFTER successful replay and require exact original block RLP
//! equality. Sharing Reth's assembler avoids duplicating its header algorithm;
//! it does not make the caller's inputs trusted.

use alloy_consensus::Header;
use alloy_primitives::B256;
use eez_payload_codec::derivable::Environment;
use eyre::{ensure, eyre};
use reth_chainspec::EthereumHardforks as _;
use reth_ethereum_primitives::{Block, Receipt, TransactionSigned};
use reth_evm::{
    ConfigureEvm, NextBlockEnvAttributes,
    execute::{BlockAssemblerInput, BlockExecutionOutput},
};
use reth_evm_ethereum::{EthEvmConfig, revm_spec_by_timestamp_and_block_number};
use reth_primitives_traits::SealedHeader;
use reth_storage_api::StateProvider;
use revm::primitives::hardfork::SpecId;

mod encoding;
mod no_state;
pub use encoding::encode_operations;

/// Extract supported ordinary inputs from a full block. This classifies its
/// representation only: callers still need authenticated execution and exact
/// assembly equality before authorizing a settlement.
pub fn ordinary_environment(
    config: &EthEvmConfig,
    parent: &SealedHeader,
    block: &Block,
) -> eyre::Result<Option<Environment>> {
    let attributes = full_attributes(config, parent, block)?;
    Ok(attributes
        .withdrawals
        .as_ref()
        .is_none_or(|w| w.is_empty())
        .then(|| Environment {
            timestamp_step_seconds: block.header.timestamp - parent.timestamp,
            fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            gas_limit: attributes.gas_limit,
            extra_data: attributes.extra_data,
            cancun_parent_beacon_root: attributes.parent_beacon_block_root.unwrap_or_default(),
        }))
}

/// Translate published inputs using the actual parent and trusted chain rules.
/// No operator defaults or claimed child header values participate.
///
/// This is a supported-profile check, not parent authentication or consensus
/// validation. In particular, gas-limit changes still need consensus validation.
pub fn next_attributes(
    config: &EthEvmConfig,
    parent: &Header,
    environment: &Environment,
) -> eyre::Result<NextBlockEnvAttributes> {
    environment.validate()?;
    let number = parent
        .number
        .checked_add(1)
        .ok_or_else(|| eyre!("derived block number overflow"))?;
    let timestamp = parent
        .timestamp
        .checked_add(environment.timestamp_step_seconds)
        .ok_or_else(|| eyre!("derived block timestamp overflow"))?;
    let chain_spec = config.chain_spec();
    let spec = revm_spec_by_timestamp_and_block_number(chain_spec.as_ref(), timestamp, number);
    // An explicit allowlist deliberately rejects new execution forks until their
    // assembly and replay behavior has been audited for this profile.
    ensure!(
        matches!(
            spec,
            SpecId::MERGE | SpecId::SHANGHAI | SpecId::CANCUN | SpecId::PRAGUE | SpecId::OSAKA
        ),
        "unsupported profile-1 execution fork {spec:?}"
    );
    Ok(NextBlockEnvAttributes {
        timestamp,
        suggested_fee_recipient: environment.fee_recipient,
        prev_randao: environment.prev_randao,
        gas_limit: environment.gas_limit,
        parent_beacon_block_root: chain_spec
            .is_cancun_active_at_timestamp(timestamp)
            .then_some(environment.cancun_parent_beacon_root),
        withdrawals: chain_spec
            .is_shanghai_active_at_timestamp(timestamp)
            .then(Default::default),
        extra_data: environment.extra_data.clone(),
        slot_number: None,
    })
}

/// Reconstruct the entire ordinary block from parent, recipe and replay output.
///
/// The returned block carries no validation authority. The caller must compare
/// its exact canonical RLP with the authenticated original, and reject any
/// mismatch. That comparison includes every header field and body component;
/// unsupported ommers/withdrawals/optional fields cannot be silently dropped.
///
/// `execution_output` and `post_state_root` must come from successful execution
/// of these exact transactions with this environment and authenticated pre-state.
/// The pinned Ethereum assembler does not read `state_provider`, but the actual
/// provider is passed through rather than substituting an empty database.
pub fn reassemble(
    config: &EthEvmConfig,
    parent: &SealedHeader,
    environment: &Environment,
    transactions: Vec<TransactionSigned>,
    execution_output: &BlockExecutionOutput<Receipt>,
    post_state_root: B256,
    state_provider: &dyn StateProvider,
) -> eyre::Result<Block> {
    let attributes = next_attributes(config, parent, environment)?;
    assemble(
        config,
        parent,
        attributes,
        transactions,
        execution_output,
        post_state_root,
        state_provider,
    )
}

/// Full-record execution inputs supported by BOTH the follower builder and the
/// replay assembler. Full records do not bypass the supported-fork boundary.
pub fn full_attributes(
    config: &EthEvmConfig,
    parent: &SealedHeader,
    block: &Block,
) -> eyre::Result<NextBlockEnvAttributes> {
    let header = &block.header;
    ensure!(
        header.parent_hash == parent.hash(),
        "full block does not extend parent"
    );
    ensure!(
        parent.number.checked_add(1) == Some(header.number),
        "full block number does not extend parent"
    );
    ensure!(
        block.body.ommers.is_empty(),
        "full block ommers are unsupported"
    );
    ensure!(
        header.slot_number.is_none() && header.block_access_list_hash.is_none(),
        "full block slot/access-list fields are unsupported"
    );
    let step = header
        .timestamp
        .checked_sub(parent.timestamp)
        .filter(|step| *step > 0)
        .ok_or_else(|| eyre!("full block timestamp does not advance"))?;
    let env = Environment {
        timestamp_step_seconds: step,
        fee_recipient: header.beneficiary,
        prev_randao: header.mix_hash,
        gas_limit: header.gas_limit,
        extra_data: header.extra_data.clone(),
        cancun_parent_beacon_root: header.parent_beacon_block_root.unwrap_or_default(),
    };
    let mut attributes = next_attributes(config, parent, &env)?;
    ensure!(
        attributes.parent_beacon_block_root == header.parent_beacon_block_root,
        "full block beacon root presence differs from fork rules"
    );
    ensure!(
        attributes.withdrawals.is_some() == block.body.withdrawals.is_some(),
        "full block withdrawals presence differs from fork rules"
    );
    // Full records retain actual withdrawals; the derived profile uses empties.
    attributes.withdrawals = block.body.withdrawals.clone();
    Ok(attributes)
}

/// Reassemble a validated full block using only its execution INPUTS and actual
/// replay output. The caller must require exact equality with its original RLP.
/// No live provider is required: the pinned assembler is a pure output builder.
/// A dependency change which tries to access state fails explicitly.
pub fn reassemble_full_from_replay(
    config: &EthEvmConfig,
    parent: &SealedHeader,
    block: &Block,
    execution_output: &BlockExecutionOutput<Receipt>,
    post_state_root: B256,
) -> eyre::Result<Block> {
    let attributes = full_attributes(config, parent, block)?;
    let provider = no_state::NoStateReads::default();
    let assembled = assemble(
        config,
        parent,
        attributes,
        block.body.transactions.clone(),
        execution_output,
        post_state_root,
        &provider,
    );
    provider.ensure_unused()?;
    assembled
}

fn assemble(
    config: &EthEvmConfig,
    parent: &SealedHeader,
    attributes: NextBlockEnvAttributes,
    transactions: Vec<TransactionSigned>,
    execution_output: &BlockExecutionOutput<Receipt>,
    post_state_root: B256,
    state_provider: &dyn StateProvider,
) -> eyre::Result<Block> {
    ensure!(
        parent.hash() == parent.header().hash_slow(),
        "reassembly parent seal does not match header"
    );
    ensure!(
        execution_output.result.receipts.len() == transactions.len(),
        "reassembly receipt count does not match transaction count"
    );
    let evm_env = config.next_evm_env(parent, &attributes)?;
    let execution_ctx = config.context_for_next_block(parent, attributes)?;
    Ok(config.block_assembler().assemble_block(
        BlockAssemblerInput::<<EthEvmConfig as ConfigureEvm>::BlockExecutorFactory>::new(
            evm_env,
            execution_ctx,
            parent,
            transactions,
            &execution_output.result,
            &execution_output.state,
            state_provider,
            post_state_root,
            None, // Profile 1 excludes Amsterdam's block-access-list field.
        ),
        None,
        None,
        None,
    )?)
}

/// Require exact canonical whole-block equality. This checks representation,
/// not execution/authentication; it must follow successful independent replay.
pub fn require_exact_block(reconstructed: &Block, original_rlp: &[u8]) -> eyre::Result<()> {
    ensure!(
        alloy_rlp::encode(reconstructed) == original_rlp,
        "derived ordinary block differs from authenticated full block"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
