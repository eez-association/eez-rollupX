//! Native EEZ blob semantics ⇄ resolved composition call forests.
//!
//! The standardized blob codec owns byte framing. This module owns the
//! deployment's table translation: each bracketed semantic transaction is
//! converted to the exact [`ExecutedAction`] preorder consumed by the same
//! materializer used during live composition.

use alloy_primitives::{Bytes, U256};
use eez_blob_codec::Message;
use thiserror::Error;

use crate::{
    CallMode, Composition, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions,
};

/// One cross-chain transaction represented by the native semantic messages.
#[derive(Clone, Debug)]
pub struct SemanticTransaction {
    /// Chain containing the originating signed transaction (`0` denotes L1).
    pub origin_chain: RollupId,
    /// Chain-defined transaction bytes carried verbatim by the protocol.
    pub tx_data: Bytes,
    /// Resolved calls in strict preorder, including exact outcomes and regions.
    pub actions: Vec<ExecutedAction>,
}

impl SemanticTransaction {
    /// Materialize the semantic forest through the production table builder.
    ///
    /// # Errors
    ///
    /// Returns an error when the decoded forest is outside the supported table
    /// profile or violates preorder, call-result, or rollback invariants.
    pub fn materialize(&self) -> Result<Composition, BlobSemanticError> {
        materialize_actions(self.origin_chain, self.actions.clone())
            .map_err(|error| BlobSemanticError::Materialization(error.to_string()))
    }
}

/// Native semantic translation failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlobSemanticError {
    /// A semantic message appeared without an open transaction.
    #[error("{message} appears outside a cross-chain transaction")]
    OutsideTransaction {
        /// Human-readable message kind found outside a transaction bracket.
        message: &'static str,
    },
    /// A transaction boundary was malformed.
    #[error("invalid semantic transaction boundary: {0}")]
    TransactionBoundary(&'static str),
    /// A result did not have a corresponding open call.
    #[error("semantic return has no open call")]
    ReturnWithoutCall,
    /// A rollback marker did not have a corresponding snapshot.
    #[error("semantic Revert has no open Snapshot")]
    RevertWithoutSnapshot,
    /// A rollback region was structurally invalid for table translation.
    #[error("invalid semantic rollback region: {0}")]
    InvalidRegion(&'static str),
    /// This deployment fixes manager call gas to zero.
    #[error("semantic call gas {actual} is unsupported; expected zero")]
    UnsupportedCallGas {
        /// Forwarded gas value found on the wire.
        actual: u64,
    },
    /// Live table materialization rejected the semantic forest.
    #[error("semantic table materialization failed: {0}")]
    Materialization(String),
    /// An action result had not been resolved.
    #[error("semantic action {index} has no resolved result")]
    PendingOutcome {
        /// Preorder index of the unresolved action.
        index: usize,
    },
    /// A parent edge was inconsistent with the strict preorder.
    #[error("semantic action {index} has invalid parent {parent:?}")]
    InvalidParent {
        /// Preorder index of the invalid action.
        index: usize,
        /// Parent index supplied by the action.
        parent: Option<usize>,
    },
    /// A rollback boundary did not end after complete sibling subtrees.
    #[error("semantic action {index} has an invalid rollback span")]
    InvalidRollbackSpan {
        /// Preorder index carrying the invalid rollback annotation.
        index: usize,
    },
}

#[derive(Debug)]
struct PartialTransaction {
    origin_chain: RollupId,
    tx_data: Bytes,
    actions: Vec<ExecutedAction>,
    open_calls: Vec<usize>,
    contexts: Vec<RollupId>,
    snapshots: Vec<(usize, usize)>,
}

/// Decode all native cross-chain semantic transactions from a validated EEZ
/// message stream. `ChainOperation` and `CloseBlobStream` remain available to
/// their own consumers and are skipped here.
///
/// # Errors
///
/// Returns an error for malformed semantic boundaries, unsupported non-zero
/// call gas, or a forest the production materializer cannot represent.
pub fn decode_semantic_transactions(
    messages: &[Message],
) -> Result<Vec<SemanticTransaction>, BlobSemanticError> {
    let mut transactions = Vec::new();
    let mut current: Option<PartialTransaction> = None;

    for message in messages {
        match message {
            Message::ChainOperation { .. } | Message::CloseBlobStream => {
                if current.is_some() {
                    return Err(BlobSemanticError::TransactionBoundary(
                        "chain-local marker inside semantic transaction",
                    ));
                }
            }
            Message::InitiateCrossChainTransaction { chain_id, tx_data } => {
                if current.is_some() {
                    return Err(BlobSemanticError::TransactionBoundary(
                        "nested InitiateCrossChainTransaction",
                    ));
                }
                current = Some(PartialTransaction {
                    origin_chain: RollupId(*chain_id),
                    tx_data: tx_data.clone(),
                    actions: Vec::new(),
                    open_calls: Vec::new(),
                    contexts: vec![RollupId(*chain_id)],
                    snapshots: Vec::new(),
                });
            }
            Message::Call {
                to_chain,
                from_address,
                to_address,
                value,
                gas,
                data,
            } => {
                push_call(
                    current.as_mut(),
                    CallMode::Mutable,
                    *to_chain,
                    *from_address,
                    *to_address,
                    *value,
                    *gas,
                    data,
                )?;
            }
            Message::StaticCall {
                to_chain,
                from_address,
                to_address,
                gas,
                data,
            } => {
                push_call(
                    current.as_mut(),
                    CallMode::Static,
                    *to_chain,
                    *from_address,
                    *to_address,
                    U256::ZERO,
                    *gas,
                    data,
                )?;
            }
            Message::ReturnSuccess { return_data } | Message::ReturnFail { return_data } => {
                let transaction = current
                    .as_mut()
                    .ok_or(BlobSemanticError::OutsideTransaction { message: "Return" })?;
                let action_index = transaction
                    .open_calls
                    .pop()
                    .ok_or(BlobSemanticError::ReturnWithoutCall)?;
                transaction.contexts.pop();
                let success = matches!(message, Message::ReturnSuccess { .. });
                transaction.actions[action_index].outcome = ExecutionOutcome::Resolved {
                    return_data: return_data.to_vec(),
                    // Forwarded call gas is a wire field; gas consumed is not.
                    gas_used: 0,
                    success,
                };
            }
            Message::Snapshot => {
                let transaction =
                    current
                        .as_mut()
                        .ok_or(BlobSemanticError::OutsideTransaction {
                            message: "Snapshot",
                        })?;
                transaction
                    .snapshots
                    .push((transaction.open_calls.len(), transaction.actions.len()));
            }
            Message::Revert => {
                let transaction = current
                    .as_mut()
                    .ok_or(BlobSemanticError::OutsideTransaction { message: "Revert" })?;
                let (depth, first_action) = transaction
                    .snapshots
                    .pop()
                    .ok_or(BlobSemanticError::RevertWithoutSnapshot)?;
                if transaction.open_calls.len() != depth {
                    return Err(BlobSemanticError::InvalidRegion(
                        "Revert crosses an open call",
                    ));
                }
                let span = transaction.actions.len().saturating_sub(first_action);
                if span == 0 {
                    return Err(BlobSemanticError::InvalidRegion("empty Snapshot region"));
                }
                if transaction.actions[first_action].revert_span.is_some() {
                    return Err(BlobSemanticError::InvalidRegion(
                        "overlapping Snapshot regions",
                    ));
                }
                transaction.actions[first_action].revert_span = Some(
                    u32::try_from(span)
                        .map_err(|_| BlobSemanticError::InvalidRegion("region exceeds u32"))?,
                );
            }
            Message::FinishCrossChainTransaction => {
                let transaction = current
                    .take()
                    .ok_or(BlobSemanticError::OutsideTransaction {
                        message: "FinishCrossChainTransaction",
                    })?;
                if !transaction.open_calls.is_empty() || transaction.contexts.len() != 1 {
                    return Err(BlobSemanticError::TransactionBoundary(
                        "FinishCrossChainTransaction with open call",
                    ));
                }
                if !transaction.snapshots.is_empty() {
                    return Err(BlobSemanticError::TransactionBoundary(
                        "FinishCrossChainTransaction with open Snapshot",
                    ));
                }
                let semantic = SemanticTransaction {
                    origin_chain: transaction.origin_chain,
                    tx_data: transaction.tx_data,
                    actions: transaction.actions,
                };
                semantic.materialize()?;
                transactions.push(semantic);
            }
        }
    }
    if current.is_some() {
        return Err(BlobSemanticError::TransactionBoundary(
            "stream ended before FinishCrossChainTransaction",
        ));
    }
    Ok(transactions)
}

#[allow(clippy::too_many_arguments)]
fn push_call(
    current: Option<&mut PartialTransaction>,
    call_mode: CallMode,
    to_chain: u64,
    from_address: alloy_primitives::Address,
    to_address: alloy_primitives::Address,
    value: U256,
    gas: u64,
    data: &Bytes,
) -> Result<(), BlobSemanticError> {
    if gas != 0 {
        return Err(BlobSemanticError::UnsupportedCallGas { actual: gas });
    }
    let transaction = current.ok_or(BlobSemanticError::OutsideTransaction { message: "Call" })?;
    let source_rollup_id =
        *transaction
            .contexts
            .last()
            .ok_or(BlobSemanticError::TransactionBoundary(
                "missing root context",
            ))?;
    let parent_index = transaction.open_calls.last().copied();
    let action_index = transaction.actions.len();
    transaction.actions.push(ExecutedAction {
        parent_index,
        call_mode,
        target_address: to_address,
        target_rollup_id: RollupId(to_chain),
        source_rollup_id,
        source_address: from_address,
        data: data.clone(),
        value,
        outcome: ExecutionOutcome::Pending,
        revert_span: None,
    });
    transaction.open_calls.push(action_index);
    transaction.contexts.push(RollupId(to_chain));
    Ok(())
}

/// Encode one resolved action forest as the native semantic message bracket.
///
/// # Errors
///
/// Returns an error when the forest cannot be materialized by the current
/// protocol profile or its preorder/rollback metadata is inconsistent.
pub fn encode_semantic_transaction(
    transaction: &SemanticTransaction,
) -> Result<Vec<Message>, BlobSemanticError> {
    transaction.materialize()?;
    let (children, subtree_ends) = action_forest(&transaction.actions)?;
    let roots = transaction
        .actions
        .iter()
        .enumerate()
        .filter_map(|(index, action)| action.parent_index.is_none().then_some(index))
        .collect::<Vec<_>>();
    let mut messages = vec![Message::InitiateCrossChainTransaction {
        chain_id: transaction.origin_chain.0,
        tx_data: transaction.tx_data.clone(),
    }];
    emit_siblings(
        &roots,
        &transaction.actions,
        &children,
        &subtree_ends,
        &mut messages,
    )?;
    messages.push(Message::FinishCrossChainTransaction);
    Ok(messages)
}

fn action_forest(
    actions: &[ExecutedAction],
) -> Result<(Vec<Vec<usize>>, Vec<usize>), BlobSemanticError> {
    let mut children = vec![Vec::new(); actions.len()];
    let mut roots = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        match action.parent_index {
            None => roots.push(index),
            Some(parent) if parent < index => children[parent].push(index),
            parent => return Err(BlobSemanticError::InvalidParent { index, parent }),
        }
    }
    let mut traversal = Vec::with_capacity(actions.len());
    let mut pending = roots.iter().rev().copied().collect::<Vec<_>>();
    while let Some(index) = pending.pop() {
        traversal.push(index);
        pending.extend(children[index].iter().rev().copied());
    }
    if traversal.iter().copied().ne(0..actions.len()) {
        let index = traversal
            .iter()
            .copied()
            .zip(0..actions.len())
            .find_map(|(actual, expected)| (actual != expected).then_some(expected))
            .unwrap_or(actions.len());
        return Err(BlobSemanticError::InvalidParent {
            index,
            parent: actions.get(index).and_then(|action| action.parent_index),
        });
    }
    let mut subtree_ends = (1..=actions.len()).collect::<Vec<_>>();
    for index in (0..actions.len()).rev() {
        if let Some(last_child) = children[index].last() {
            subtree_ends[index] = subtree_ends[*last_child];
        }
    }
    Ok((children, subtree_ends))
}

fn emit_siblings(
    siblings: &[usize],
    actions: &[ExecutedAction],
    children: &[Vec<usize>],
    subtree_ends: &[usize],
    messages: &mut Vec<Message>,
) -> Result<(), BlobSemanticError> {
    let mut active_region_end = None;
    for &index in siblings {
        if let Some(span) = actions[index].revert_span {
            if active_region_end.is_some() || span == 0 {
                return Err(BlobSemanticError::InvalidRollbackSpan { index });
            }
            let end = index
                .checked_add(span as usize)
                .filter(|end| *end <= actions.len())
                .ok_or(BlobSemanticError::InvalidRollbackSpan { index })?;
            messages.push(Message::Snapshot);
            active_region_end = Some(end);
        }
        emit_subtree(index, actions, children, subtree_ends, messages)?;
        if active_region_end == Some(subtree_ends[index]) {
            messages.push(Message::Revert);
            active_region_end = None;
        }
    }
    if active_region_end.is_some() {
        return Err(BlobSemanticError::InvalidRollbackSpan {
            index: *siblings.last().unwrap_or(&0),
        });
    }
    Ok(())
}

fn emit_subtree(
    index: usize,
    actions: &[ExecutedAction],
    children: &[Vec<usize>],
    subtree_ends: &[usize],
    messages: &mut Vec<Message>,
) -> Result<(), BlobSemanticError> {
    let action = &actions[index];
    messages.push(match action.call_mode {
        CallMode::Mutable => Message::Call {
            to_chain: action.target_rollup_id.0,
            from_address: action.source_address,
            to_address: action.target_address,
            value: action.value,
            gas: 0,
            data: action.data.clone(),
        },
        CallMode::Static => Message::StaticCall {
            to_chain: action.target_rollup_id.0,
            from_address: action.source_address,
            to_address: action.target_address,
            gas: 0,
            data: action.data.clone(),
        },
    });
    emit_siblings(&children[index], actions, children, subtree_ends, messages)?;
    let return_data = action
        .outcome
        .return_data()
        .ok_or(BlobSemanticError::PendingOutcome { index })?;
    messages.push(if action.outcome.is_success() {
        Message::ReturnSuccess {
            return_data: Bytes::copy_from_slice(return_data),
        }
    } else {
        Message::ReturnFail {
            return_data: Bytes::copy_from_slice(return_data),
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::Address;
    use alloy_sol_types::SolValue;

    use super::*;

    fn action(parent_index: Option<usize>, source: u64, target: u64, marker: u8) -> ExecutedAction {
        ExecutedAction {
            parent_index,
            call_mode: CallMode::Mutable,
            target_address: Address::with_last_byte(marker),
            target_rollup_id: RollupId(target),
            source_rollup_id: RollupId(source),
            source_address: Address::with_last_byte(marker.wrapping_add(1)),
            data: Bytes::from(vec![marker]),
            value: U256::from(marker),
            outcome: ExecutionOutcome::Resolved {
                return_data: vec![marker.wrapping_add(2)],
                gas_used: 21_000,
                success: true,
            },
            revert_span: None,
        }
    }

    #[test]
    fn nested_semantics_round_trip_through_messages_and_tables() {
        let transaction = SemanticTransaction {
            origin_chain: RollupId(1),
            tx_data: Bytes::from_static(b"signed tx"),
            actions: vec![action(None, 1, 0, 1), action(Some(0), 0, 1, 2)],
        };
        let messages = encode_semantic_transaction(&transaction).unwrap();
        assert!(matches!(
            messages[0],
            Message::InitiateCrossChainTransaction { .. }
        ));
        assert!(matches!(messages[1], Message::Call { to_chain: 0, .. }));
        assert!(matches!(messages[2], Message::Call { to_chain: 1, .. }));
        assert!(matches!(messages[3], Message::ReturnSuccess { .. }));
        assert!(matches!(messages[4], Message::ReturnSuccess { .. }));
        assert!(matches!(messages[5], Message::FinishCrossChainTransaction));

        let decoded = decode_semantic_transactions(&messages).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].origin_chain, RollupId(1));
        assert_eq!(decoded[0].tx_data, Bytes::from_static(b"signed tx"));
        assert_eq!(decoded[0].actions.len(), 2);
        assert_eq!(
            decoded[0]
                .materialize()
                .unwrap()
                .source
                .batch
                .entries
                .abi_encode(),
            transaction
                .materialize()
                .unwrap()
                .source
                .batch
                .entries
                .abi_encode(),
        );
    }

    #[test]
    fn rollback_span_emits_snapshot_after_complete_sibling_subtrees() {
        let mut first = action(None, 1, 0, 1);
        first.revert_span = Some(2);
        let transaction = SemanticTransaction {
            origin_chain: RollupId(1),
            tx_data: Bytes::new(),
            actions: vec![first, action(None, 1, 0, 2)],
        };
        let messages = encode_semantic_transaction(&transaction).unwrap();
        assert!(matches!(messages[1], Message::Snapshot));
        assert!(matches!(messages[6], Message::Revert));
        let decoded = decode_semantic_transactions(&messages).unwrap();
        assert_eq!(decoded[0].actions[0].revert_span, Some(2));
    }

    #[test]
    fn nonzero_call_gas_fails_closed() {
        let messages = vec![
            Message::InitiateCrossChainTransaction {
                chain_id: 1,
                tx_data: Bytes::new(),
            },
            Message::Call {
                to_chain: 0,
                from_address: Address::ZERO,
                to_address: Address::ZERO,
                value: U256::ZERO,
                gas: 1,
                data: Bytes::new(),
            },
            Message::ReturnSuccess {
                return_data: Bytes::new(),
            },
            Message::FinishCrossChainTransaction,
        ];
        assert!(matches!(
            decode_semantic_transactions(&messages),
            Err(BlobSemanticError::UnsupportedCallGas { actual: 1 })
        ));
    }
}
