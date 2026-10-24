//! Engine API adapters for EEZ blocks. Payload formats and fork rules are Ethereum's;
//! transaction bytes inside those payloads are decoded using the EEZ envelope.
//! L2 has no blob transactions, so the required Engine blob bundles are empty.
//!
//! Adapted from reth's Engine types and payload conversions at the pinned revision:
//! <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/ethereum/engine-primitives/src/lib.rs>
//! and <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/ethereum/engine-primitives/src/payload.rs>.

use crate::{Block, EezPrimitives};
use alloy_eips::eip7685::Requests;
use alloy_primitives::{Bytes, U256};
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, CancunPayloadFields, ExecutionData, ExecutionPayload,
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadEnvelopeV5, ExecutionPayloadEnvelopeV6, ExecutionPayloadFieldV2,
    ExecutionPayloadSidecar, ExecutionPayloadV1, ExecutionPayloadV3, ExecutionPayloadV4,
    PayloadAttributes, PraguePayloadFields,
};
use reth_engine_primitives::EngineTypes;
use reth_ethereum_engine_primitives::{BuiltPayloadConversionError, EthBuiltPayload};
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{RecoveredBlock, SealedBlock};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct EezBuiltPayload(EthBuiltPayload<EezPrimitives>);

impl EezBuiltPayload {
    pub fn new(
        block: Arc<RecoveredBlock<Block>>,
        fees: U256,
        requests: Option<Requests>,
        bal: Option<Bytes>,
    ) -> Self {
        Self(EthBuiltPayload::new(block, fees, requests, bal))
    }

    pub fn block(&self) -> &SealedBlock<Block> {
        self.0.block()
    }

    pub fn block_arc(&self) -> &Arc<RecoveredBlock<Block>> {
        self.0.block_arc()
    }

    pub fn fees(&self) -> U256 {
        self.0.fees()
    }
}

impl BuiltPayload for EezBuiltPayload {
    type Primitives = EezPrimitives;

    fn block(&self) -> &SealedBlock<Block> {
        self.0.block()
    }

    fn fees(&self) -> U256 {
        self.0.fees()
    }

    fn block_access_list(&self) -> Option<&Bytes> {
        self.0.block_access_list()
    }

    fn requests(&self) -> Option<Requests> {
        self.0.requests()
    }
}

impl From<EezBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: EezBuiltPayload) -> Self {
        Self::from_block_unchecked(value.block().hash(), &value.block().clone_block())
    }
}

impl From<EezBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: EezBuiltPayload) -> Self {
        Self {
            block_value: value.fees(),
            execution_payload: ExecutionPayloadFieldV2::from_block_unchecked(
                value.block().hash(),
                &value.block().clone_block(),
            ),
        }
    }
}

impl TryFrom<EezBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EezBuiltPayload) -> Result<Self, Self::Error> {
        Ok(Self {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.block().hash(),
                &value.block().clone_block(),
            ),
            block_value: value.fees(),
            should_override_builder: false,
            blobs_bundle: BlobsBundleV1::empty(),
        })
    }
}

impl TryFrom<EezBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EezBuiltPayload) -> Result<Self, Self::Error> {
        Ok(Self {
            execution_requests: value.requests().unwrap_or_default(),
            envelope_inner: value.try_into()?,
        })
    }
}

impl TryFrom<EezBuiltPayload> for ExecutionPayloadEnvelopeV5 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EezBuiltPayload) -> Result<Self, Self::Error> {
        Ok(Self {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.block().hash(),
                &value.block().clone_block(),
            ),
            block_value: value.fees(),
            should_override_builder: false,
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: value.requests().unwrap_or_default(),
        })
    }
}

impl TryFrom<EezBuiltPayload> for ExecutionPayloadEnvelopeV6 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EezBuiltPayload) -> Result<Self, Self::Error> {
        let bal = value
            .block_access_list()
            .cloned()
            .ok_or(BuiltPayloadConversionError::MissingBlockAccessList)?;
        Ok(Self {
            execution_payload: ExecutionPayloadV4::from_block_unchecked_with_bal(
                value.block().hash(),
                &value.block().clone_block(),
                bal,
            ),
            block_value: value.fees(),
            should_override_builder: false,
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: value.requests().unwrap_or_default(),
        })
    }
}

impl From<EezBuiltPayload> for ExecutionData {
    fn from(value: EezBuiltPayload) -> Self {
        let block = value.block();
        let (payload, sidecar) = ExecutionPayload::from_block_unchecked_with_extras(
            block.hash(),
            &block.clone_block(),
            value.block_access_list().cloned(),
        );
        let sidecar = if let (Some(requests), Some(parent_beacon_block_root)) =
            (value.requests(), block.header().parent_beacon_block_root)
        {
            ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root,
                    versioned_hashes: block.body().blob_versioned_hashes_iter().copied().collect(),
                },
                PraguePayloadFields::new(requests),
            )
        } else {
            sidecar
        };
        Self { payload, sidecar }
    }
}

/// Exact producer inputs. The optional native history transaction is carried
/// into the block body, never hidden in header metadata. Payload IDs bind it so
/// two choices sharing PREVRANDAO cannot reuse each other's cached payload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EezPayloadAttributes {
    #[serde(flatten)]
    pub ethereum: PayloadAttributes,
    pub history_transaction: Option<Bytes>,
    /// Structural Sync requests exclude public-pool transactions.
    pub no_tx_pool: bool,
}
impl From<PayloadAttributes> for EezPayloadAttributes {
    fn from(ethereum: PayloadAttributes) -> Self {
        Self {
            ethereum,
            history_transaction: None,
            no_tx_pool: false,
        }
    }
}
impl std::ops::Deref for EezPayloadAttributes {
    type Target = PayloadAttributes;
    fn deref(&self) -> &Self::Target {
        &self.ethereum
    }
}
impl std::ops::DerefMut for EezPayloadAttributes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ethereum
    }
}
impl reth_payload_primitives::PayloadAttributes for EezPayloadAttributes {
    fn payload_id(&self, parent: &alloy_primitives::B256) -> alloy_rpc_types_engine::PayloadId {
        let base = self.ethereum.payload_id(parent);
        let mut input = base.0.to_vec();
        input.push(u8::from(self.no_tx_pool));
        input.push(u8::from(self.history_transaction.is_some()));
        if let Some(raw) = &self.history_transaction {
            input.extend_from_slice(raw);
        }
        let hash = alloy_primitives::keccak256(input);
        alloy_rpc_types_engine::PayloadId::new(hash[..8].try_into().expect("eight-byte payload ID"))
    }
    fn timestamp(&self) -> u64 {
        self.ethereum.timestamp
    }
    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.ethereum.withdrawals.as_ref()
    }
    fn parent_beacon_block_root(&self) -> Option<alloy_primitives::B256> {
        self.ethereum.parent_beacon_block_root
    }
    fn slot_number(&self) -> Option<u64> {
        self.ethereum.slot_number
    }
    fn target_gas_limit(&self) -> Option<u64> {
        self.ethereum.target_gas_limit
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct EezEngineTypes;

impl PayloadTypes for EezEngineTypes {
    type BuiltPayload = EezBuiltPayload;
    type PayloadAttributes = EezPayloadAttributes;
    type ExecutionData = ExecutionData;

    fn block_to_payload(block: SealedBlock<Block>, bal: Option<Bytes>) -> ExecutionData {
        let (payload, sidecar) = ExecutionPayload::from_block_unchecked_with_extras(
            block.hash(),
            &block.clone_block(),
            bal,
        );
        ExecutionData { payload, sidecar }
    }
}

impl EngineTypes for EezEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV5;
    type ExecutionPayloadEnvelopeV6 = ExecutionPayloadEnvelopeV6;
}
