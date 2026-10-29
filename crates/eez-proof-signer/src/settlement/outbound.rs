//! Outbound event binding and settlement-effect authorization.

use std::fmt;
use std::num::NonZeroU64;

use alloy_primitives::{Address, B256, I256, U256};
use alloy_sol_types::{SolCall as _, SolValue as _};
use eez_protocol::RollupId;
use eez_protocol::abi::{ExecutionEntrySol, loadExecutionTableCall};
use eez_protocol::entries::{NestedPairVerification, verify_outbound_source_group};
use eez_protocol::system_tx::{
    OutboundSourceEntry, check_entry_shape, check_l2_static_entry_shape, committed_l2_table_value,
    lift_entry_from_l2,
};
use thiserror::Error;

use super::effect_binding::{BoundEffect, BoundEffectSequence, EffectKind};
use crate::validate::OutboundEventObservation;

/// One selector-level source-table load and its strict decoding result.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OutboundLoadCandidate {
    pub(crate) transaction_index: usize,
    pub(crate) inspection: Result<LoadedOutboundEntries, OutboundLoadObservationError>,
}

#[cfg(test)]
impl OutboundLoadCandidate {
    pub(crate) fn decoded_for_test(transaction_index: usize, entry: ExecutionEntrySol) -> Self {
        Self {
            transaction_index,
            inspection: Ok(LoadedOutboundEntries(vec![OutboundSourceEntry::Mutable(
                entry,
            )])),
        }
    }

    pub(crate) fn decoded_many_for_test(
        transaction_index: usize,
        entries: Vec<ExecutionEntrySol>,
    ) -> Self {
        Self {
            transaction_index,
            inspection: Ok(LoadedOutboundEntries(
                entries
                    .into_iter()
                    .map(OutboundSourceEntry::Mutable)
                    .collect(),
            )),
        }
    }
}

/// Canonically comparable wrapper for an ABI-generated entry type.
pub(crate) struct LoadedOutboundEntries(Vec<OutboundSourceEntry>);

impl LoadedOutboundEntries {
    pub(super) fn as_entries(&self) -> &[OutboundSourceEntry] {
        &self.0
    }

    fn encoded(&self) -> Vec<Vec<u8>> {
        self.0
            .iter()
            .map(|entry| match entry {
                OutboundSourceEntry::Mutable(entry) => entry.abi_encode(),
                OutboundSourceEntry::Static(entry) => entry.abi_encode(),
            })
            .collect()
    }
}

impl fmt::Debug for LoadedOutboundEntries {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("LoadedOutboundEntry")
            .field(&self.encoded())
            .finish()
    }
}

impl PartialEq for LoadedOutboundEntries {
    fn eq(&self, other: &Self) -> bool {
        self.encoded() == other.encoded()
    }
}

impl Eq for LoadedOutboundEntries {}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub(crate) enum OutboundLoadObservationError {
    #[error("top-level load transaction reverted")]
    RevertedTransaction,
    #[error("load transaction value is {actual}; committed L2 table value is {expected}")]
    NativeValueMismatch { expected: U256, actual: U256 },
    #[error("load calldata does not decode: {reason}")]
    InvalidAbi { reason: String },
    #[error("load calldata is not its canonical complete ABI encoding")]
    NonCanonicalAbi,
    #[error("load calldata carries no execution entries")]
    EntryCount { actual: usize },
    #[error("load execution entry is invalid: {reason}")]
    InvalidEntry { reason: String },
}

/// Decode the exact L2 source entry staged by one system load transaction.
pub(super) fn inspect_outbound_load_candidate(
    transaction_value: U256,
    calldata: &[u8],
    transaction_succeeded: bool,
) -> Result<LoadedOutboundEntries, OutboundLoadObservationError> {
    if !transaction_succeeded {
        return Err(OutboundLoadObservationError::RevertedTransaction);
    }
    let call = loadExecutionTableCall::abi_decode(calldata).map_err(|error| {
        OutboundLoadObservationError::InvalidAbi {
            reason: error.to_string(),
        }
    })?;
    if call.abi_encode().as_slice() != calldata {
        return Err(OutboundLoadObservationError::NonCanonicalAbi);
    }
    if call._entries.is_empty() && call._staticEntries.is_empty() {
        return Err(OutboundLoadObservationError::EntryCount { actual: 0 });
    }
    // The load funds every call committed to execute on L2, including
    // value-bearing callbacks in reentrant rows, exactly as the shared builder
    // prices it. Static sources commit no incoming value.
    let committed = committed_l2_table_value(&call._entries)
        .map_err(|reason| OutboundLoadObservationError::InvalidEntry { reason })?;
    if transaction_value != committed {
        return Err(OutboundLoadObservationError::NativeValueMismatch {
            expected: committed,
            actual: transaction_value,
        });
    }
    let mut sidecars = Vec::with_capacity(call._entries.len() + call._staticEntries.len());
    for entry in &call._entries {
        let sidecar = lift_entry_from_l2(entry, RollupId::MAINNET);
        check_entry_shape(&sidecar, "outbound load")
            .map_err(|reason| OutboundLoadObservationError::InvalidEntry { reason })?;
        if sidecar.proxyEntryHash == B256::ZERO {
            return Err(OutboundLoadObservationError::InvalidEntry {
                reason: "source entry proxy hash is zero".into(),
            });
        }
        sidecars.push(OutboundSourceEntry::Mutable(sidecar));
    }
    for entry in &call._staticEntries {
        let sidecar = entry.clone();
        check_l2_static_entry_shape(&sidecar, "outbound static load")
            .map_err(|reason| OutboundLoadObservationError::InvalidEntry { reason })?;
        sidecars.push(OutboundSourceEntry::Static(sidecar));
    }
    Ok(LoadedOutboundEntries(sidecars))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum OutboundEffectError {
    #[error(
        "outbound effect entry {entry_index} at transaction {transaction_index} is not immediately preceded by a system transaction"
    )]
    MissingPrecedingSystemTransaction {
        entry_index: usize,
        transaction_index: usize,
    },
    #[error(
        "outbound effect entry {entry_index} has no source-table load at transaction {transaction_index}"
    )]
    MissingLoadCandidate {
        entry_index: usize,
        transaction_index: usize,
    },
    #[error("source-table load at transaction {transaction_index} has no matching outbound effect")]
    UnexpectedLoadCandidate { transaction_index: usize },
    #[error(
        "source-table load for entry {entry_index} at transaction {transaction_index} is invalid: {source}"
    )]
    InvalidLoadCandidate {
        entry_index: usize,
        transaction_index: usize,
        source: OutboundLoadObservationError,
    },
    #[error(
        "outbound effect entry {entry_index} at transaction {transaction_index} follows an inbound effect"
    )]
    NonCanonicalEffectOrder {
        entry_index: usize,
        transaction_index: usize,
    },
    #[error(
        "outbound effect entry {entry_index} at transaction {transaction_index} has no matching event"
    )]
    MissingObservation {
        entry_index: usize,
        transaction_index: usize,
    },
    #[error(
        "outbound event at transaction {transaction_index}, receipt log {receipt_log_index}, has no matching effect"
    )]
    UnexpectedObservation {
        transaction_index: usize,
        receipt_log_index: usize,
    },
    #[error(
        "outbound event at transaction {transaction_index}, receipt log {receipt_log_index}, is malformed"
    )]
    MalformedObservation {
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
        "outbound entry {entry_index} and its source table are structurally inconsistent: {reason}"
    )]
    ExecutionTableMismatch { entry_index: usize, reason: String },
    #[error(
        "outbound entry {entry_index} expects {expected} L2 event(s), but transaction {transaction_index} emitted {actual}"
    )]
    ObservationCountMismatch {
        entry_index: usize,
        transaction_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error(
        "outbound entry {entry_index} event {event_index} recomputes hash {recomputed}; transaction {transaction_index} emitted {observed}"
    )]
    NestedCallHashMismatch {
        entry_index: usize,
        event_index: usize,
        transaction_index: usize,
        recomputed: B256,
        observed: B256,
    },
    #[error("outbound entry {entry_index} targets rollup {actual}; expected rollup {expected}")]
    DestinationRollupMismatch {
        entry_index: usize,
        expected: u64,
        actual: u64,
    },
    #[error(
        "outbound entry {entry_index} call has source rollup {actual}; expected rollup {expected}"
    )]
    SourceRollupMismatch {
        entry_index: usize,
        expected: u64,
        actual: u64,
    },
    #[error("outbound entry {entry_index} uses the reserved system account as its source")]
    ReservedSourceAddress { entry_index: usize },
    #[error("outbound entry {entry_index} value {value} exceeds the int192 outflow range")]
    ValueOutOfRange { entry_index: usize, value: U256 },
    #[error(
        "outbound entry {entry_index} has ether delta {actual}; expected released value delta {expected}"
    )]
    EtherDeltaMismatch {
        entry_index: usize,
        expected: I256,
        actual: I256,
    },
}

impl OutboundEffectError {
    /// Return the user transaction responsible for an execution observation
    /// failure. Structural and claim-only mismatches are not eviction hints.
    pub(crate) const fn poisoned_transaction_index(&self) -> Option<usize> {
        match self {
            Self::MissingObservation {
                transaction_index, ..
            }
            | Self::MalformedObservation {
                transaction_index, ..
            }
            | Self::UnsupportedCallGas {
                transaction_index, ..
            } => Some(*transaction_index),
            Self::MissingPrecedingSystemTransaction { .. }
            | Self::MissingLoadCandidate { .. }
            | Self::UnexpectedLoadCandidate { .. }
            | Self::InvalidLoadCandidate { .. }
            | Self::NonCanonicalEffectOrder { .. }
            | Self::UnexpectedObservation { .. }
            | Self::ExecutionTableMismatch { .. }
            | Self::ObservationCountMismatch { .. }
            | Self::NestedCallHashMismatch { .. }
            | Self::DestinationRollupMismatch { .. }
            | Self::SourceRollupMismatch { .. }
            | Self::ReservedSourceAddress { .. }
            | Self::ValueOutOfRange { .. }
            | Self::EtherDeltaMismatch { .. } => None,
        }
    }
}

/// Outbound effects bound to positioned EEZL2 events and exact loaded L2
/// source-table sidecars.
///
/// `verify_da_payload` binds each projection to its canonical `[load, user]`
/// transaction pair.
/// Only [`authorize_outbound_effects`] can construct this in production.
#[cfg_attr(test, derive(Default))]
pub(crate) struct AuthorizedOutboundEffects {
    bindings: Vec<AuthorizedOutboundEffect>,
}

/// One authorized effect: the preceding system-load position, the user
/// transaction position, and the complete source entry carried in DA.
pub(super) struct AuthorizedOutboundEffect {
    claimed_entry_index: usize,
    claimed_entry: ExecutionEntrySol,
    load_transaction_index: usize,
    user_transaction_index: usize,
    derived_da_entry: OutboundSourceEntry,
}

impl AuthorizedOutboundEffects {
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.bindings.len()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &AuthorizedOutboundEffect> {
        self.bindings.iter()
    }

    pub(super) fn groups(&self) -> impl Iterator<Item = &[AuthorizedOutboundEffect]> {
        self.bindings.chunk_by(|left, right| {
            left.load_transaction_index == right.load_transaction_index
                && left.user_transaction_index == right.user_transaction_index
        })
    }
}

impl AuthorizedOutboundEffect {
    pub(super) const fn claimed_entry_index(&self) -> usize {
        self.claimed_entry_index
    }

    pub(super) const fn claimed_entry(&self) -> &ExecutionEntrySol {
        &self.claimed_entry
    }

    pub(super) const fn load_transaction_index(&self) -> usize {
        self.load_transaction_index
    }

    pub(super) const fn transaction_index(&self) -> usize {
        self.user_transaction_index
    }

    #[cfg(test)]
    pub(super) fn derived_da_entry(&self) -> &ExecutionEntrySol {
        self.derived_da_entry
            .as_mutable()
            .expect("mutable outbound effect has a mutable DA entry")
    }

    pub(super) const fn derived_source_entry(&self) -> &OutboundSourceEntry {
        &self.derived_da_entry
    }
}

/// Authorize outbound effects from re-executed transaction events and
/// settlement entries.
///
/// Each outbound effect must occupy a user transaction immediately after a
/// system load. The function enforces outbound-before-inbound ordering, exact
/// recursive agreement between the L1 host and loaded L2 source table, one
/// ordered event per L2-origin call, non-system call sources, and exact net
/// ether accounting.
pub(crate) fn authorize_outbound_effects(
    bound_effects: &BoundEffectSequence<'_, '_>,
    expected_rollup_id: NonZeroU64,
    expected_l2_system_address: Address,
) -> Result<AuthorizedOutboundEffects, OutboundEffectError> {
    let observations = bound_effects
        .settling_observations()
        .outbound_event_candidates();
    let mut consumed_observations = vec![false; observations.len()];
    let loads = bound_effects
        .settling_observations()
        .outbound_load_candidates();
    let mut authorized_bindings = Vec::with_capacity(bound_effects.outbound_count());
    let mut saw_inbound = false;

    for effect in bound_effects.effects() {
        if let Some((_, observation)) =
            observations
                .iter()
                .enumerate()
                .find(|(index, observation)| {
                    !consumed_observations[*index]
                        && observation.transaction_index() < effect.user_transaction_index()
                })
        {
            return Err(unexpected_outbound_observation(observation));
        }
        match effect.kind() {
            EffectKind::Inbound => {
                saw_inbound = true;
                // An inbound call tree may itself call back to L1. Those
                // EEZL2 events belong to the inbound pair verifier, which has
                // the complementary L1/L2 tables needed to authorize them.
                for (index, observation) in observations.iter().enumerate() {
                    if observation.transaction_index() == effect.transaction_index() {
                        consumed_observations[index] = true;
                    }
                }
                if let Some(load) = loads
                    .iter()
                    .find(|load| load.transaction_index >= effect.start_transaction_index())
                {
                    return Err(OutboundEffectError::UnexpectedLoadCandidate {
                        transaction_index: load.transaction_index,
                    });
                }
            }
            EffectKind::Outbound => {
                if saw_inbound {
                    return Err(OutboundEffectError::NonCanonicalEffectOrder {
                        entry_index: effect.entry_index(),
                        transaction_index: effect.transaction_index(),
                    });
                }
                let load_transaction_index = effect.start_transaction_index();
                if bound_effects
                    .settling_observations()
                    .system_sender_flags()
                    .get(load_transaction_index)
                    .copied()
                    != Some(true)
                {
                    return Err(OutboundEffectError::MissingPrecedingSystemTransaction {
                        entry_index: effect.entry_index(),
                        transaction_index: effect.transaction_index(),
                    });
                }
                let load = loads
                    .iter()
                    .find(|load| load.transaction_index == load_transaction_index)
                    .ok_or(OutboundEffectError::MissingLoadCandidate {
                        entry_index: effect.entry_index(),
                        transaction_index: load_transaction_index,
                    })?;
                let loaded = load.inspection.as_ref().map_err(|source| {
                    OutboundEffectError::InvalidLoadCandidate {
                        entry_index: effect.entry_index(),
                        transaction_index: load_transaction_index,
                        source: source.clone(),
                    }
                })?;
                let loaded_entries = loaded.as_entries();
                let source_start = effect.source_entry_start();
                let source_end = source_start
                    .checked_add(effect.source_entry_count())
                    .filter(|end| *end <= loaded_entries.len())
                    .ok_or_else(|| OutboundEffectError::ExecutionTableMismatch {
                        entry_index: effect.entry_index(),
                        reason: format!(
                            "load at {load_transaction_index} has no source range {source_start}..{}",
                            source_start.saturating_add(effect.source_entry_count()),
                        ),
                    })?;
                let source_entries = &loaded_entries[source_start..source_end];
                // The static pool is shared by the complete user transaction.
                // Authorizing only a slice could miss an earlier conflicting
                // lookup or assign mutable receipt events to static rows.
                if loaded_entries.len() > 1
                    && loaded_entries
                        .iter()
                        .any(|entry| matches!(entry, OutboundSourceEntry::Static(_)))
                    && (source_start != 0 || source_end != loaded_entries.len())
                {
                    return Err(OutboundEffectError::ExecutionTableMismatch {
                        entry_index: effect.entry_index(),
                        reason: "mixed outbound host must cover the complete source load".into(),
                    });
                }
                let verification = verify_outbound_effect_table(
                    effect,
                    source_entries,
                    expected_rollup_id,
                    expected_l2_system_address,
                )?;
                // Source-table slots count roots, not receipt events: one
                // nested root can emit several events. Consume the verified
                // event count independently for each host of this user tx.
                // The final host owns every remaining event, so surplus
                // observations fail the exact count check as well.
                let event_limit = if source_end == loaded_entries.len() {
                    usize::MAX
                } else {
                    verification.l2_outbound_hashes.len()
                };
                let mut effect_observations = Vec::new();
                for (index, observation) in observations.iter().enumerate() {
                    if !consumed_observations[index]
                        && observation.transaction_index() == effect.user_transaction_index()
                        && effect_observations.len() < event_limit
                    {
                        consumed_observations[index] = true;
                        effect_observations.push(observation);
                    }
                }
                authorize_outbound_observations(effect, &verification, &effect_observations)?;
                for source_entry in source_entries {
                    authorized_bindings.push(AuthorizedOutboundEffect {
                        claimed_entry_index: effect.entry_index(),
                        claimed_entry: effect.claimed_entry().clone(),
                        load_transaction_index,
                        user_transaction_index: effect.user_transaction_index(),
                        derived_da_entry: source_entry.clone(),
                    });
                }
            }
        }
    }

    if let Some((_, observation)) = observations
        .iter()
        .enumerate()
        .find(|(index, _)| !consumed_observations[*index])
    {
        return Err(unexpected_outbound_observation(observation));
    }
    Ok(AuthorizedOutboundEffects {
        bindings: authorized_bindings,
    })
}

fn unexpected_outbound_observation(observation: &OutboundEventObservation) -> OutboundEffectError {
    OutboundEffectError::UnexpectedObservation {
        transaction_index: observation.transaction_index(),
        receipt_log_index: observation.receipt_log_index(),
    }
}

fn verify_outbound_effect_table(
    effect: &BoundEffect<'_>,
    source_entries: &[OutboundSourceEntry],
    expected_rollup_id: NonZeroU64,
    expected_l2_system_address: Address,
) -> Result<NestedPairVerification, OutboundEffectError> {
    let entry_index = effect.entry_index();
    let entry = effect.claimed_entry();
    if entry.destinationRollupId != expected_rollup_id.get() {
        return Err(OutboundEffectError::DestinationRollupMismatch {
            entry_index,
            expected: expected_rollup_id.get(),
            actual: entry.destinationRollupId,
        });
    }
    if let Some(call) = entry.l2ToL1Calls.first()
        && call.sourceRollupId != expected_rollup_id.get()
    {
        return Err(OutboundEffectError::SourceRollupMismatch {
            entry_index,
            expected: expected_rollup_id.get(),
            actual: call.sourceRollupId,
        });
    }
    for call in entry.l2ToL1Calls.iter().chain(
        entry
            .expectedL1ToL2Calls
            .iter()
            .flat_map(|row| row.l2ToL1Calls.iter()),
    ) {
        let max_outbound_value = U256::from(1) << 191;
        if call.sourceRollupId == expected_rollup_id.get() && call.value > max_outbound_value {
            return Err(OutboundEffectError::ValueOutOfRange {
                entry_index,
                value: call.value,
            });
        }
    }
    let verification =
        verify_outbound_source_group(entry, source_entries, RollupId(expected_rollup_id.get()))
            .map_err(|error| OutboundEffectError::ExecutionTableMismatch {
                entry_index,
                reason: error.to_string(),
            })?;

    let l2_calls = entry.l2ToL1Calls.iter().chain(
        entry
            .expectedL1ToL2Calls
            .iter()
            .flat_map(|row| row.l2ToL1Calls.iter()),
    );
    if l2_calls
        .filter(|call| call.sourceRollupId == expected_rollup_id.get())
        .any(|call| call.sourceAddress == expected_l2_system_address)
    {
        return Err(OutboundEffectError::ReservedSourceAddress { entry_index });
    }
    Ok(verification)
}

fn authorize_outbound_observations(
    effect: &BoundEffect<'_>,
    verification: &NestedPairVerification,
    observations: &[&OutboundEventObservation],
) -> Result<(), OutboundEffectError> {
    let entry_index = effect.entry_index();
    if observations.is_empty() && !verification.l2_outbound_hashes.is_empty() {
        return Err(OutboundEffectError::MissingObservation {
            entry_index,
            transaction_index: effect.transaction_index(),
        });
    }
    if observations.len() != verification.l2_outbound_hashes.len() {
        return Err(OutboundEffectError::ObservationCountMismatch {
            entry_index,
            transaction_index: effect.transaction_index(),
            expected: verification.l2_outbound_hashes.len(),
            actual: observations.len(),
        });
    }
    for (event_index, (observation, recomputed)) in observations
        .iter()
        .zip(&verification.l2_outbound_hashes)
        .enumerate()
    {
        let decoded =
            observation
                .decoded_event()
                .ok_or(OutboundEffectError::MalformedObservation {
                    transaction_index: observation.transaction_index(),
                    receipt_log_index: observation.receipt_log_index(),
                })?;
        if decoded.call_gas() != 0 {
            return Err(OutboundEffectError::UnsupportedCallGas {
                transaction_index: observation.transaction_index(),
                receipt_log_index: observation.receipt_log_index(),
                actual: decoded.call_gas(),
            });
        }
        if decoded.call_hash() != *recomputed {
            return Err(OutboundEffectError::NestedCallHashMismatch {
                entry_index,
                event_index,
                transaction_index: observation.transaction_index(),
                recomputed: *recomputed,
                observed: decoded.call_hash(),
            });
        }
    }

    let update = effect.claimed_root_update();
    let expected_delta = verification.ether_delta;
    let actual = I256::from(update.etherDelta);
    if actual != expected_delta {
        return Err(OutboundEffectError::EtherDeltaMismatch {
            entry_index,
            expected: expected_delta,
            actual,
        });
    }
    Ok(())
}
