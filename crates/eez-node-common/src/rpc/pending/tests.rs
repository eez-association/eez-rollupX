use super::*;
use crate::{
    pool_transaction::EezPooledTransaction,
    rpc::{ReceiptBuilder, build_receipt, tests},
};
use eez_primitives::{EezPrimitives, engine::EezEngineTypes};
use eez_protocol::l1_history::{HISTORY_ADDRESS, IMPLEMENTATION_SLOT};
use reth_chainspec::{ChainSpec, EthChainSpec as _};
use reth_db::mock::DatabaseMock;
use reth_eth_wire_types::BasicNetworkPrimitives;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_network_api::noop::NoopNetwork;
use reth_node_api::FullNodeTypesAdapter;
use reth_node_builder::{NodeAdapter, components::Components};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::test_utils::NoopProvider;
use reth_rpc::EthApiBuilder;
use reth_rpc_eth_types::receipt::EthReceiptConverter;
use reth_transaction_pool::noop::NoopTransactionPool;
use std::{sync::mpsc, time::Duration};
use tokio::{sync::oneshot, time::timeout};

const WATCHDOG: Duration = Duration::from_secs(10);
type TestProvider = NoopProvider<ChainSpec, EezPrimitives>;
type TestTypes = FullNodeTypesAdapter<EezNode, DatabaseMock, TestProvider>;
type TestComponents = Components<
    TestTypes,
    NoopNetwork<BasicNetworkPrimitives<EezPrimitives, PooledTransactionVariant>>,
    NoopTransactionPool<EezPooledTransaction>,
    EezEvmConfig,
    Arc<EthBeaconConsensus<ChainSpec>>,
>;
type TestNode = NodeAdapter<TestTypes, TestComponents>;

fn eth_api(spec: Arc<ChainSpec>) -> EthApi<RpcNodeCoreFor<TestNode>, Converter> {
    let receipts =
        EthReceiptConverter::new(spec.clone()).with_builder(build_receipt as ReceiptBuilder);
    let node = NodeAdapter {
        provider: TestProvider::new(spec.clone()),
        task_executor: Runtime::test(),
        components: TestComponents {
            transaction_pool: NoopTransactionPool::new(),
            evm_config: EezEvmConfig::new(spec.clone()),
            consensus: Arc::new(EthBeaconConsensus::new(spec)),
            network: NoopNetwork::new(),
            payload_builder_handle: PayloadBuilderHandle::<EezEngineTypes>::noop(),
        },
    };
    EthApiBuilder::new_with_components(reth_rpc_eth_api::node::RpcNodeCoreAdapter::new(
        node.provider,
        node.components.transaction_pool,
        node.components.network,
        node.components.evm_config,
    ))
    .map_converter(|_| {
        reth_rpc_convert::RpcConverter::new(receipts)
            .with_tx_env_converter(super::super::EezTxEnvConverter)
    })
    .with_pending_env_builder(EezEthApiBuilder::default())
    .blocking_task_pool(BlockingTaskPool::new(
        BlockingTaskPool::builder().num_threads(1).build().unwrap(),
    ))
    .build()
}

async fn assert_callback_retains_trace_quota(cached: bool) {
    let spec = tests::profile(&tests::l1_blocks(false)[0]);
    let parent = SealedHeader::seal_slow(spec.genesis_header().clone());
    let inner = eth_api(spec.clone());
    let io_guard = Arc::clone(inner.blocking_io_task_guard());
    let initial_io_permits = io_guard.available_permits();
    let context = if cached {
        let attributes = NextBlockEnvAttributes {
            timestamp: parent.timestamp + 1,
            suggested_fee_recipient: parent.beneficiary,
            prev_randao: parent.mix_hash,
            gas_limit: parent.gas_limit,
            parent_beacon_block_root: Some(B256::ZERO),
            withdrawals: Some(Default::default()),
            extra_data: parent.extra_data.clone(),
            slot_number: None,
        };
        Some(Arc::new(PendingContext {
            parent_hash: parent.hash(),
            evm_config: inner.evm_config().clone(),
            env: inner
                .evm_config()
                .next_evm_env(&parent, &attributes)
                .unwrap(),
            state: Mutex::new(Some(tests::pending_genesis_db(&spec))),
            _permit: inner.acquire_owned_blocking_io().await.unwrap(),
        }))
    } else {
        None
    };
    let expected_implementation = if cached {
        U256::from_be_bytes(
            eez_protocol::l1_history::HISTORY_IMPLEMENTATION_ADDRESS
                .into_word()
                .0,
        )
    } else {
        U256::ZERO
    };
    let trace_guard = BlockingTaskGuard::new(1);
    let api = SimulationApi {
        inner,
        context,
        trace_permit: Some(Arc::new(trace_guard.clone().acquire_owned().await.unwrap())),
    };
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = oneshot::channel();
    // Only the request owns the adapter. The callback must acquire its copy
    // through the production dispatch path; an external clone would mask a leak.
    let request = tokio::spawn(async move {
        api.spawn_with_state_at_block(parent.hash(), move |worker, mut db| {
            assert_eq!(
                db.get_storage(
                    &HISTORY_ADDRESS,
                    &U256::from_be_bytes(IMPLEMENTATION_SLOT.0)
                )
                .unwrap(),
                expected_implementation,
            );
            entered_tx.send(()).unwrap();
            // Closing the sender also unblocks this worker if an assertion in
            // the controlling task fails, so failure cannot hang runtime teardown.
            if release_rx.recv().is_err() {
                return Err(internal("callback test released during failure"));
            }
            assert_eq!(worker.context.is_some(), cached);
            assert_eq!(
                db.get_storage(
                    &HISTORY_ADDRESS,
                    &U256::from_be_bytes(IMPLEMENTATION_SLOT.0)
                )
                .unwrap(),
                expected_implementation,
            );
            finished_tx.send(()).unwrap();
            Ok(())
        })
        .await
    });
    timeout(WATCHDOG, entered_rx)
        .await
        .expect("callback did not start")
        .expect("callback failed before entering");
    request.abort();
    assert!(
        timeout(WATCHDOG, request)
            .await
            .expect("request cancellation did not finish")
            .unwrap_err()
            .is_cancelled(),
    );

    // The cancelled request has been dropped, while the real callback is still
    // blocked on release_rx. No time interval is used to infer that ordering.
    assert_eq!(
        io_guard.available_permits(),
        initial_io_permits - usize::from(cached),
        "cancellation must retain the prepared prefix's IO permit until its worker exits",
    );
    let next = trace_guard.acquire_owned();
    tokio::pin!(next);
    assert!(futures_util::poll!(next.as_mut()).is_pending());
    release_tx.send(()).unwrap();
    let permit = timeout(WATCHDOG, next)
        .await
        .expect("callback did not release the shared trace quota")
        .unwrap();
    timeout(WATCHDOG, finished_rx)
        .await
        .expect("callback did not finish")
        .expect("callback failed after cancellation");
    drop(permit);
    let all_io = timeout(
        WATCHDOG,
        io_guard.acquire_many_owned(u32::try_from(initial_io_permits).unwrap()),
    )
    .await
    .expect("finished callback did not release its IO permit")
    .unwrap();
    drop(all_io);
}

#[tokio::test]
async fn cancelled_cached_callback_retains_trace_quota() {
    assert_callback_retains_trace_quota(true).await;
}

#[tokio::test]
async fn cancelled_stored_callback_retains_trace_quota() {
    assert_callback_retains_trace_quota(false).await;
}

#[test]
fn estimate_provider_preserves_cached_writes_clears_deletion_and_code() {
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_storage_api::StateProviderBox;

    let address = Address::with_last_byte(1);
    let deleted = Address::with_last_byte(2);
    let backing = MockEthProvider::default();
    for account in [address, deleted] {
        backing.add_account(
            account,
            ExtendedAccount::new(1, U256::from(3)).extend_storage([
                (B256::ZERO, U256::from(9)),
                (B256::with_last_byte(1), U256::from(7)),
            ]),
        );
    }
    let provider: StateProviderBox = Box::new(backing);
    let mut db = CacheDB::new(Db::new(StateProviderDatabase::new(
        provider.into_evm_state_provider(),
    )));
    let code = evm2::bytecode::Bytecode::new_raw(Bytes::from_static(&[0x60, 0x01, 0x00]));
    let hash = code.hash_slow();
    db.insert_account_info(
        &address,
        evm2::evm::AccountInfo {
            nonce: 2,
            balance: U256::from(4),
            code_hash: hash,
            code: Some(code.clone()),
            ..Default::default()
        },
    );
    db.insert_account_storage(&address, &U256::ZERO, &U256::from(42));
    db.insert_account_storage(&address, &U256::from(1), &U256::ZERO);
    db.cache.accounts.insert(deleted, None);
    db.cache.storage.entry(deleted).or_default().wipe();
    let block_hash = B256::repeat_byte(0x17);
    db.cache.block_hashes.insert(U256::from(1), block_hash);
    let prefix = ExecutedPrefix(Mutex::new(db));
    assert_eq!(
        prefix.storage(address, B256::ZERO).unwrap(),
        Some(U256::from(42))
    );
    assert_eq!(
        prefix.storage(address, B256::with_last_byte(1)).unwrap(),
        Some(U256::ZERO)
    );
    assert_eq!(
        prefix.storage(deleted, B256::ZERO).unwrap(),
        Some(U256::ZERO)
    );
    assert!(prefix.basic_account(&deleted).unwrap().is_none());
    let account = prefix.basic_account(&address).unwrap().unwrap();
    assert_eq!((account.nonce, account.balance), (2, U256::from(4)));
    assert_eq!(
        prefix.bytecode_by_hash(&hash).unwrap(),
        Some(Bytecode(code))
    );
    assert_eq!(prefix.block_hash(1).unwrap(), Some(block_hash));
}
