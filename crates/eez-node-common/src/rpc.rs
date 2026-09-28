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
pub struct EezEthApiBuilder;

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> EthApiBuilder<N>
    for EezEthApiBuilder
{
    type EthApi = EthApi<N, Converter>;

    fn build_eth_api(
        self,
        ctx: EthApiCtx<'_, N>,
    ) -> impl Future<Output = eyre::Result<Self::EthApi>> {
        let pending = EezPendingEnvBuilder(ctx.components.provider().chain_spec());
        let receipts = EthReceiptConverter::new(ctx.components.provider().chain_spec())
            .with_builder(build_receipt as ReceiptBuilder);
        ready(Ok(ctx
            .eth_api_builder()
            .map_converter(|_| Converter::new(receipts))
            .with_pending_env_builder(pending)
            .build()))
    }
}

/// RPC simulation defaults when no executed pending payload is available.
/// Reth's generic defaults use a 12-second interval and random RANDAO; neither
/// is an L2 construction rule. Inherit the parent's seed for this uncommitted
/// simulation. Actual candidates obtain authenticated descendant seeds from
/// the canonical L1 history and remain subject to strict execution validation.
#[derive(Debug, Clone)]
struct EezPendingEnvBuilder(std::sync::Arc<ChainSpec>);

impl PendingEnvBuilder<EezEvmConfig> for EezPendingEnvBuilder {
    fn pending_env_attributes(
        &self,
        parent: &SealedHeader,
        overrides: Option<&alloy_rpc_types_eth::BlockOverrides>,
    ) -> Result<reth_evm::NextBlockEnvAttributes, EthApiError> {
        let (_, timestamp) = eez_evm::profile::schedule(&self.0)
            .and_then(|schedule| schedule.next(parent))
            .map_err(|error| EthApiError::InvalidParams(error.to_string()))?;
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
            prev_randao: parent.mix_hash,
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

pub fn eez_add_ons<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>>()
-> RpcAddOns<N, EezEthApiBuilder, EezEngineValidatorBuilder> {
    RpcAddOns::new(
        EezEthApiBuilder,
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
        let builder = EezPendingEnvBuilder(spec.clone());
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
}
