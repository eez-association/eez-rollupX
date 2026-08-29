//! Inbound transaction inspection and settlement-effect authorization.

use std::fmt;
use std::num::NonZeroU64;

use alloy_primitives::{Address, B256, Bytes, I256, U256};
use alloy_sol_types::{SolCall as _, SolValue as _};
use eez_protocol::abi::{ExecutionEntrySol, executeIncomingCrossChainCallCall};
use eez_protocol::entries::{NestedPairDirection, verify_nested_pair};
use eez_protocol::system_tx::{check_entry_shape, committed_l2_table_value, lift_entry_from_l2};
use eez_protocol::{CallHashInput, CallMode, RollupId, common_cross_chain_call_hash};
use thiserror::Error;

use super::effect_binding::{BoundEffect, BoundEffectSequence, EffectKind};
use crate::validate::OutboundEventObservation;

/// Strict semantic evidence extracted from one top-level inbound system tx.
/// `derived_da_entry` is the canonical sidecar entry the DA payload must carry
/// for this delivery, derived from the calldata that was actually re-executed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InboundObservation {
    pub(crate) recomputed_call_hash: B256,
    pub(crate) value: U256,
    pub(crate) return_data: Bytes,
    pub(super) derived_da_entry: DerivedInboundDaEntry,
}

/// Sidecar wrapper whose diagnostics and equality use canonical ABI bytes.
pub(super) struct DerivedInboundDaEntry(ExecutionEntrySol);

impl DerivedInboundDaEntry {
    /// Borrow the typed entry for canonical Sync reconstruction.
    pub(super) fn as_entry(&self) -> &ExecutionEntrySol {
        &self.0
    }

    /// Encode the exact wire identity expected in `l2Entries`.
    pub(super) fn encoded(&self) -> Vec<u8> {
        self.0.abi_encode()
    }
}

impl fmt::Debug for DerivedInboundDaEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DerivedInboundDaEntry")
            .field(&self.encoded())
            .finish()
    }
}

impl PartialEq for DerivedInboundDaEntry {
    fn eq(&self, other: &Self) -> bool {
        self.encoded() == other.encoded()
    }
}

impl Eq for DerivedInboundDaEntry {}

/// One selector-level inbound candidate and the result of checking its call
/// envelope and execution-table shape. Byte-exact transaction reconstruction
/// remains a separate DA requirement.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InboundCandidate {
    pub(crate) transaction_index: usize,
    pub(crate) inspection: Result<InboundObservation, InboundObservationError>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub(crate) enum InboundObservationError {
    #[error("top-level system transaction reverted")]
    RevertedTransaction,
    #[error("inbound calldata does not decode: {reason}")]
    InvalidAbi { reason: String },
    #[error("inbound calldata is not its canonical complete ABI encoding")]
    NonCanonicalAbi,
    #[error("native transaction value is {actual}; committed L2 table value is {expected}")]
    NativeValueMismatch { expected: U256, actual: U256 },
    #[error("inbound source rollup is {actual}; expected L1 rollup 0")]
    SourceRollup { actual: u64 },
    #[error("inbound calldata carries {actual} execution entries; expected exactly one")]
    EntryCount { actual: usize },
    #[error("inbound calldata carries {actual} static entries; expected none")]
    StaticEntryCount { actual: usize },
    #[error("inbound execution entry has invalid {field}")]
    InvalidEntryShape { field: &'static str },
    #[error("inbound claimed call hash is {claimed}; recomputed {recomputed}")]
    CallHashMismatch { recomputed: B256, claimed: B256 },
    #[error("inbound call hash is the reserved zero value")]
    ZeroCallHash,
}

/// Validate the envelope, canonical calldata, entry shape, and root call hash
/// of one inbound candidate. The complementary L1 table authorizes its rolling
/// hash, nested outcomes, and outbound events later.
///
/// `authorize_inbound_effects` and `verify_da_payload` separately bind the
/// observation to its settlement entry and DA sidecar.
pub(super) fn inspect_inbound_candidate(
    transaction_value: U256,
    calldata: &[u8],
    transaction_succeeded: bool,
    expected_rollup_id: NonZeroU64,
) -> Result<InboundObservation, InboundObservationError> {
    if !transaction_succeeded {
        return Err(InboundObservationError::RevertedTransaction);
    }

    let call = executeIncomingCrossChainCallCall::abi_decode(calldata).map_err(|error| {
        InboundObservationError::InvalidAbi {
            reason: error.to_string(),
        }
    })?;
    if call.abi_encode().as_slice() != calldata {
        return Err(InboundObservationError::NonCanonicalAbi);
    }
    let [entry] = call._entries.as_slice() else {
        return Err(InboundObservationError::EntryCount {
            actual: call._entries.len(),
        });
    };
    if !call._staticEntries.is_empty() {
        return Err(InboundObservationError::StaticEntryCount {
            actual: call._staticEntries.len(),
        });
    }
    let [inner] = entry.incomingCalls.as_slice() else {
        return Err(InboundObservationError::InvalidEntryShape {
            field: "incomingCalls",
        });
    };
    if inner.sourceRollupId != RollupId::MAINNET.0 {
        return Err(InboundObservationError::SourceRollup {
            actual: inner.sourceRollupId,
        });
    }
    for (valid, field) in [(!inner.isStatic, "isStatic"), (inner.gas == 0, "gas")] {
        if !valid {
            return Err(InboundObservationError::InvalidEntryShape { field });
        }
    }
    let committed_value = committed_l2_table_value(call._entries.as_slice()).map_err(|_| {
        InboundObservationError::InvalidEntryShape {
            field: "committedValue",
        }
    })?;
    if transaction_value != committed_value {
        return Err(InboundObservationError::NativeValueMismatch {
            expected: committed_value,
            actual: transaction_value,
        });
    }

    let recomputed_call_hash = common_cross_chain_call_hash(CallHashInput {
        call_mode: CallMode::Mutable,
        source_address: inner.sourceAddress,
        source_rollup_id: RollupId::MAINNET,
        target_address: inner.targetAddress,
        target_rollup_id: RollupId(expected_rollup_id.get()),
        value: inner.value,
        data: &inner.data,
    });
    if recomputed_call_hash == B256::ZERO {
        return Err(InboundObservationError::ZeroCallHash);
    }
    if entry.proxyEntryHash != recomputed_call_hash {
        return Err(InboundObservationError::CallHashMismatch {
            recomputed: recomputed_call_hash,
            claimed: entry.proxyEntryHash,
        });
    }
    let derived_da_entry = lift_entry_from_l2(entry, RollupId(expected_rollup_id.get()));
    if check_entry_shape(&derived_da_entry, "inbound").is_err() {
        return Err(InboundObservationError::InvalidEntryShape {
            field: "executionTable",
        });
    }
    Ok(InboundObservation {
        recomputed_call_hash,
        value: inner.value,
        return_data: entry.returnData.clone(),
        derived_da_entry: DerivedInboundDaEntry(derived_da_entry),
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum InboundEffectError {
    #[error(
        "inbound effect entry {entry_index} at transaction {transaction_index} has no matching candidate"
    )]
    MissingCandidate {
        entry_index: usize,
        transaction_index: usize,
    },
    #[error("inbound candidate at transaction {transaction_index} has no matching effect")]
    UnexpectedCandidate { transaction_index: usize },
    #[error(
        "inbound candidate for entry {entry_index} at transaction {transaction_index} is invalid: {source}"
    )]
    InvalidObservation {
        entry_index: usize,
        transaction_index: usize,
        source: InboundObservationError,
    },
    #[error("deferred inbound entry {entry_index} does not match its L2 execution table: {reason}")]
    ExecutionTableMismatch { entry_index: usize, reason: String },
    #[error(
        "deferred inbound entry {entry_index} targets rollup {actual}; expected rollup {expected}"
    )]
    DestinationRollupMismatch {
        entry_index: usize,
        expected: u64,
        actual: u64,
    },
    #[error(
        "deferred inbound entry {entry_index} claims call hash {claimed}; transaction recomputed {recomputed}"
    )]
    CallHashMismatch {
        entry_index: usize,
        recomputed: B256,
        claimed: B256,
    },
    #[error(
        "deferred inbound entry {entry_index} return data differs from transaction observation"
    )]
    ReturnDataMismatch { entry_index: usize },
    #[error(
        "outbound event at transaction {transaction_index}, receipt log {receipt_log_index}, has no matching inbound effect"
    )]
    UnexpectedOutboundObservation {
        transaction_index: usize,
        receipt_log_index: usize,
    },
    #[error(
        "inbound entry {entry_index} expects {expected} nested L2 event(s), but transaction {transaction_index} emitted {actual}"
    )]
    OutboundObservationCountMismatch {
        entry_index: usize,
        transaction_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error(
        "outbound event at transaction {transaction_index}, receipt log {receipt_log_index}, is malformed"
    )]
    MalformedOutboundObservation {
        transaction_index: usize,
        receipt_log_index: usize,
    },
    #[error(
        "outbound event at transaction {transaction_index}, receipt log {receipt_log_index}, uses callGas {actual}; expected 0"
    )]
    UnsupportedCallGas {
        transaction_index: usize,
        receipt_log_index: usize,
        actual: u64,
    },
    #[error(
        "inbound entry {entry_index} nested L2 call {event_index} recomputes hash {recomputed}; transaction {transaction_index} emitted {observed}"
    )]
    NestedCallHashMismatch {
        entry_index: usize,
        event_index: usize,
        transaction_index: usize,
        recomputed: B256,
        observed: B256,
    },
    #[error("inbound entry {entry_index} uses the reserved system account as a nested L2 source")]
    ReservedSourceAddress { entry_index: usize },
    #[error(
        "inbound transaction {transaction_index} for entry {entry_index} has value {value}, which exceeds the positive int192 range"
    )]
    ValueOutOfRange {
        entry_index: usize,
        transaction_index: usize,
        value: U256,
    },
    #[error(
        "deferred inbound entry {entry_index} ether delta is {actual}; expected deposited value {expected}"
    )]
    EtherDeltaMismatch {
        entry_index: usize,
        expected: I256,
        actual: I256,
    },
}

impl InboundEffectError {
    /// Return the batch entry whose positioned inbound delivery reverted.
    /// Envelope and claim mismatches stay non-actionable because they can
    /// indicate a malformed request rather than a poisoned held transaction.
    pub(crate) const fn poisoned_entry_index(&self) -> Option<usize> {
        match self {
            Self::InvalidObservation {
                entry_index,
                source: InboundObservationError::RevertedTransaction,
                ..
            } => Some(*entry_index),
            Self::MissingCandidate { .. }
            | Self::UnexpectedCandidate { .. }
            | Self::InvalidObservation { .. }
            | Self::ExecutionTableMismatch { .. }
            | Self::DestinationRollupMismatch { .. }
            | Self::CallHashMismatch { .. }
            | Self::ReturnDataMismatch { .. }
            | Self::UnexpectedOutboundObservation { .. }
            | Self::OutboundObservationCountMismatch { .. }
            | Self::MalformedOutboundObservation { .. }
            | Self::UnsupportedCallGas { .. }
            | Self::NestedCallHashMismatch { .. }
            | Self::ReservedSourceAddress { .. }
            | Self::ValueOutOfRange { .. }
            | Self::EtherDeltaMismatch { .. } => None,
        }
    }
}

/// Successful inbound effects whose transaction, settlement entry, value, and
/// canonical DA projection have all been bound positionally.
/// Only [`authorize_inbound_effects`] can construct this in production.
#[cfg_attr(test, derive(Default))]
pub(crate) struct AuthorizedInboundEffects<'settling> {
    bindings: Vec<AuthorizedInboundEffect<'settling>>,
}

pub(super) struct AuthorizedInboundEffect<'settling> {
    transaction_index: usize,
    observation: &'settling InboundObservation,
}

impl<'settling> AuthorizedInboundEffects<'settling> {
    pub(super) fn len(&self) -> usize {
        self.bindings.len()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &AuthorizedInboundEffect<'settling>> {
        self.bindings.iter()
    }
}

impl AuthorizedInboundEffect<'_> {
    pub(super) const fn transaction_index(&self) -> usize {
        self.transaction_index
    }

    pub(super) const fn observation(&self) -> &InboundObservation {
        self.observation
    }
}

/// Bind each successful inbound candidate to the deferred settlement entry at
/// the same effect position.
///
/// Rejects missing, extra, invalid, failed, or reordered candidates and checks
/// entry shape, rollup and call identity, return data, both rolling hashes,
/// nested outbound events, and net ether delta.
pub(crate) fn authorize_inbound_effects<'settling>(
    bound_effects: &BoundEffectSequence<'_, 'settling>,
    expected_rollup_id: NonZeroU64,
    expected_l2_system_address: Address,
) -> Result<AuthorizedInboundEffects<'settling>, InboundEffectError> {
    let settling_observations = bound_effects.settling_observations();
    let mut candidates = settling_observations.inbound_candidates().iter().peekable();
    let mut outbound_observations = settling_observations
        .outbound_event_candidates()
        .iter()
        .peekable();
    let mut authorized_bindings = Vec::with_capacity(bound_effects.inbound_count());

    // Effects and candidates both ascend by transaction index, so one forward
    // merge suffices; any candidate no inbound effect claims fails closed.
    for effect in bound_effects.effects() {
        if let Some(candidate) = candidates.peek()
            && candidate.transaction_index < effect.start_transaction_index()
        {
            return Err(InboundEffectError::UnexpectedCandidate {
                transaction_index: candidate.transaction_index,
            });
        }
        if let Some(observation) = outbound_observations.peek()
            && observation.transaction_index() < effect.user_transaction_index()
        {
            return Err(unexpected_outbound_observation(observation));
        }

        match effect.kind() {
            EffectKind::Outbound => {
                while candidates
                    .next_if(|candidate| {
                        candidate.transaction_index > effect.user_transaction_index()
                            && candidate.transaction_index <= effect.transaction_index()
                    })
                    .is_some()
                {}
                // The outbound authorizer owns events emitted by outbound
                // user transactions. Consume their positions here so both
                // directional gates can independently reject unbound events.
                while outbound_observations
                    .next_if(|observation| {
                        observation.transaction_index() >= effect.user_transaction_index()
                            && observation.transaction_index() <= effect.transaction_index()
                    })
                    .is_some()
                {}
            }
            EffectKind::Inbound => {
                let candidate = candidates
                    .next_if(|candidate| candidate.transaction_index == effect.transaction_index());
                let candidate = candidate.ok_or(InboundEffectError::MissingCandidate {
                    entry_index: effect.entry_index(),
                    transaction_index: effect.transaction_index(),
                })?;
                let observation = candidate.inspection.as_ref().map_err(|source| {
                    InboundEffectError::InvalidObservation {
                        entry_index: effect.entry_index(),
                        transaction_index: effect.transaction_index(),
                        source: source.clone(),
                    }
                })?;
                let mut effect_outbound_observations = Vec::new();
                while let Some(outbound) = outbound_observations
                    .next_if(|outbound| outbound.transaction_index() == effect.transaction_index())
                {
                    effect_outbound_observations.push(outbound);
                }
                authorize_inbound_effect(
                    effect,
                    observation,
                    &effect_outbound_observations,
                    expected_rollup_id,
                    expected_l2_system_address,
                )?;
                authorized_bindings.push(AuthorizedInboundEffect {
                    transaction_index: effect.transaction_index(),
                    observation,
                });
            }
        }
    }

    if let Some(candidate) = candidates.next() {
        return Err(InboundEffectError::UnexpectedCandidate {
            transaction_index: candidate.transaction_index,
        });
    }
    if let Some(observation) = outbound_observations.next() {
        return Err(unexpected_outbound_observation(observation));
    }
    Ok(AuthorizedInboundEffects {
        bindings: authorized_bindings,
    })
}

fn authorize_inbound_effect(
    effect: &BoundEffect<'_>,
    observation: &InboundObservation,
    outbound_observations: &[&OutboundEventObservation],
    expected_rollup_id: NonZeroU64,
    expected_l2_system_address: Address,
) -> Result<(), InboundEffectError> {
    let entry_index = effect.entry_index();
    let entry = effect.claimed_entry();
    if entry.destinationRollupId != expected_rollup_id.get() {
        return Err(InboundEffectError::DestinationRollupMismatch {
            entry_index,
            expected: expected_rollup_id.get(),
            actual: entry.destinationRollupId,
        });
    }
    if entry.proxyEntryHash != observation.recomputed_call_hash {
        return Err(InboundEffectError::CallHashMismatch {
            entry_index,
            recomputed: observation.recomputed_call_hash,
            claimed: entry.proxyEntryHash,
        });
    }
    let expected_delivery_return = if entry.success {
        entry.returnData.as_ref()
    } else {
        &[]
    };
    if observation.return_data.as_ref() != expected_delivery_return {
        return Err(InboundEffectError::ReturnDataMismatch { entry_index });
    }
    let max_inbound_value = (U256::from(1) << 191) - U256::from(1);
    if observation.value > max_inbound_value {
        return Err(InboundEffectError::ValueOutOfRange {
            entry_index,
            transaction_index: effect.transaction_index(),
            value: observation.value,
        });
    }
    let verification = verify_nested_pair(
        entry,
        observation.derived_da_entry.as_entry(),
        NestedPairDirection::Inbound,
        RollupId(expected_rollup_id.get()),
    )
    .map_err(|error| InboundEffectError::ExecutionTableMismatch {
        entry_index,
        reason: error.to_string(),
    })?;
    if entry
        .l2ToL1Calls
        .iter()
        .chain(
            entry
                .expectedL1ToL2Calls
                .iter()
                .flat_map(|row| row.l2ToL1Calls.iter()),
        )
        .filter(|call| call.sourceRollupId == expected_rollup_id.get())
        .any(|call| call.sourceAddress == expected_l2_system_address)
    {
        return Err(InboundEffectError::ReservedSourceAddress { entry_index });
    }
    if outbound_observations.len() != verification.l2_outbound_hashes.len() {
        return Err(InboundEffectError::OutboundObservationCountMismatch {
            entry_index,
            transaction_index: effect.transaction_index(),
            expected: verification.l2_outbound_hashes.len(),
            actual: outbound_observations.len(),
        });
    }
    for (event_index, (outbound, recomputed)) in outbound_observations
        .iter()
        .zip(&verification.l2_outbound_hashes)
        .enumerate()
    {
        let decoded =
            outbound
                .decoded_event()
                .ok_or(InboundEffectError::MalformedOutboundObservation {
                    transaction_index: outbound.transaction_index(),
                    receipt_log_index: outbound.receipt_log_index(),
                })?;
        if decoded.call_gas() != 0 {
            return Err(InboundEffectError::UnsupportedCallGas {
                transaction_index: outbound.transaction_index(),
                receipt_log_index: outbound.receipt_log_index(),
                actual: decoded.call_gas(),
            });
        }
        if decoded.call_hash() != *recomputed {
            return Err(InboundEffectError::NestedCallHashMismatch {
                entry_index,
                event_index,
                transaction_index: outbound.transaction_index(),
                recomputed: *recomputed,
                observed: decoded.call_hash(),
            });
        }
    }
    let update = effect.claimed_root_update();
    let expected = verification.ether_delta;
    let actual = I256::from(update.etherDelta);
    if actual != expected {
        return Err(InboundEffectError::EtherDeltaMismatch {
            entry_index,
            expected,
            actual,
        });
    }
    Ok(())
}

fn unexpected_outbound_observation(observation: &OutboundEventObservation) -> InboundEffectError {
    InboundEffectError::UnexpectedOutboundObservation {
        transaction_index: observation.transaction_index(),
        receipt_log_index: observation.receipt_log_index(),
    }
}
