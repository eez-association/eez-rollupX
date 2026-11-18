//! M-of-N attestation: collect proofs for one settlement window from several
//! independent attesters.
//!
//! Each attester is one registered proof system with its own signer. The L1
//! rollup manager accepts a batch once it carries proofs from at least
//! `threshold` distinct proof systems (`Rollup.checkProofSystemsAndGetVkeys`),
//! so the manager enforces the threshold on-chain. It counts proof-system
//! contracts; independent attestation also requires independent signing keys.
//!
//! An attester's digest is `keccak256(sharedPublicInput || acc)`, where `acc`
//! folds only `(rollupId, vkey)` of that proof system. It does not depend on
//! which other proof systems the batch lists or at which index, so each
//! attester is asked to prove a copy of the batch naming only its own proof
//! system, and its proof stays valid in the final batch that lists every
//! attester that answered.
//!
//! `EEZ.sol` reverts the whole batch on one invalid proof, so every proof is
//! checked here against the digest the composer computes itself before it is
//! counted, and an attester whose registration cannot be read sits the batch
//! out: one faulty or hostile attester must not be able to stall settlement
//! for the others.

use std::{
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use alloy_primitives::{Address, B256, Bytes};
use alloy_sol_types::SolCall;
use eez_protocol::{EvmBatch, abi::RollupIdWithProofSystemsSol};
use eez_prover::{Prover, ProverError, ProvingContext, RetryableProverError};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use tokio::time::{Instant, timeout_at};
use tracing::{Level, event};

use crate::prover_retry::ProofBudget;

/// Upper bound on the attester set. Every proof system is a verify call the
/// batch pays for, and the gas floor the composer accepts is sized for this
/// many.
pub const MAX_ATTESTERS: usize = 16;

/// Native cold verifier/manager/accumulator pin. The Solidity ladder covers
/// 1, 2, 4, 8 and 16 systems, 0/4 outbound entries and 0/64 blob references.
/// Worst measured marginal: 12,370 gas; 14,000 retains more than 10% slack.
const ADDITIONAL_VERIFIER_GAS_PIN: u64 = 14_000;

pub(crate) fn additional_verifier_gas(members: usize) -> u64 {
    (members.saturating_sub(1) as u64).saturating_mul(ADDITIONAL_VERIFIER_GAS_PIN)
}

/// Each additional system adds one address, one assignment index, one proof
/// offset, one length word and a 96-byte padded signature. Existing metadata
/// already prices every byte nonzero, including top-level offset growth.
pub(crate) fn additional_verifier_calldata_gas(members: usize) -> u64 {
    (members.saturating_sub(1) as u64).saturating_mul(224 * 16)
}

/// One registered attester: the proof system its proof is verified by, the key
/// that signs it, and the prover that produces it.
#[derive(Debug, Clone)]
pub struct QuorumMember {
    /// The `ECDSAProofSystem` contract this attester's proof is checked against.
    pub proof_system: Address,
    /// The signer that proof system accepts.
    pub attester: Address,
    /// The prover that returns this attester's proof.
    pub prover: Arc<dyn Prover>,
}

/// A proof and the proof system that verifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    /// The proof system the proof belongs to.
    pub proof_system: Address,
    /// The proof bytes `IProofSystem.verify` accepts.
    pub proof: Bytes,
}

/// A malformed attester set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuorumConfigError {
    /// No attester was configured.
    #[error("at least one attester is required")]
    Empty,
    /// More attesters than [`MAX_ATTESTERS`].
    #[error("{count} attesters configured, at most {MAX_ATTESTERS} are supported")]
    TooMany {
        /// Number configured.
        count: usize,
    },
    /// A proof system address is zero.
    #[error("attester {index} has the zero proof-system address")]
    ZeroProofSystem {
        /// Position in the configured list.
        index: usize,
    },
    /// An attester address is zero.
    #[error("attester {index} has the zero signer address")]
    ZeroAttester {
        /// Position in the configured list.
        index: usize,
    },
    /// Counting one ECDSA key twice would defeat independent attestation.
    #[error("duplicate attester {0}")]
    DuplicateAttester(Address),
    /// Two attesters share a proof system, which the rollup manager counts once.
    #[error("proof system {proof_system} is configured more than once")]
    DuplicateProofSystem {
        /// The repeated proof system.
        proof_system: Address,
    },
}

/// Why the manager's attestation settings for the next batch are unusable.
/// None of these is a candidate's fault, so the composer requeues the window
/// rather than charging its transactions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RegistrationError {
    #[error("registry returned a different rollup manager")]
    ManagerMismatch,
    #[error("threshold() unavailable or malformed")]
    Threshold,
    /// Fewer attesters are usable than the manager requires.
    #[error(
        "attestation threshold {threshold} is not reachable: {usable} of the {configured} configured attesters are usable"
    )]
    Unreachable {
        /// Distinct proof systems the manager requires.
        threshold: usize,
        /// Configured attesters this batch can ask.
        usable: usize,
        /// Configured attesters.
        configured: usize,
    },
}

/// The attesters of one rollup and how their proofs are collected.
#[derive(Debug, Clone)]
pub struct AttestationQuorum {
    /// Sorted by strictly ascending proof system, the order the registry
    /// requires for `batch.proofSystems`.
    members: Vec<QuorumMember>,
    manager: Address,
    runtime: reth_tasks::Runtime,
    registration_worker: Arc<tokio::sync::Semaphore>,
    // A new window preempts diagnostics from the previous window.
    diagnostics: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl AttestationQuorum {
    /// Build an attester set. Members may be given in any order; they are
    /// sorted by proof system.
    ///
    /// # Errors
    ///
    /// Returns [`QuorumConfigError`] for an empty or oversized set, a zero
    /// address, or a proof system configured twice.
    pub fn new(
        mut members: Vec<QuorumMember>,
        manager: Address,
        runtime: reth_tasks::Runtime,
    ) -> Result<Self, QuorumConfigError> {
        if members.is_empty() {
            return Err(QuorumConfigError::Empty);
        }
        if members.len() > MAX_ATTESTERS {
            return Err(QuorumConfigError::TooMany {
                count: members.len(),
            });
        }
        if let Some(index) = members.iter().position(|m| m.proof_system == Address::ZERO) {
            return Err(QuorumConfigError::ZeroProofSystem { index });
        }
        if let Some(index) = members.iter().position(|m| m.attester == Address::ZERO) {
            return Err(QuorumConfigError::ZeroAttester { index });
        }
        members.sort_by_key(|m| m.proof_system);
        if let Some(pair) = members
            .windows(2)
            .find(|w| w[0].proof_system == w[1].proof_system)
        {
            return Err(QuorumConfigError::DuplicateProofSystem {
                proof_system: pair[0].proof_system,
            });
        }
        let mut signers = std::collections::HashSet::new();
        for member in &members {
            if !signers.insert(member.attester) {
                return Err(QuorumConfigError::DuplicateAttester(member.attester));
            }
        }
        Ok(Self {
            members,
            manager,
            runtime,
            diagnostics: Arc::default(),
            registration_worker: Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }

    /// The configured proof systems, strictly ascending.
    pub fn proof_systems(&self) -> impl Iterator<Item = Address> + '_ {
        self.members.iter().map(|m| m.proof_system)
    }

    /// Number of configured attesters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Always false: an attester set is never empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Resolve the same usable set in request-local native estimation state.
    /// The caller pins every read to one immutable L1 snapshot.
    pub(crate) fn usable_members(
        &self,
        manager: Address,
        supported: impl FnMut(Address) -> bool,
        read: impl FnMut(Address, Vec<u8>) -> Result<Bytes, String>,
    ) -> Result<Vec<(Address, B256)>, ProverError> {
        let registration = self
            .local_registration(manager, supported, read)
            .map_err(|e| ProverError::Backend(e.to_string()))?;
        Ok(self
            .members
            .iter()
            .zip(registration.vkeys)
            .filter_map(|(member, key)| Some((member.proof_system, key?)))
            .take(registration.threshold)
            .collect())
    }

    pub(crate) fn local_registration(
        &self,
        manager: Address,
        mut supported: impl FnMut(Address) -> bool,
        mut read: impl FnMut(Address, Vec<u8>) -> Result<Bytes, String>,
    ) -> Result<QuorumRegistration, RegistrationError> {
        if manager != self.manager {
            return Err(RegistrationError::ManagerMismatch);
        }
        let threshold = decode::<IRollupAttestationReader::thresholdCall>(read(
            manager,
            IRollupAttestationReader::thresholdCall {}.abi_encode(),
        ))
        .map_err(|_| RegistrationError::Threshold)?;
        let threshold = usize::try_from(threshold).unwrap_or(usize::MAX).max(1);
        let keys = self
            .members
            .iter()
            .map(|member| {
                if !supported(member.proof_system) {
                    return None;
                }
                let key = decode::<IRollupAttestationReader::verificationKeyCall>(read(
                    manager,
                    IRollupAttestationReader::verificationKeyCall {
                        proofSystem: member.proof_system,
                    }
                    .abi_encode(),
                ));
                let signer = decode::<IEcdsaProofSystemReader::signerCall>(read(
                    member.proof_system,
                    IEcdsaProofSystemReader::signerCall {}.abi_encode(),
                ));
                usable_vkey(member, manager, key, signer)
            })
            .collect();
        QuorumRegistration::new(threshold, keys)
    }

    /// Check the embedded canonical head before opening the production gate.
    pub async fn startup_policy(
        &self,
        local: &crate::LocalChainClient,
        registry: Address,
        rollup_id: u64,
    ) -> Result<(usize, Vec<Address>), ProverError> {
        let registration = self
            .policy_at(local.chain_provider(), registry, rollup_id, None)
            .await?;
        let systems = self
            .members
            .iter()
            .zip(registration.vkeys)
            .filter_map(|(member, key)| key.map(|_| member.proof_system))
            .collect();
        Ok((registration.threshold, systems))
    }
}

impl AttestationQuorum {
    /// Use the embedded parent state for startup and submission. The worker keeps
    /// its permit through cancellation so a slow state read cannot accumulate
    /// abandoned blocking tasks across slots.
    pub(crate) async fn policy_at(
        &self,
        chain: &crate::local::provider::ChainProvider,
        registry: Address,
        rollup_id: u64,
        parent: Option<alloy_consensus::Header>,
    ) -> Result<QuorumRegistration, ProverError> {
        let permit = Arc::clone(&self.registration_worker)
            .try_acquire_owned()
            .map_err(|_| ProverError::Backend("previous registration read still running".into()))?;
        let chain = chain.clone();
        let quorum = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let header = match parent {
                Some(header) => header,
                None => {
                    let missing =
                        || ProverError::Backend("embedded startup anchor unavailable".into());
                    let height = chain.headers.best_block_number().map_err(|_| missing())?;
                    chain
                        .headers
                        .header_by_number(height)
                        .map_err(|_| missing())?
                        .ok_or_else(missing)?
                }
            };
            crate::composer::proof_registration(&chain, &quorum, registry, rollup_id, &header)
        })
        .await
        .map_err(|_| ProverError::Backend("registration worker stopped".into()))?
    }

    fn expected_hashes(
        &self,
        ctx: &ProvingContext,
        registration: &QuorumRegistration,
        blobs: &[B256],
    ) -> Result<Vec<Option<B256>>, ProverError> {
        let (systems, keys): (Vec<_>, Vec<_>) = self
            .members
            .iter()
            .zip(&registration.vkeys)
            .filter_map(|(member, key)| Some((member.proof_system, (*key)?)))
            .unzip();
        let mut batch = ctx.batch.clone();
        assign_proof_systems(&mut batch, ctx.rollup_id, systems);
        let mut hashes = member_digests(&batch, &keys, blobs, ctx.proof_context)?.into_iter();
        Ok(registration
            .vkeys
            .iter()
            .map(|key| key.and_then(|_| hashes.next()))
            .collect())
    }
    pub(crate) async fn collect_proofs(
        &self,
        ctx: &ProvingContext,
        blobs: &[B256],
        registration: &QuorumRegistration,
        budget: ProofBudget,
    ) -> Result<Vec<Attestation>, ProverError> {
        let threshold = registration.threshold;
        let usable = registration.usable();
        // The sidecar has already committed these physical blobs. Reuse its versioned hashes.
        let started = Instant::now();
        let expected = self.expected_hashes(ctx, registration, blobs)?;
        let complete = Arc::new(AtomicBool::new(false));
        // Replacing the sender closes only the older round's diagnostic receiver,
        // even if an older foreground collection hands off after a newer one.
        let (owner, mut superseded) = tokio::sync::oneshot::channel();
        *self
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(owner);
        let mut outstanding: std::collections::BTreeSet<_> = self
            .members
            .iter()
            .zip(&expected)
            .filter_map(|(member, hash)| hash.map(|_| member.proof_system))
            .collect();
        let mut pending: FuturesUnordered<_> = self
            .members
            .iter()
            .zip(expected.iter().copied())
            .filter_map(|(member, hash)| Some((member.clone(), hash?)))
            .map(|(member, expected)| {
                let request = single_attester_context(ctx, member.proof_system);
                let complete = Arc::clone(&complete);
                async move {
                    let result = AssertUnwindSafe(prove_verified(
                        &member, expected, request, budget, &complete,
                    ))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err(ProverError::Backend("attester task panicked".into())));
                    (member.proof_system, result)
                }
            })
            .collect();
        let terminal = ctx.to_block;
        let terminal_hash = ctx
            .blocks
            .last()
            .map_or_else(|| ctx.parent.hash_slow(), |block| block.hash);
        let mut attestations = Vec::with_capacity(threshold);
        while attestations.len() < threshold {
            if attestations.len() + pending.len() < threshold {
                return Err(deadline_error());
            }
            let (system, result) = timeout_at(budget.expires_at(), pending.next())
                .await
                .map_err(|_| deadline_error())?
                .ok_or_else(deadline_error)?;
            if Instant::now() >= budget.expires_at() {
                return Err(deadline_error());
            }
            outstanding.remove(&system);
            log_member(
                system,
                terminal,
                terminal_hash,
                started,
                result.as_ref().err(),
                false,
            );
            if let Ok(proof) = result {
                attestations.push(Attestation {
                    proof_system: system,
                    proof,
                });
            }
        }
        // The foreground is the sole poller until this handoff. Freeze all retry
        // starts before moving the still-issued exchanges to the node runtime.
        complete.store(true, Ordering::Relaxed);
        if !pending.is_empty() {
            self.runtime.spawn_task(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut superseded => break,
                        _ = tokio::time::sleep_until(budget.expires_at()) => {
                            for system in outstanding {
                                log_member(system, terminal, terminal_hash, started, Some(&deadline_error()), true);
                            }
                            break;
                        }
                        next = pending.next() => {
                            let Some((system, result)) = next else { break };
                            outstanding.remove(&system);
                            log_member(system, terminal, terminal_hash, started, result.as_ref().err(), true);
                        }
                    }
                }
            });
        }
        attestations.sort_by_key(|a| a.proof_system);
        let systems = attestations
            .iter()
            .map(|a| a.proof_system)
            .collect::<Vec<_>>();
        let mut selected = ctx.batch.clone();
        let (keys, signed): (Vec<_>, Vec<_>) = self
            .members
            .iter()
            .zip(registration.vkeys.iter().zip(expected))
            .filter_map(|(member, (key, digest))| {
                systems.binary_search(&member.proof_system).ok()?;
                Some(((*key)?, digest?))
            })
            .unzip();
        assign_proof_systems(&mut selected, ctx.rollup_id, systems);
        if member_digests(&selected, &keys, blobs, ctx.proof_context)? != signed {
            return Err(ProverError::Backend(
                "final proof assignment changed a signed digest".into(),
            ));
        }
        event!(
            name: "eez.composer.attestation.quorum_reached",
            Level::INFO,
            event_name = "eez.composer.attestation.quorum_reached",
            threshold,
            usable,
            attesters = self.members.len(),
            proofs = attestations.len(),
            "attestation quorum reached",
        );
        Ok(attestations)
    }
}

/// The manager's attestation settings for one batch, valid by construction:
/// one entry per quorum member and a threshold the usable members can meet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuorumRegistration {
    /// Distinct proof systems a batch must carry.
    threshold: usize,
    /// Parallel to the quorum's members: the verification key the manager
    /// registers for a usable member, `None` for one that sits this batch out.
    vkeys: Vec<Option<B256>>,
}

impl QuorumRegistration {
    fn new(threshold: usize, vkeys: Vec<Option<B256>>) -> Result<Self, RegistrationError> {
        let registration = Self { threshold, vkeys };
        let usable = registration.usable();
        if threshold > usable {
            return Err(RegistrationError::Unreachable {
                threshold,
                usable,
                configured: registration.vkeys.len(),
            });
        }
        Ok(registration)
    }

    fn usable(&self) -> usize {
        self.vkeys.iter().flatten().count()
    }
}

alloy_sol_types::sol! {
    /// The attestation settings of a rollup's manager contract (`Rollup.sol`).
    #[sol(rpc)]
    interface IRollupAttestationReader {
        function threshold() external view returns (uint256);
        function verificationKey(address proofSystem) external view returns (bytes32);
    }

    /// The signer an `ECDSAProofSystem` accepts.
    #[sol(rpc)]
    interface IEcdsaProofSystemReader {
        function signer() external view returns (address);
    }
}

fn decode<C: SolCall>(output: Result<Bytes, impl std::fmt::Display>) -> Result<C::Return, String> {
    let output = output.map_err(|error| error.to_string())?;
    C::abi_decode_returns(&output).map_err(|error| error.to_string())
}

/// The verification key `member` is asked under, or `None` when it sits this
/// batch out: unreadable, unregistered, or accepting another signer.
fn usable_vkey(
    member: &QuorumMember,
    manager: Address,
    vkey: Result<B256, String>,
    signer: Result<Address, String>,
) -> Option<B256> {
    let warn = |reason: &str| {
        event!(
            name: "eez.composer.attestation.member_skipped",
            Level::WARN,
            event_name = "eez.composer.attestation.member_skipped",
            proof_system = %member.proof_system,
            attester = %member.attester,
            %manager,
            reason,
            "an attester is not asked for this batch",
        );
    };
    let Ok(vkey) = vkey else {
        warn("its verification key cannot be read");
        return None;
    };
    if vkey.is_zero() || vkey != member.prover.vkey() {
        warn("its registered key does not match the configured network profile");
        return None;
    }
    match signer {
        Ok(signer) if signer == member.attester => Some(vkey),
        // A proof from the configured key would revert the batch on-chain, so
        // the member sits out until its config matches.
        Ok(_) => {
            warn("its proof system accepts another signer");
            None
        }
        Err(_) => {
            warn("its proof system's signer cannot be read");
            None
        }
    }
}

/// `member`'s proof for `request`, counted only if it verifies.
async fn prove_verified(
    member: &QuorumMember,
    expected: B256,
    request: ProvingContext,
    budget: ProofBudget,
    complete: &AtomicBool,
) -> Result<Bytes, ProverError> {
    let proof = budget
        .prove(member.prover.as_ref(), request, complete)
        .await?;
    eez_prover::verify_attestation(&proof, expected.as_slice(), member.attester)?;
    Ok(proof)
}

pub(crate) fn deadline_error() -> ProverError {
    ProverError::Retryable {
        kind: RetryableProverError::DeadlineExceeded,
        message: "attestation quorum did not reach threshold before its deadline".into(),
    }
}

/// Make `proof_systems`, strictly ascending, the ones `batch` settles
/// `rollup_id` with.
pub(crate) fn assign_proof_systems(
    batch: &mut EvmBatch,
    rollup_id: u64,
    proof_systems: Vec<Address>,
) {
    batch.rollupIdsWithProofSystems = vec![RollupIdWithProofSystemsSol {
        rollupId: rollup_id,
        proofSystemIndexes: (0..proof_systems.len() as u64).collect(),
    }];
    batch.proofSystems = proof_systems;
}

/// The batch as one attester is asked to prove it: naming only that
/// attester's proof system, the shape a single-proof-system signer validates.
fn single_attester_context(ctx: &ProvingContext, proof_system: Address) -> ProvingContext {
    let mut request = ctx.clone();
    assign_proof_systems(&mut request.batch, ctx.rollup_id, vec![proof_system]);
    request
}

/// Native per-system digests for one rollup, using the same canonical helper
/// as each singleton signer. Proof membership and indexes do not enter the
/// shared digest; each registered key enters only its own accumulator.
pub(crate) fn member_digests(
    batch: &EvmBatch,
    keys: &[B256],
    blob_hashes: &[B256],
    context: eez_protocol::l1_history::ProofContext,
) -> Result<Vec<B256>, ProverError> {
    if keys.len() != batch.proofSystems.len() || keys.is_empty() || keys.len() > MAX_ATTESTERS {
        return Err(ProverError::Backend(
            "invalid attestation key coverage".into(),
        ));
    }
    eez_protocol::public_inputs::public_inputs_hashes_with_keys(batch, keys, blob_hashes, context)
        .map_err(|error| ProverError::Backend(format!("invalid attestation plan: {error}")))
}

fn log_member(
    system: Address,
    terminal: u64,
    terminal_hash: B256,
    started: Instant,
    error: Option<&ProverError>,
    late: bool,
) {
    // Remote diagnostics can contain operator URLs and credentials.
    let outcome = match error {
        None => "verified",
        Some(ProverError::Retryable {
            kind: RetryableProverError::Unavailable,
            ..
        }) => "unavailable",
        Some(ProverError::Retryable {
            kind: RetryableProverError::DeadlineExceeded,
            ..
        }) => "deadline",
        Some(ProverError::Retryable {
            kind: RetryableProverError::Aborted,
            ..
        }) => "aborted",
        Some(ProverError::Actionable { .. }) => "rejected",
        Some(ProverError::Backend(_)) => "invalid_or_failed",
    };
    tracing::info!(%system, terminal, %terminal_hash, elapsed_ms = started.elapsed().as_millis(), late, outcome, "attestation member result");
}

#[cfg(test)]
mod tests;
