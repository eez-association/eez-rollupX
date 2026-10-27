use super::*;
use crate::{
    pool_transaction::EezPooledTransaction,
    rpc::{ReceiptBuilder, build_receipt, tests},
};
use eez_primitives::{EezPrimitives, engine::EezEngineTypes};
use eez_protocol::l1_history::{FIRST_CUTOFF_SLOT, HISTORY_ADDRESS};
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
use revm::Database;
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

fn eth_api(spec: Arc<ChainSpec>) -> EthApi<TestNode, Converter> {
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
    EthApiBuilder::new_with_components(node)
        .map_converter(|_| Converter::new(receipts))
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
    let expected_cutoff = if cached { U256::from(1004) } else { U256::ZERO };
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
                db.storage(HISTORY_ADDRESS, U256::from(FIRST_CUTOFF_SLOT))
                    .unwrap(),
                expected_cutoff,
            );
            entered_tx.send(()).unwrap();
            // Closing the sender also unblocks this worker if an assertion in
            // the controlling task fails, so failure cannot hang runtime teardown.
            if release_rx.recv().is_err() {
                return Err(internal("callback test released during failure"));
            }
            assert_eq!(worker.context.is_some(), cached);
            assert_eq!(
                db.storage(HISTORY_ADDRESS, U256::from(FIRST_CUTOFF_SLOT))
                    .unwrap(),
                expected_cutoff,
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
}

#[tokio::test]
async fn cancelled_cached_callback_retains_trace_quota() {
    assert_callback_retains_trace_quota(true).await;
}

#[tokio::test]
async fn cancelled_stored_callback_retains_trace_quota() {
    assert_callback_retains_trace_quota(false).await;
}
