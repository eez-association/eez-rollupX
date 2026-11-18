//! Exercise ownership at the launch sites; probes never retain a completion sender themselves.
use super::*;
use alloy_consensus::constants::EMPTY_ROOT_HASH;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm_ethereum::EthEvmConfig;
use reth_provider::test_utils::MockEthProvider;
use reth_stages_api::{StageCheckpoint, StageId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct ProbeFactory {
    inner: MockEthProvider,
    finished: CrossbeamReceiver<()>,
    ordered: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
    opened: CrossbeamSender<()>,
    release: Option<CrossbeamReceiver<()>>,
    fail_open: bool,
}

impl Drop for ProbeFactory {
    fn drop(&mut self) {
        self.ordered.fetch_and(
            matches!(self.finished.try_recv(), Err(crossbeam_channel::TryRecvError::Empty)),
            Ordering::SeqCst,
        );
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl DatabaseProviderFactory for ProbeFactory {
    type DB = <MockEthProvider as DatabaseProviderFactory>::DB;
    type Provider = <MockEthProvider as DatabaseProviderFactory>::Provider;
    type ProviderRW = <MockEthProvider as DatabaseProviderFactory>::ProviderRW;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        self.opened.send(()).unwrap();
        if let Some(release) = &self.release {
            assert_eq!(
                release.recv_timeout(WAIT),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            );
        }
        if self.fail_open {
            return Err(ProviderError::other(std::io::Error::other(
                "intentional provider-open failure",
            )));
        }
        DatabaseProviderFactory::database_provider_ro(&self.inner)
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        DatabaseProviderFactory::database_provider_rw(&self.inner)
    }
}

fn factory(finished: CrossbeamReceiver<()>) -> (ProbeFactory, CrossbeamReceiver<()>) {
    let inner = MockEthProvider::default();
    inner.enable_database_provider();
    inner.add_header(B256::ZERO, Default::default());
    inner.add_stage_checkpoint(StageId::Finish, StageCheckpoint::new(0));
    let (opened, rx) = crossbeam_channel::unbounded();
    (
        ProbeFactory {
            inner,
            finished,
            ordered: Arc::new(AtomicBool::new(true)),
            drops: Arc::new(AtomicUsize::new(0)),
            opened,
            release: None,
            fail_open: false,
        },
        rx,
    )
}

fn prepare(
    runtime: &reth_tasks::Runtime,
    factory: ProbeFactory,
    owner: CrossbeamSender<()>,
    timeout: Option<Duration>,
) -> PreparedStateRootJob<EthPrimitives> {
    let overlay = OverlayManager::default();
    let config = TreeConfig::default()
        .with_has_enough_parallelism(true)
        .with_state_root_task_timeout(timeout);
    let mut state = EngineApiTreeState::new(
        10,
        10,
        config.invalid_header_hit_eviction_threshold(),
        alloy_eips::BlockNumHash::new(0, B256::ZERO),
        crate::engine::EngineApiKind::Ethereum,
        overlay.clone(),
    );
    let parent = SealedHeader::new(
        alloy_consensus::Header { state_root: EMPTY_ROOT_HASH, ..Default::default() },
        B256::ZERO,
    );
    DefaultStateRootStrategy::default()
        .prepare(StateRootJobContext::new(
            runtime,
            &overlay,
            &ExecutionEnv::<EthEvmConfig>::test_default(),
            &parent,
            OverlayStateProviderFactory::new(factory, overlay.overlay_builder(B256::ZERO)),
            &config,
            false,
            owner,
            &mut state,
        ))
        .unwrap()
}

// Dropping the returned sender releases the task even if an assertion unwinds the test.
fn hold_named(runtime: &reth_tasks::Runtime, name: &'static str) -> CrossbeamSender<()> {
    let (entered, rx) = crossbeam_channel::bounded(1);
    let (release, gate) = crossbeam_channel::bounded(0);
    runtime.spawn_blocking_named(name, move || {
        entered.send(()).unwrap();
        assert_eq!(gate.recv_timeout(WAIT), Err(crossbeam_channel::RecvTimeoutError::Disconnected));
    });
    rx.recv_timeout(WAIT).unwrap();
    release
}

fn drain(runtime: &reth_tasks::Runtime) {
    for name in ["sparse-trie", "account-workers", "storage-workers"] {
        runtime.spawn_blocking_named(name, || {}).get();
    }
}

fn assert_pending(finished: &CrossbeamReceiver<()>) {
    assert_eq!(
        finished.try_recv(),
        Err(crossbeam_channel::TryRecvError::Empty),
        "engine completion disconnected while launched work still owns resources"
    );
}

fn assert_done(finished: &CrossbeamReceiver<()>, ordered: &AtomicBool, drops: &AtomicUsize) {
    assert_eq!(finished.recv_timeout(WAIT), Err(crossbeam_channel::RecvTimeoutError::Disconnected));
    assert!(drops.load(Ordering::SeqCst) > 0);
    assert!(ordered.load(Ordering::SeqCst), "factory dropped after completion disconnected");
}

fn finish_empty(mut prepared: PreparedStateRootJob<EthPrimitives>) {
    drop(prepared.take_execution_hook());
    drop(prepared.take_hint_stream());
    let block = RecoveredBlock::new_unhashed(
        Block {
            header: alloy_consensus::Header { state_root: EMPTY_ROOT_HASH, ..Default::default() },
            ..Default::default()
        },
        Vec::new(),
    );
    let outcome = prepared
        .finish(
            &block,
            Arc::new(BlockExecutionOutput::default()),
            &LazyHashedPostState::ready(Default::default()),
        )
        .unwrap();
    assert_eq!(outcome.state_root, EMPTY_ROOT_HASH);
}

#[test]
fn prepare_owns_sparse_root_until_success_or_cancellation() {
    for cancel in [false, true] {
        let runtime = reth_tasks::Runtime::test();
        let release = hold_named(&runtime, "sparse-trie");
        let (owner, finished) = crossbeam_channel::bounded(0);
        let (factory, _opened) = factory(finished.clone());
        let (ordered, drops) = (factory.ordered.clone(), factory.drops.clone());
        let prepared = prepare(&runtime, factory, owner, None);
        if cancel {
            drop(prepared);
            assert_pending(&finished);
            drop(release);
        } else {
            drop(release);
            finish_empty(prepared);
        }
        drain(&runtime);
        assert_done(&finished, &ordered, &drops);
    }
}

#[test]
fn prepare_owns_proof_workers_after_cancellation_or_open_failure() {
    for fail_open in [false, true] {
        let runtime = reth_tasks::Runtime::test();
        let (owner, finished) = crossbeam_channel::bounded(0);
        let (mut factory, opened) = factory(finished.clone());
        let (ordered, drops) = (factory.ordered.clone(), factory.drops.clone());
        let (release, gate) = crossbeam_channel::bounded(0);
        factory.release = Some(gate);
        factory.fail_open = fail_open;
        let prepared = prepare(&runtime, factory, owner, None);
        opened.recv_timeout(WAIT).unwrap();
        drop(prepared);
        // The sparse task has exited; only dispatched proof-worker factories retain ownership.
        runtime.spawn_blocking_named("sparse-trie", || {}).get();
        assert_pending(&finished);
        drop(release);
        drain(&runtime);
        assert_done(&finished, &ordered, &drops);
    }
}

#[test]
fn prepare_owns_serial_fallback_after_sparse_root_wins() {
    let runtime = reth_tasks::Runtime::test();
    let sparse_release = hold_named(&runtime, "sparse-trie");
    let serial_release = hold_named(&runtime, "serial-root");
    let (owner, finished) = crossbeam_channel::bounded(0);
    let (factory, opened) = factory(finished.clone());
    let (ordered, drops) = (factory.ordered.clone(), factory.drops.clone());
    let prepared = prepare(&runtime, factory, owner, Some(Duration::ZERO));
    let finish = std::thread::spawn(move || finish_empty(prepared));
    // Sparse work is held, so this open can only be the zero-timeout serial fallback.
    opened.recv_timeout(WAIT).unwrap();
    drop(sparse_release);
    finish.join().unwrap();
    drain(&runtime);
    assert_pending(&finished);
    drop(serial_release);
    runtime.spawn_blocking_named("serial-root", || {}).get();
    assert_done(&finished, &ordered, &drops);
}

#[derive(Clone)]
struct PanicFactory {
    probe: ProbeFactory,
    opens: Arc<AtomicUsize>,
    release: CrossbeamReceiver<()>,
}

impl DatabaseProviderROFactory for PanicFactory {
    type Provider = PanicProvider;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        let opened = self.opens.fetch_add(1, Ordering::SeqCst);
        self.probe.opened.send(()).unwrap();
        // One initial provider per pool; the third open proves catch-unwind reached replacement.
        if opened >= 2 {
            assert_eq!(
                self.release.recv_timeout(WAIT),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            );
        }
        Ok(PanicProvider(self.probe.clone()))
    }
}

struct PanicProvider(ProbeFactory);

impl reth_provider::StateProofProvider for PanicProvider {
    fn proof(
        &self,
        _: reth_trie_common::TrieInput,
        _: alloy_primitives::Address,
        _: &[B256],
    ) -> ProviderResult<reth_trie_common::AccountProof> {
        unreachable!("proof worker uses multiproof_v2")
    }

    fn multiproof(
        &self,
        _: reth_trie_common::TrieInput,
        _: reth_trie_common::MultiProofTargets,
    ) -> ProviderResult<reth_trie_common::MultiProof> {
        unreachable!("proof worker uses multiproof_v2")
    }

    fn multiproof_v2(
        &self,
        _: reth_trie_common::MultiProofTargetsV2,
        _: bool,
    ) -> ProviderResult<reth_trie_common::DecodedMultiProofV2> {
        assert_pending(&self.0.finished);
        panic!("intentional proof-job panic")
    }

    fn witness(
        &self,
        _: reth_trie_common::TrieInput,
        _: HashedPostState,
        _: reth_trie_common::ExecutionWitnessMode,
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
        unreachable!("proof worker uses multiproof_v2")
    }
}

#[test]
fn proof_job_panic_retains_owner_until_provider_replacement_exits() {
    let runtime = reth_tasks::Runtime::test();
    let (owner, finished) = crossbeam_channel::bounded(0);
    let (probe, opened) = factory(finished.clone());
    let (ordered, drops) = (probe.ordered.clone(), probe.drops.clone());
    let (release, gate) = crossbeam_channel::bounded(0);
    let factory = PanicFactory { probe, opens: Arc::new(AtomicUsize::new(0)), release: gate };
    let workers = ProofWorkerHandle::new(
        &runtime,
        ProofTaskCtx::new(ShutdownOwned::new(factory, Some(owner))),
        ProofWorkerCounts { storage: 1, account: 1 },
        Default::default(),
    );
    for _ in 0..2 {
        opened.recv_timeout(WAIT).unwrap();
    }
    assert_eq!(
        workers
            .dispatch(reth_trie_common::MultiProofTargetsV2 {
                account_targets: vec![B256::ZERO.into()],
                storage_targets: Default::default(),
            })
            .unwrap(),
        1
    );
    let error = workers.results().recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(
        error.to_string().contains("proof job panicked: intentional proof-job panic"),
        "{error}"
    );
    opened.recv_timeout(WAIT).unwrap();
    drop(workers);
    assert_pending(&finished);
    drop(release);
    drain(&runtime);
    assert_done(&finished, &ordered, &drops);
}
