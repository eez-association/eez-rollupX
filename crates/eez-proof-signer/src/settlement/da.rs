//! Decoding and exact verification of the batch data-availability payload.

use alloy_consensus::Transaction as _;
use alloy_primitives::Bytes;
use reth_primitives_traits::BlockBody as _;
use thiserror::Error;

use super::{
    AuthorizedInboundEffects, AuthorizedOutboundEffects, EezBlock, SystemTransactionReconstructor,
};
use super::{inbound::AuthorizedInboundEffect, outbound::AuthorizedOutboundEffect};
use crate::validate::ValidatedWindow;

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum DaPayloadError {
    #[error("block {block_number} has no authenticated reconstruction evidence")]
    MissingReconstruction { block_number: u64 },
    #[error("published environment differs from reconstructed block {block_number}")]
    EnvironmentMismatch { block_number: u64 },
    #[error("batch callData does not decode: {reason}")]
    Decode { reason: String },
    #[error("batch callData covers {actual} blocks; validated window has {expected}")]
    BlockCount { expected: usize, actual: usize },
    #[error("validated block {block_number} RLP does not decode exactly: {reason}")]
    InvalidBlockRlp { block_number: u64, reason: String },
    #[error(
        "batch callData claims {submitted} retained transactions for block {block_number}; expected projection has {expected}"
    )]
    ProjectedTransactionCount {
        block_number: u64,
        submitted: usize,
        expected: usize,
    },
    #[error(
        "batch callData transaction {transaction_index} for block {block_number} does not match the validated transaction bytes"
    )]
    TransactionMismatch {
        block_number: u64,
        transaction_index: usize,
    },
    #[error("canonical Sync-block reconstruction failed: {reason}")]
    SystemTransactionReconstruction { reason: String },
    #[error(
        "canonical Sync-block transaction {transaction_index} does not match the validated transaction bytes"
    )]
    SyncBlockTransactionMismatch { transaction_index: usize },
    #[error(
        "canonical Sync block has {actual} transactions; reconstructed effect sequence has {expected}"
    )]
    SyncBlockTransactionCount { expected: usize, actual: usize },
    #[error("validated effect positions are inconsistent with the settling block transactions")]
    InvalidEffectTransactionLayout,
}

fn invalid_da_payload(reason: impl Into<String>) -> DaPayloadError {
    DaPayloadError::Decode {
        reason: reason.into(),
    }
}

/// Bind every V0 position to replay and full canonical assembly evidence.
/// Dynamic fields omitted from DA are checked against authenticated context by
/// the candidate gate. The terminal action suffix is reconstructed byte for byte.
pub(crate) fn verify_da_payload(
    encoded_payload: &[u8],
    window: &ValidatedWindow,
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    reconstructor: &SystemTransactionReconstructor,
) -> Result<(), DaPayloadError> {
    use alloy_eips::Encodable2718 as _;
    let payload = eez_payload_codec::v0::Payload::decode(encoded_payload)
        .map_err(|error| invalid_da_payload(error.to_string()))?;
    let terminal = window.settling_block().block();
    let expected_blocks = window.preceding_blocks().len() + 1;
    if payload.block_count() as usize != expected_blocks {
        return Err(DaPayloadError::BlockCount {
            expected: expected_blocks,
            actual: payload.block_count() as usize,
        });
    }
    for (position, (plan, validated)) in payload
        .blocks()
        .zip(
            window
                .preceding_blocks()
                .iter()
                .chain(std::iter::once(terminal)),
        )
        .enumerate()
    {
        let number = validated.number();
        let fact = validated
            .reconstruction()
            .ok_or(DaPayloadError::MissingReconstruction {
                block_number: number,
            })?;
        if !fact.matches_metadata(plan.beneficiary, plan.extra_data) {
            return Err(DaPayloadError::EnvironmentMismatch {
                block_number: number,
            });
        }
        let block = decode_validated_block(number, validated.rlp())?;
        let pure = if position + 1 == expected_blocks {
            eez_driver::derivation::ordinary_prefix(&block)
        } else {
            &block.body.transactions
        };
        if plan.transactions.len() != pure.len() {
            return Err(DaPayloadError::ProjectedTransactionCount {
                block_number: number,
                submitted: plan.transactions.len(),
                expected: pure.len(),
            });
        }
        for (transaction_index, (published, actual)) in plan.transactions.zip(pure).enumerate() {
            if published != actual.encoded_2718() {
                return Err(DaPayloadError::TransactionMismatch {
                    block_number: number,
                    transaction_index,
                });
            }
        }
    }
    if outbound_effects.iter().next().is_some() || inbound_effects.len() != 0 {
        let block = decode_validated_block(terminal.number(), terminal.rlp())?;
        let transactions = block
            .body
            .encoded_2718_transactions_iter()
            .map(Bytes::from)
            .collect::<Vec<_>>();
        verify_reconstructed_sync_block(
            &block,
            &transactions,
            outbound_effects,
            inbound_effects,
            reconstructor,
        )?;
    }
    Ok(())
}

fn decode_validated_block(block_number: u64, block_rlp: &[u8]) -> Result<EezBlock, DaPayloadError> {
    alloy_rlp::decode_exact(block_rlp).map_err(|error| DaPayloadError::InvalidBlockRlp {
        block_number,
        reason: error.to_string(),
    })
}

fn verify_reconstructed_sync_block(
    block: &EezBlock,
    validated_transactions: &[Bytes],
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    reconstructor: &SystemTransactionReconstructor,
) -> Result<(), DaPayloadError> {
    let first_system_position = outbound_effects
        .iter()
        .next()
        .map(AuthorizedOutboundEffect::load_transaction_index)
        .or_else(|| {
            inbound_effects
                .iter()
                .next()
                .map(AuthorizedInboundEffect::transaction_index)
        })
        .ok_or(DaPayloadError::InvalidEffectTransactionLayout)?;
    let starting_nonce = block
        .body
        .transactions
        .get(first_system_position)
        .ok_or(DaPayloadError::InvalidEffectTransactionLayout)?
        .nonce();
    let outbound_reconstruction_inputs = outbound_effects
        .groups()
        .map(|bindings| {
            let first = &bindings[0];
            let user_transaction = validated_transactions
                .get(first.transaction_index())
                .ok_or(DaPayloadError::InvalidEffectTransactionLayout)?;
            Ok(eez_protocol::system_tx::OutboundSyncGroup {
                effects: bindings
                    .iter()
                    .map(|binding| eez_protocol::system_tx::OutboundSyncEffect {
                        source_entry: binding.derived_source_entry().clone(),
                        callback_entries: binding.derived_callback_entries().to_vec(),
                    })
                    .collect(),
                user_tx: user_transaction.clone(),
            })
        })
        .collect::<Result<Vec<_>, DaPayloadError>>()?;
    let inbound_reconstruction_entries = inbound_effects
        .iter()
        .map(|binding| binding.observation().derived_da_entry.as_sidecar().clone())
        .collect::<Vec<_>>();
    let pairs = reconstructor
        .reconstruct_sync_pairs(
            &outbound_reconstruction_inputs,
            &inbound_reconstruction_entries,
            starting_nonce,
        )
        .map_err(|reason| DaPayloadError::SystemTransactionReconstruction { reason })?;
    let reconstructed = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);

    // The V0 ordinary prefix was compared separately. No transaction may follow
    // the reconstructed action suffix or be smuggled between its whole groups.
    let suffix = validated_transactions
        .get(first_system_position..)
        .ok_or(DaPayloadError::InvalidEffectTransactionLayout)?;
    if reconstructed.len() != suffix.len() {
        return Err(DaPayloadError::SyncBlockTransactionCount {
            expected: reconstructed.len(),
            actual: suffix.len(),
        });
    }
    for (transaction_index, (validated_transaction, reconstructed_transaction)) in
        suffix.iter().zip(reconstructed).enumerate()
    {
        if validated_transaction.as_ref() != reconstructed_transaction.as_ref() {
            return Err(DaPayloadError::SyncBlockTransactionMismatch {
                transaction_index: first_system_position + transaction_index,
            });
        }
    }
    Ok(())
}
