//! Shared L2 node configuration. L1 nodes retain their upstream Ethereum/Gnosis types.
//!
//! Component wiring adapted from reth's Ethereum node at the pinned revision:
//! <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/ethereum/node/src/node.rs>.
//! Engine validation delegates to:
//! <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/ethereum/node/src/engine.rs>.

use crate::{EezPayloadBuilder, EezPoolBuilder};
use alloy_rpc_types_engine::ExecutionData;
use eez_evm::EezEvmConfig;
use eez_primitives::engine::EezPayloadAttributes as PayloadAttributes;
use eez_primitives::{Block, EezPrimitives, EezTxEnvelope, engine::EezEngineTypes};
use reth_chainspec::ChainSpec;
use reth_engine_primitives::{EngineApiValidator, PayloadValidator};
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_node_api::{
    AddOnsContext, FullNodeComponents, FullNodeTypes, NodeTypes, PayloadAttributesBuilder,
};
use reth_node_builder::{
    BuilderContext, DebugNode,
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
    },
    rpc::PayloadValidatorBuilder,
};
use reth_node_ethereum::{EthereumEngineValidator, node::EthereumNetworkBuilder};
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
};
use reth_primitives_traits::SealedBlock;
use reth_provider::EthStorage;
use std::{future::ready, sync::Arc};

#[derive(Debug, Clone)]
pub struct EezNode {
    history: Arc<dyn eez_evm::history::L1HistorySource>,
}

impl NodeTypes for EezNode {
    type Primitives = EezPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage<EezTxEnvelope>;
    type Payload = EezEngineTypes;
}

impl EezNode {
    /// Every node instance requires a canonical evidence source. It can remain
    /// empty during boot, but execution fails closed until derivation prepares it.
    pub fn new(history: Arc<dyn eez_evm::history::L1HistorySource>) -> Self {
        Self { history }
    }

    pub fn components<N: FullNodeTypes<Types = Self>>(
        history: Arc<dyn eez_evm::history::L1HistorySource>,
    ) -> ComponentsBuilder<
        N,
        EezPoolBuilder,
        BasicPayloadServiceBuilder<EezPayloadBuilder>,
        EthereumNetworkBuilder,
        EezExecutorBuilder,
        EezConsensusBuilder,
    > {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EezPoolBuilder::new(eez_primitives::SYSTEM_ADDRESS))
            .executor(EezExecutorBuilder { history })
            .payload(BasicPayloadServiceBuilder::new(EezPayloadBuilder::default()))
            .network(EthereumNetworkBuilder::default())
            .consensus(EezConsensusBuilder)
    }
}

impl<N: FullNodeComponents<Types = Self>> DebugNode<N> for EezNode {
    type RpcBlock = alloy_rpc_types_eth::Block<alloy_rpc_types_eth::Transaction<EezTxEnvelope>>;

    fn rpc_to_primitive_block(block: Self::RpcBlock) -> Block {
        block
            .into_consensus()
            .map_transactions(|tx| tx.inner.into_inner())
    }

    fn local_payload_attributes_builder(
        chain_spec: &ChainSpec,
    ) -> impl PayloadAttributesBuilder<PayloadAttributes> {
        let builder =
            reth_engine_local::LocalPayloadAttributesBuilder::new(Arc::new(chain_spec.clone()));
        move |parent: reth_primitives_traits::SealedHeader| builder.build(&parent).into()
    }
}

#[derive(Debug, Clone)]
pub struct EezExecutorBuilder {
    history: Arc<dyn eez_evm::history::L1HistorySource>,
}

impl<N: FullNodeTypes<Types = EezNode>> ExecutorBuilder<N> for EezExecutorBuilder {
    type EVM = EezEvmConfig;

    fn build_evm(self, ctx: &BuilderContext<N>) -> impl Future<Output = eyre::Result<Self::EVM>> {
        ready(
            EezEvmConfig::new(ctx.chain_spec())
                .with_l1_history(self.history)
                .map_err(Into::into),
        )
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EezConsensusBuilder;

impl<N: FullNodeTypes<Types = EezNode>> ConsensusBuilder<N> for EezConsensusBuilder {
    type Consensus = Arc<EthBeaconConsensus<ChainSpec>>;

    fn build_consensus(
        self,
        ctx: &BuilderContext<N>,
    ) -> impl Future<Output = eyre::Result<Self::Consensus>> {
        ready(Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec()))))
    }
}

#[derive(Debug, Clone)]
pub struct EezEngineValidator {
    payload: EthereumExecutionPayloadValidator<ChainSpec>,
    api: EthereumEngineValidator<ChainSpec>,
}

impl EezEngineValidator {
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            payload: EthereumExecutionPayloadValidator::new(chain_spec.clone()),
            api: EthereumEngineValidator::new(chain_spec),
        }
    }
}

impl PayloadValidator<EezEngineTypes> for EezEngineValidator {
    type Block = Block;

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block>, NewPayloadError> {
        self.payload
            .ensure_well_formed_payload(payload)
            .map_err(Into::into)
    }
}

impl EngineApiValidator<EezEngineTypes> for EezEngineValidator {
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload: PayloadOrAttributes<'_, ExecutionData, PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        let payload = match payload {
            PayloadOrAttributes::ExecutionPayload(payload) => {
                PayloadOrAttributes::ExecutionPayload(payload)
            }
            PayloadOrAttributes::PayloadAttributes(attrs) => {
                PayloadOrAttributes::PayloadAttributes(&attrs.ethereum)
            }
        };
        <EthereumEngineValidator as EngineApiValidator<
            reth_ethereum_engine_primitives::EthEngineTypes,
        >>::validate_version_specific_fields(&self.api, version, payload)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attrs: &PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        <EthereumEngineValidator as EngineApiValidator<
            reth_ethereum_engine_primitives::EthEngineTypes,
        >>::ensure_well_formed_attributes(&self.api, version, &attrs.ethereum)
    }
}

#[derive(Debug, Default, Clone)]
pub struct EezEngineValidatorBuilder;

impl<N: FullNodeComponents<Types = EezNode>> PayloadValidatorBuilder<N>
    for EezEngineValidatorBuilder
{
    type Validator = EezEngineValidator;

    fn build(
        self,
        ctx: &AddOnsContext<'_, N>,
    ) -> impl Future<Output = eyre::Result<Self::Validator>> {
        ready(Ok(EezEngineValidator::new(ctx.config.chain.clone())))
    }
}

impl<N: FullNodeTypes<Types = Self>> reth_node_builder::Node<N> for EezNode {
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EezPoolBuilder,
        BasicPayloadServiceBuilder<EezPayloadBuilder>,
        EthereumNetworkBuilder,
        EezExecutorBuilder,
        EezConsensusBuilder,
    >;
    type AddOns = reth_node_builder::rpc::RpcAddOns<
        reth_node_builder::NodeAdapter<N>,
        crate::rpc::EezEthApiBuilder,
        EezEngineValidatorBuilder,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components(Arc::clone(&self.history))
    }

    fn add_ons(&self) -> Self::AddOns {
        crate::rpc::eez_add_ons(Default::default())
    }
}
