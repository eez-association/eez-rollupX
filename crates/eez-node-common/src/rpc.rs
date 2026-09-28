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
// A live anchor changes its descendants' seed without changing its own mixHash,
// so pending simulation must consult canonical history on every request.
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

    #[test]
    fn rpc_pending_defaults_follow_l2_schedule_and_remain_deterministic() {
        let genesis =
            eez_l1::chain_config::parse_l2_genesis(include_bytes!("../../../genesis.json"))
                .unwrap()
                .into_genesis();
        let spec = std::sync::Arc::new(eez_evm::profile::chain_spec(genesis).unwrap());
        let parent = SealedHeader::seal_slow(spec.genesis_header().clone());
        let l1_head = std::sync::Arc::new(eez_l1::L1CanonicalHead::default());
        l1_head.restore_derivation(eez_l1::derivation::SeedHistory::new(
            eez_evm::profile::schedule(&spec).unwrap(),
        ));
        let correlations = std::sync::Arc::new(eez_deriver::CorrelationStore::in_memory());
        correlations.mark_ready();
        let builder = EezEthApiBuilder::new(l1_head, correlations);
        let first = builder.pending_env_attributes(&parent, None).unwrap();
        let second = builder.pending_env_attributes(&parent, None).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.timestamp,
            eez_evm::profile::schedule(&spec)
                .unwrap()
                .timestamp(1)
                .unwrap()
        );
        assert_ne!(first.timestamp, parent.timestamp + 12);
        assert_eq!(first.prev_randao, parent.mix_hash);
        EezEvmConfig::new(spec)
            .next_evm_env(&parent, &first)
            .unwrap();
        let overrides = alloy_rpc_types_eth::BlockOverrides {
            beacon_root: Some(alloy_primitives::B256::repeat_byte(1)),
            ..Default::default()
        };
        assert!(matches!(
            builder.pending_env_attributes(&parent, Some(&overrides)),
            Err(EthApiError::InvalidParams(_))
        ));
    }

    #[tokio::test]
    async fn rpc_pending_seed_tracks_live_anchor_and_canonical_history_rebuild() {
        use alloy_primitives::{B256, U64};
        use alloy_provider::ProviderBuilder;
        use alloy_transport::mock::Asserter;
        use eez_l1::{L1BlockRef, L1CanonicalHead, derivation::DerivationSource};
        use std::sync::Arc;

        let mut reference: alloy_rpc_types_eth::Block = Default::default();
        reference.header.inner.number = 10;
        reference.header.inner.timestamp = 1000;
        reference.header.inner.mix_hash = B256::with_last_byte(7);
        reference.header.hash = reference.header.inner.hash_slow();
        let mut genesis =
            eez_l1::chain_config::parse_l2_genesis(include_bytes!("../../../genesis.json"))
                .unwrap()
                .into_genesis();
        genesis.timestamp = reference.header.timestamp;
        genesis.mix_hash = reference.header.mix_hash;
        genesis.config.extra_fields.insert(
            "rollupX".into(),
            serde_json::json!({
                "blockIntervalSeconds": 1,
                "syncIntervalBlocks": 4,
                "l1Reference": {
                    "chainId": 31337,
                    "number": reference.header.number,
                    "hash": reference.header.hash,
                    "timestamp": reference.header.timestamp,
                    "prevRandao": reference.header.mix_hash,
                }
            }),
        );
        let spec = Arc::new(eez_evm::profile::chain_spec(genesis).unwrap());
        let schedule = eez_evm::profile::schedule(&spec).unwrap();
        let replies = Asserter::new();
        replies.push_success(&U64::from(31337));
        replies.push_success(&reference);
        replies.push_success(&reference);
        let source = DerivationSource::authenticate(
            ProviderBuilder::new().connect_mocked_client(replies.clone()),
            schedule,
        )
        .await
        .unwrap();
        let mut history = source.seed_history();
        let l1_head = Arc::new(L1CanonicalHead::default());
        l1_head.restore_derivation(history.clone());
        let correlations = Arc::new(eez_deriver::CorrelationStore::in_memory());
        let builder = EezEthApiBuilder::new(l1_head.clone(), correlations.clone());
        let mut parent = spec.genesis_header().clone();
        parent.number = 4;
        parent.timestamp = schedule.timestamp(parent.number).unwrap();
        let parent = SealedHeader::seal_slow(parent);
        let error = builder.pending_env_attributes(&parent, None).unwrap_err();
        assert!(error.to_string().contains("still validating/catching up"));
        correlations.mark_ready();
        let before = builder.pending_env_attributes(&parent, None).unwrap();
        assert_eq!(before.prev_randao, reference.header.mix_hash);

        let mut live = reference.clone();
        live.header.inner.number += 1;
        live.header.inner.timestamp = parent.timestamp;
        live.header.inner.mix_hash = B256::with_last_byte(8);
        live.header.hash = live.header.inner.hash_slow();
        replies.push_success(&live);
        let accepted = source
            .canonical_header(L1BlockRef {
                number: live.header.number,
                hash: live.header.hash,
            })
            .await
            .unwrap();
        // Model the deriver publishing a locally validated accepted endpoint.
        history
            .record_accepted_endpoint(&accepted, B256::with_last_byte(1), 0, &parent)
            .unwrap();
        l1_head.restore_derivation(history.clone());
        let after = builder.pending_env_attributes(&parent, None).unwrap();
        assert_eq!(after.timestamp, schedule.timestamp(5).unwrap());
        assert_eq!(after.prev_randao, live.header.mix_hash);
        assert_ne!(after.prev_randao, parent.mix_hash);
        let env = EezEvmConfig::new(spec)
            .next_evm_env(&parent, &after)
            .unwrap();
        assert_eq!(env.block_env.prevrandao, Some(live.header.mix_hash));

        // A later same-height catch-up must retain the live refresh even though
        // both endpoint variants were executed with the original seed.
        let mut catch_up = live.clone();
        catch_up.header.inner.number += 1;
        catch_up.header.inner.timestamp += 4;
        catch_up.header.inner.mix_hash = B256::with_last_byte(9);
        catch_up.header.hash = catch_up.header.inner.hash_slow();
        replies.push_success(&catch_up);
        let accepted = source
            .canonical_header(L1BlockRef {
                number: catch_up.header.number,
                hash: catch_up.header.hash,
            })
            .await
            .unwrap();
        history
            .record_accepted_endpoint(&accepted, B256::with_last_byte(2), 0, &parent)
            .unwrap();
        l1_head.restore_derivation(history.clone());
        assert_eq!(
            builder.pending_env_attributes(&parent, None).unwrap(),
            after
        );

        replies.push_success(&reference);
        let original = source
            .canonical_header(L1BlockRef {
                number: reference.header.number,
                hash: reference.header.hash,
            })
            .await
            .unwrap();
        history.reorg_to(&original).unwrap();
        l1_head.restore_derivation(history);
        let restored = builder.pending_env_attributes(&parent, None).unwrap();
        assert_eq!(
            restored, before,
            "pending RPC must observe rebuilt canonical history"
        );
    }
}

#[cfg(test)]
mod unavailable_history_tests {
    use super::*;

    #[test]
    fn pending_without_seed_provenance_fails_instead_of_guessing() {
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
            assert!(
                error
                    .to_string()
                    .contains("seed provenance not initialized")
            );
        }
    }
}
