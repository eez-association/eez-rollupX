use alloy_primitives::{Signature, U256, keccak256};
use async_trait::async_trait;
use eez_protocol::EcdsaProofSigner;
use eez_prover::ActionableProverFailure;
use std::time::Duration;

use super::*;
use eez_driver::RollupTiming;
use eez_l1::BundleTarget;

/// How a scripted attester answers.
#[derive(Debug, Clone, Copy)]
enum Answer {
    /// A valid proof over the digest of the batch it was asked to prove.
    Sign,
    /// A proof over some other digest.
    SignElsewhere,
    /// A valid signature in its malleable high-s form.
    SignHighS,
    /// A valid signature with the 0/1 recovery byte `ECDSA.recover` rejects.
    SignRawRecovery,
    /// A proof of the wrong length.
    Truncated,
    /// A failure returned by the prover.
    Fail(fn() -> ProverError),
}

/// A prover that answers once after `delay`, recording the request it saw.
#[derive(Debug)]
struct ScriptedAttester {
    key: EcdsaProofSigner,
    delay: Duration,
    answer: Answer,
    seen: Mutex<Option<ProvingContext>>,
    finished: AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedAttester {
    fn new(key_byte: u8, delay_ms: u64, answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            key: EcdsaProofSigner::from_private_key(B256::repeat_byte(key_byte)).unwrap(),
            delay: Duration::from_millis(delay_ms),
            answer,
            seen: Mutex::new(None),
            finished: AtomicBool::new(false),
            calls: Default::default(),
        })
    }

    fn sign(&self, digest: B256) -> Bytes {
        self.key.sign_prehash(digest).unwrap()
    }
}

#[async_trait]
impl Prover for ScriptedAttester {
    async fn prove(&self, ctx: ProvingContext) -> Result<Bytes, ProverError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let digest = eez_protocol::public_inputs_hashes_with_blob_hashes(
            &ctx.batch,
            self.vkey(),
            &[],
            ctx.proof_context,
        )
        .unwrap()[0];
        *self.seen.lock().unwrap() = Some(ctx);
        tokio::time::sleep(self.delay).await;
        self.finished.store(true, Ordering::SeqCst);
        match self.answer {
            Answer::Sign => Ok(self.sign(digest)),
            Answer::SignElsewhere => Ok(self.sign(keccak256(digest))),
            Answer::SignHighS => {
                let signature = Signature::try_from(self.sign(digest).as_ref()).unwrap();
                let high =
                    Signature::new(signature.r(), SECP256K1N - signature.s(), !signature.v());
                Ok(Bytes::copy_from_slice(&high.as_bytes()))
            }
            Answer::SignRawRecovery => {
                let mut proof = self.sign(digest).to_vec();
                proof[64] -= 27;
                Ok(Bytes::from(proof))
            }
            Answer::Truncated => Ok(self.sign(digest).slice(..64)),
            Answer::Fail(error) => Err(error()),
        }
    }

    fn vkey(&self) -> B256 {
        vkey_of(self.key.address())
    }
}

/// The secp256k1 group order.
const SECP256K1N: U256 = U256::from_be_slice(&[
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
]);

/// The membership-ticket vkey the devnet registers for an ECDSA signer.
fn vkey_of(attester: Address) -> B256 {
    attester.into_word()
}

fn rejected() -> ProverError {
    ProverError::Backend("settlement validation rejected".to_owned())
}

fn unavailable() -> ProverError {
    ProverError::Retryable {
        kind: RetryableProverError::Unavailable,
        message: "connect refused".to_owned(),
    }
}

fn poisoned() -> ProverError {
    ProverError::Actionable {
        failure: ActionableProverFailure::Inbound {
            entry_index: 1,
            entry_hash: B256::repeat_byte(0x11),
        },
        message: "reverted delivery".to_owned(),
    }
}

const SIGN: Answer = Answer::Sign;
const REJECTED: Answer = Answer::Fail(rejected);
const UNAVAILABLE: Answer = Answer::Fail(unavailable);
const POISONED: Answer = Answer::Fail(poisoned);

fn proof_system(byte: u8) -> Address {
    Address::repeat_byte(byte)
}

/// Attesters with proof systems 0x01, 0x02, ... in the given (delay, answer)
/// order, given to the quorum in reverse.
fn quorum(attesters: &[(u64, Answer)]) -> (AttestationQuorum, Vec<Arc<ScriptedAttester>>) {
    let provers: Vec<_> = attesters
        .iter()
        .enumerate()
        .map(|(i, &(delay, answer))| {
            ScriptedAttester::new(u8::try_from(i + 1).unwrap(), delay, answer)
        })
        .collect();
    let members = provers
        .iter()
        .enumerate()
        .rev()
        .map(|(i, prover)| QuorumMember {
            proof_system: proof_system(u8::try_from(i + 1).unwrap()),
            attester: prover.key.address(),
            prover: Arc::clone(prover) as Arc<dyn Prover>,
        })
        .collect();
    (
        AttestationQuorum::new(members, manager(), reth_tasks::Runtime::test()).unwrap(),
        provers,
    )
}

/// Every member registered, with `threshold`.
fn registered(provers: &[Arc<ScriptedAttester>], threshold: usize) -> QuorumRegistration {
    QuorumRegistration::new(
        threshold,
        provers
            .iter()
            .map(|p| Some(vkey_of(p.key.address())))
            .collect(),
    )
    .unwrap()
}

fn context() -> ProvingContext {
    let mut batch = EvmBatch {
        blockNumber: u64::MAX,
        ..Default::default()
    };
    batch.expectedRootPerRollup = vec![eez_protocol::abi::ExpectedRootPerRollupSol {
        rollupId: 7,
        root: B256::repeat_byte(1),
    }];
    ProvingContext {
        rollup_id: 7,
        from_block: 1,
        to_block: 1,
        batch,
        blobs: Vec::new(),
        blocks: Arc::from([]),
        l1_posting: None,
        parent: Default::default(),
        proof_context: eez_protocol::l1_history::ProofContext::new(
            U256::from(700),
            7,
            eez_protocol::l1_history::EvidenceAnchor::ActualParent {
                number: 99,
                hash: B256::repeat_byte(0x42),
                target_timestamp: 12345,
            },
            12340,
        )
        .unwrap(),
    }
}

fn budget() -> ProofBudget {
    ProofBudget::new(
        timing(),
        BundleTarget::NextBlock,
        crate::prover_retry::unix_time_millis(),
        None,
        Duration::ZERO,
    )
}

/// A 2 s proof budget, so retryable attesters keep retrying well past every
/// scripted delay.
fn timing() -> RollupTiming {
    RollupTiming::new(12_000, 2_000, 2_000, 100, 0)
}

async fn attest_with(
    quorum: &AttestationQuorum,
    registration: QuorumRegistration,
) -> Result<Vec<Attestation>, ProverError> {
    quorum
        .collect_proofs(&context(), &[], &registration, budget())
        .await
}

async fn attest(
    quorum: &AttestationQuorum,
    provers: &[Arc<ScriptedAttester>],
    threshold: usize,
) -> Result<Vec<Attestation>, ProverError> {
    attest_with(quorum, registered(provers, threshold)).await
}

fn systems(attestations: &[Attestation]) -> Vec<Address> {
    attestations.iter().map(|a| a.proof_system).collect()
}

/// `batch` as the composer settles it with `attestations`.
fn settled(attestations: Vec<Attestation>) -> EvmBatch {
    let mut batch = context().batch;
    assign_proof_systems(
        &mut batch,
        7,
        attestations.iter().map(|a| a.proof_system).collect(),
    );
    batch.proofs = attestations.into_iter().map(|a| a.proof).collect();
    batch
}

#[test]
fn new_sorts_members_and_rejects_ambiguous_sets() {
    let (quorum, _) = quorum(&[(0, SIGN), (0, SIGN), (0, SIGN)]);
    assert_eq!(
        quorum.proof_systems().collect::<Vec<_>>(),
        vec![proof_system(1), proof_system(2), proof_system(3)],
        "members are ordered as the registry requires batch.proofSystems"
    );

    let prover = ScriptedAttester::new(1, 0, SIGN) as Arc<dyn Prover>;
    let member = |ps, attester| QuorumMember {
        proof_system: ps,
        attester,
        prover: Arc::clone(&prover),
    };
    let signer = Address::repeat_byte(0xee);
    assert_eq!(
        AttestationQuorum::new(
            vec![
                member(proof_system(1), signer),
                member(proof_system(2), signer)
            ],
            manager(),
            reth_tasks::Runtime::test()
        )
        .unwrap_err(),
        QuorumConfigError::DuplicateAttester(signer)
    );
    assert_eq!(
        AttestationQuorum::new(Vec::new(), manager(), reth_tasks::Runtime::test()).unwrap_err(),
        QuorumConfigError::Empty
    );
    assert_eq!(
        AttestationQuorum::new(
            (1..=17).map(|b| member(proof_system(b), signer)).collect(),
            manager(),
            reth_tasks::Runtime::test()
        )
        .unwrap_err(),
        QuorumConfigError::TooMany { count: 17 }
    );
    assert_eq!(
        AttestationQuorum::new(
            vec![
                member(proof_system(1), signer),
                member(Address::ZERO, signer)
            ],
            manager(),
            reth_tasks::Runtime::test()
        )
        .unwrap_err(),
        QuorumConfigError::ZeroProofSystem { index: 1 }
    );
    assert_eq!(
        AttestationQuorum::new(
            vec![member(proof_system(1), Address::ZERO)],
            manager(),
            reth_tasks::Runtime::test()
        )
        .unwrap_err(),
        QuorumConfigError::ZeroAttester { index: 0 }
    );
    assert_eq!(
        AttestationQuorum::new(
            vec![
                member(proof_system(2), signer),
                member(proof_system(2), signer)
            ],
            manager(),
            reth_tasks::Runtime::test()
        )
        .unwrap_err(),
        QuorumConfigError::DuplicateProofSystem {
            proof_system: proof_system(2)
        }
    );
}

#[tokio::test(start_paused = true)]
async fn each_attester_proves_a_batch_naming_only_its_proof_system() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN)]);

    attest(&quorum, &provers, 2).await.unwrap();

    for (i, prover) in provers.iter().enumerate() {
        let seen = prover.seen.lock().unwrap().clone().unwrap();
        assert_eq!(
            seen.batch.proofSystems,
            vec![proof_system(u8::try_from(i + 1).unwrap())]
        );
        assert_eq!(seen.batch.rollupIdsWithProofSystems.len(), 1);
        assert_eq!(seen.batch.rollupIdsWithProofSystems[0].rollupId, 7);
        assert_eq!(
            seen.batch.rollupIdsWithProofSystems[0].proofSystemIndexes,
            vec![0]
        );
    }
}

/// Each proof, signed over its single-attester batch, is valid in the batch
/// that lists every attester: the contract's per-proof-system digest folds
/// only that proof system's vkey.
#[tokio::test(start_paused = true)]
async fn every_proof_verifies_against_the_settled_batch() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN), (0, SIGN)]);

    let batch = settled(attest(&quorum, &provers, 3).await.unwrap());

    assert_eq!(batch.proofSystems.len(), 3);
    for (index, prover) in provers.iter().enumerate() {
        let digests = eez_protocol::public_inputs_hashes_with_blob_hashes(
            &batch,
            vkey_of(prover.key.address()),
            &[],
            context().proof_context,
        )
        .unwrap();
        eez_prover::verify_attestation(
            &batch.proofs[index],
            digests[index].as_slice(),
            prover.key.address(),
        )
        .unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn first_two_release_the_round_while_the_third_finishes_for_diagnostics() {
    for late in [SIGN, REJECTED] {
        let (quorum, provers) = quorum(&[(5, SIGN), (10, SIGN), (500, late)]);
        let got = attest(&quorum, &provers, 2).await.unwrap();
        assert_eq!(systems(&got), vec![proof_system(1), proof_system(2)]);
        assert!(!provers[2].finished.load(Ordering::SeqCst));
        tokio::time::advance(Duration::from_millis(501)).await;
        tokio::task::yield_now().await;
        assert!(provers[2].finished.load(Ordering::SeqCst));
    }
}

#[tokio::test(start_paused = true)]
async fn late_unavailable_is_observed_without_another_attempt() {
    let (quorum, provers) = quorum(&[(5, SIGN), (10, SIGN), (50, UNAVAILABLE)]);
    assert_eq!(attest(&quorum, &provers, 2).await.unwrap().len(), 2);
    tokio::time::advance(Duration::from_millis(60)).await;
    tokio::task::yield_now().await;
    assert!(provers[2].finished.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert_eq!(provers[2].calls.load(Ordering::SeqCst), 1);
}
#[tokio::test(start_paused = true)]
async fn missed_threshold_and_late_observation_share_the_original_cutoff() {
    let (quorum, provers) = quorum(&[(5, SIGN), (10, SIGN), (10_000, SIGN)]);
    let expected = vec![proof_system(1), proof_system(2)];
    assert_eq!(
        systems(&attest(&quorum, &provers, 2).await.unwrap()),
        expected
    );
    assert_eq!(
        systems(&attest(&quorum, &provers, 2).await.unwrap()),
        expected
    );
    tokio::time::advance(Duration::from_secs(12)).await;
    tokio::task::yield_now().await;
    assert!(!provers[2].finished.load(Ordering::SeqCst));
    assert_eq!(provers[2].calls.load(Ordering::SeqCst), 2);
    assert!(attest(&quorum, &provers, 3).await.is_err());
    assert!(!provers[2].finished.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn invalid_proofs_are_not_counted() {
    for answer in [
        Answer::SignElsewhere,
        Answer::SignHighS,
        Answer::SignRawRecovery,
        Answer::Truncated,
    ] {
        let (quorum, provers) = quorum(&[(0, SIGN), (0, answer), (0, SIGN)]);

        let attestations = attest(&quorum, &provers, 2).await.unwrap();

        assert_eq!(
            systems(&attestations),
            vec![proof_system(1), proof_system(3)],
            "{answer:?} must not reach the batch, where it would revert every proof"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_proof_from_another_key_is_not_counted() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN)]);
    let mut members = quorum.members.clone();
    members[1].attester = Address::repeat_byte(0xee);
    let quorum = AttestationQuorum::new(members, manager(), reth_tasks::Runtime::test()).unwrap();

    let attestations = attest_with(&quorum, registered(&provers, 1)).await.unwrap();

    assert_eq!(systems(&attestations), vec![proof_system(1)]);
}

#[tokio::test(start_paused = true)]
async fn attesters_sitting_out_are_not_asked_and_do_not_count_towards_blocking() {
    let (quorum, provers) = quorum(&[(0, POISONED), (0, SIGN), (0, SIGN)]);
    let mut vkeys = registered(&provers, 2).vkeys;
    vkeys[2] = None;
    let registration = QuorumRegistration::new(2, vkeys).unwrap();

    // A member can withhold its signature, but its rejection cannot evict a source.
    let error = attest_with(&quorum, registration).await.unwrap_err();

    assert!(error.actionable_failure().is_none(), "{error:?}");
    assert!(provers[2].seen.lock().unwrap().is_none());
}

#[test]
fn a_threshold_beyond_the_usable_attesters_is_refused() {
    assert_eq!(
        QuorumRegistration::new(2, vec![None, Some(B256::repeat_byte(1))]).unwrap_err(),
        RegistrationError::Unreachable {
            threshold: 2,
            usable: 1,
            configured: 2,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_member_rejection_cannot_dispose_of_a_user() {
    let (quorum, provers) = quorum(&[(10, SIGN), (5, POISONED)]);
    let error = attest(&quorum, &provers, 2).await.unwrap_err();
    assert_eq!(
        error.retryable_kind(),
        Some(RetryableProverError::DeadlineExceeded)
    );
}

fn manager() -> Address {
    Address::repeat_byte(0x99)
}

fn answering(
    threshold: u64,
    members: &[(Answered, Answered)],
) -> impl FnMut(Address, Vec<u8>) -> Result<Bytes, String> + use<> {
    let mut replies = std::collections::VecDeque::from([Ok(Bytes::copy_from_slice(
        &U256::from(threshold).to_be_bytes::<32>(),
    ))]);
    for (vkey, signer) in members {
        for answer in [vkey, signer] {
            replies.push_back(match answer {
                Answered::Word(value) => Ok(Bytes::copy_from_slice(value.as_slice())),
                Answered::Revert => Err("reverted".into()),
            });
        }
    }
    move |_, _| replies.pop_front().expect("test view response")
}

enum Answered {
    Word(B256),
    Revert,
}

/// `prover`'s registration as the manager and its proof system report it.
fn answers_for(prover: &ScriptedAttester) -> (Answered, Answered) {
    (
        Answered::Word(vkey_of(prover.key.address())),
        Answered::Word(prover.key.address().into_word()),
    )
}

#[tokio::test]
async fn registration_reads_the_threshold_vkeys_and_signers() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN)]);
    let provider = answering(2, &[answers_for(&provers[0]), answers_for(&provers[1])]);

    let registration = quorum
        .local_registration(manager(), |_| true, provider, |_, _| {})
        .unwrap();

    assert_eq!(registration, registered(&provers, 2));
}

#[tokio::test]
async fn a_member_whose_proof_system_changed_signer_sits_out() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN)]);
    let provider = answering(
        1,
        &[
            (
                Answered::Word(vkey_of(provers[0].key.address())),
                Answered::Word(Address::repeat_byte(0xee).into_word()),
            ),
            answers_for(&provers[1]),
        ],
    );

    let registration = quorum
        .local_registration(manager(), |_| true, provider, |_, _| {})
        .unwrap();

    let mut expected = registered(&provers, 1).vkeys;
    expected[0] = None;
    assert_eq!(registration, QuorumRegistration::new(1, expected).unwrap());
}

/// A proof system the manager registers but whose `signer()` cannot be read,
/// such as one that is not an `ECDSAProofSystem`, costs the batch only that
/// member.
#[tokio::test]
async fn a_member_whose_registration_cannot_be_read_sits_out() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN), (0, SIGN)]);
    let provider = answering(
        2,
        &[
            (
                Answered::Word(vkey_of(provers[0].key.address())),
                Answered::Revert,
            ),
            (Answered::Revert, Answered::Revert),
            answers_for(&provers[2]),
        ],
    );

    let error = quorum
        .local_registration(manager(), |_| true, provider, |_, _| {})
        .unwrap_err();

    assert_eq!(
        error,
        RegistrationError::Unreachable {
            threshold: 2,
            usable: 1,
            configured: 3,
        }
    );

    let provider = answering(
        1,
        &[
            (
                Answered::Word(vkey_of(provers[0].key.address())),
                Answered::Revert,
            ),
            (Answered::Revert, Answered::Revert),
            answers_for(&provers[2]),
        ],
    );
    let registration = quorum
        .local_registration(manager(), |_| true, provider, |_, _| {})
        .unwrap();
    let mut expected = registered(&provers, 1).vkeys;
    expected[0] = None;
    expected[1] = None;
    assert_eq!(registration, QuorumRegistration::new(1, expected).unwrap());
}

#[tokio::test]
async fn an_unregistered_member_sits_out() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN)]);
    let provider = answering(
        1,
        &[
            answers_for(&provers[0]),
            (Answered::Word(B256::ZERO), Answered::Revert),
        ],
    );

    let registration = quorum
        .local_registration(manager(), |_| true, provider, |_, _| {})
        .unwrap();

    let mut expected = registered(&provers, 1).vkeys;
    expected[1] = None;
    assert_eq!(registration, QuorumRegistration::new(1, expected).unwrap());
}

#[tokio::test]
async fn a_failed_threshold_read_is_reported() {
    let (quorum, _) = quorum(&[(0, SIGN), (0, SIGN)]);
    assert_eq!(
        quorum
            .local_registration(
                manager(),
                |_| true,
                |_, _| Err("unavailable".into()),
                |_, _| {}
            )
            .unwrap_err(),
        RegistrationError::Threshold
    );
}

#[tokio::test]
async fn a_registered_key_for_another_network_profile_sits_out() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN)]);
    let provider = answering(
        1,
        &[
            (
                Answered::Word(B256::repeat_byte(0x77)),
                Answered::Word(provers[0].key.address().into_word()),
            ),
            answers_for(&provers[1]),
        ],
    );
    let mut skipped = Vec::new();
    let registration = quorum
        .local_registration(
            manager(),
            |_| true,
            provider,
            |member, reason| {
                skipped.push((member.proof_system, reason));
            },
        )
        .unwrap();
    assert_eq!(
        skipped,
        [(
            proof_system(1),
            "its registered key does not match the configured network profile"
        )]
    );
    assert_eq!(registration.vkeys[0], None);
    assert_eq!(registration.vkeys[1], Some(provers[1].vkey()));
}

#[test]
fn native_member_digest_binds_physical_blob_hashes_and_proof_context() {
    let mut ctx = context();
    assign_proof_systems(&mut ctx.batch, 7, vec![proof_system(1), proof_system(2)]);
    ctx.batch.blobIndices = vec![U256::ZERO];
    let keys = [B256::repeat_byte(1), B256::repeat_byte(2)];
    let hashes = [B256::repeat_byte(0x11)];
    let native = member_digests(&ctx.batch, &keys, &hashes, ctx.proof_context).unwrap();
    assert_ne!(
        native[0], native[1],
        "each member has its own key accumulator"
    );
    assert_ne!(
        native,
        member_digests(
            &ctx.batch,
            &keys,
            &[B256::repeat_byte(0x22)],
            ctx.proof_context
        )
        .unwrap()
    );
    let other = eez_protocol::l1_history::ProofContext::new(
        U256::from(700),
        7,
        eez_protocol::l1_history::EvidenceAnchor::ActualParent {
            number: 99,
            hash: B256::repeat_byte(0x43),
            target_timestamp: 12345,
        },
        12340,
    )
    .unwrap();
    assert_ne!(
        native,
        member_digests(&ctx.batch, &keys, &hashes, other).unwrap()
    );
    assert!(member_digests(&ctx.batch, &keys[..1], &hashes, ctx.proof_context).is_err());
    for (index, system) in ctx.batch.proofSystems.iter().copied().enumerate() {
        let request = single_attester_context(&ctx, system);
        let singleton = eez_protocol::public_inputs_hashes_with_blob_hashes(
            &request.batch,
            keys[index],
            &hashes,
            ctx.proof_context,
        )
        .unwrap();
        assert_eq!(native[index], singleton[0]);
    }
}

#[test]
fn verifier_reserve_grows_for_every_additional_member() {
    assert_eq!(additional_verifier_gas(1), 0);
    assert_eq!(additional_verifier_gas(2), 14_000);
    assert_eq!(additional_verifier_gas(MAX_ATTESTERS), 210_000);
}

#[test]
fn estimation_excludes_wrong_profile_members_and_still_requires_the_threshold() {
    let (quorum, provers) = quorum(&[(0, SIGN), (0, SIGN), (0, SIGN)]);
    let read = |threshold: u64| {
        let mut answers = std::collections::VecDeque::from([
            U256::from(threshold).to_be_bytes::<32>().to_vec().into(),
            provers[0].vkey().as_slice().to_vec().into(),
            provers[0]
                .key
                .address()
                .into_word()
                .as_slice()
                .to_vec()
                .into(),
            provers[1].vkey().as_slice().to_vec().into(),
            provers[1]
                .key
                .address()
                .into_word()
                .as_slice()
                .to_vec()
                .into(),
            B256::repeat_byte(0x77).as_slice().to_vec().into(),
            provers[2]
                .key
                .address()
                .into_word()
                .as_slice()
                .to_vec()
                .into(),
        ]);
        move |_, _| Ok(answers.pop_front().expect("bounded registration reads"))
    };
    assert_eq!(
        quorum
            .usable_proof_systems(manager(), |_| true, read(2))
            .unwrap(),
        vec![proof_system(1), proof_system(2)]
    );
    assert!(
        quorum
            .usable_proof_systems(manager(), |_| true, read(3))
            .is_err()
    );
}

#[test]
fn registration_rejects_a_different_manager_before_reading_members() {
    let (quorum, _) = quorum(&[(0, SIGN)]);
    assert_eq!(
        quorum
            .local_registration(
                Address::ZERO,
                |_| true,
                |_, _| panic!("unexpected read"),
                |_, _| {}
            )
            .unwrap_err(),
        RegistrationError::ManagerMismatch
    );
}
