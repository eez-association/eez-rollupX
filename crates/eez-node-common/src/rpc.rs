//! Ethereum RPC methods with EEZ transaction and receipt response types.
//!
//! RPC wiring adapted from reth's Ethereum node at the pinned revision:
//! <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/ethereum/node/src/node.rs>.
//! Receipt/log conversion adapted from `EthReceiptConverter`:
//! <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/rpc/rpc-eth-types/src/receipt.rs>.

use crate::node::{EezEngineValidatorBuilder, EezNode};
use alloy_consensus::{Receipt as RpcReceipt, TxReceipt};
use alloy_consensus_any::AnyReceiptEnvelope;
use alloy_rpc_types_eth::{Header, Log, Transaction, TransactionReceipt, TransactionRequest};
use eez_evm::EezEvmConfig;
use eez_primitives::{EezTxEnvelope, Receipt};
use reth_chainspec::{ChainSpec, ChainSpecProvider};
use reth_node_api::FullNodeComponents;
use reth_node_builder::rpc::{
    BasicEngineApiBuilder, BasicEngineValidatorBuilder, EthApiBuilder, EthApiCtx, Identity,
    RpcAddOns,
};
use reth_primitives_traits::{SealedHeader, TransactionMeta};
use reth_rpc::EthApi;
use reth_rpc_convert::{RpcConverter, RpcTypes};
use reth_rpc_eth_api::helpers::pending_block::PendingEnvBuilder;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_eth_types::receipt::EthReceiptConverter;
use std::future::ready;

#[cfg(feature = "l1-startup")]
mod pending;

#[derive(Debug, Clone)]
pub struct EezRpcTypes;

impl RpcTypes for EezRpcTypes {
    type Log = Log;
    type Header = Header;
    type Receipt = TransactionReceipt<AnyReceiptEnvelope<Log>>;
    type TransactionResponse = Transaction<EezTxEnvelope>;
    type TransactionRequest = TransactionRequest;
}

type ReceiptBuilder = fn(Receipt, usize, TransactionMeta) -> AnyReceiptEnvelope<Log>;
type Converter =
    RpcConverter<EezRpcTypes, EezEvmConfig, EthReceiptConverter<ChainSpec, ReceiptBuilder>>;

#[derive(Debug, Default, Clone)]
pub struct EezEthApiBuilder {
    #[cfg(feature = "l1-startup")]
    state: Option<(
        std::sync::Arc<eez_l1::L1CanonicalHead>,
        std::sync::Arc<eez_deriver::CorrelationStore>,
    )>,
}

impl EezEthApiBuilder {
    /// Use the deriver's shared canonical history for pending RPC simulation.
    /// The default builder rejects derived pending requests until configured.
    #[cfg(feature = "l1-startup")]
    pub fn new(
        l1_head: std::sync::Arc<eez_l1::L1CanonicalHead>,
        correlations: std::sync::Arc<eez_deriver::CorrelationStore>,
    ) -> Self {
        Self {
            state: Some((l1_head, correlations)),
        }
    }

    fn next_block_context(
        &self,
        parent: &SealedHeader,
    ) -> Result<(u64, alloy_primitives::B256), EthApiError> {
        #[cfg(feature = "l1-startup")]
        {
            let (l1_head, correlations) = self.state.as_ref().ok_or_else(|| {
                EthApiError::Internal(reth_errors::RethError::msg(
                    "L1 derivation is not configured for pending RPC simulation",
                ))
            })?;
            // Genesis provenance is installed before boot restores the accepted
            // anchors. Reuse the public-query gate for that entire catch-up.
            if !correlations.is_ready() {
                return Err(EthApiError::Internal(reth_errors::RethError::msg(
                    "L1 derivation is still validating/catching up",
                )));
            }
            l1_head
                .next_block_context(parent)
                .and_then(|context @ (timestamp, _)| {
                    let history = l1_head.derivation_schedule()?.history()?;
                    // Reth's derived pending call uses the parent's state; its
                    // environment builder cannot apply a possible nonempty import.
                    // Only the executed-prefix adapter can safely serve that boundary.
                    if history.source_cutoff(parent.timestamp)?
                        != history.source_cutoff(timestamp)?
                    {
                        return Err(eez_l1::derivation::DerivationError::Unavailable(
                            "pending L1 history import has not executed",
                        ));
                    }
                    Ok(context)
                })
                .map_err(|error| EthApiError::Internal(reth_errors::RethError::other(error)))
        }
        #[cfg(not(feature = "l1-startup"))]
        {
            let _ = parent;
            Err(EthApiError::Internal(reth_errors::RethError::msg(
                "L1 derivation is not configured for pending RPC simulation",
            )))
        }
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> EthApiBuilder<N>
    for EezEthApiBuilder
{
    type EthApi = EthApi<N, Converter>;

    fn build_eth_api(
        self,
        ctx: EthApiCtx<'_, N>,
    ) -> impl Future<Output = eyre::Result<Self::EthApi>> {
        let receipts = EthReceiptConverter::new(ctx.components.provider().chain_spec())
            .with_builder(build_receipt as ReceiptBuilder);
        ready(Ok(ctx
            .eth_api_builder()
            .map_converter(|_| Converter::new(receipts))
            .with_pending_env_builder(self)
            .build()))
    }
}

// Only the fallback for an absent executed pending payload reaches this builder.
// Pending simulation must consult canonical history and cannot synthesize an
// import in the environment alone. Executed pending payloads use their own state.
impl PendingEnvBuilder<EezEvmConfig> for EezEthApiBuilder {
    fn pending_env_attributes(
        &self,
        parent: &SealedHeader,
        overrides: Option<&alloy_rpc_types_eth::BlockOverrides>,
    ) -> Result<reth_evm::NextBlockEnvAttributes, EthApiError> {
        let (timestamp, prev_randao) = self.next_block_context(parent)?;
        if overrides
            .and_then(|overrides| overrides.beacon_root)
            .is_some_and(|root| root != alloy_primitives::B256::ZERO)
        {
            return Err(EthApiError::InvalidParams(
                "L2 beacon root must be zero".into(),
            ));
        }
        Ok(reth_evm::NextBlockEnvAttributes {
            timestamp,
            suggested_fee_recipient: parent.beneficiary,
            prev_randao,
            gas_limit: parent.gas_limit,
            parent_beacon_block_root: Some(alloy_primitives::B256::ZERO),
            withdrawals: Some(Default::default()),
            extra_data: parent.extra_data.clone(),
            slot_number: None,
        })
    }
}

fn build_receipt(
    receipt: Receipt,
    next_log_index: usize,
    meta: TransactionMeta,
) -> AnyReceiptEnvelope<Log> {
    let logs_bloom = receipt.bloom();
    let r#type = receipt.tx_type.into();
    let logs = receipt
        .logs
        .into_iter()
        .enumerate()
        .map(|(index, inner)| Log {
            inner,
            block_hash: Some(meta.block_hash),
            block_number: Some(meta.block_number),
            block_timestamp: Some(meta.timestamp),
            transaction_hash: Some(meta.tx_hash),
            transaction_index: Some(meta.index),
            log_index: Some((next_log_index + index) as u64),
            removed: false,
        })
        .collect();
    AnyReceiptEnvelope {
        r#type,
        inner: alloy_consensus::ReceiptWithBloom {
            logs_bloom,
            receipt: RpcReceipt {
                status: receipt.success.into(),
                cumulative_gas_used: receipt.cumulative_gas_used,
                logs,
            },
        },
    }
}

pub fn eez_add_ons<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>>(
    eth: EezEthApiBuilder,
) -> RpcAddOns<N, EezEthApiBuilder, EezEngineValidatorBuilder> {
    #[cfg(feature = "l1-startup")]
    let pending = eth.clone();
    let add_ons = RpcAddOns::new(
        eth,
        EezEngineValidatorBuilder,
        BasicEngineApiBuilder::default(),
        BasicEngineValidatorBuilder::default(),
        Default::default(),
        Identity::new(),
    );
    #[cfg(feature = "l1-startup")]
    let add_ons = add_ons.extend_rpc_modules(move |ctx| pending::install(ctx, pending));
    add_ons
}

#[cfg(all(test, feature = "l1-startup"))]
mod tests {
    use super::*;
    use reth_chainspec::EthChainSpec as _;
    use reth_evm::ConfigureEvm as _;

    use alloy_primitives::{B256, U64};
    use alloy_provider::{ProviderBuilder, mock::Asserter};
    use eez_l1::{L1BlockRef, L1CanonicalHead, derivation::DerivationSource};
    use std::sync::Arc;

    pub(super) fn l1_blocks(replacement: bool) -> Vec<alloy_rpc_types_eth::Block> {
        let mut parent = B256::ZERO;
        [1000, 1004, 1012]
            .into_iter()
            .enumerate()
            .map(|(number, timestamp)| {
                let header = alloy_consensus::Header {
                    number: number as u64,
                    timestamp,
                    parent_hash: parent,
                    mix_hash: B256::with_last_byte(if replacement && number == 1 {
                        10
                    } else {
                        7 + number as u8
                    }),
                    ..Default::default()
                };
                parent = header.hash_slow();
                alloy_rpc_types_eth::Block {
                    header: alloy_rpc_types_eth::Header {
                        hash: parent,
                        inner: header,
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .collect()
    }

    pub(super) fn profile(reference: &alloy_rpc_types_eth::Block) -> Arc<ChainSpec> {
        let mut genesis =
            eez_l1::chain_config::parse_l2_genesis(include_bytes!("../../../genesis.json"))
                .unwrap()
                .into_genesis();
        genesis.timestamp = 1004;
        genesis.mix_hash = reference.header.mix_hash;
        genesis.config.extra_fields.insert("rollupX".into(), serde_json::json!({
            "feeCollector": "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
            "blockIntervalSeconds": 1, "syncIntervalBlocks": 4, "historyUpdateOffset": 2,
            "l1Reference": { "chainId": 31337, "number": 0, "hash": reference.header.hash,
                "timestamp": reference.header.timestamp, "prevRandao": reference.header.mix_hash }
        }));
        genesis
            .alloc
            .get_mut(&eez_protocol::l1_history::HISTORY_ADDRESS)
            .unwrap()
            .storage = Some(
            eez_protocol::l1_history::genesis_storage()
                .into_iter()
                .collect(),
        );
        Arc::new(eez_evm::profile::chain_spec(genesis).unwrap())
    }

    async fn publish(
        head: &L1CanonicalHead,
        spec: &ChainSpec,
        blocks: &[alloy_rpc_types_eth::Block],
    ) {
        let schedule = eez_evm::profile::schedule(spec).unwrap();
        let replies = Asserter::new();
        replies.push_success(&U64::from(31337));
        replies.push_success(&blocks[0]);
        let source = DerivationSource::authenticate(
            ProviderBuilder::new().connect_mocked_client(replies.clone()),
            schedule,
        )
        .await
        .unwrap();
        // Endpoint, binary source search, complete acquisition, canonical recheck.
        for index in [0, 0, 1, 2, 2] {
            replies.push_success(&blocks[index]);
        }
        let prepared = source
            .prepare_history(
                0,
                L1BlockRef {
                    number: 2,
                    hash: blocks[2].header.hash,
                },
                eez_protocol::l1_history::HeaderRules {
                    london_block: None,
                    shanghai_time: None,
                    cancun_time: None,
                    prague_time: None,
                    unsupported_time: None,
                },
                None,
            )
            .await
            .unwrap();
        head.publish_history(prepared, head.policy_anchor_snapshot().generation)
            .unwrap();
    }

    #[tokio::test]
    async fn rpc_pending_defaults_follow_l2_schedule_and_remain_deterministic() {
        let blocks = l1_blocks(false);
        let spec = profile(&blocks[0]);
        let parent = SealedHeader::seal_slow(spec.genesis_header().clone());
        let head = Arc::new(L1CanonicalHead::default());
        head.initialize_history(eez_evm::profile::schedule(&spec).unwrap());
        let correlations = Arc::new(eez_deriver::CorrelationStore::in_memory());
        correlations.mark_ready();
        let builder = EezEthApiBuilder::new(head.clone(), correlations);
        assert!(
            builder.pending_env_attributes(&parent, None).is_ok(),
            "non-update simulation inherits the authenticated parent randomness"
        );
        publish(&head, &spec, &blocks).await;
        let first = builder.pending_env_attributes(&parent, None).unwrap();
        let second = builder.pending_env_attributes(&parent, None).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.timestamp, 1005);
        assert_eq!(first.prev_randao, blocks[0].header.mix_hash);
        EezEvmConfig::new(spec)
            .next_evm_env(&parent, &first)
            .unwrap();
        let overrides = alloy_rpc_types_eth::BlockOverrides {
            beacon_root: Some(B256::repeat_byte(1)),
            ..Default::default()
        };
        assert!(matches!(
            builder.pending_env_attributes(&parent, Some(&overrides)),
            Err(EthApiError::InvalidParams(_))
        ));
    }

    #[tokio::test]
    async fn derived_pending_rejects_required_import_with_complete_headers() {
        let blocks = l1_blocks(false);
        let spec = profile(&blocks[0]);
        let schedule = eez_evm::profile::schedule(&spec).unwrap();
        let head = Arc::new(L1CanonicalHead::default());
        head.initialize_history(schedule);
        publish(&head, &spec, &blocks).await;
        let correlations = Arc::new(eez_deriver::CorrelationStore::in_memory());
        correlations.mark_ready();
        let builder = EezEthApiBuilder::new(head.clone(), correlations);
        // The second update selects the same L1 source but still advances the
        // history contract's schedule. Neither import may be omitted from a
        // synthesized environment that executes against its parent's state.
        for number in [2, 6] {
            let mut parent = spec.genesis_header().clone();
            parent.number = number;
            parent.timestamp = schedule.timestamp(number).unwrap();
            let parent = SealedHeader::seal_slow(parent);
            assert!(
                head.next_block_context(&parent).is_ok(),
                "evidence is complete"
            );
            let error = builder.pending_env_attributes(&parent, None).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("pending L1 history import has not executed")
            );
        }
    }

    pub(super) fn pending_genesis_db(spec: &ChainSpec) -> reth_rpc_eth_types::StateCacheDb {
        use reth_revm::{database::StateProviderDatabase, db::State};
        use reth_storage_api::{StateProvider, StateProviderBox, noop::NoopProvider};
        use revm::state::{AccountInfo, Bytecode};
        let provider: StateProviderBox = Box::new(NoopProvider::eth(Arc::new(spec.clone())));
        let mut db = State::builder()
            .with_database(StateProviderDatabase::new(
                provider.into_evm_state_provider(),
            ))
            .build();
        for (address, account) in &spec.genesis().alloc {
            let code = Bytecode::new_raw(account.code.clone().unwrap_or_default());
            db.cache.contracts.insert(code.hash_slow(), code.clone());
            db.insert_account_with_storage(
                *address,
                AccountInfo {
                    balance: account.balance,
                    nonce: account.nonce.unwrap_or_default(),
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
                account
                    .storage
                    .iter()
                    .flatten()
                    .map(|(key, value)| ((*key).into(), (*value).into()))
                    .collect(),
            );
        }
        db
    }

    #[test]
    fn pending_prefix_executes_optional_import_with_matching_randao_and_fixed_phase() {
        use eez_protocol::l1_history::{
            ENDPOINT_SLOT, HISTORY_ADDRESS, HeaderChain, HeaderRules, WitnessHistory,
        };
        use reth_evm::Evm;
        use revm::{Database, context::TxEnv};
        let blocks = l1_blocks(false);
        let spec = profile(&blocks[0]);
        let schedule = eez_evm::profile::schedule(&spec).unwrap();
        // Exercise eligible and ineligible positions with a delayed L1 snapshot
        // and one that permits an import.
        for (count, parent_number) in [(1, 0), (3, 0), (1, 2), (3, 2)] {
            let mut chain = HeaderChain::default();
            chain
                .append(
                    &blocks[..count]
                        .iter()
                        .map(|block| alloy_rlp::encode(&block.header.inner).into())
                        .collect::<Vec<_>>(),
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
            let config = EezEvmConfig::new(spec.clone())
                .with_l1_history(Arc::new(WitnessHistory(Arc::new(chain))))
                .unwrap();
            let mut parent = spec.genesis_header().clone();
            parent.number = parent_number;
            parent.timestamp = schedule.timestamp(parent_number).unwrap();
            let parent = SealedHeader::seal_slow(parent);
            let mut db = pending_genesis_db(&spec);
            db.insert_account(
                eez_primitives::SYSTEM_ADDRESS,
                revm::state::AccountInfo {
                    balance: alloy_primitives::U256::from(9),
                    ..Default::default()
                },
            );
            let (config, mut env, mut db) =
                super::pending::execute_prefix(&config, &parent, db).unwrap();
            let history = config.history_state(&mut db).unwrap();
            let selected = usize::from(count != 1 && parent_number == 2);
            assert_eq!(history.endpoint, selected as u64);
            assert_eq!(history.endpoint_hash, blocks[selected].header.hash);
            assert_eq!(history.imported, selected != 0);
            assert_eq!(
                env.block_env.timestamp,
                alloy_primitives::U256::from(parent.timestamp + 1)
            );
            assert_eq!(
                env.block_env.prevrandao,
                Some(blocks[selected].header.mix_hash)
            );

            // Observe the actual opcode in the application EVM, not just the
            // prepared attributes. No system nonce or fee is introduced.
            let probe = alloy_primitives::Address::with_last_byte(90);
            let code =
                revm::state::Bytecode::new_raw(alloy_primitives::bytes!("4460005260206000f3"));
            db.cache.contracts.insert(code.hash_slow(), code.clone());
            db.insert_account(
                probe,
                revm::state::AccountInfo {
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
            );
            let system = db
                .basic(eez_primitives::SYSTEM_ADDRESS)
                .unwrap()
                .unwrap_or_default();
            assert_eq!(system.nonce, 0);
            // The application continues from an open prefix. Neither block
            // preparation nor a history import performs finalization cleanup.
            assert_eq!(system.balance, alloy_primitives::U256::from(9));
            assert_eq!(
                db.storage(
                    alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS,
                    alloy_primitives::U256::from(parent_number)
                )
                .unwrap(),
                alloy_primitives::U256::from_be_slice(parent.hash().as_slice())
            );
            env.cfg_env.disable_base_fee = true;
            let chain_id = env.cfg_env.chain_id;
            let mut evm = config.evm_with_env(&mut db, env);
            let result = evm
                .transact(
                    TxEnv::builder()
                        .caller(alloy_primitives::Address::with_last_byte(91))
                        .kind(alloy_primitives::TxKind::Call(probe))
                        .chain_id(Some(chain_id))
                        .gas_limit(100_000)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(
                result.result.output().unwrap().as_ref(),
                blocks[selected].header.mix_hash.as_slice()
            );
            let mut original = pending_genesis_db(&spec);
            assert_eq!(
                original
                    .storage(HISTORY_ADDRESS, alloy_primitives::U256::from(ENDPOINT_SLOT))
                    .unwrap(),
                alloy_primitives::U256::ZERO
            );
        }
    }

    #[tokio::test]
    async fn pending_prefix_rejects_bad_parent_state_and_freezes_evidence() {
        use eez_protocol::l1_history::{ENDPOINT_SLOT, HISTORY_ADDRESS};
        use revm::Database;
        let blocks = l1_blocks(false);
        let spec = profile(&blocks[0]);
        let head = Arc::new(L1CanonicalHead::default());
        head.initialize_history(eez_evm::profile::schedule(&spec).unwrap());
        publish(&head, &spec, &blocks).await;
        let config = EezEvmConfig::new(spec.clone())
            .with_l1_history(head.clone())
            .unwrap();
        let mut parent = spec.genesis_header().clone();
        parent.number = 2;
        parent.timestamp = 1006;
        let parent = SealedHeader::seal_slow(parent);
        let (frozen, env, mut executed) =
            super::pending::execute_prefix(&config, &parent, pending_genesis_db(&spec)).unwrap();
        let before = frozen.l1_headers(1007).unwrap();
        head.initialize_history(eez_evm::profile::schedule(&spec).unwrap());
        assert_eq!(
            frozen.l1_headers(1007).unwrap().at(1).unwrap().hash(),
            before.at(1).unwrap().hash()
        );
        assert_eq!(env.block_env.prevrandao, Some(blocks[1].header.mix_hash));
        assert_eq!(
            executed
                .storage(HISTORY_ADDRESS, alloy_primitives::U256::from(ENDPOINT_SLOT))
                .unwrap(),
            alloy_primitives::U256::from(1)
        );
        let mut corrupt = pending_genesis_db(&spec);
        corrupt
            .cache
            .accounts
            .get_mut(&HISTORY_ADDRESS)
            .unwrap()
            .account
            .as_mut()
            .unwrap()
            .storage
            .insert(
                alloy_primitives::U256::from(ENDPOINT_SLOT),
                alloy_primitives::U256::from(999),
            );
        let error = super::pending::execute_prefix(&frozen, &parent, corrupt)
            .expect_err("malformed endpoint must fail before application overrides");
        assert!(error.to_string().contains("history"), "{error}");
    }

    #[tokio::test]
    async fn rpc_pending_retains_installed_randao_across_sync_and_reorg_rebuild() {
        let blocks = l1_blocks(false);
        let spec = profile(&blocks[0]);
        let schedule = eez_evm::profile::schedule(&spec).unwrap();
        let head = Arc::new(L1CanonicalHead::default());
        head.initialize_history(schedule);
        publish(&head, &spec, &blocks).await;
        let correlations = Arc::new(eez_deriver::CorrelationStore::in_memory());
        let builder = EezEthApiBuilder::new(head.clone(), correlations.clone());
        let parent_at = |number, installed_randao| {
            let mut parent = spec.genesis_header().clone();
            parent.number = number;
            parent.timestamp = schedule.timestamp(number).unwrap();
            parent.mix_hash = installed_randao;
            SealedHeader::seal_slow(parent)
        };
        let before_import = parent_at(1, blocks[0].header.mix_hash);
        let error = builder
            .pending_env_attributes(&before_import, None)
            .unwrap_err();
        assert!(error.to_string().contains("still validating/catching up"));
        correlations.mark_ready();
        assert_eq!(
            builder
                .pending_env_attributes(&before_import, None)
                .unwrap()
                .prev_randao,
            blocks[0].header.mix_hash,
            "before U(1004)=1007, retain the bootstrap source"
        );
        // Model a parent whose U(1004)=1007 import selected header 1. The
        // pending Sync at 1008 inherits that executed choice, not the L1 cache.
        let imported = parent_at(3, blocks[1].header.mix_hash);
        let updated = builder.pending_env_attributes(&imported, None).unwrap();
        assert_eq!(updated.timestamp, 1008);
        assert_eq!(updated.prev_randao, blocks[1].header.mix_hash);
        assert_eq!(
            builder
                .pending_env_attributes(&parent_at(4, updated.prev_randao), None)
                .unwrap()
                .prev_randao,
            updated.prev_randao,
            "a Sync boundary does not change the selection"
        );
        let env = EezEvmConfig::new(spec.clone())
            .next_evm_env(&imported, &updated)
            .unwrap();
        assert_eq!(env.block_env.prevrandao, Some(blocks[1].header.mix_hash));
        let repeated = parent_at(3, blocks[0].header.mix_hash);
        assert_eq!(
            builder
                .pending_env_attributes(&repeated, None)
                .unwrap()
                .prev_randao,
            blocks[0].header.mix_hash,
            "an executed repeat retains its source even when newer L1 headers are cached"
        );
        head.retreat_on_l1_reorg(0);
        assert_eq!(
            builder.pending_env_attributes(&imported, None).unwrap(),
            updated,
            "invalidating L1 evidence cannot rewrite an executed parent's environment"
        );
        let replacement = l1_blocks(true);
        publish(&head, &spec, &replacement).await;
        assert_eq!(
            builder.pending_env_attributes(&imported, None).unwrap(),
            updated,
            "acquiring replacement evidence alone does not execute a new L2 parent"
        );
        let rebuilt = parent_at(3, replacement[1].header.mix_hash);
        let restored = builder.pending_env_attributes(&rebuilt, None).unwrap();
        assert_eq!(restored.timestamp, updated.timestamp);
        assert_eq!(restored.prev_randao, replacement[1].header.mix_hash);
        assert_ne!(restored.prev_randao, updated.prev_randao);
        let restored_env = EezEvmConfig::new(spec)
            .next_evm_env(&rebuilt, &restored)
            .unwrap();
        assert_eq!(
            restored_env.block_env.prevrandao,
            Some(restored.prev_randao)
        );
    }
}

#[cfg(test)]
mod unavailable_history_tests {
    use super::*;

    #[test]
    fn pending_without_l1_history_fails_instead_of_guessing() {
        let builder = EezEthApiBuilder::default();
        let parent = SealedHeader::seal_slow(alloy_consensus::Header::default());
        let error = builder.pending_env_attributes(&parent, None).unwrap_err();
        assert!(matches!(error, EthApiError::Internal(_)));
        assert!(
            error
                .to_string()
                .contains("L1 derivation is not configured")
        );

        #[cfg(feature = "l1-startup")]
        {
            let correlations = std::sync::Arc::new(eez_deriver::CorrelationStore::in_memory());
            correlations.mark_ready();
            let builder = EezEthApiBuilder::new(Default::default(), correlations);
            let error = builder.pending_env_attributes(&parent, None).unwrap_err();
            assert!(error.to_string().contains("L1 history not initialized"));
        }
    }
}
