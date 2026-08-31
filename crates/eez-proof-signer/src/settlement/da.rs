//! Decoding and exact verification of the batch data-availability payload.

use alloy_consensus::Transaction as _;
use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Header};
use alloy_sol_types::SolValue as _;
use reth_primitives_traits::BlockBody as _;
use thiserror::Error;

use super::{
    AuthorizedInboundEffects, AuthorizedOutboundEffects, EthereumBlock,
    SystemTransactionReconstructor,
};
use super::{inbound::AuthorizedInboundEffect, outbound::AuthorizedOutboundEffect};
use crate::validate::ValidatedWindow;

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum DaPayloadError {
    #[error("batch callData does not decode: {reason}")]
    Decode { reason: String },
    #[error("batch callData has {trailing} trailing bytes")]
    TrailingBytes { trailing: usize },
    #[error("batch callData covers {actual} blocks; validated window has {expected}")]
    BlockCount { expected: usize, actual: usize },
    #[error("batch callData contains unexpected {field} items after the expected {expected}")]
    UnexpectedItems {
        field: &'static str,
        expected: usize,
    },
    #[error("validated block {block_number} RLP does not decode exactly: {reason}")]
    InvalidBlockRlp { block_number: u64, reason: String },
    #[error("self-contained block {block_number} does not exactly match the validated block RLP")]
    BlockRlpMismatch { block_number: u64 },
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
    #[error(
        "batch callData is missing L2 entry {entry_index} for effect transaction {transaction_index}"
    )]
    MissingL2Entry {
        entry_index: usize,
        transaction_index: usize,
    },
    #[error(
        "batch callData L2 entry {entry_index} does not match effect transaction {transaction_index}"
    )]
    L2EntryMismatch {
        entry_index: usize,
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
    #[error("batch callData outbound group sizes do not match the authorized Sync layout")]
    OutboundGroupSizes,
}

const CALL_DATA_TAG: u8 = 0;
const GROUPED_CALL_DATA_TAG: u8 = 1;
const SELF_CONTAINED_BLOCKS_TAG: u8 = 2;

/// Borrowed RLP-list bodies that advance as their items are verified.
///
/// Keeping slices into the original calldata avoids copying transactions and
/// effect sidecars supplied by an untrusted composer.
struct DaPayloadCursor<'a> {
    block_tx_counts: &'a [u8],
    transactions: &'a [u8],
    l2_entries: &'a [u8],
    outbound_group_sizes: Option<&'a [u8]>,
}

impl<'a> DaPayloadCursor<'a> {
    fn decode(encoded_payload: &'a [u8]) -> Result<Self, DaPayloadError> {
        let Some((&tag, encoded_body)) = encoded_payload.split_first() else {
            return Err(invalid_da_payload("payload is empty"));
        };
        if tag != CALL_DATA_TAG && tag != GROUPED_CALL_DATA_TAG {
            return Err(invalid_da_payload(format!(
                "unsupported tag byte 0x{tag:02x}; expected 0x{CALL_DATA_TAG:02x} or 0x{GROUPED_CALL_DATA_TAG:02x}"
            )));
        }

        let mut remainder = encoded_body;
        let mut body = decode_list(&mut remainder)?;
        if !remainder.is_empty() {
            return Err(DaPayloadError::TrailingBytes {
                trailing: remainder.len(),
            });
        }

        let block_tx_counts = decode_list(&mut body)?;
        let transactions = decode_list(&mut body)?;
        let l2_entries = decode_list(&mut body)?;
        let outbound_group_sizes = if tag == GROUPED_CALL_DATA_TAG {
            Some(decode_list(&mut body)?)
        } else {
            None
        };
        if !body.is_empty() {
            return Err(invalid_da_payload("payload body has unexpected fields"));
        }
        Ok(Self {
            block_tx_counts,
            transactions,
            l2_entries,
            outbound_group_sizes,
        })
    }
}

fn decode_list<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], DaPayloadError> {
    Header::decode_bytes(input, true).map_err(|error| invalid_da_payload(error.to_string()))
}

fn invalid_da_payload(reason: impl Into<String>) -> DaPayloadError {
    DaPayloadError::Decode {
        reason: reason.into(),
    }
}

/// Bind the decoded batch's submitted DA payload to the validated window and
/// authorized effects.
///
/// The preceding blocks carry every transaction verbatim. In the terminal
/// settling block, system loads and inbound deliveries are omitted while
/// outbound user transactions remain. Each effect has one exact, ordered
/// derivation sidecar, and the resulting mixed Sync sequence is reconstructed
/// byte for byte.
pub(crate) fn verify_da_payload(
    encoded_payload: &[u8],
    validated_window: &ValidatedWindow,
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    system_transaction_reconstructor: &SystemTransactionReconstructor,
) -> Result<(), DaPayloadError> {
    let settling = validated_window.settling_block().block();
    verify_encoded_da_payload(
        encoded_payload,
        validated_window
            .preceding_blocks()
            .iter()
            .map(|block| (block.number(), block.rlp())),
        (settling.number(), settling.rlp()),
        outbound_effects,
        inbound_effects,
        system_transaction_reconstructor,
    )
}

/// Raw-input adapter for focused DA unit tests.
#[cfg(test)]
pub(crate) fn verify_da_payload_for_test<'a, I>(
    encoded_payload: &[u8],
    intermediate_blocks: I,
    settling_block: (u64, &'a [u8]),
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    system_transaction_reconstructor: &SystemTransactionReconstructor,
) -> Result<(), DaPayloadError>
where
    I: ExactSizeIterator<Item = (u64, &'a [u8])>,
{
    verify_encoded_da_payload(
        encoded_payload,
        intermediate_blocks,
        settling_block,
        outbound_effects,
        inbound_effects,
        system_transaction_reconstructor,
    )
}

fn verify_encoded_da_payload<'a, I>(
    encoded_payload: &[u8],
    intermediate_blocks: I,
    settling_block: (u64, &'a [u8]),
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    system_transaction_reconstructor: &SystemTransactionReconstructor,
) -> Result<(), DaPayloadError>
where
    I: ExactSizeIterator<Item = (u64, &'a [u8])>,
{
    if encoded_payload.first() == Some(&SELF_CONTAINED_BLOCKS_TAG) {
        return verify_self_contained_da_payload(
            encoded_payload,
            intermediate_blocks,
            settling_block,
            outbound_effects,
            inbound_effects,
            system_transaction_reconstructor,
        );
    }
    let mut payload_cursor = DaPayloadCursor::decode(encoded_payload)?;
    let expected_blocks = intermediate_blocks.len() + 1;
    let omitted_terminal_count = outbound_effects
        .groups()
        .try_fold(0usize, |count, group| {
            let callbacks = group
                .iter()
                .map(|effect| effect.callback_transaction_indices().len())
                .sum::<usize>();
            count.checked_add(1 + callbacks)
        })
        .and_then(|count| count.checked_add(inbound_effects.len()))
        .ok_or(DaPayloadError::InvalidEffectTransactionLayout)?;
    let preceding_transaction_count = verify_preceding_block_transactions(
        &mut payload_cursor,
        intermediate_blocks,
        expected_blocks,
    )?;
    let settling = verify_settling_block_transactions(
        &mut payload_cursor,
        settling_block,
        expected_blocks,
        outbound_effects,
        inbound_effects,
        omitted_terminal_count,
    )?;
    // Cannot overflow: every counted transaction occupies at least one byte of
    // memory-resident block RLP; the sum only feeds an error diagnostic.
    let retained_transactions =
        preceding_transaction_count.saturating_add(settling.retained_transaction_count);

    if !payload_cursor.block_tx_counts.is_empty() {
        return Err(DaPayloadError::UnexpectedItems {
            field: "blockTxCounts",
            expected: expected_blocks,
        });
    }
    if !payload_cursor.transactions.is_empty() {
        return Err(DaPayloadError::UnexpectedItems {
            field: "transactions",
            expected: retained_transactions,
        });
    }

    verify_outbound_group_sizes(&mut payload_cursor, outbound_effects)?;
    verify_effect_sidecars(&mut payload_cursor, outbound_effects, inbound_effects)?;
    if settling.requires_sync_reconstruction {
        verify_reconstructed_sync_block(
            &settling.block,
            &settling.encoded_transactions,
            outbound_effects,
            inbound_effects,
            system_transaction_reconstructor,
        )?;
    }
    Ok(())
}

/// Verify the self-contained profile byte-for-byte against the exact blocks
/// accepted by Stateless. Unlike the legacy projection, complete terminal
/// system transactions are present, so no transaction omission is inferred.
fn verify_self_contained_da_payload<'a, I>(
    encoded_payload: &[u8],
    intermediate_blocks: I,
    settling_block: (u64, &'a [u8]),
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    system_transaction_reconstructor: &SystemTransactionReconstructor,
) -> Result<(), DaPayloadError>
where
    I: ExactSizeIterator<Item = (u64, &'a [u8])>,
{
    let decoded = eez_payload_codec::decode(encoded_payload)
        .map_err(|error| invalid_da_payload(error.to_string()))?;
    let expected_blocks = intermediate_blocks.len() + 1;
    if decoded.block_rlps.len() != expected_blocks {
        return Err(DaPayloadError::BlockCount {
            expected: expected_blocks,
            actual: decoded.block_rlps.len(),
        });
    }
    let expected_rlps = intermediate_blocks.chain(std::iter::once(settling_block));
    for ((block_number, validated_rlp), submitted_rlp) in expected_rlps.zip(&decoded.block_rlps) {
        if submitted_rlp.as_slice() != validated_rlp {
            return Err(DaPayloadError::BlockRlpMismatch { block_number });
        }
    }

    let expected_group_sizes = outbound_effects
        .groups()
        .map(|group| u16::try_from(group.len()).map_err(|_| DaPayloadError::OutboundGroupSizes))
        .collect::<Result<Vec<_>, _>>()?;
    if decoded.outbound_group_sizes != expected_group_sizes {
        return Err(DaPayloadError::OutboundGroupSizes);
    }

    let expected_sidecars = outbound_effects
        .iter()
        .flat_map(|outbound| {
            let source_encoding = match outbound.derived_source_entry() {
                eez_protocol::system_tx::OutboundSourceEntry::Mutable(entry) => entry.abi_encode(),
                eez_protocol::system_tx::OutboundSourceEntry::Static(entry) => entry.abi_encode(),
            };
            std::iter::once((outbound.transaction_index(), source_encoding)).chain(
                outbound
                    .callback_transaction_indices()
                    .iter()
                    .copied()
                    .zip(outbound.derived_callback_entries())
                    .map(|(transaction_index, entry)| (transaction_index, entry.abi_encode())),
            )
        })
        .chain(inbound_effects.iter().map(|inbound| {
            (
                inbound.transaction_index(),
                inbound.observation().derived_da_entry.encoded(),
            )
        }))
        .collect::<Vec<_>>();
    for (entry_index, (transaction_index, expected)) in expected_sidecars.iter().enumerate() {
        let Some(actual) = decoded.l2_entries.get(entry_index) else {
            return Err(DaPayloadError::MissingL2Entry {
                entry_index,
                transaction_index: *transaction_index,
            });
        };
        if actual != expected {
            return Err(DaPayloadError::L2EntryMismatch {
                entry_index,
                transaction_index: *transaction_index,
            });
        }
    }
    if decoded.l2_entries.len() != expected_sidecars.len() {
        return Err(DaPayloadError::UnexpectedItems {
            field: "l2Entries",
            expected: expected_sidecars.len(),
        });
    }

    if outbound_effects.iter().next().is_some() || inbound_effects.len() != 0 {
        let (settling_number, settling_rlp) = settling_block;
        let block = decode_validated_block(settling_number, settling_rlp)?;
        let encoded_transactions = block
            .body
            .encoded_2718_transactions_iter()
            .map(Bytes::from)
            .collect::<Vec<_>>();
        verify_reconstructed_sync_block(
            &block,
            &encoded_transactions,
            outbound_effects,
            inbound_effects,
            system_transaction_reconstructor,
        )?;
    }
    Ok(())
}

/// Verify that every preceding block is reproduced transaction-for-transaction.
fn verify_preceding_block_transactions<'a, I>(
    payload_cursor: &mut DaPayloadCursor<'_>,
    preceding_blocks: I,
    expected_blocks: usize,
) -> Result<usize, DaPayloadError>
where
    I: Iterator<Item = (u64, &'a [u8])>,
{
    let mut retained_transaction_count = 0usize;
    for (block_index, (block_number, block_rlp)) in preceding_blocks.enumerate() {
        let block = decode_validated_block(block_number, block_rlp)?;
        let submitted_count = next_block_count(payload_cursor, expected_blocks, block_index)?;
        let validated_count = block.body.transactions.len();
        if submitted_count != validated_count {
            return Err(DaPayloadError::ProjectedTransactionCount {
                block_number,
                submitted: submitted_count,
                expected: validated_count,
            });
        }
        retained_transaction_count = retained_transaction_count.saturating_add(validated_count);
        for (transaction_index, transaction) in
            block.body.encoded_2718_transactions_iter().enumerate()
        {
            expect_transaction(
                &mut payload_cursor.transactions,
                block_number,
                transaction_index,
                &transaction,
            )?;
        }
    }
    Ok(retained_transaction_count)
}

struct SettlingBlockDaVerification {
    block: EthereumBlock,
    encoded_transactions: Vec<Bytes>,
    retained_transaction_count: usize,
    requires_sync_reconstruction: bool,
}

/// Verify the settling-block transaction projection and retain exact bytes only
/// when the canonical Sync sequence must be reconstructed.
fn verify_settling_block_transactions(
    payload_cursor: &mut DaPayloadCursor<'_>,
    (block_number, block_rlp): (u64, &[u8]),
    expected_blocks: usize,
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    omitted_transaction_count: usize,
) -> Result<SettlingBlockDaVerification, DaPayloadError> {
    let block = decode_validated_block(block_number, block_rlp)?;
    let submitted_count = next_block_count(payload_cursor, expected_blocks, expected_blocks - 1)?;
    let validated_count = block.body.transactions.len();
    let retained_transaction_count = validated_count
        .checked_sub(omitted_transaction_count)
        .ok_or(DaPayloadError::InvalidEffectTransactionLayout)?;
    if submitted_count != retained_transaction_count {
        return Err(DaPayloadError::ProjectedTransactionCount {
            block_number,
            submitted: submitted_count,
            expected: retained_transaction_count,
        });
    }

    // With no authorized effects the settling block is ordinary: nothing was
    // omitted and there is no canonical Sync layout to reconstruct.
    let requires_sync_reconstruction = omitted_transaction_count != 0;
    let mut encoded_transactions = if requires_sync_reconstruction {
        Vec::with_capacity(validated_count)
    } else {
        Vec::new()
    };
    // Outbound-before-inbound ordering was established by authorization, so
    // chaining the two position lists preserves block order.
    let mut omitted_positions = outbound_effects
        .groups()
        .flat_map(|group| {
            std::iter::once(group[0].load_transaction_index()).chain(
                group
                    .iter()
                    .flat_map(|effect| effect.callback_transaction_indices().iter().copied()),
            )
        })
        .chain(
            inbound_effects
                .iter()
                .map(AuthorizedInboundEffect::transaction_index),
        )
        .peekable();
    for (transaction_index, transaction) in block
        .body
        .encoded_2718_transactions_iter()
        .map(Bytes::from)
        .enumerate()
    {
        if requires_sync_reconstruction {
            encoded_transactions.push(transaction.clone());
        }
        if omitted_positions
            .peek()
            .is_some_and(|&position| position == transaction_index)
        {
            omitted_positions.next();
            continue;
        }
        expect_transaction(
            &mut payload_cursor.transactions,
            block_number,
            transaction_index,
            transaction.as_ref(),
        )?;
    }
    if omitted_positions.next().is_some() {
        return Err(DaPayloadError::InvalidEffectTransactionLayout);
    }

    Ok(SettlingBlockDaVerification {
        block,
        encoded_transactions,
        retained_transaction_count,
        requires_sync_reconstruction,
    })
}

/// Verify one ordered canonical sidecar for every authorized effect.
fn verify_effect_sidecars(
    payload_cursor: &mut DaPayloadCursor<'_>,
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
) -> Result<(), DaPayloadError> {
    let expected_sidecar_count = outbound_effects
        .iter()
        .map(|effect| 1 + effect.callback_transaction_indices().len())
        .sum::<usize>()
        + inbound_effects.len();
    let expected_sidecars = outbound_effects
        .iter()
        .flat_map(|outbound| {
            let source_encoding = match outbound.derived_source_entry() {
                eez_protocol::system_tx::OutboundSourceEntry::Mutable(entry) => entry.abi_encode(),
                eez_protocol::system_tx::OutboundSourceEntry::Static(entry) => entry.abi_encode(),
            };
            std::iter::once((outbound.transaction_index(), source_encoding)).chain(
                outbound
                    .callback_transaction_indices()
                    .iter()
                    .copied()
                    .zip(outbound.derived_callback_entries())
                    .map(|(transaction_index, entry)| (transaction_index, entry.abi_encode())),
            )
        })
        .chain(inbound_effects.iter().map(|inbound| {
            (
                inbound.transaction_index(),
                inbound.observation().derived_da_entry.encoded(),
            )
        }));
    for (entry_index, (transaction_index, expected_encoding)) in expected_sidecars.enumerate() {
        verify_l2_entry(
            &mut payload_cursor.l2_entries,
            entry_index,
            transaction_index,
            &expected_encoding,
        )?;
    }
    if !payload_cursor.l2_entries.is_empty() {
        return Err(DaPayloadError::UnexpectedItems {
            field: "l2Entries",
            expected: expected_sidecar_count,
        });
    }
    Ok(())
}

fn verify_outbound_group_sizes(
    payload_cursor: &mut DaPayloadCursor<'_>,
    outbound_effects: &AuthorizedOutboundEffects,
) -> Result<(), DaPayloadError> {
    let expected = outbound_effects
        .groups()
        .map(<[AuthorizedOutboundEffect]>::len)
        .collect::<Vec<_>>();
    let Some(mut encoded) = payload_cursor.outbound_group_sizes.take() else {
        return if expected.iter().all(|&size| size == 1) {
            Ok(())
        } else {
            Err(DaPayloadError::OutboundGroupSizes)
        };
    };
    for expected_size in expected {
        if encoded.is_empty() {
            return Err(DaPayloadError::OutboundGroupSizes);
        }
        let actual =
            u16::decode(&mut encoded).map_err(|error| invalid_da_payload(error.to_string()))?;
        if usize::from(actual) != expected_size {
            return Err(DaPayloadError::OutboundGroupSizes);
        }
    }
    if !encoded.is_empty() {
        return Err(DaPayloadError::OutboundGroupSizes);
    }
    Ok(())
}

fn decode_validated_block(
    block_number: u64,
    block_rlp: &[u8],
) -> Result<EthereumBlock, DaPayloadError> {
    alloy_rlp::decode_exact(block_rlp).map_err(|error| DaPayloadError::InvalidBlockRlp {
        block_number,
        reason: error.to_string(),
    })
}

fn next_block_count(
    payload_cursor: &mut DaPayloadCursor<'_>,
    expected_blocks: usize,
    block_index: usize,
) -> Result<usize, DaPayloadError> {
    if payload_cursor.block_tx_counts.is_empty() {
        return Err(DaPayloadError::BlockCount {
            expected: expected_blocks,
            actual: block_index,
        });
    }
    u16::decode(&mut payload_cursor.block_tx_counts)
        .map(usize::from)
        .map_err(|error| invalid_da_payload(error.to_string()))
}

fn expect_transaction(
    transactions: &mut &[u8],
    block_number: u64,
    transaction_index: usize,
    expected: &[u8],
) -> Result<(), DaPayloadError> {
    if transactions.is_empty() {
        return Err(DaPayloadError::TransactionMismatch {
            block_number,
            transaction_index,
        });
    }
    let encoded_transaction = decode_list(transactions)?;
    if !encoded_bytes_match(encoded_transaction, expected)? {
        return Err(DaPayloadError::TransactionMismatch {
            block_number,
            transaction_index,
        });
    }
    Ok(())
}

fn verify_l2_entry(
    encoded_entries: &mut &[u8],
    entry_index: usize,
    transaction_index: usize,
    expected: &[u8],
) -> Result<(), DaPayloadError> {
    if encoded_entries.is_empty() {
        return Err(DaPayloadError::MissingL2Entry {
            entry_index,
            transaction_index,
        });
    }
    let encoded_entry = decode_list(encoded_entries)?;
    if !encoded_bytes_match(encoded_entry, expected)? {
        return Err(DaPayloadError::L2EntryMismatch {
            entry_index,
            transaction_index,
        });
    }
    Ok(())
}

fn verify_reconstructed_sync_block(
    block: &EthereumBlock,
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
        .map(|binding| binding.observation().derived_da_entry.as_entry().clone())
        .collect::<Vec<_>>();
    let pairs = reconstructor
        .reconstruct_sync_pairs(
            &outbound_reconstruction_inputs,
            &inbound_reconstruction_entries,
            starting_nonce,
        )
        .map_err(|reason| DaPayloadError::SystemTransactionReconstruction { reason })?;
    let reconstructed = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);

    // The canonical Sync layout contains exactly `2 * outbound + inbound`
    // transactions. Reject a different terminal-block length before comparing
    // transaction bytes.
    if reconstructed.len() != validated_transactions.len() {
        return Err(DaPayloadError::SyncBlockTransactionCount {
            expected: reconstructed.len(),
            actual: validated_transactions.len(),
        });
    }
    for (transaction_index, (validated_transaction, reconstructed_transaction)) in
        validated_transactions.iter().zip(reconstructed).enumerate()
    {
        if validated_transaction.as_ref() != reconstructed_transaction.as_ref() {
            return Err(DaPayloadError::SyncBlockTransactionMismatch { transaction_index });
        }
    }
    Ok(())
}

pub(super) fn encoded_bytes_match(
    mut encoded: &[u8],
    expected: &[u8],
) -> Result<bool, DaPayloadError> {
    // `Vec<u8>` is represented here as an RLP list of byte items. Decode it
    // incrementally so a malformed or non-canonical byte fails closed.
    for expected_byte in expected {
        if encoded.is_empty() {
            return Ok(false);
        }
        let actual =
            u8::decode(&mut encoded).map_err(|error| invalid_da_payload(error.to_string()))?;
        if actual != *expected_byte {
            return Ok(false);
        }
    }
    Ok(encoded.is_empty())
}

#[cfg(test)]
pub(crate) fn encode_da_payload(blocks: &[Vec<Vec<u8>>], l2_entries: &[Vec<u8>]) -> Vec<u8> {
    use alloy_rlp::Encodable as _;

    #[derive(alloy_rlp::RlpEncodable)]
    struct Body {
        block_tx_counts: Vec<u16>,
        transactions: Vec<Vec<u8>>,
        l2_entries: Vec<Vec<u8>>,
    }

    let body = Body {
        block_tx_counts: blocks
            .iter()
            .map(|block| u16::try_from(block.len()).expect("test block exceeds u16::MAX txs"))
            .collect(),
        transactions: blocks.iter().flatten().cloned().collect(),
        l2_entries: l2_entries.to_vec(),
    };
    let mut encoded = Vec::with_capacity(1 + body.length());
    encoded.push(CALL_DATA_TAG);
    body.encode(&mut encoded);
    encoded
}

#[cfg(test)]
pub(crate) fn encode_grouped_da_payload(
    blocks: &[Vec<Vec<u8>>],
    l2_entries: &[Vec<u8>],
    outbound_group_sizes: &[u16],
) -> Vec<u8> {
    use alloy_rlp::Encodable as _;

    #[derive(alloy_rlp::RlpEncodable)]
    struct Body {
        block_tx_counts: Vec<u16>,
        transactions: Vec<Vec<u8>>,
        l2_entries: Vec<Vec<u8>>,
        outbound_group_sizes: Vec<u16>,
    }

    let body = Body {
        block_tx_counts: blocks
            .iter()
            .map(|block| u16::try_from(block.len()).expect("test block exceeds u16::MAX txs"))
            .collect(),
        transactions: blocks.iter().flatten().cloned().collect(),
        l2_entries: l2_entries.to_vec(),
        outbound_group_sizes: outbound_group_sizes.to_vec(),
    };
    let mut encoded = Vec::with_capacity(1 + body.length());
    encoded.push(GROUPED_CALL_DATA_TAG);
    body.encode(&mut encoded);
    encoded
}
