//! Actual engine newPayload validation against a disposable genesis database.
use super::history_observations::Fixture;
use alloy_primitives::B256;
use alloy_rpc_types_engine::ExecutionData;
use eez_evm::EezEvmConfig;
use eez_primitives::{Block, EezPrimitives, EezTxEnvelope, engine::EezEngineTypes};
use reth_chainspec::ChainSpec;
use reth_engine_primitives::{BeaconEngineMessage, NoopInvalidBlockHook, PayloadValidator};
use reth_engine_tree::{
    chain::FromOrchestrator,
    engine::{EngineApiKind, EngineApiRequest, FromEngine},
    persistence::PersistenceHandle,
    tree::{
        BasicEngineValidator, EngineApiTreeHandler, TreeConfig,
        state_root_strategy::{
            DefaultStateRootStrategy, LazyHashedPostState, PreparedStateRootJob, StateRootJob,
            StateRootJobContext, StateRootJobOutcome, StateRootStrategy,
        },
    },
};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{Block as _, RecoveredBlock, SealedBlock};
use reth_provider::{BlockExecutionOutput, ProviderResult, providers::BlockchainProvider};
use reth_storage_overlay::OverlayManager;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
struct PayloadAdapter(reth_ethereum_payload_builder::EthereumExecutionPayloadValidator<ChainSpec>);
impl PayloadValidator<EezEngineTypes> for PayloadAdapter {
    type Block = Block;
    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block>, reth_payload_primitives::NewPayloadError> {
        self.0
            .ensure_well_formed_payload(payload)
            .map_err(Into::into)
    }
}

#[derive(Debug)]
struct RootRecord {
    hash: B256,
    root: B256,
    fallback: bool,
    receipts: Vec<eez_primitives::Receipt>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RootMode {
    #[default]
    Sparse,
    FreshSparse,
    Synchronous,
}

#[derive(Default)]
struct AuditedStrategy {
    mode: RootMode,
    fresh_overlay: Option<OverlayManager<EezPrimitives>>,
    default: DefaultStateRootStrategy,
    records: Arc<Mutex<Vec<RootRecord>>>,
}
struct AuditedJob {
    inner: PreparedStateRootJob<EezPrimitives>,
    records: Arc<Mutex<Vec<RootRecord>>>,
}
impl<P, Evm> StateRootStrategy<EezPrimitives, P, Evm> for AuditedStrategy
where
    Evm: ConfigureEvm<Primitives = EezPrimitives>,
    DefaultStateRootStrategy: StateRootStrategy<EezPrimitives, P, Evm>,
{
    fn prepare(
        &self,
        ctx: StateRootJobContext<'_, EezPrimitives, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<EezPrimitives>> {
        if let Some(overlay) = &self.fresh_overlay {
            let preserved = overlay.take_sparse_trie();
            if ctx.parent_header().number > 0 {
                assert!(
                    preserved.is_some(),
                    "discard an actual preserved trie before rebuilding"
                );
            }
        }
        let mut inner = self.default.prepare(ctx)?;
        let synchronous = self.mode == RootMode::Synchronous;
        assert_eq!(
            inner.name(),
            if synchronous {
                "synchronous"
            } else {
                "sparse-trie"
            }
        );
        let hook = inner.take_execution_hook();
        assert_eq!(
            hook.is_some(),
            !synchronous,
            "serial EVM streams only for the sparse strategy"
        );
        let hints = inner.take_hint_stream();
        assert!(inner.take_hashed_update_stream().is_none());
        let hashed = inner.take_hashed_state_rx();
        let mut outer = PreparedStateRootJob::new(
            Box::new(AuditedJob {
                inner,
                records: self.records.clone(),
            }),
            hashed,
        );
        if let Some(hook) = hook {
            outer = outer.with_execution_hook(hook);
        }
        if let Some(hints) = hints {
            outer = outer.with_hint_stream(hints);
        }
        Ok(outer)
    }
}
impl StateRootJob<EezPrimitives> for AuditedJob {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn finish(
        &mut self,
        block: &RecoveredBlock<Block>,
        output: Arc<BlockExecutionOutput<eez_primitives::Receipt>>,
        hashed: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        let receipts = output.result.receipts.clone();
        let result = self.inner.finish(block, output, hashed)?;
        self.records.lock().unwrap().push(RootRecord {
            hash: block.hash(),
            root: result.state_root,
            fallback: result.hashed_state.is_some(),
            receipts,
        });
        Ok(result)
    }
}

// Keep a failed test's disposable database on disk for diagnosis. The process
// closes the retained handle; successful cases use Reth's normal temp cleanup.
struct KeepFailedDatabase<D>(Option<Arc<D>>);
impl<D> Drop for KeepFailedDatabase<D> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::mem::forget(self.0.take());
        }
    }
}

type TestNode = reth_node_types::AnyNodeTypesWithEngine<
    EezPrimitives,
    EezEngineTypes,
    ChainSpec,
    reth_provider::EthStorage<EezTxEnvelope>,
    EezEngineTypes,
>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_new_payload_native_history_storage_matrix() {
    let cases = scenarios(false, false);
    assert_eq!(cases.len(), 19);
    for (name, fixture) in cases
        .into_iter()
        .filter(|(name, _)| !name.starts_with("create-over-prior-storage"))
    {
        run_engine_case(name, fixture, false).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_new_payload_create_prior_storage_diagnostic() {
    let cases: Vec<_> = scenarios(false, false)
        .into_iter()
        .filter(|(name, _)| name.starts_with("create-over-prior-storage"))
        .collect();
    assert_eq!(cases.len(), 2);
    for (name, fixture) in cases {
        run_engine_case(name, fixture, true).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_new_payload_create_prior_storage_retained_root() {
    let cases: Vec<_> = scenarios(true, false)
        .into_iter()
        .filter(|(name, _)| name.starts_with("create-over-prior-storage"))
        .collect();
    assert_eq!(cases.len(), 2);
    for (name, fixture) in cases {
        run_engine_case(name.replace("diagnostic", "retained-root"), fixture, false).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_new_payload_consumes_transaction_parent_state() {
    let cases = scenarios(true, true);
    assert_eq!(cases.len(), 19);
    for (name, fixture) in cases {
        assert_eq!(fixture.blocks.len(), 7);
        run_engine_case(format!("{name}-child"), fixture, false).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_new_payload_fresh_sparse_and_synchronous_paths() {
    for mode in [RootMode::FreshSparse, RootMode::Synchronous] {
        let cases = scenarios(true, true);
        assert_eq!(cases.len(), 19);
        for (name, fixture) in cases {
            run_engine_case_mode(format!("{name}-{mode:?}"), fixture, false, mode).await;
        }
    }
}

async fn run_engine_case(name: String, fixture: Fixture, prior_storage_diagnostic: bool) {
    run_engine_case_mode(name, fixture, prior_storage_diagnostic, RootMode::Sparse).await;
}

async fn run_engine_case_mode(
    name: String,
    fixture: Fixture,
    prior_storage_diagnostic: bool,
    mode: RootMode,
) {
    if name.starts_with("create-over-prior-storage") && !prior_storage_diagnostic {
        let vectors: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("engine-create-reth27-vectors.json")).unwrap();
        let balance = u64::from(name.contains("balance1"));
        let expected = vectors.iter().find(|v| v["balance"] == balance).unwrap();
        for (index, wire) in fixture.blocks.iter().enumerate() {
            assert_eq!(
                hex::encode(&wire.rlp),
                expected["blocks"][index].as_str().unwrap(),
                "Engine payload must equal the independent baseline vector"
            );
        }
    }
    let spec = Arc::new(eez_evm::profile::chain_spec(fixture.genesis.clone()).unwrap());
    let factory = reth_provider::test_utils::create_test_provider_factory_with_node_types::<TestNode>(
        spec.clone(),
    );
    assert_eq!(
        reth_db_common::init::init_genesis(&factory).unwrap(),
        spec.genesis_hash()
    );
    println!(
        "ENGINE_DB case={name} path={}",
        factory.db_ref().path().display()
    );
    let _diagnostic_db = KeepFailedDatabase(Some(factory.db_ref().clone()));
    let provider = BlockchainProvider::new(factory).unwrap();
    let canonical = provider.canonical_in_memory_state();
    let overlay = OverlayManager::default();
    let config = TreeConfig::default()
        .with_has_enough_parallelism(mode != RootMode::Synchronous)
        .with_state_root_fallback(false)
        .with_state_root_task_timeout(None);
    let strategy = Arc::new(AuditedStrategy {
        mode,
        fresh_overlay: (mode == RootMode::FreshSparse).then(|| overlay.clone()),
        ..Default::default()
    });
    let consensus = Arc::new(reth_ethereum_consensus::EthBeaconConsensus::new(
        spec.clone(),
    ));
    let evm: EezEvmConfig = fixture.capture_config.clone();
    let runtime = reth_tasks::Runtime::test();
    let validator = BasicEngineValidator::new(
        provider.clone(),
        consensus.clone(),
        evm.clone(),
        PayloadAdapter(reth_ethereum_payload_builder::EthereumExecutionPayloadValidator::new(spec)),
        config.clone(),
        Box::new(NoopInvalidBlockHook::default()),
        overlay.clone(),
        runtime.clone(),
    )
    .with_state_root_strategy(strategy.clone());
    // No forkchoice or payload-building request is sent: accepted side branches remain in
    // memory. Any persistence/build command would violate this test's bounded scope.
    let (persist_tx, persist_rx) = std::sync::mpsc::channel();
    let (builder_tx, mut builder_rx) = tokio::sync::mpsc::unbounded_channel();
    let (engine, _events) = EngineApiTreeHandler::spawn_new(
        provider,
        consensus,
        validator,
        PersistenceHandle::new(persist_tx),
        reth_payload_builder::PayloadBuilderHandle::new(builder_tx),
        canonical,
        overlay,
        config,
        EngineApiKind::Ethereum,
        evm,
        runtime,
    );
    let mut statuses = Vec::new();
    for wire in &fixture.blocks {
        let block: Block = alloy_rlp::decode_exact(&wire.rlp).unwrap();
        let payload = ExecutionData::from_block_unchecked(block.header.hash_slow(), &block);
        let (tx, rx) = tokio::sync::oneshot::channel();
        engine
            .send(FromEngine::Request(EngineApiRequest::Beacon(
                BeaconEngineMessage::NewPayload {
                    cause: tracing::Span::none(),
                    payload,
                    tx,
                },
            )))
            .unwrap();
        statuses.push(
            tokio::time::timeout(Duration::from_secs(30), rx)
                .await
                .expect("engine timeout")
                .unwrap()
                .unwrap(),
        );
    }
    let mut bad: Block = alloy_rlp::decode_exact(&fixture.blocks.last().unwrap().rlp).unwrap();
    bad.header.state_root = B256::repeat_byte(0xa5);
    let bad_hash = bad.header.hash_slow();
    let (tx, rx) = tokio::sync::oneshot::channel();
    engine
        .send(FromEngine::Request(EngineApiRequest::Beacon(
            BeaconEngineMessage::NewPayload {
                cause: tracing::Span::none(),
                payload: ExecutionData::from_block_unchecked(bad.header.hash_slow(), &bad),
                tx,
            },
        )))
        .unwrap();
    let bad_status = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    engine
        .send(FromEngine::Event(FromOrchestrator::Terminate { tx }))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .expect("engine shutdown timeout")
        .unwrap();
    assert!(persist_rx.try_recv().is_err());
    assert!(builder_rx.try_recv().is_err());
    if prior_storage_diagnostic {
        // This synthetic allocation probes a suspected baseline limitation;
        // acceptance of its cleared-storage header is NOT asserted or authorized here.
        assert!(
            statuses[..5]
                .iter()
                .all(alloy_rpc_types_engine::PayloadStatus::is_valid)
        );
        assert!(statuses[5].is_invalid());
        assert!(bad_status.is_invalid());
        let records = strategy.records.lock().unwrap();
        assert_eq!(records.len(), 7);
        assert!(records[5].fallback);
        let (receipts, projected_root) = trace_commits(&fixture, 5, &name);
        assert_eq!(receipts, records[5].receipts);
        assert_eq!(projected_root, records[5].root);
        let declared: Block = alloy_rlp::decode_exact(&fixture.blocks[5].rlp).unwrap();
        assert_ne!(projected_root, declared.header.state_root);
        let replay = crate::validate::Validator::stateless_for_test(
            fixture.genesis.clone(),
            eez_primitives::SYSTEM_ADDRESS,
        )
        .validate(&fixture.admitted());
        let Err(crate::validate::ValidationError::Rejected(message)) = replay else {
            panic!("independent replay must reject at its terminal root check");
        };
        assert_eq!(
            message,
            format!(
                "stateless validation rejected block 6: mismatched post-state root: {projected_root:?}\n {:?}",
                declared.header.state_root
            )
        );
        println!(
            "CREATE_PRIOR_STORAGE_DIAGNOSTIC case={name} engine_root={projected_root} fixture_reset_root={} signer_error={message:?}",
            declared.header.state_root,
        );
        return;
    }
    for (wire, status) in fixture.blocks.iter().zip(&statuses) {
        assert!(status.is_valid(), "block {}: {status:?}", wire.number);
        assert_eq!(status.latest_valid_hash, Some(B256::from_slice(&wire.hash)));
    }
    assert!(bad_status.is_invalid(), "wrong root: {bad_status:?}");
    let records = strategy.records.lock().unwrap();
    assert_eq!(records.len(), fixture.blocks.len() + 1);
    for (index, (wire, record)) in fixture.blocks.iter().zip(records.iter()).enumerate() {
        let block: Block = alloy_rlp::decode_exact(&wire.rlp).unwrap();
        assert_eq!(record.hash, block.header.hash_slow());
        assert_eq!(record.root, block.header.state_root);
        assert!(
            !record.fallback,
            "valid block used serial fallback: {record:?}"
        );
        let (receipts, projected_root) = trace_commits(&fixture, index, &name);
        assert_eq!(record.receipts, receipts);
        assert_eq!(record.root, projected_root);
    }
    let bad_record = records.last().unwrap();
    assert_eq!(bad_record.hash, bad_hash);
    assert_eq!(
        bad_record.fallback,
        mode != RootMode::Synchronous,
        "sparse mismatch falls back; the synchronous job directly computes its root"
    );
    assert_ne!(bad_record.root, bad.header.state_root);
    assert_eq!(bad_record.root, records[fixture.blocks.len() - 1].root);
    assert_eq!(bad_status.latest_valid_hash, Some(bad.header.parent_hash));
    let validated = crate::validate::Validator::stateless_for_test(
        fixture.genesis.clone(),
        eez_primitives::SYSTEM_ADDRESS,
    )
    .validate(&fixture.admitted())
    .unwrap();
    assert_eq!(
        validated.window_post_block_hash(),
        B256::from_slice(&fixture.blocks.last().unwrap().hash)
    );
    let label = if mode == RootMode::Synchronous {
        "ENGINE_SYNC"
    } else {
        "ENGINE_ROOT"
    };
    println!(
        "{label} case={name} valid={} no_fallback={} bad_root_invalid=true independent_replay=true",
        statuses.len(),
        records
            .iter()
            .take(statuses.len())
            .filter(|r| !r.fallback)
            .count()
    );
}

fn scenarios(retain_prior_storage: bool, followup: bool) -> Vec<(String, Fixture)> {
    use super::history_observations::{CaptureScenario, PROBE};
    use alloy_genesis::GenesisAccount;
    use alloy_primitives::{Address, Bytes, TxKind, U256};
    use eez_protocol::{
        RollupId,
        entries::{InboundSidecar, IncomingEntry},
        system_tx::{SystemTxContext, build_inbound_system_txs},
    };
    let followup_probe = followup.then(|| (PROBE, Bytes::new()));
    let mut cases = vec![
        (
            "ordinary-history".into(),
            Fixture::with_capture_scenario(
                false,
                None,
                CaptureScenario {
                    followup_call: followup_probe.clone(),
                    ..Default::default()
                },
            ),
        ),
        (
            "load-user-history".into(),
            Fixture::with_capture_scenario(
                true,
                None,
                CaptureScenario {
                    followup_call: followup_probe.clone(),
                    ..Default::default()
                },
            ),
        ),
    ];
    let frozen = Fixture::with_capture_scenario(
        false,
        Some(hex::decode("5a5f5260205ff3").unwrap()),
        CaptureScenario {
            followup_call: followup_probe.clone(),
            ..Default::default()
        },
    );
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../validate/stateless/tests/capture-reth27-vectors.json"
    ))
    .unwrap();
    assert_eq!(
        serde_json::json!(
            frozen
                .blocks
                .iter()
                .take(6)
                .map(|b| hex::encode(&b.hash))
                .collect::<Vec<_>>()
        ),
        expected[0]["blocks"]
    );
    cases.push(("frozen-reth27-gas-commitments".into(), frozen));
    for (outcome, code) in [
        ("success", "60015f5500"),
        ("revert", "60015f555f5ffd"),
        ("halt", "60015f55fe"),
    ] {
        for present in [false, true] {
            for value in [0u64, 5] {
                let sidecar = InboundSidecar::new(IncomingEntry {
                    target: PROBE,
                    source: Address::with_last_byte(0x50),
                    value: U256::from(value),
                    data: Bytes::new(),
                    source_rollup_id: RollupId::MAINNET,
                    l2_rollup_id: RollupId(1),
                    return_data: Bytes::new(),
                    // A failed target must fail native execution against this claimed success;
                    // the signer profile does not authorize unsuccessful settlement effects.
                    success: true,
                })
                .unwrap();
                let plans = build_inbound_system_txs(
                    &[sidecar],
                    &SystemTxContext {
                        eezl2_address: eez_primitives::EEZL2_ADDRESS,
                        l2_chain_id: 1,
                        this_rollup_id: 1,
                    },
                )
                .unwrap();
                let native_prefix = [1u8, 2]
                    .into_iter()
                    .map(|ordinal| {
                        plans[0]
                            .at_position(
                                eez_protocol::l2::GAS_LIMIT,
                                0,
                                B256::with_last_byte(ordinal),
                            )
                            .unwrap()
                            .into()
                    })
                    .collect();
                let accounts = if present {
                    vec![(
                        eez_primitives::SYSTEM_ADDRESS,
                        GenesisAccount {
                            nonce: Some(37),
                            storage: Some(
                                [(B256::ZERO, B256::with_last_byte(99))]
                                    .into_iter()
                                    .collect(),
                            ),
                            ..Default::default()
                        },
                    )]
                } else {
                    vec![]
                };
                let fixture = Fixture::with_capture_scenario(
                    false,
                    Some(hex::decode(code).unwrap()),
                    CaptureScenario {
                        accounts,
                        native_prefix,
                        omit_donation: true,
                        followup_call: followup_probe.clone(),
                        ..Default::default()
                    },
                );
                cases.push((
                    format!("two-incoming-{outcome}-present{present}-value{value}"),
                    fixture,
                ));
            }
        }
    }
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let created = signer.address().create(0);
    let runtime = hex::decode("36156009575f5fff005b602a5f5260205ff3").unwrap();
    let mut init = vec![
        0x60,
        runtime.len() as u8,
        0x60,
        12,
        0x60,
        0,
        0x39,
        0x60,
        runtime.len() as u8,
        0x60,
        0,
        0xf3,
    ];
    init.extend(runtime);
    cases.push((
        "create-then-later-selfdestruct".into(),
        Fixture::with_capture_scenario(
            true,
            None,
            CaptureScenario {
                followup_call: followup.then(|| (created, Bytes::new())),
                transactions: vec![(TxKind::Create, init.clone().into())],
                suffix_call: Some((created, Bytes::from_static(&[1]))),
                ..Default::default()
            },
        ),
    ));
    for balance in [0u64, 1] {
        cases.push((
            format!("create-over-prior-storage-balance{balance}-diagnostic"),
            Fixture::with_capture_scenario(
                true,
                None,
                CaptureScenario {
                    accounts: vec![(
                        created,
                        GenesisAccount {
                            balance: U256::from(balance),
                            storage: Some(
                                [(B256::ZERO, B256::with_last_byte(99))]
                                    .into_iter()
                                    .collect(),
                            ),
                            ..Default::default()
                        },
                    )],
                    retain_prior_storage_at: retain_prior_storage.then_some(created),
                    followup_call: followup.then(|| (created, Bytes::new())),
                    transactions: vec![(TxKind::Create, init.clone().into())],
                    suffix_call: Some((created, Bytes::from_static(&[1]))),
                    ..Default::default()
                },
            ),
        ));
    }
    cases.push((
        "storage-cleared-in-paired-user".into(),
        Fixture::with_capture_scenario(
            true,
            None,
            CaptureScenario {
                followup_call: followup_probe,
                accounts: vec![(
                    PROBE,
                    GenesisAccount {
                        nonce: Some(1),
                        code: Some(hex::decode("5f5f5500").unwrap().into()),
                        storage: Some(
                            [(B256::ZERO, B256::with_last_byte(9))]
                                .into_iter()
                                .collect(),
                        ),
                        ..Default::default()
                    },
                )],
                suffix_call: Some((PROBE, Bytes::new())),
                ..Default::default()
            },
        ),
    ));
    cases
}

/// Record every committed native update separately; the old witness probe retained only the last.
/// This is a second deterministic executor run, not instrumentation of the engine thread.
fn trace_commits(
    fixture: &Fixture,
    index: usize,
    name: &str,
) -> (Vec<eez_primitives::Receipt>, B256) {
    use alloy_consensus::{Transaction as _, Typed2718 as _};
    use alloy_primitives::U256;
    use reth_evm::{BlockExecutor as _, BlockTransactionResult as _};
    use reth_storage_api::HashedPostStateProvider as _;
    let wire = &fixture.blocks[index];
    let block: Block = alloy_rlp::decode_exact(&wire.rlp).unwrap();
    let recovered = block.clone().try_into_recovered().unwrap();
    let provider = super::capture_provider::Provider::new(
        &fixture.capture_states[index],
        &wire.witness.as_ref().unwrap().headers,
    );
    let db = reth_evm::database::StateProviderDatabase::new(
        reth_storage_api::EvmStateProviderAdapter(&provider),
    );
    let mut cache = evm2::evm::CacheDB::new(evm2::evm::Db::new(db));
    let sealed = SealedBlock::seal_slow(block.clone());
    let mut executor = fixture
        .capture_config
        .executor_for_block(&mut cache, &sealed)
        .unwrap();
    let updates = Arc::new(Mutex::new(Vec::<reth_evm::StateUpdate>::new()));
    let sink = updates.clone();
    assert!(executor.set_state_hook(move |update| sink.lock().unwrap().push(update)));
    executor.apply_pre_execution_changes().unwrap();
    let mut natives = 0;
    for (tx_index, tx) in recovered.transactions_recovered().enumerate() {
        let before = updates.lock().unwrap().len();
        let pending = executor.execute_transaction_without_commit(tx).unwrap();
        let tx = &block.body.transactions[tx_index];
        let native = eez_primitives::is_native_type(tx.ty()).then_some(tx);
        if let Some(tx) = native {
            natives += usize::from(tx.is_cross_chain_system());
            let delta = pending
                .result()
                .pending_state
                .account_changes_for(&eez_primitives::SYSTEM_ADDRESS);
            if name.contains("presenttrue") && tx.value() > U256::ZERO {
                let delta = delta.expect("native caller must be loaded");
                assert_eq!(delta.original, delta.current);
                let current = delta.current.unwrap();
                assert_eq!(current.nonce, 37);
                assert_eq!(current.balance, U256::ZERO);
                assert_eq!(current.code_hash, alloy_primitives::keccak256([]));
                assert!(!delta.storage.wiped);
                assert!(delta.storage.slots.is_empty());
            }
            println!(
                "NATIVE_PENDING case={name} block={} index={tx_index} value={} system={delta:?}",
                wire.number,
                tx.value()
            );
        }
        executor.commit_transaction(pending).unwrap();
        if let Some(tx) = native {
            let updates = updates.lock().unwrap();
            let slice = &updates[before..];
            // A zero-value no-op can emit no update; record that native separately too.
            println!(
                "NATIVE_COMMIT case={name} block={} index={tx_index} type={} updates={}",
                wire.number,
                tx.ty(),
                slice.len()
            );
            if tx.is_cross_chain_system() && name.contains("two-incoming") {
                assert_eq!(
                    executor.receipts().last().unwrap().success,
                    name.contains("incoming-success")
                );
            }
            let system = slice
                .iter()
                .filter_map(|u| u.account(&eez_primitives::SYSTEM_ADDRESS))
                .collect::<Vec<_>>();
            if name.contains("two-incoming") && tx.value() > U256::ZERO {
                if name.contains("presentfalse") {
                    assert_eq!(
                        system.len(),
                        1,
                        "every absent value-bearing native must retain its wipe update"
                    );
                    assert!(system[0].original.is_none());
                    assert!(system[0].current.is_none());
                    assert!(system[0].wiped);
                } else {
                    assert!(
                        system.is_empty(),
                        "unchanged retained SYSTEM must not emit a delta"
                    );
                }
            }
        }
    }
    if name.contains("two-incoming") && wire.number == 6 {
        assert_eq!(natives, 2);
    }
    let output = executor.finish().unwrap();
    let projected_root = provider.root(provider.hashed_post_state(&output.state).unwrap());
    if name.starts_with("create-over-prior-storage") {
        let signer =
            alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
        println!(
            "CREATE_BUNDLE {:?}",
            output.state.state.get(&signer.address().create(0))
        );
    }
    (output.result.receipts, projected_root)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_new_payload_dynamic_child_storage_matches_baseline() {
    let vectors: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("child-sload-reth27-vectors.json")).unwrap();
    for mode in [
        RootMode::Sparse,
        RootMode::FreshSparse,
        RootMode::Synchronous,
    ] {
        for balance in [0u64, 1] {
            let fixture = super::engine_child_sload::fixture(balance);
            let expected = vectors
                .iter()
                .find(|row| row["balance"] == balance)
                .unwrap();
            assert_eq!(fixture.blocks.len(), 7);
            for (index, block) in fixture.blocks.iter().enumerate() {
                assert_eq!(
                    hex::encode(&block.rlp),
                    expected["blocks"][index].as_str().unwrap(),
                    "dynamic child payload must equal the independent baseline"
                );
            }
            run_engine_case_mode(
                format!("dynamic-child-sload-balance{balance}-{mode:?}"),
                fixture,
                false,
                mode,
            )
            .await;
        }
    }
}

mod persistence;
