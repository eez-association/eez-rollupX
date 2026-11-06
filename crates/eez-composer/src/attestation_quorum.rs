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

use std::{cmp::Reverse, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use alloy_eips::BlockId;
use alloy_network::{Ethereum, Network, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, Signature};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_client::BatchRequest;
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

/// How long one batch's read of the manager's attestation settings may take,
/// including the one-time lookup of the manager itself.
const REGISTRATION_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// `r || s || v`, the proof `ECDSAProofSystem` verifies.
const ECDSA_PROOF_BYTES: usize = 65;

/// How long the composer keeps collecting after the threshold is met, so
/// attesters that answer shortly after the fastest ones are still recorded
/// on-chain instead of the same fastest subset being included every time.
pub const DEFAULT_ATTESTATION_GRACE: Duration = Duration::from_millis(250);

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
    /// A read the batch cannot do without failed.
    #[error("read {call} from {contract}: {message}")]
    Read {
        /// The contract read.
        contract: Address,
        /// The view called.
        call: String,
        /// The transport or decoding failure.
        message: String,
    },
    /// The registry has no manager for the rollup.
    #[error("rollup {rollup_id} has no manager in registry {registry}")]
    NoManager {
        /// The settled rollup.
        rollup_id: u64,
        /// The registry read.
        registry: Address,
    },
    /// The L1 node did not answer the reads in time.
    #[error(
        "the L1 node did not answer the attestation settings reads within {REGISTRATION_READ_TIMEOUT:?}"
    )]
    TimedOut,
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
    grace: Duration,
}

impl AttestationQuorum {
    /// Build an attester set. Members may be given in any order; they are
    /// sorted by proof system.
    ///
    /// # Errors
    ///
    /// Returns [`QuorumConfigError`] for an empty or oversized set, a zero
    /// address, or a proof system configured twice.
    pub fn new(mut members: Vec<QuorumMember>, grace: Duration) -> Result<Self, QuorumConfigError> {
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
        Ok(Self { members, grace })
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
        mut read: impl FnMut(Address, Vec<u8>) -> Result<Bytes, String>,
    ) -> Result<Vec<(Address, B256)>, ProverError> {
        let threshold = decode::<IRollupAttestationReader::thresholdCall>(read(
            manager,
            IRollupAttestationReader::thresholdCall {}.abi_encode(),
        ))
        .map_err(ProverError::Backend)?;
        let threshold = usize::try_from(threshold).unwrap_or(usize::MAX).max(1);
        let keys = self
            .members
            .iter()
            .filter_map(|member| {
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
                usable_vkey(member, manager, key, signer).map(|key| (member.proof_system, key))
            })
            .collect::<Vec<_>>();
        if keys.len() < threshold {
            return Err(ProverError::Backend(format!(
                "attestation threshold {threshold} is unreachable with {} usable members",
                keys.len(),
            )));
        }
        Ok(keys)
    }

    /// Read the anchored policy and collect within the same proof budget.
    /// The caller boxes this optional quorum boundary so its RPC state does
    /// not enlarge every native preparation future.
    pub(crate) async fn attest_at(
        &self,
        provider: &RootProvider,
        registry: Address,
        ctx: &ProvingContext,
        anchor: B256,
        budget: ProofBudget,
    ) -> Result<Vec<Attestation>, ProverError> {
        let round = timeout_at(
            budget.expires_at(),
            self.round(provider, registry, ctx.rollup_id, anchor),
        )
        .await
        .map_err(|_| deadline_error())?
        .map_err(|error| ProverError::Backend(format!("attestation registration: {error}")))?;
        round.attest(ctx, budget).await
    }

    /// Read a fresh policy snapshot at the same L1 anchor as the settlement.
    /// Missing registration never authorizes charging or evicting candidates.
    pub(crate) async fn round(
        &self,
        provider: &RootProvider,
        registry: Address,
        rollup_id: u64,
        anchor: B256,
    ) -> Result<AttestationRound<'_>, RegistrationError> {
        let deadline = Instant::now() + REGISTRATION_READ_TIMEOUT;
        let registration = timeout_at(deadline, async {
            let manager = rollup_manager(provider, registry, rollup_id, anchor).await?;
            self.registration_at(provider, manager, anchor).await
        })
        .await
        .map_err(|_| RegistrationError::TimedOut)??;
        Ok(AttestationRound {
            quorum: self,
            registration,
        })
    }

    /// Reads the threshold and every member's registration in one JSON-RPC
    /// batch.
    async fn registration_at(
        &self,
        provider: &RootProvider,
        manager: Address,
        anchor: B256,
    ) -> Result<QuorumRegistration, RegistrationError> {
        let mut batch = BatchRequest::new(provider.client());
        let threshold = batch
            .add_call(
                "eth_call",
                &view(manager, &IRollupAttestationReader::thresholdCall {}, anchor),
            )
            .map_err(|error| read_error(manager, "threshold()", &error))?;
        let mut member_reads = Vec::with_capacity(self.members.len());
        for member in &self.members {
            let vkey = batch.add_call(
                "eth_call",
                &view(
                    manager,
                    &IRollupAttestationReader::verificationKeyCall {
                        proofSystem: member.proof_system,
                    },
                    anchor,
                ),
            );
            let signer = batch.add_call(
                "eth_call",
                &view(
                    member.proof_system,
                    &IEcdsaProofSystemReader::signerCall {},
                    anchor,
                ),
            );
            member_reads.push((vkey, signer));
        }
        batch
            .send()
            .await
            .map_err(|error| read_error(manager, "the attestation settings", &error))?;

        let threshold = decode::<IRollupAttestationReader::thresholdCall>(threshold.await)
            .map_err(|message| RegistrationError::Read {
                contract: manager,
                call: "threshold()".to_owned(),
                message,
            })?;
        // The manager accepts any batch once `threshold` proofs verify; zero
        // still needs one proof for the batch to be attested at all.
        let threshold = usize::try_from(threshold).unwrap_or(usize::MAX).max(1);
        let mut vkeys = Vec::with_capacity(self.members.len());
        for (member, (vkey, signer)) in self.members.iter().zip(member_reads) {
            let vkey = match vkey {
                Ok(vkey) => decode::<IRollupAttestationReader::verificationKeyCall>(vkey.await),
                Err(error) => Err(error.to_string()),
            };
            let signer = match signer {
                Ok(signer) => decode::<IEcdsaProofSystemReader::signerCall>(signer.await),
                Err(error) => Err(error.to_string()),
            };
            vkeys.push(usable_vkey(member, manager, vkey, signer));
        }
        QuorumRegistration::new(threshold, vkeys)
    }
}

/// The attesters asked for one batch.
#[derive(Debug)]
pub(crate) struct AttestationRound<'a> {
    quorum: &'a AttestationQuorum,
    registration: QuorumRegistration,
}

impl AttestationRound<'_> {
    /// Collect the proofs `ctx` settles with.
    ///
    /// A quorum collects at least the registered threshold of verified proofs
    /// from the usable attesters, each retried independently within the
    /// Composer proof budget. Once the threshold is met, collection continues
    /// for the grace period (never past the attempts' own budget) and returns
    /// every verified proof received, sorted by proof system.
    ///
    /// # Errors
    ///
    /// When the threshold can no longer be met: an actionable failure if
    /// enough usable attesters to block the quorum on their own
    /// (`usable - threshold + 1`) report the same one; otherwise a retryable
    /// error if every failure was retryable, else a backend error naming each
    /// failure.
    pub(crate) async fn attest(
        &self,
        ctx: &ProvingContext,
        budget: ProofBudget,
    ) -> Result<Vec<Attestation>, ProverError> {
        self.quorum
            .collect_proofs(ctx, &self.registration, budget)
            .await
    }
}

impl AttestationQuorum {
    async fn expected_hashes(
        &self,
        ctx: &ProvingContext,
        registration: &QuorumRegistration,
    ) -> Result<Vec<Option<B256>>, ProverError> {
        let ctx = ctx.clone();
        let members: Vec<_> = self
            .members
            .iter()
            .zip(&registration.vkeys)
            .map(|(member, key)| (member.proof_system, *key))
            .collect();
        // KZG is CPU work. Derive physical blob commitments once for the round,
        // outside async workers; every member still verifies its own response.
        tokio::task::spawn_blocking(move || {
            let blob_hashes = eez_blob_codec::blob_versioned_hashes(&ctx.blobs)
                .map_err(|error| ProverError::Backend(format!("blob commitments: {error}")))?;
            members
                .into_iter()
                .map(|(proof_system, key)| {
                    let Some(key) = key else { return Ok(None) };
                    let request = single_attester_context(&ctx, proof_system);
                    let hashes = eez_protocol::public_inputs_hashes_with_blob_hashes(
                        &request.batch,
                        key,
                        &blob_hashes,
                        request.proof_context,
                    )
                    .map_err(|error| ProverError::Backend(format!("public inputs: {error}")))?;
                    match hashes.as_slice() {
                        [hash] => Ok(Some(*hash)),
                        _ => Err(ProverError::Backend("expected one member digest".into())),
                    }
                })
                .collect()
        })
        .await
        .map_err(|error| ProverError::Backend(format!("quorum preflight: {error}")))?
    }

    async fn collect_proofs(
        &self,
        ctx: &ProvingContext,
        registration: &QuorumRegistration,
        budget: ProofBudget,
    ) -> Result<Vec<Attestation>, ProverError> {
        let threshold = registration.threshold;
        let usable = registration.usable();
        let expected = timeout_at(budget.expires_at(), self.expected_hashes(ctx, registration))
            .await
            .map_err(|_| deadline_error())??;
        let mut pending: FuturesUnordered<_> = self
            .members
            .iter()
            .zip(expected)
            .filter_map(|(member, hash)| hash.map(|hash| (member, hash)))
            .map(|(member, expected)| async move {
                let request = single_attester_context(ctx, member.proof_system);
                let proof = AssertUnwindSafe(prove_verified(member, expected, request, budget))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(ProverError::Backend("the attester panicked".to_owned()))
                    });
                (member.proof_system, proof)
            })
            .collect();

        let mut tally = Tally::new(usable, threshold);
        // The verdict ends collection as soon as the threshold is out of reach,
        // so nothing is awaited once every attester has answered.
        while tally.attestations.len() < threshold {
            if let Some(error) = tally.verdict(pending.len()) {
                return Err(error);
            }
            match timeout_at(budget.expires_at(), pending.next()).await {
                Ok(Some((proof_system, proof))) => tally.record(proof_system, proof),
                Ok(None) => return Err(ProverError::Backend("quorum exhausted".into())),
                Err(_) => return Err(deadline_error()),
            }
        }
        let deadline = (Instant::now() + self.grace).min(budget.expires_at());
        while let Ok(Some((proof_system, proof))) = timeout_at(deadline, pending.next()).await {
            tally.record(proof_system, proof);
        }
        // Attesters still running are cancelled here, so none outlives its window.
        drop(pending);

        let mut attestations = tally.attestations;
        attestations.sort_by_key(|a| a.proof_system);
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

/// Read every policy value at the exact settlement anchor, including on reorg.
fn view(
    contract: Address,
    call: &impl SolCall,
    anchor: B256,
) -> (<Ethereum as Network>::TransactionRequest, BlockId) {
    (
        <Ethereum as Network>::TransactionRequest::default()
            .with_to(contract)
            .with_input(call.abi_encode()),
        anchor.into(),
    )
}

fn decode<C: SolCall>(output: Result<Bytes, impl std::fmt::Display>) -> Result<C::Return, String> {
    let output = output.map_err(|error| error.to_string())?;
    C::abi_decode_returns(&output).map_err(|error| error.to_string())
}

fn read_error(contract: Address, call: &str, error: &impl std::fmt::Display) -> RegistrationError {
    RegistrationError::Read {
        contract,
        call: call.to_owned(),
        message: error.to_string(),
    }
}

/// The verification key `member` is asked under, or `None` when it sits this
/// batch out: unreadable, unregistered, or accepting another signer.
fn usable_vkey(
    member: &QuorumMember,
    manager: Address,
    vkey: Result<B256, String>,
    signer: Result<Address, String>,
) -> Option<B256> {
    let warn = |reason: &str, error: Option<&str>| {
        event!(
            name: "eez.composer.attestation.member_skipped",
            Level::WARN,
            event_name = "eez.composer.attestation.member_skipped",
            proof_system = %member.proof_system,
            attester = %member.attester,
            %manager,
            reason,
            error = error.unwrap_or_default(),
            "an attester is not asked for this batch",
        );
    };
    let vkey = match vkey {
        Ok(vkey) => vkey,
        Err(error) => {
            warn("its verification key cannot be read", Some(&error));
            return None;
        }
    };
    if vkey.is_zero() || vkey != member.prover.vkey() {
        warn(
            "its registered key does not match the configured network profile",
            None,
        );
        return None;
    }
    match signer {
        Ok(signer) if signer == member.attester => Some(vkey),
        // A proof from the configured key would revert the batch on-chain, so
        // the member sits out until its config matches.
        Ok(_) => {
            warn("its proof system accepts another signer", None);
            None
        }
        Err(error) => {
            warn("its proof system's signer cannot be read", Some(&error));
            None
        }
    }
}

/// The rollup manager contract registered for `rollup_id` in the EEZ registry.
async fn rollup_manager(
    provider: &RootProvider,
    registry: Address,
    rollup_id: u64,
    anchor: B256,
) -> Result<Address, RegistrationError> {
    let manager = crate::composer::IEEZReader::new(registry, provider)
        .rollups(rollup_id)
        .block(anchor.into())
        .call()
        .await
        .map_err(|error| read_error(registry, &format!("rollups({rollup_id})"), &error))?
        .rollupContract;
    if manager == Address::ZERO {
        return Err(RegistrationError::NoManager {
            rollup_id,
            registry,
        });
    }
    Ok(manager)
}

/// `member`'s proof for `request`, counted only if it verifies.
async fn prove_verified(
    member: &QuorumMember,
    expected: B256,
    request: ProvingContext,
    budget: ProofBudget,
) -> Result<Bytes, ProverError> {
    let proof = budget.prove(member.prover.as_ref(), request).await?;
    verify_proof(&proof, expected, member.attester)?;
    Ok(proof)
}

fn deadline_error() -> ProverError {
    ProverError::Retryable {
        kind: RetryableProverError::DeadlineExceeded,
        message: "attestation quorum did not reach threshold before its deadline".into(),
    }
}

/// Accept `proof` only if `ECDSAProofSystem` will: 65 bytes, `v` of 27 or 28,
/// low `s` (OpenZeppelin `ECDSA.recover` rejects the malleable high half), and
/// recovering to `attester` over the digest the composer computed.
fn verify_proof(proof: &[u8], expected: B256, attester: Address) -> Result<(), ProverError> {
    let invalid = |reason: String| ProverError::Backend(format!("invalid attestation: {reason}"));
    if proof.len() != ECDSA_PROOF_BYTES {
        return Err(invalid(format!(
            "{} bytes, expected {ECDSA_PROOF_BYTES}",
            proof.len()
        )));
    }
    let v = proof[ECDSA_PROOF_BYTES - 1];
    if v != 27 && v != 28 {
        return Err(invalid(format!("recovery byte {v}, expected 27 or 28")));
    }
    let signature = Signature::try_from(proof).map_err(|error| invalid(error.to_string()))?;
    if signature.normalize_s().is_some() {
        return Err(invalid("high-s signature".to_owned()));
    }
    let signer = signature
        .recover_address_from_prehash(&expected)
        .map_err(|error| invalid(error.to_string()))?;
    if signer != attester {
        return Err(invalid(format!(
            "recovers to {signer}, not attester {attester}, over digest {expected}"
        )));
    }
    Ok(())
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
    batch
        .proofSystems
        .iter()
        .zip(keys)
        .map(|(system, key)| {
            let mut member = batch.clone();
            assign_proof_systems(&mut member, context.rollup_id(), vec![*system]);
            let hashes = eez_protocol::public_inputs_hashes_with_blob_hashes(
                &member,
                *key,
                blob_hashes,
                context,
            )
            .map_err(|error| ProverError::Backend(format!("public inputs: {error}")))?;
            match hashes.as_slice() {
                [hash] => Ok(*hash),
                _ => Err(ProverError::Backend("expected one member digest".into())),
            }
        })
        .collect()
}

/// Proofs and failures collected so far for one window.
struct Tally {
    /// Attesters asked for this window.
    usable: usize,
    threshold: usize,
    attestations: Vec<Attestation>,
    failures: Vec<(Address, ProverError)>,
}

impl Tally {
    fn new(usable: usize, threshold: usize) -> Self {
        Self {
            usable,
            threshold,
            attestations: Vec::with_capacity(usable),
            failures: Vec::new(),
        }
    }

    fn record(&mut self, proof_system: Address, proof: Result<Bytes, ProverError>) {
        match proof {
            Ok(proof) => self.attestations.push(Attestation {
                proof_system,
                proof,
            }),
            Err(error) => {
                event!(
                    name: "eez.composer.attestation.attester_failed",
                    Level::WARN,
                    event_name = "eez.composer.attestation.attester_failed",
                    %proof_system,
                    error = %error,
                    "an attester did not return a valid proof for this window",
                );
                self.failures.push((proof_system, error));
            }
        }
    }

    /// The error to report once the threshold is out of reach, or `None` while
    /// `pending` attesters could still meet it or still decide an eviction.
    /// Below the threshold with nothing pending, always an error.
    fn verdict(&self, pending: usize) -> Option<ProverError> {
        if self.attestations.len() + pending >= self.threshold {
            return None;
        }

        // An eviction censors a candidate, so it needs as many attesters as it
        // takes to block the quorum: fewer than that cannot outvote the rest.
        let blocking = self.usable - self.threshold + 1;
        // The most reported failure; on a tie, the one reported first.
        let leading = self
            .failures
            .iter()
            .enumerate()
            .filter_map(|(index, (_, e))| e.actionable_failure().map(|failure| (index, failure)))
            .map(|(index, failure)| {
                let count = self
                    .failures
                    .iter()
                    .filter(|(_, e)| e.actionable_failure() == Some(failure))
                    .count();
                (failure, count, index)
            })
            .max_by_key(|&(_, count, index)| (count, Reverse(index)));
        if let Some((failure, count, _)) = leading
            && count >= blocking
        {
            return Some(ProverError::Actionable {
                failure,
                message: format!(
                    "{count} of {} attesters reported this candidate, enough to block the quorum",
                    self.usable
                ),
            });
        }
        if leading.map_or(0, |(_, count, _)| count) + pending >= blocking {
            return None;
        }

        let summary = self
            .failures
            .iter()
            .map(|(ps, e)| format!("{ps}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        let message = format!(
            "{} of {} attesters proved the window, {} required: {summary}",
            self.attestations.len(),
            self.usable,
            self.threshold
        );
        if self
            .failures
            .iter()
            .all(|(_, e)| e.retryable_kind().is_some())
            && let Some(kind) = self.failures.first().and_then(|(_, e)| e.retryable_kind())
        {
            return Some(ProverError::Retryable { kind, message });
        }
        Some(ProverError::Backend(message))
    }
}

#[cfg(test)]
mod tests;
