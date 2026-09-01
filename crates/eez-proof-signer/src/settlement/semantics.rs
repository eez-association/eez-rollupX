//! Binding of native blob semantics to independently authorized effects.

use std::num::NonZeroU64;

use alloy_sol_types::SolValue as _;
use eez_protocol::abi::{ExecutionEntrySol, StaticExecutionEntrySol};
use eez_protocol::blob_semantics::SemanticTransaction;
use eez_protocol::entries::finalize_l1_entry_traces;
use eez_protocol::system_tx::OutboundSourceEntry;
use eez_protocol::{Composition, RollupId};
use reth_primitives_traits::BlockBody as _;
use thiserror::Error;

use super::{AuthorizedInboundEffects, AuthorizedOutboundEffects, EthereumBlock};
use crate::validate::ValidatedWindow;

/// A native semantic stream did not reproduce the effects authorized from
/// validated execution and the canonical settlement batch.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum SemanticBindingError {
    #[error("native semantic stream has {actual} transactions; expected at least {minimum}")]
    TransactionCount { minimum: usize, actual: usize },
    #[error(
        "semantic transaction {transaction} originates on chain {actual}; expected chain {expected}"
    )]
    Origin {
        transaction: usize,
        expected: u64,
        actual: u64,
    },
    #[error("validated settling block RLP does not decode for semantic binding: {0}")]
    InvalidSettlingBlock(String),
    #[error(
        "outbound semantic transaction {transaction} has no validated L2 transaction at index {transaction_index}"
    )]
    MissingOutboundTransaction {
        transaction: usize,
        transaction_index: usize,
    },
    #[error(
        "outbound semantic transaction {transaction} txData differs from validated L2 transaction {transaction_index}"
    )]
    OutboundTransactionData {
        transaction: usize,
        transaction_index: usize,
    },
    #[error("semantic transaction {transaction} cannot be materialized: {reason}")]
    Materialization { transaction: usize, reason: String },
    #[error(
        "semantic transaction {transaction} materializes a target on unsupported chain {chain_id}"
    )]
    UnsupportedTarget { transaction: usize, chain_id: u64 },
    #[error("semantic transaction {transaction} carries unsupported static target entries")]
    StaticTargetEntries { transaction: usize },
    #[error("semantic transaction {transaction} has {actual} L1 entries; expected {expected}")]
    L1EntryCount {
        transaction: usize,
        expected: usize,
        actual: usize,
    },
    #[error(
        "semantic transaction {transaction} has {actual} L1 static entries; expected {expected}"
    )]
    L1StaticEntryCount {
        transaction: usize,
        expected: usize,
        actual: usize,
    },
    #[error("semantic transaction {transaction} L1 table finalization failed: {reason}")]
    L1Finalization { transaction: usize, reason: String },
    #[error("outbound semantic transaction {transaction} does not reproduce its L2 sidecars")]
    OutboundSidecars { transaction: usize },
    #[error("outbound semantic transaction {transaction} does not reproduce its L1 host entries")]
    OutboundSettlement { transaction: usize },
    #[error("L1-origin semantic transactions do not reproduce the authorized inbound L1 entries")]
    InboundSettlement,
    #[error("L1-origin semantic transactions do not reproduce the authorized inbound L2 sidecars")]
    InboundSidecars,
    #[error("L1-origin semantic transactions do not reproduce the authorized L1 static entries")]
    InboundStaticSettlement,
}

/// Verify that the native cross-chain message stream is a lossless semantic
/// representation of the effects admitted by the signer.
///
/// L2-origin transaction bytes are independently available in the validated
/// Sync block and therefore match exactly. L1-origin `txData` is chain-defined
/// sidecar data: this pipeline does not execute the later bundled L1 user
/// transactions, so soundness comes from exact bidirectional table binding.
pub(crate) fn verify_native_semantics(
    transactions: &[SemanticTransaction],
    validated_window: &ValidatedWindow,
    outbound_effects: &AuthorizedOutboundEffects,
    inbound_effects: &AuthorizedInboundEffects<'_>,
    expected_inbound_static_entries: &[StaticExecutionEntrySol],
    expected_rollup_id: NonZeroU64,
) -> Result<(), SemanticBindingError> {
    let outbound_groups = outbound_effects.groups().collect::<Vec<_>>();
    if transactions.len() < outbound_groups.len() {
        return Err(SemanticBindingError::TransactionCount {
            minimum: outbound_groups.len(),
            actual: transactions.len(),
        });
    }

    let settling_block: EthereumBlock =
        alloy_rlp::decode_exact(validated_window.settling_block().block().rlp())
            .map_err(|error| SemanticBindingError::InvalidSettlingBlock(error.to_string()))?;
    let settling_transactions = settling_block
        .body
        .encoded_2718_transactions_iter()
        .collect::<Vec<_>>();
    let rollup_id = expected_rollup_id.get();

    for (transaction_index, (transaction, group)) in
        transactions.iter().zip(outbound_groups.iter()).enumerate()
    {
        require_origin(transaction, transaction_index, rollup_id)?;
        let l2_transaction_index = group[0].transaction_index();
        let expected_tx = settling_transactions.get(l2_transaction_index).ok_or(
            SemanticBindingError::MissingOutboundTransaction {
                transaction: transaction_index,
                transaction_index: l2_transaction_index,
            },
        )?;
        if transaction.tx_data.as_ref() != expected_tx.as_slice() {
            return Err(SemanticBindingError::OutboundTransactionData {
                transaction: transaction_index,
                transaction_index: l2_transaction_index,
            });
        }

        let mut composition = materialize(transaction, transaction_index)?;
        require_supported_targets(&composition, transaction_index, rollup_id)?;
        if outbound_sidecars(&composition, rollup_id, transaction_index)?
            != expected_outbound_sidecars(group)
        {
            return Err(SemanticBindingError::OutboundSidecars {
                transaction: transaction_index,
            });
        }
        let expected_settlement = expected_outbound_settlement(group);
        if finalized_target_entries(
            &mut composition,
            RollupId::MAINNET,
            &expected_settlement,
            transaction_index,
        )? != encode_entries(&expected_settlement)
        {
            return Err(SemanticBindingError::OutboundSettlement {
                transaction: transaction_index,
            });
        }
    }

    let expected_inbound_entries = inbound_effects
        .iter()
        .map(super::inbound::AuthorizedInboundEffect::claimed_entry)
        .collect::<Vec<_>>();
    let mut expected_inbound_cursor = 0usize;
    let mut expected_inbound_static_cursor = 0usize;
    let mut actual_inbound_settlement = Vec::new();
    let mut actual_inbound_static_settlement = Vec::new();
    let mut actual_inbound_sidecars = Vec::new();
    for (transaction_index, transaction) in
        transactions.iter().enumerate().skip(outbound_groups.len())
    {
        require_origin(transaction, transaction_index, RollupId::MAINNET.0)?;
        let mut composition = materialize(transaction, transaction_index)?;
        require_supported_targets(&composition, transaction_index, rollup_id)?;
        let entry_count = composition.source.batch.entries.len();
        let expected_end = expected_inbound_cursor.saturating_add(entry_count);
        let expected_entries = expected_inbound_entries
            .get(expected_inbound_cursor..expected_end)
            .ok_or(SemanticBindingError::InboundSettlement)?;
        actual_inbound_settlement.extend(finalized_source_entries(
            &mut composition,
            expected_entries,
            transaction_index,
        )?);
        expected_inbound_cursor = expected_end;
        let static_entry_count = composition.source.batch.staticEntries.len();
        let expected_static_end = expected_inbound_static_cursor.saturating_add(static_entry_count);
        let expected_static_entries = expected_inbound_static_entries
            .get(expected_inbound_static_cursor..expected_static_end)
            .ok_or(SemanticBindingError::InboundStaticSettlement)?;
        actual_inbound_static_settlement.extend(finalized_source_static_entries(
            &mut composition,
            expected_static_entries,
            transaction_index,
        )?);
        expected_inbound_static_cursor = expected_static_end;
        actual_inbound_sidecars.extend(target_entries(
            &composition,
            RollupId(rollup_id),
            transaction_index,
        )?);
    }

    if expected_inbound_cursor != expected_inbound_entries.len() {
        return Err(SemanticBindingError::InboundSettlement);
    }
    if expected_inbound_static_cursor != expected_inbound_static_entries.len() {
        return Err(SemanticBindingError::InboundStaticSettlement);
    }
    let expected_inbound_settlement = encode_entries(&expected_inbound_entries);
    if actual_inbound_settlement != expected_inbound_settlement {
        return Err(SemanticBindingError::InboundSettlement);
    }
    let expected_inbound_static_settlement = expected_inbound_static_entries
        .iter()
        .map(StaticExecutionEntrySol::abi_encode)
        .collect::<Vec<_>>();
    if actual_inbound_static_settlement != expected_inbound_static_settlement {
        return Err(SemanticBindingError::InboundStaticSettlement);
    }
    let expected_inbound_sidecars = inbound_effects
        .iter()
        .map(|effect| effect.observation().derived_da_entry.encoded())
        .collect::<Vec<_>>();
    if actual_inbound_sidecars != expected_inbound_sidecars {
        return Err(SemanticBindingError::InboundSidecars);
    }
    Ok(())
}

fn require_origin(
    transaction: &SemanticTransaction,
    transaction_index: usize,
    expected: u64,
) -> Result<(), SemanticBindingError> {
    if transaction.origin_chain.0 != expected {
        return Err(SemanticBindingError::Origin {
            transaction: transaction_index,
            expected,
            actual: transaction.origin_chain.0,
        });
    }
    Ok(())
}

fn materialize(
    transaction: &SemanticTransaction,
    transaction_index: usize,
) -> Result<Composition, SemanticBindingError> {
    transaction
        .materialize()
        .map_err(|error| SemanticBindingError::Materialization {
            transaction: transaction_index,
            reason: error.to_string(),
        })
}

fn require_supported_targets(
    composition: &Composition,
    transaction_index: usize,
    rollup_id: u64,
) -> Result<(), SemanticBindingError> {
    for target in &composition.targets {
        if target.rollup_id != RollupId::MAINNET && target.rollup_id != RollupId(rollup_id) {
            return Err(SemanticBindingError::UnsupportedTarget {
                transaction: transaction_index,
                chain_id: target.rollup_id.0,
            });
        }
    }
    Ok(())
}

fn outbound_sidecars(
    composition: &Composition,
    rollup_id: u64,
    transaction_index: usize,
) -> Result<Vec<Vec<u8>>, SemanticBindingError> {
    let callback_entries = target_entries(composition, RollupId(rollup_id), transaction_index)?;
    let counts = &composition.source.callback_entry_counts;
    if counts.len() != composition.source.batch.entries.len()
        || counts.iter().sum::<usize>() != callback_entries.len()
    {
        return Err(SemanticBindingError::OutboundSidecars {
            transaction: transaction_index,
        });
    }
    let mut encoded = Vec::with_capacity(
        composition.source.batch.entries.len()
            + callback_entries.len()
            + composition.source.batch.staticEntries.len(),
    );
    let mut callback_cursor = 0usize;
    for (entry, callback_count) in composition
        .source
        .batch
        .entries
        .iter()
        .zip(counts.iter().copied())
    {
        encoded.push(entry.abi_encode());
        let callback_end = callback_cursor + callback_count;
        encoded.extend_from_slice(&callback_entries[callback_cursor..callback_end]);
        callback_cursor = callback_end;
    }
    encoded.extend(
        composition
            .source
            .batch
            .staticEntries
            .iter()
            .map(eez_protocol::abi::StaticExecutionEntrySol::abi_encode),
    );
    Ok(encoded)
}

fn expected_outbound_sidecars(group: &[super::outbound::AuthorizedOutboundEffect]) -> Vec<Vec<u8>> {
    group
        .iter()
        .flat_map(|effect| {
            let source = match effect.derived_source_entry() {
                OutboundSourceEntry::Mutable(entry) => entry.abi_encode(),
                OutboundSourceEntry::Static(entry) => entry.abi_encode(),
            };
            std::iter::once(source).chain(
                effect
                    .derived_callback_entries()
                    .iter()
                    .map(ExecutionEntrySol::abi_encode),
            )
        })
        .collect()
}

fn expected_outbound_settlement(
    group: &[super::outbound::AuthorizedOutboundEffect],
) -> Vec<&ExecutionEntrySol> {
    let mut last_entry_index = None;
    group
        .iter()
        .filter_map(|effect| {
            let index = effect.claimed_entry_index();
            if last_entry_index == Some(index) {
                return None;
            }
            last_entry_index = Some(index);
            Some(effect.claimed_entry())
        })
        .collect()
}

fn target_entries(
    composition: &Composition,
    rollup_id: RollupId,
    transaction_index: usize,
) -> Result<Vec<Vec<u8>>, SemanticBindingError> {
    let mut encoded = Vec::new();
    for target in composition
        .targets
        .iter()
        .filter(|target| target.rollup_id == rollup_id)
    {
        if !target.batch.staticEntries.is_empty() {
            return Err(SemanticBindingError::StaticTargetEntries {
                transaction: transaction_index,
            });
        }
        encoded.extend(
            target
                .batch
                .entries
                .iter()
                .map(ExecutionEntrySol::abi_encode),
        );
    }
    Ok(encoded)
}

fn finalized_target_entries(
    composition: &mut Composition,
    rollup_id: RollupId,
    expected_entries: &[&ExecutionEntrySol],
    transaction_index: usize,
) -> Result<Vec<Vec<u8>>, SemanticBindingError> {
    let mut encoded = Vec::new();
    let mut expected_cursor = 0usize;
    for target in composition
        .targets
        .iter_mut()
        .filter(|target| target.rollup_id == rollup_id)
    {
        if !target.batch.staticEntries.is_empty() {
            return Err(SemanticBindingError::StaticTargetEntries {
                transaction: transaction_index,
            });
        }
        let expected_end = expected_cursor.saturating_add(target.batch.entries.len());
        let expected = expected_entries.get(expected_cursor..expected_end).ok_or(
            SemanticBindingError::L1EntryCount {
                transaction: transaction_index,
                expected: expected_entries.len(),
                actual: expected_end,
            },
        )?;
        hydrate_root_updates(&mut target.batch.entries, expected);
        finalize_l1_entry_traces(&mut target.batch, &target.entry_traces).map_err(|error| {
            SemanticBindingError::L1Finalization {
                transaction: transaction_index,
                reason: error.to_string(),
            }
        })?;
        encoded.extend(
            target
                .batch
                .entries
                .iter()
                .map(ExecutionEntrySol::abi_encode),
        );
        expected_cursor = expected_end;
    }
    if expected_cursor != expected_entries.len() {
        return Err(SemanticBindingError::L1EntryCount {
            transaction: transaction_index,
            expected: expected_entries.len(),
            actual: expected_cursor,
        });
    }
    Ok(encoded)
}

fn finalized_source_entries(
    composition: &mut Composition,
    expected_entries: &[&ExecutionEntrySol],
    transaction_index: usize,
) -> Result<Vec<Vec<u8>>, SemanticBindingError> {
    let actual = composition.source.batch.entries.len();
    if actual != expected_entries.len() {
        return Err(SemanticBindingError::L1EntryCount {
            transaction: transaction_index,
            expected: expected_entries.len(),
            actual,
        });
    }
    hydrate_root_updates(&mut composition.source.batch.entries, expected_entries);
    finalize_l1_entry_traces(
        &mut composition.source.batch,
        &composition.source.entry_traces,
    )
    .map_err(|error| SemanticBindingError::L1Finalization {
        transaction: transaction_index,
        reason: error.to_string(),
    })?;
    Ok(composition
        .source
        .batch
        .entries
        .iter()
        .map(ExecutionEntrySol::abi_encode)
        .collect())
}

fn finalized_source_static_entries(
    composition: &mut Composition,
    expected_entries: &[StaticExecutionEntrySol],
    transaction_index: usize,
) -> Result<Vec<Vec<u8>>, SemanticBindingError> {
    let actual = composition.source.batch.staticEntries.len();
    if actual != expected_entries.len() {
        return Err(SemanticBindingError::L1StaticEntryCount {
            transaction: transaction_index,
            expected: expected_entries.len(),
            actual,
        });
    }
    for (entry, expected) in composition
        .source
        .batch
        .staticEntries
        .iter_mut()
        .zip(expected_entries)
    {
        entry.expectedRoots.clone_from(&expected.expectedRoots);
    }
    Ok(composition
        .source
        .batch
        .staticEntries
        .iter()
        .map(StaticExecutionEntrySol::abi_encode)
        .collect())
}

fn hydrate_root_updates(entries: &mut [ExecutionEntrySol], expected: &[&ExecutionEntrySol]) {
    for (entry, expected) in entries.iter_mut().zip(expected) {
        entry.rootUpdates.clone_from(&expected.rootUpdates);
    }
}

fn encode_entries(entries: &[&ExecutionEntrySol]) -> Vec<Vec<u8>> {
    entries.iter().map(|entry| entry.abi_encode()).collect()
}
