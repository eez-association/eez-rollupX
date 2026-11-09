//! Bounded structural admission of a v2 Block frame. Execution and signing
//! remain separate trust transitions; decoded claims never authorize either.
use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use eez_control_rpc::v2::{BlockWitness, ExecutionWitness as WireExecutionWitness};
use thiserror::Error;

/// Resource quotas for one structurally admitted window.
///
/// The byte quota bounds canonical known fields accounted across accepted
/// chunks, including fields discarded after checking. The item quota is
/// checked before witness conversion and bounds accounted collection
/// cardinality and downstream traversal. Neither quota can undo allocations
/// Prost made while decoding the current message; tonic's per-message limit
/// bounds that stage. These are service defenses, not protocol bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowLimits {
    pub(crate) blocks: usize,
    pub(crate) payload_bytes: usize,
    pub(crate) witness_items: usize,
}

/// One block retained after stream-level structural checks.
///
/// Admission checks its declared number, hash widths, witness presence, and
/// adjacency between streamed hash claims. Backend validation binds and checks
/// the RLP, hashes, and witness.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct AdmittedBlock {
    /// Composer-declared number, checked against the admitted window range.
    pub(crate) declared_number: u64,
    /// Composer-claimed hash; backend validation must re-derive it from RLP.
    pub(crate) claimed_hash: B256,
    /// Composer-claimed parent; admission compares it with the preceding
    /// streamed hash when one exists.
    pub(crate) claimed_parent_hash: B256,
    pub(crate) rlp: Vec<u8>,
    pub(crate) witness: ExecutionWitness,
    pub(crate) l1_headers_rlp: Vec<alloy_primitives::Bytes>,
}

impl AdmittedBlock {
    pub(crate) fn from_wire(
        submitted_block: BlockWitness,
        accounted_witness_items: &mut usize,
        limits: WindowLimits,
    ) -> Result<Self, WindowError> {
        let BlockWitness {
            number: declared_number,
            hash: claimed_hash_bytes,
            parent_hash: claimed_parent_hash_bytes,
            rlp,
            witness,
        } = submitted_block;
        let claimed_hash = B256::try_from(claimed_hash_bytes.as_slice()).map_err(|_| {
            WindowError::BlockHashLength {
                number: declared_number,
                actual: claimed_hash_bytes.len(),
            }
        })?;
        let claimed_parent_hash =
            B256::try_from(claimed_parent_hash_bytes.as_slice()).map_err(|_| {
                WindowError::ParentHashLength {
                    number: declared_number,
                    actual: claimed_parent_hash_bytes.len(),
                }
            })?;
        let wire_witness = witness.ok_or(WindowError::MissingWitness {
            number: declared_number,
        })?;
        charge(
            accounted_witness_items,
            wire_witness_item_count(&wire_witness),
            limits.witness_items,
            |attempted, max| WindowError::WitnessItemLimit { attempted, max },
        )?;
        let (witness, l1_headers_rlp) = into_execution_witness(wire_witness);
        Ok(Self {
            declared_number,
            claimed_hash,
            claimed_parent_hash,
            rlp,
            witness,
            l1_headers_rlp,
        })
    }

    /// Construct the minimal admitted block used by downstream unit tests.
    #[cfg(test)]
    pub(crate) fn test(
        declared_number: u64,
        claimed_parent_hash_byte: u8,
        claimed_hash_byte: u8,
    ) -> Self {
        Self {
            declared_number,
            claimed_hash: B256::repeat_byte(claimed_hash_byte),
            claimed_parent_hash: B256::repeat_byte(claimed_parent_hash_byte),
            rlp: Vec::new(),
            witness: ExecutionWitness::default(),
            l1_headers_rlp: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum WindowError {
    #[error("block {number} has a {actual}-byte hash")]
    BlockHashLength { number: u64, actual: usize },
    #[error("block {number} has a {actual}-byte parent hash")]
    ParentHashLength { number: u64, actual: usize },
    #[error("block {number} has no execution witness")]
    MissingWitness { number: u64 },
    #[error("witness item count {attempted} exceeds {max}")]
    WitnessItemLimit { attempted: usize, max: usize },
}

fn charge(
    current: &mut usize,
    add: usize,
    max: usize,
    over: impl Fn(usize, usize) -> WindowError,
) -> Result<(), WindowError> {
    let attempted = current
        .checked_add(add)
        .ok_or_else(|| over(usize::MAX, max))?;
    if attempted > max {
        return Err(over(attempted, max));
    }
    *current = attempted;
    Ok(())
}

/// Convert the protobuf witness into the Alloy type consumed by Stateless.
/// Converting each payload into `Bytes` reuses its source allocation.
fn into_execution_witness(
    witness: WireExecutionWitness,
) -> (ExecutionWitness, Vec<alloy_primitives::Bytes>) {
    let WireExecutionWitness {
        state,
        codes,
        keys,
        headers,
        l1_headers_rlp,
    } = witness;
    (
        ExecutionWitness {
            state: state.into_iter().map(Into::into).collect(),
            codes: codes.into_iter().map(Into::into).collect(),
            keys: keys.into_iter().map(Into::into).collect(),
            headers: headers.into_iter().map(Into::into).collect(),
        },
        l1_headers_rlp.into_iter().map(Into::into).collect(),
    )
}

/// Count protobuf witness items before allocating the retained representation.
pub(crate) fn wire_witness_item_count(witness: &WireExecutionWitness) -> usize {
    witness
        .state
        .len()
        .saturating_add(witness.codes.len())
        .saturating_add(witness.keys.len())
        .saturating_add(witness.headers.len())
        .saturating_add(witness.l1_headers_rlp.len())
}

#[cfg(test)]
mod tests;
