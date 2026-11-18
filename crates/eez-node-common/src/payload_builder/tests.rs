//! Actual production payload construction using an Engine-loaned sparse-root task.
use super::*;
use crate::{EezNode, pool_transaction::EezPooledTransaction};
use alloy_consensus::{Header, SignableTransaction, TxLegacy};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind};
use alloy_rpc_types_engine::{ExecutionData, ForkchoiceState};
use eez_evm::EezEvmConfig;
use eez_primitives::{Block, EEZL2_ADDRESS, SYSTEM_ADDRESS, engine::EezEngineTypes};
use eez_protocol::l1_history::{HeaderChain, HeaderRules, WitnessHistory};
use reth_chainspec::EthChainSpec;
use reth_engine_primitives::{BeaconEngineMessage, NoopInvalidBlockHook, PayloadValidator};
use reth_engine_tree::{
    chain::FromOrchestrator,
    engine::{EngineApiKind, EngineApiRequest, FromEngine},
    persistence::{PersistenceAction, PersistenceHandle},
    tree::{BasicEngineValidator, EngineApiTreeHandler, TreeConfig},
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadServiceCommand};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use reth_provider::providers::BlockchainProvider;
use reth_storage_overlay::OverlayManager;
use reth_transaction_pool::{TransactionOrigin, identifier::TransactionId};
use reth_trie_parallel::state_root_task::PayloadStateRootHandle;
use std::{
    collections::VecDeque,
    sync::mpsc,
    time::{Duration, Instant},
};

// Retain only this test's disposable database if an assertion fails.
struct KeepFailedDatabase<D>(Option<Arc<D>>);
impl<D> Drop for KeepFailedDatabase<D> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::mem::forget(self.0.take());
        }
    }
}

const WATCHDOG: Duration = Duration::from_secs(30);
type PoolTx = Arc<ValidPoolTransaction<EezPooledTransaction>>;
struct Candidates(VecDeque<PoolTx>);
impl Iterator for Candidates {
    type Item = PoolTx;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.pop_front()
    }
}
impl BestTransactions for Candidates {
    fn mark_invalid(&mut self, _: &PoolTx, reason: InvalidPoolTransactionError) {
        panic!("fixture transaction rejected: {reason:?}");
    }
    fn no_updates(&mut self) {}
    fn set_skip_blobs(&mut self, _: bool) {}
}

#[derive(Clone)]
struct PayloadAdapter(
    reth_ethereum_payload_builder::EthereumExecutionPayloadValidator<reth_chainspec::ChainSpec>,
);
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

fn fixture(donations: usize, history: bool) -> (EezEvmConfig, Vec<PoolTx>) {
    let l1 = Header {
        number: u64::from(history),
        ..Default::default()
    };
    let mut chain = HeaderChain::default();
    chain
        .append(
            &[alloy_rlp::encode(&l1).into()],
            &HeaderRules {
                london_block: None,
                shanghai_time: None,
                cancun_time: None,
                prague_time: None,
                unsupported_time: None,
            },
            64,
        )
        .unwrap();
    let mut genesis: alloy_genesis::Genesis =
        serde_json::from_slice(include_bytes!("../../../../genesis.json")).unwrap();
    genesis.timestamp = 4;
    genesis.mix_hash = B256::ZERO;
    genesis.config.chain_id = 901;
    let profile = genesis.config.extra_fields.get_mut("rollupX").unwrap();
    profile["l1Reference"] = serde_json::json!({"chainId":1337,"number":l1.number,"hash":l1.hash_slow(),"timestamp":0,"prevRandao":B256::ZERO});
    profile["blockIntervalSeconds"] = serde_json::json!(1);
    profile["syncIntervalBlocks"] = serde_json::json!(4);
    profile["historyUpdateOffset"] = serde_json::json!(if history { 0 } else { 2 });
    genesis
        .alloc
        .get_mut(&eez_protocol::l1_history::HISTORY_ADDRESS)
        .unwrap()
        .storage = Some(eez_protocol::l1_history::genesis_storage());
    // A payable STOP stub isolates terminal cleanup from the manager's ABI.
    genesis.alloc.get_mut(&EEZL2_ADDRESS).unwrap().code = Some(Bytes::from_static(&[0]));
    let mut transactions = Vec::new();
    if donations != 0 {
        for index in 0..donations {
            let target = [SYSTEM_ADDRESS, EEZL2_ADDRESS][index % 2];
            let tx: EezTxEnvelope = TxLegacy {
                chain_id: Some(901),
                gas_limit: 21_000,
                gas_price: 1_000_000_000_000,
                to: TxKind::Call(target),
                value: U256::from(11 + index),
                ..Default::default()
            }
            .into_signed(Signature::new(U256::ONE, U256::ONE, false))
            .into();
            let recovered = tx.try_into_recovered().unwrap();
            genesis.alloc.entry(recovered.signer()).or_default().balance =
                U256::from(10u64).pow(U256::from(20));
            transactions.push(Arc::new(ValidPoolTransaction {
                transaction: EezPooledTransaction::try_from_consensus(recovered).unwrap(),
                transaction_id: TransactionId::new((index as u64).into(), 0),
                propagate: true,
                timestamp: Instant::now(),
                origin: TransactionOrigin::Local,
                authority_ids: None,
            }));
        }
    }
    let spec = Arc::new(eez_evm::profile::chain_spec(genesis).unwrap());
    let config = EezEvmConfig::new(spec)
        .with_l1_history(Arc::new(WitnessHistory(Arc::new(chain))))
        .unwrap();
    (config, transactions)
}

// Calculate this fixture's complete terminal state without executing either builder path.
fn expected_root(spec: &reth_chainspec::ChainSpec, txs: &[PoolTx], recorded_history: bool) -> B256 {
    let mut alloc = spec.genesis.alloc.clone();
    assert!(
        !alloc.contains_key(&alloy_eips::eip4788::BEACON_ROOTS_ADDRESS),
        "fixture has no beacon-roots contract"
    );
    let history = alloc
        .get_mut(&alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS)
        .unwrap();
    history
        .storage
        .get_or_insert_default()
        .insert(B256::ZERO, spec.genesis_header().hash_slow());
    if recorded_history {
        let storage = alloc
            .get_mut(&eez_protocol::l1_history::HISTORY_ADDRESS)
            .unwrap()
            .storage
            .as_mut()
            .unwrap();
        storage.insert(
            B256::from(U256::from(eez_protocol::l1_history::ENDPOINT_SLOT)),
            B256::from(U256::ONE),
        );
        storage.insert(
            B256::from(eez_protocol::l1_history::hash_storage_key(1)),
            serde_json::from_value(
                spec.genesis.config.extra_fields["rollupX"]["l1Reference"]["hash"].clone(),
            )
            .unwrap(),
        );
    }
    let base_fee = spec
        .genesis_header()
        .next_block_base_fee(eez_protocol::l2::BASE_FEE_PARAMS)
        .unwrap();
    let collector: Address =
        serde_json::from_value(spec.genesis.config.extra_fields["rollupX"]["feeCollector"].clone())
            .unwrap();
    for (index, tx) in txs.iter().enumerate() {
        let sender = alloc.get_mut(&tx.transaction.sender()).unwrap();
        assert_eq!(sender.nonce.unwrap_or_default(), 0);
        sender.nonce = Some(1);
        sender.balance -=
            U256::from(21_000u64) * U256::from(1_000_000_000_000u64) + U256::from(11 + index);
        alloc.entry(collector).or_default().balance += U256::from(21_000u64) * U256::from(base_fee);
        alloc.entry(Address::ZERO).or_default().balance +=
            U256::from(21_000u64) * U256::from(1_000_000_000_000u64 - base_fee);
    }
    // The SYSTEM donation creates an empty account that terminal cleanup deletes.
    // The manager's code-bearing account survives with its original zero balance.
    assert!(!alloc.contains_key(&SYSTEM_ADDRESS));
    assert!(alloc[&EEZL2_ADDRESS].balance.is_zero());
    reth_trie_common::root::state_root_ref_unhashed(&alloc)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_payload_streamed_root_includes_terminal_cleanup() {
    for (donations, history) in [
        (0, false),
        (2, false),
        (0, true),
        (2, true),
        (6, false),
        (6, true),
    ] {
        let (evm, transactions) = fixture(donations, history);
        let spec = evm.chain_spec().clone();
        let expected = expected_root(&spec, &transactions, history);
        let factory = reth_provider::test_utils::create_test_provider_factory_with_node_types::<
            EezNode,
        >(spec.clone());
        let genesis_hash = reth_db_common::init::init_genesis(&factory).unwrap();
        println!(
            "PAYLOAD_ROOT_DB donations={donations} history={history} path={}",
            factory.db_ref().path().display()
        );
        let _diagnostic_db = KeepFailedDatabase(Some(factory.db_ref().clone()));
        let provider = BlockchainProvider::new(factory).unwrap();
        let canonical = provider.canonical_in_memory_state();
        let overlay = OverlayManager::default();
        let config = TreeConfig::default()
            .with_has_enough_parallelism(true)
            .with_share_sparse_trie_with_payload_builder(true)
            .with_state_root_fallback(false)
            .with_state_root_task_timeout(None);
        let runtime = reth_tasks::Runtime::test();
        let consensus = Arc::new(reth_ethereum_consensus::EthBeaconConsensus::new(
            spec.clone(),
        ));
        let validator = BasicEngineValidator::new(
            provider.clone(),
            consensus.clone(),
            evm.clone(),
            PayloadAdapter(
                reth_ethereum_payload_builder::EthereumExecutionPayloadValidator::new(spec.clone()),
            ),
            config.clone(),
            Box::new(NoopInvalidBlockHook::default()),
            overlay.clone(),
            runtime.clone(),
        );
        let (persist_tx, persist_rx) = mpsc::channel();
        let (builder_tx, mut builder_rx) = tokio::sync::mpsc::unbounded_channel();
        let (engine, _events) = EngineApiTreeHandler::spawn_new(
            provider.clone(),
            consensus,
            validator,
            PersistenceHandle::new(persist_tx),
            PayloadBuilderHandle::new(builder_tx),
            canonical,
            overlay,
            config,
            EngineApiKind::Ethereum,
            evm.clone(),
            runtime,
        );
        let mut attributes =
            EthPayloadAttributes::from(alloy_rpc_types_engine::PayloadAttributes {
                timestamp: 5,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Some(Vec::new()),
                parent_beacon_block_root: Some(B256::ZERO),
                ..Default::default()
            });
        if history {
            let tx: EezTxEnvelope = eez_primitives::HistoryTransaction {
                version: eez_primitives::HISTORY_TX_VERSION,
                chain_id: 901,
                blocks: vec![eez_primitives::HistoryBlock {
                    number: 1,
                    block_hash: serde_json::from_value(
                        spec.genesis.config.extra_fields["rollupX"]["l1Reference"]["hash"].clone(),
                    )
                    .unwrap(),
                }],
            }
            .into();
            attributes.history_transaction = Some(tx.encoded_2718().into());
        }
        let payload_id = attributes.payload_id(&genesis_hash);
        let (tx, rx) = tokio::sync::oneshot::channel();
        engine
            .send(FromEngine::Request(EngineApiRequest::Beacon(
                BeaconEngineMessage::ForkchoiceUpdated {
                    cause: tracing::Span::none(),
                    state: ForkchoiceState {
                        head_block_hash: genesis_hash,
                        safe_block_hash: genesis_hash,
                        finalized_block_hash: genesis_hash,
                    },
                    payload_attrs: Some(attributes.clone()),
                    tx,
                },
            )))
            .unwrap();
        let command = tokio::time::timeout(WATCHDOG, builder_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let PayloadServiceCommand::BuildNewPayload(mut request, _, response) = command else {
            panic!("expected real payload build request")
        };
        assert_eq!(request.parent_hash, genesis_hash);
        response.send(Ok(payload_id)).unwrap();
        let update = tokio::time::timeout(WATCHDOG, rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(WATCHDOG, update)
                .await
                .unwrap()
                .unwrap()
                .payload_status
                .is_valid()
        );
        let mut original = request
            .resources
            .take_state_root_handle()
            .expect("actual sparse task required");
        // Forward the real task's result so fallback cannot masquerade as streamed success.
        let hook = original.take_state_hook();
        let (root_tx, root_rx) = mpsc::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let outcome = original
                .take_state_root_rx()
                .recv_timeout(WATCHDOG)
                .expect("real root task timed out; dropping its handle requests cancellation");
            drop(original);
            observed_tx
                .send(
                    outcome
                        .as_ref()
                        .map(|o| o.state_root)
                        .map_err(ToString::to_string),
                )
                .unwrap();
            root_tx.send(outcome).unwrap();
        });
        let handle = PayloadStateRootHandle::new("audited-real-task", Some(hook), root_rx, None);
        let parent = Arc::new(SealedHeader::new(
            spec.genesis_header().clone(),
            genesis_hash,
        ));
        let mut built = Vec::new();
        for task in [None, Some(handle)] {
            let args = BuildArguments::new(
                Default::default(),
                None,
                task,
                PayloadConfig::new(parent.clone(), attributes.clone(), payload_id),
                Default::default(),
                None,
            );
            let txs = transactions.clone();
            let outcome = tokio::task::spawn_blocking({
                let evm = evm.clone();
                let provider = provider.clone();
                move || {
                    build_payload(
                        evm,
                        provider,
                        EthereumBuilderConfig::default(),
                        args,
                        |_| Box::new(Candidates(txs.into())),
                    )
                }
            });
            built.push(
                tokio::time::timeout(WATCHDOG, outcome)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_payload()
                    .unwrap(),
            );
        }
        let streamed_root = observed_rx
            .recv_timeout(WATCHDOG)
            .unwrap()
            .expect("sparse task must succeed without fallback");
        worker.join().unwrap();
        assert_eq!(streamed_root, built[1].block().header().state_root);
        assert_eq!(streamed_root, expected, "independent complete-state oracle");
        let ordinary_gas = donations as u64 * 21_000;
        if history {
            assert!(built[1].block().header().gas_used > ordinary_gas + 21_000);
        } else {
            assert_eq!(built[1].block().header().gas_used, ordinary_gas);
        }
        assert_eq!(
            alloy_rlp::encode(built[0].block()),
            alloy_rlp::encode(built[1].block())
        );
        assert_eq!(
            built[1].block().body().transactions.len(),
            donations + usize::from(history)
        );
        // Re-execute the actual producer bytes through Engine validation; wrong roots must fail.
        for valid in [true, false] {
            let mut block = built[1].block().clone_block();
            if !valid {
                block.header.state_root = B256::repeat_byte(0xa5);
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            engine
                .send(FromEngine::Request(EngineApiRequest::Beacon(
                    BeaconEngineMessage::NewPayload {
                        cause: tracing::Span::none(),
                        payload: ExecutionData::from_block_unchecked(
                            block.header.hash_slow(),
                            &block,
                        ),
                        tx,
                    },
                )))
                .unwrap();
            let status = tokio::time::timeout(WATCHDOG, rx)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if valid {
                assert!(status.is_valid(), "{status:?}");
            } else {
                assert!(status.is_invalid(), "{status:?}");
                assert!(
                    status
                        .status
                        .validation_error()
                        .unwrap()
                        .to_ascii_lowercase()
                        .contains("state root"),
                    "{status:?}"
                );
            }
        }
        let (done, completion) = tokio::sync::oneshot::channel();
        engine
            .send(FromEngine::Event(FromOrchestrator::Terminate { tx: done }))
            .unwrap();
        tokio::time::timeout(WATCHDOG, completion)
            .await
            .unwrap()
            .unwrap();
        drop(provider);
        for action in persist_rx.try_iter() {
            assert!(
                matches!(
                    action,
                    PersistenceAction::SaveFinalizedBlock(0) | PersistenceAction::SaveSafeBlock(0)
                ),
                "genesis-only forkchoice must not save or remove blocks: {action:?}"
            );
        }
    }
}
