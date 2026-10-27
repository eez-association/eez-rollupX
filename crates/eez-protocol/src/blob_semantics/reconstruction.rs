//! Checked reconstruction inputs derived from the enclosing semantic stream.
//! These are shape and table-binding facts, never execution authorization.

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{Decodable2718, Encodable2718};
use alloy_sol_types::SolValue;
use reth_primitives_traits::SignerRecoverable;

use super::SemanticTransaction;
use crate::abi::ExecutionEntrySol;
use crate::entries::{InboundSidecar, finalize_l1_entry_traces};
use crate::native_source::{LiveSourceContext, SourceEntry};
use crate::system_tx::{OutboundSourceEntry, OutboundSyncEffect, OutboundSyncGroup};
use crate::{Composition, EvmBatch, RollupId};

/// A supplied semantic stream or its settlement tables cannot be reconstructed
/// under the local two-chain profile. No partial result is returned.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReconstructionError {
    /// The configured identity denotes L1 instead of a rollup.
    #[error("reconstruction requires a nonzero local rollup identity")]
    RollupIdentity,
    /// A transaction starts on neither L1 nor the configured rollup.
    #[error("semantic transaction originates on unsupported rollup {0}")]
    Origin(u64),
    /// The stream would interleave the two protocol execution phases.
    #[error("outbound semantic transaction follows an inbound transaction")]
    DirectionOrder,
    /// A semantic bracket has no represented action.
    #[error("semantic transaction has no cross-chain actions")]
    EmptyTransaction,
    /// The signed L2 origin cannot be included verbatim on this chain.
    #[error("outbound origin transaction is invalid or unsupported")]
    OriginTransaction,
    /// The call forest or its deferred rolling-hash finalization is invalid.
    #[error("semantic materialization failed: {0}")]
    Materialization(String),
    /// The materialized tables exceed the supported reconstruction profile.
    #[error("semantic execution table is invalid: {0}")]
    Table(&'static str),
    /// An inbound delivery failed checked shape/routing conversion.
    #[error("semantic inbound conversion failed: {0}")]
    Inbound(String),
    /// The exact settlement table coverage or bytes differ.
    #[error("semantic settlement binding failed: {0}")]
    Settlement(&'static str),
}

/// Whole outbound groups followed by checked inbound deliveries. Direction comes
/// from the semantic origin chain, not a hash sentinel or an ABI decoding guess.
///
/// Construction does not authorize a deposit. Consumers must bind settlement,
/// execute the exact reconstruction, and validate the resulting commitments.
#[derive(Clone, Debug)]
pub struct ReconstructionActions {
    rollup: RollupId,
    outbound: Vec<OutboundSyncGroup>,
    inbound: Vec<InboundSidecar>,
    outbound_entry_indices: Vec<u64>,
    inbound_entry_indices: Vec<u64>,
    compositions: Vec<Composition>,
}

impl ReconstructionActions {
    /// Convert the complete supplied stream without discarding foreign entries,
    /// changing outcomes, or flattening nested/static/rollback semantics.
    pub fn from_semantics(
        transactions: &[SemanticTransaction],
        rollup: RollupId,
        l2_chain_id: u64,
    ) -> Result<Self, ReconstructionError> {
        if rollup.is_mainnet() {
            return Err(ReconstructionError::RollupIdentity);
        }
        let mut result = Self {
            rollup,
            outbound: Vec::new(),
            inbound: Vec::new(),
            outbound_entry_indices: Vec::new(),
            inbound_entry_indices: Vec::new(),
            compositions: Vec::new(),
        };
        let mut inbound_started = false;
        let mut next_entry = 1u64; // Mutable row zero is the ordinary anchor.
        for transaction in transactions {
            if transaction.actions.is_empty() {
                return Err(ReconstructionError::EmptyTransaction);
            }
            let outbound = if transaction.origin_chain == rollup {
                if inbound_started {
                    return Err(ReconstructionError::DirectionOrder);
                }
                check_origin_transaction(&transaction.tx_data, l2_chain_id)?;
                true
            } else if transaction.origin_chain.is_mainnet() {
                inbound_started = true;
                false
            } else {
                return Err(ReconstructionError::Origin(transaction.origin_chain.0));
            };
            // Failed/reverted roots still occupy original positions. They may
            // not smuggle a foreign chain through an empty materialized target.
            if transaction.actions.iter().any(|action| {
                [action.source_rollup_id, action.target_rollup_id]
                    .iter()
                    .any(|id| !id.is_mainnet() && *id != rollup)
            }) {
                return Err(ReconstructionError::Table("foreign action chain"));
            }
            let composition = transaction
                .materialize()
                .map_err(|e| ReconstructionError::Materialization(e.to_string()))?;
            for target in &composition.targets {
                if target.rollup_id != rollup && !target.rollup_id.is_mainnet() {
                    return Err(ReconstructionError::Table("foreign destination"));
                }
                if !target.batch.staticEntries.is_empty() {
                    return Err(ReconstructionError::Table("static target entries"));
                }
            }
            let deliveries = composition
                .targets
                .iter()
                .filter(|target| target.rollup_id == rollup)
                .flat_map(|target| &target.batch.entries)
                .map(|entry| {
                    InboundSidecar::for_rollup(entry, rollup)
                        .map_err(|e| ReconstructionError::Inbound(e.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if outbound {
                let source = &composition.source;
                if !source.batch.staticEntries.is_empty() || !deliveries.is_empty() {
                    return Err(ReconstructionError::Table(
                        "standalone outbound callbacks are unsupported",
                    ));
                }
                let mut effects = Vec::new();
                for entry in &source.batch.entries {
                    if entry.destinationRollupId != RollupId::MAINNET.0
                        || !entry.rootUpdates.is_empty()
                    {
                        return Err(ReconstructionError::Table("outbound source routing"));
                    }
                    effects.push(OutboundSyncEffect {
                        source_entry: OutboundSourceEntry::Mutable(entry.clone()),
                    });
                }
                effects.extend(source.l2_static_entries.iter().cloned().map(|entry| {
                    OutboundSyncEffect {
                        source_entry: OutboundSourceEntry::Static(entry),
                    }
                }));
                if effects.is_empty() {
                    return Err(ReconstructionError::Table(
                        "outbound transaction has no source table",
                    ));
                }
                result.outbound.push(OutboundSyncGroup {
                    effects,
                    user_tx: transaction.tx_data.clone(),
                });
                let hosts = composition
                    .targets
                    .iter()
                    .filter(|target| target.rollup_id.is_mainnet())
                    .flat_map(|target| &target.batch.entries)
                    .take(2)
                    .count();
                if hosts != 1 {
                    return Err(ReconstructionError::Table(
                        "outbound bracket requires one settlement host",
                    ));
                }
                result.outbound_entry_indices.push(next_entry);
                next_entry = next_entry
                    .checked_add(1)
                    .ok_or(ReconstructionError::Table("entry index exceeds uint64"))?;
            } else {
                if !composition.source.l2_static_entries.is_empty() {
                    return Err(ReconstructionError::Table(
                        "L1 source has L2 static entries",
                    ));
                }
                let applied = crate::settlement::l1_applied_root_mask(&transaction.actions)
                    .map_err(ReconstructionError::Materialization)?;
                let roots = transaction
                    .actions
                    .iter()
                    .enumerate()
                    .filter(|(_, action)| {
                        action.parent_index.is_none()
                            && action.call_mode == crate::CallMode::Mutable
                    })
                    .enumerate()
                    .collect::<Vec<_>>();
                if roots.len() != composition.source.batch.entries.len() {
                    return Err(ReconstructionError::Table(
                        "mutable root/settlement row count differs",
                    ));
                }
                let mutable_count = u64::try_from(roots.len())
                    .map_err(|_| ReconstructionError::Table("entry count exceeds uint64"))?;
                let roots = roots
                    .into_iter()
                    .filter(|(_, (index, _))| applied[*index])
                    .collect::<Vec<_>>();
                if roots.len() != deliveries.len() {
                    return Err(ReconstructionError::Table(
                        "inbound root/delivery count differs",
                    ));
                }
                for ((root_index, (_, action)), delivery) in roots.into_iter().zip(deliveries) {
                    let call_hash = crate::common_cross_chain_call_hash(crate::CallHashInput {
                        call_mode: action.call_mode,
                        source_address: action.source_address,
                        source_rollup_id: action.source_rollup_id,
                        target_address: action.target_address,
                        target_rollup_id: action.target_rollup_id,
                        value: action.value,
                        data: &action.data,
                    });
                    if call_hash != delivery.as_abi_entry().proxyEntryHash {
                        return Err(ReconstructionError::Table(
                            "inbound root/delivery identity differs",
                        ));
                    }
                    result.inbound_entry_indices.push(
                        next_entry
                            .checked_add(u64::try_from(root_index).map_err(|_| {
                                ReconstructionError::Table("root index exceeds uint64")
                            })?)
                            .ok_or(ReconstructionError::Table("entry index exceeds uint64"))?,
                    );
                    result.inbound.push(delivery);
                }
                next_entry = next_entry
                    .checked_add(mutable_count)
                    .ok_or(ReconstructionError::Table("entry index exceeds uint64"))?;
            }
            result.compositions.push(composition);
        }
        Ok(result)
    }

    /// Complete signed L2-origin groups in execution order.
    pub fn outbound(&self) -> &[OutboundSyncGroup] {
        &self.outbound
    }

    /// Applying L1-origin deliveries; failed outer calls and static lookups
    /// remain represented by settlement, without an L2 transaction.
    pub fn inbound(&self) -> &[InboundSidecar] {
        &self.inbound
    }

    /// Original mutable settlement rows in whole-group execution order. Derive
    /// these from the complete stream before selecting a partially consumed suffix.
    /// Each hash is resolved only once execution establishes its raw pre-group B[i].
    pub fn source_entries(
        &self,
        context: &LiveSourceContext,
    ) -> Result<Vec<SourceEntry>, ReconstructionError> {
        self.outbound_entry_indices
            .iter()
            .chain(&self.inbound_entry_indices)
            .map(|&index| {
                context
                    .entry(index)
                    .map_err(|error| ReconstructionError::Table(error.0))
            })
            .collect()
    }

    /// Match every semantic L1 table to its exact supplied settlement table.
    /// Only root updates/pins are supplied by settlement: their values still
    /// require independent execution/commitment authorization by the caller.
    /// No extra semantic or settlement suffix is accepted.
    pub fn bind_settlement(&self, batch: &EvmBatch) -> Result<(), ReconstructionError> {
        if !batch.immediateStaticEntryCount.is_zero() {
            return Err(ReconstructionError::Settlement(
                "immediate static entries are unsupported",
            ));
        }
        let immediate = usize::try_from(batch.immediateEntryCount)
            .map_err(|_| ReconstructionError::Settlement("immediate count overflow"))?;
        if immediate == 0 || immediate > batch.entries.len() {
            return Err(ReconstructionError::Settlement("invalid immediate count"));
        }
        let (anchor, rest) = batch
            .entries
            .split_first()
            .ok_or(ReconstructionError::Settlement("missing anchor"))?;
        if !crate::settlement::is_root_only_entry(anchor, self.rollup.0) {
            return Err(ReconstructionError::Settlement("invalid anchor shape"));
        }
        let mut outbound = &rest[..immediate - 1];
        let mut inbound = &batch.entries[immediate..];
        let mut statics = batch.staticEntries.as_slice();
        for composition in &self.compositions {
            let mut composition = composition.clone();
            if composition.source.rollup_id == self.rollup {
                for target in composition
                    .targets
                    .iter_mut()
                    .filter(|t| t.rollup_id.is_mainnet())
                {
                    bind_entries(
                        &mut target.batch,
                        &target.entry_traces,
                        &mut outbound,
                        self.rollup,
                    )?;
                }
            } else {
                bind_entries(
                    &mut composition.source.batch,
                    &composition.source.entry_traces,
                    &mut inbound,
                    self.rollup,
                )?;
                for entry in &mut composition.source.batch.staticEntries {
                    let (expected, remaining) = statics
                        .split_first()
                        .ok_or(ReconstructionError::Settlement("missing static entry"))?;
                    let [pin] = expected.expectedRoots.as_slice() else {
                        return Err(ReconstructionError::Settlement("static root pin count"));
                    };
                    if pin.rollupId != self.rollup.0 {
                        return Err(ReconstructionError::Settlement("static root identity"));
                    }
                    entry.expectedRoots.clone_from(&expected.expectedRoots);
                    if entry.abi_encode() != expected.abi_encode() {
                        return Err(ReconstructionError::Settlement("static table mismatch"));
                    }
                    statics = remaining;
                }
            }
        }
        if !outbound.is_empty() || !inbound.is_empty() || !statics.is_empty() {
            return Err(ReconstructionError::Settlement("unused settlement entries"));
        }
        Ok(())
    }
}

fn check_origin_transaction(raw: &[u8], chain_id: u64) -> Result<(), ReconstructionError> {
    let mut remainder = raw;
    let tx = TxEnvelope::decode_2718(&mut remainder)
        .map_err(|_| ReconstructionError::OriginTransaction)?;
    if !remainder.is_empty()
        || tx.encoded_2718() != raw
        || tx.is_eip4844()
        || tx.chain_id().is_some_and(|actual| actual != chain_id)
        || tx.recover_signer().is_err()
    {
        return Err(ReconstructionError::OriginTransaction);
    }
    Ok(())
}

fn bind_entries(
    batch: &mut EvmBatch,
    traces: &[crate::entries::EntryTrace],
    remaining: &mut &[ExecutionEntrySol],
    rollup: RollupId,
) -> Result<(), ReconstructionError> {
    if remaining.len() < batch.entries.len() {
        return Err(ReconstructionError::Settlement("missing mutable entry"));
    }
    let (expected, rest) = remaining.split_at(batch.entries.len());
    for (entry, supplied) in batch.entries.iter_mut().zip(expected) {
        let [update] = supplied.rootUpdates.as_slice() else {
            return Err(ReconstructionError::Settlement("mutable root update count"));
        };
        if update.rollupId != rollup.0 {
            return Err(ReconstructionError::Settlement("mutable root identity"));
        }
        entry.rootUpdates.clone_from(&supplied.rootUpdates);
    }
    finalize_l1_entry_traces(batch, traces)
        .map_err(|e| ReconstructionError::Materialization(e.to_string()))?;
    if batch
        .entries
        .iter()
        .map(ExecutionEntrySol::abi_encode)
        .ne(expected.iter().map(ExecutionEntrySol::abi_encode))
    {
        return Err(ReconstructionError::Settlement("mutable table mismatch"));
    }
    *remaining = rest;
    Ok(())
}

#[cfg(test)]
mod tests;
