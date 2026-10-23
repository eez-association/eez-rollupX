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

#[derive(Debug, Default)]
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
                    // environment builder cannot apply the mandatory import.
                    // Reject even a repeated source, whose schedule must advance.
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
    RpcAddOns::new(
        eth,
        EezEngineValidatorBuilder,
        BasicEngineApiBuilder::default(),
        BasicEngineValidatorBuilder::default(),
        Default::default(),
        Identity::new(),
    )
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

    fn l1_blocks(replacement: bool) -> Vec<alloy_rpc_types_eth::Block> {
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

    fn profile(reference: &alloy_rpc_types_eth::Block) -> Arc<ChainSpec> {
        let mut genesis =
            eez_l1::chain_config::parse_l2_genesis(include_bytes!("../../../genesis.json"))
                .unwrap()
                .into_genesis();
        genesis.timestamp = 1004;
        genesis.mix_hash = reference.header.mix_hash;
        genesis.config.extra_fields.insert("rollupX".into(), serde_json::json!({
            "blockIntervalSeconds": 1, "syncIntervalBlocks": 4, "historyUpdateOffset": 2,
            "l1Reference": { "chainId": 31337, "number": 0, "hash": reference.header.hash,
                "timestamp": reference.header.timestamp, "prevRandao": reference.header.mix_hash }
        }));
        genesis
            .alloc
            .get_mut(&eez_protocol::l1_history::HISTORY_ADDRESS)
            .unwrap()
            .storage = Some(
            eez_protocol::l1_history::bootstrap_storage(
                0,
                reference.header.hash,
                eez_protocol::l1_history::HistorySchedule::new(1004, 1, 4, 2).unwrap(),
            )
            .unwrap()
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
        for index in [2, 1, 0, 0, 1, 2, 2] {
            replies.push_success(&blocks[index]);
        }
        let prepared = source
            .prepare_history(
                1004,
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
        head.publish_history(prepared).unwrap();
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
            builder.pending_env_attributes(&parent, None).is_err(),
            "missing evidence cannot guess a source"
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
        let parent_at = |number| {
            let mut parent = spec.genesis_header().clone();
            parent.number = number;
            parent.timestamp = schedule.timestamp(number).unwrap();
            SealedHeader::seal_slow(parent)
        };
        let error = builder
            .pending_env_attributes(&parent_at(1), None)
            .unwrap_err();
        assert!(error.to_string().contains("still validating/catching up"));
        correlations.mark_ready();
        assert_eq!(
            builder
                .pending_env_attributes(&parent_at(1), None)
                .unwrap()
                .prev_randao,
            blocks[0].header.mix_hash,
            "before U(1004)=1007, retain the bootstrap source"
        );
        // U(1004)=1007 has executed in the parent; the pending Sync at 1008
        // inherits its installed source, without synthesizing an import.
        let updated = builder.pending_env_attributes(&parent_at(3), None).unwrap();
        assert_eq!(updated.timestamp, 1008);
        assert_eq!(updated.prev_randao, blocks[1].header.mix_hash);
        assert_eq!(
            builder
                .pending_env_attributes(&parent_at(4), None)
                .unwrap()
                .prev_randao,
            updated.prev_randao,
            "a Sync boundary does not change the selection"
        );
        let env = EezEvmConfig::new(spec.clone())
            .next_evm_env(&parent_at(3), &updated)
            .unwrap();
        assert_eq!(env.block_env.prevrandao, Some(blocks[1].header.mix_hash));
        head.retreat_on_l1_reorg(0);
        assert!(builder.pending_env_attributes(&parent_at(3), None).is_err());
        let replacement = l1_blocks(true);
        publish(&head, &spec, &replacement).await;
        let restored = builder.pending_env_attributes(&parent_at(3), None).unwrap();
        assert_eq!(restored.timestamp, updated.timestamp);
        assert_eq!(restored.prev_randao, replacement[1].header.mix_hash);
        assert_ne!(restored.prev_randao, updated.prev_randao);
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
