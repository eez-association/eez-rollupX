//! Real persistence, closed database owners and reopened canonical execution.
use super::*;
use alloy_consensus::TxReceipt as _;
use alloy_primitives::U256;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chainspec::ChainSpecProvider;
use reth_db::{DatabaseEnv, test_utils::TempDatabase};
use reth_node_types::NodeTypesWithDBAdapter;
use reth_provider::{
    BlockHashReader, BlockNumReader, BlockReader, HeaderProvider, ProviderFactory, ReceiptProvider,
    providers::{RocksDBProvider, StaticFileProvider},
};
use std::path::PathBuf;

type Factory = ProviderFactory<NodeTypesWithDBAdapter<TestNode, Arc<TempDatabase<DatabaseEnv>>>>;
const WATCHDOG: Duration = Duration::from_secs(30);

async fn persist_stage(factory: &Factory, fixture: &Fixture, first: usize, end: usize) {
    let initial_disk_height = factory.last_block_number().unwrap();
    let provider = BlockchainProvider::new(factory.clone()).unwrap();
    let canonical = provider.canonical_in_memory_state();
    let overlay = OverlayManager::default();
    // Keep this short chain in memory until Terminate, so acknowledgement must
    // cover the real shutdown flush rather than an earlier threshold write.
    let config = TreeConfig::default().with_state_root_fallback(false);
    let spec = factory.chain_spec();
    let consensus = Arc::new(reth_ethereum_consensus::EthBeaconConsensus::new(
        spec.clone(),
    ));
    let evm = fixture.capture_config.clone();
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
    );
    let pruner = reth_prune::PrunerBuilder::default().build_with_provider_factory(factory.clone());
    let (metrics_tx, _metrics_rx) = tokio::sync::mpsc::unbounded_channel();
    let persistence =
        PersistenceHandle::<EezPrimitives>::spawn_service(factory.clone(), pruner, metrics_tx);
    let (builder_tx, mut builder_rx) = tokio::sync::mpsc::unbounded_channel();
    let (engine, _events) = EngineApiTreeHandler::spawn_new(
        provider,
        consensus,
        validator,
        persistence.clone(),
        reth_payload_builder::PayloadBuilderHandle::new(builder_tx),
        canonical,
        overlay,
        config,
        EngineApiKind::Ethereum,
        evm,
        runtime,
    );
    for wire in &fixture.blocks[first..end] {
        let block: Block = alloy_rlp::decode_exact(&wire.rlp).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        engine
            .send(FromEngine::Request(EngineApiRequest::Beacon(
                BeaconEngineMessage::NewPayload {
                    cause: tracing::Span::none(),
                    payload: ExecutionData::from_block_unchecked(block.header.hash_slow(), &block),
                    tx,
                },
            )))
            .unwrap();
        let status = tokio::time::timeout(WATCHDOG, rx)
            .await
            .expect("newPayload deadline")
            .unwrap()
            .unwrap();
        assert!(
            status.is_valid(),
            "persisted-parent block {}: {status:?}",
            wire.number
        );
        assert_eq!(status.latest_valid_hash, Some(B256::from_slice(&wire.hash)));
    }
    let head = B256::from_slice(&fixture.blocks[end - 1].hash);
    let (tx, rx) = tokio::sync::oneshot::channel();
    engine
        .send(FromEngine::Request(EngineApiRequest::Beacon(
            BeaconEngineMessage::ForkchoiceUpdated {
                cause: tracing::Span::none(),
                state: ForkchoiceState {
                    head_block_hash: head,
                    safe_block_hash: B256::ZERO,
                    finalized_block_hash: B256::ZERO,
                },
                payload_attrs: None,
                tx,
            },
        )))
        .unwrap();
    let response = tokio::time::timeout(WATCHDOG, async {
        rx.await.unwrap().unwrap().await.unwrap()
    })
    .await
    .expect("forkchoice deadline");
    assert!(response.payload_status.is_valid(), "{response:?}");
    assert_eq!(response.payload_status.latest_valid_hash, Some(head));
    if initial_disk_height == first as u64 {
        assert_eq!(
            factory.last_block_number().unwrap(),
            first as u64,
            "threshold persistence must not make the shutdown assertion vacuous"
        );
    } else {
        assert_eq!(
            initial_disk_height, end as u64,
            "only a same-height sibling reorg is expected"
        );
        // The asynchronous unwind may already have removed the old tip, but the
        // replacement must still await the explicit shutdown flush.
        assert_ne!(factory.block_hash(end as u64).unwrap(), Some(head));
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    engine
        .send(FromEngine::Event(FromOrchestrator::Terminate { tx }))
        .unwrap();
    tokio::time::timeout(WATCHDOG, rx)
        .await
        .expect("shutdown flush deadline")
        .unwrap();
    drop(engine);
    assert!(
        builder_rx.try_recv().is_err(),
        "no payload builder request was authorized"
    );
    // The retained last handle joins the real persistence worker outside Tokio's workers.
    tokio::time::timeout(
        WATCHDOG,
        tokio::task::spawn_blocking(move || drop(persistence)),
    )
    .await
    .expect("persistence worker shutdown deadline")
    .unwrap();
    assert_eq!(factory.last_block_number().unwrap(), end as u64);
}

fn check_disk(factory: &Factory, fixture: &Fixture, height: usize, balance: u64) {
    check_disk_with_child_storage(
        factory,
        fixture,
        height,
        balance,
        if height >= 7 {
            U256::from(99)
        } else {
            U256::ZERO
        },
    );
}

fn check_disk_with_child_storage(
    factory: &Factory,
    fixture: &Fixture,
    height: usize,
    balance: u64,
    child_storage: U256,
) {
    assert_eq!(factory.last_block_number().unwrap(), height as u64);
    for wire in &fixture.blocks[..height] {
        let block: Block = alloy_rlp::decode_exact(&wire.rlp).unwrap();
        let stored = factory.block_by_number(wire.number).unwrap().unwrap();
        assert_eq!(alloy_rlp::encode(&stored), wire.rlp);
        let receipts = factory
            .receipts_by_block(wire.number.into())
            .unwrap()
            .unwrap();
        assert_eq!(receipts.len(), stored.body.transactions.len());
        assert_eq!(
            alloy_consensus::proofs::calculate_receipt_root(
                &receipts
                    .iter()
                    .map(|r| r.with_bloom_ref())
                    .collect::<Vec<_>>()
            ),
            stored.header.receipts_root
        );
        assert_eq!(
            factory.block_hash(wire.number).unwrap(),
            Some(block.header.hash_slow())
        );
        assert_eq!(
            factory.header_by_number(wire.number).unwrap().unwrap(),
            block.header
        );
    }
    if height >= 7 {
        check_persisted_witnesses(factory, fixture, balance);
    }
    let state = factory.latest().unwrap();
    let expected: Block = alloy_rlp::decode_exact(&fixture.blocks[height - 1].rlp).unwrap();
    assert_eq!(
        state.state_root(Default::default()).unwrap(),
        expected.header.state_root
    );
    let signer =
        alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let created = signer.address().create(0);
    assert_eq!(
        state.storage(created, B256::ZERO).unwrap(),
        Some(U256::from(99))
    );
    assert_eq!(
        state
            .storage(created, B256::with_last_byte(1))
            .unwrap()
            .unwrap_or_default(),
        child_storage
    );
    assert_eq!(
        state
            .basic_account(&eez_primitives::SYSTEM_ADDRESS)
            .unwrap()
            .map_or(U256::ZERO, |a| a.balance),
        U256::ZERO
    );
    println!(
        "PERSISTED_STORAGE balance={balance} height={height} root={:?} child_slot={}",
        expected.header.state_root, child_storage
    );
}

fn check_persisted_witnesses(factory: &Factory, fixture: &Fixture, balance: u64) {
    use eez_control_rpc::v2::ExecutionWitness;
    use prost::Message as _;
    let provider = BlockchainProvider::new(factory.clone()).unwrap();
    let mut admitted = Vec::new();
    for original in &fixture.blocks {
        let block: Block = alloy_rlp::decode_exact(&original.rlp).unwrap();
        let recovered = block.try_into_recovered().unwrap();
        let captured = eez_driver::witness::block_witness(
            &provider,
            &fixture.capture_config,
            &recovered,
            eez_driver::witness::ExecutionWitnessMode::Legacy,
        )
        .unwrap();
        assert_eq!(captured.rlp.as_ref(), original.rlp.as_slice());
        assert_eq!(captured.hash.as_slice(), original.hash.as_slice());
        let witness = captured.witness;
        let mut wire = original.clone();
        wire.witness = Some(ExecutionWitness {
            state: witness.state.into_iter().map(|b| b.to_vec()).collect(),
            codes: witness.codes.into_iter().map(|b| b.to_vec()).collect(),
            keys: witness.keys.into_iter().map(|b| b.to_vec()).collect(),
            headers: witness.headers.into_iter().map(|b| b.to_vec()).collect(),
            l1_headers_rlp: captured
                .l1_headers_rlp
                .into_iter()
                .map(|b| b.to_vec())
                .collect(),
        });
        println!(
            "PERSISTED_WITNESS balance={balance} block={} bytes={} digest={}",
            wire.number,
            wire.encoded_len(),
            alloy_primitives::keccak256(wire.encode_to_vec())
        );
        admitted.push(
            crate::window::AdmittedBlock::from_wire(
                wire,
                &mut 0,
                crate::window::WindowLimits {
                    blocks: 16,
                    payload_bytes: 16 * 1024 * 1024,
                    witness_items: 100_000,
                },
            )
            .unwrap(),
        );
    }
    let validated = crate::validate::Validator::stateless_for_test(
        fixture.genesis.clone(),
        eez_primitives::SYSTEM_ADDRESS,
    )
    .validate(&admitted)
    .unwrap();
    assert_eq!(
        validated.window_post_block_hash(),
        B256::from_slice(&fixture.blocks.last().unwrap().hash)
    );
    println!(
        "PERSISTED_WITNESS_REPLAY balance={balance} blocks={} valid=true",
        admitted.len()
    );
}

fn close_database(factory: Factory) -> PathBuf {
    let database = factory.db_ref().clone();
    let path = database.path().to_path_buf();
    drop(factory);
    let database = match Arc::try_unwrap(database) {
        Ok(db) => db,
        Err(db) => {
            let owners = Arc::strong_count(&db);
            std::mem::forget(db); // Preserve this task's failed disposable database.
            panic!("{owners} database owners remain after Engine/persistence shutdown: {path:?}");
        }
    };
    // Avoid TempDatabase's directory deletion while closing the actual MDBX environment.
    drop(database.into_inner_db());
    path
}

fn reopen(path: &std::path::Path, spec: Arc<ChainSpec>) -> Factory {
    let factory = ProviderFactory::new(
        Arc::new(TempDatabase::new(
            reth_db::init_db(path.join("db"), Default::default()).unwrap(),
            path.to_path_buf(),
        )),
        spec,
        StaticFileProvider::read_write(path.join("static_files")).unwrap(),
        RocksDBProvider::builder(path.join("rocksdb"))
            .with_default_tables()
            .build()
            .unwrap(),
        reth_tasks::Runtime::test(),
    )
    .unwrap();
    reth_db_common::init::init_genesis(&factory).unwrap();
    factory
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_shutdown_flush_reopens_and_executes_child_storage() {
    for balance in [0u64, 1] {
        let fixture = super::super::engine_child_sload::fixture(balance);
        let vectors: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../child-sload-reth27-vectors.json")).unwrap();
        let expected = vectors
            .iter()
            .find(|row| row["balance"] == balance)
            .unwrap();
        for (index, block) in fixture.blocks.iter().enumerate() {
            assert_eq!(
                hex::encode(&block.rlp),
                expected["blocks"][index].as_str().unwrap()
            );
        }
        let spec = Arc::new(eez_evm::profile::chain_spec(fixture.genesis.clone()).unwrap());
        let mut factory = reth_provider::test_utils::create_test_provider_factory_with_node_types::<
            TestNode,
        >(spec.clone());
        reth_db_common::init::init_genesis(&factory).unwrap();
        for (first, end) in [(0, 6), (6, 7)] {
            let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
            println!(
                "PERSISTENCE_DB balance={balance} first={first} path={}",
                factory.db_ref().path().display()
            );
            persist_stage(&factory, &fixture, first, end).await;
            check_disk(&factory, &fixture, end, balance);
            drop(diagnostic);
            let path = tokio::time::timeout(
                WATCHDOG,
                tokio::task::spawn_blocking(move || close_database(factory)),
            )
            .await
            .expect("database closure deadline")
            .unwrap();
            factory = reopen(&path, spec.clone());
            let diagnostic = KeepFailedDatabase(Some(factory.db_ref().clone()));
            check_disk(&factory, &fixture, end, balance);
            drop(diagnostic);
        }
        let path = tokio::time::timeout(
            WATCHDOG,
            tokio::task::spawn_blocking(move || close_database(factory)),
        )
        .await
        .expect("final database closure deadline")
        .unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
}

mod reorg;
