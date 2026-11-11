//! Adapts Ethereum execution to EEZ transaction and receipt types.
//!
//! The pinned `EthEvmConfig` fixes its primitives to Ethereum types, which cannot
//! represent native types `0x45` and `0x46`. This wrapper selects `EezPrimitives`, preserves
//! the native receipt type, and decodes native transactions in Engine payloads.
//! Transaction gas accounting and block assembly reuse the pinned Ethereum code.
//! The rollup lifecycle retains EIP-2935 but excludes beacon and request calls. `eez-primitives` defines the zero-fee native TxEnv;
//! `evm` adds deposit minting and rollback around the upstream Ethereum EVM.
//!
//! Configuration adapted from reth's `EthEvmConfig` at the pinned revision:
//! <https://github.com/paradigmxyz/reth/blob/444e867155c2a7a4c028e479ca7201b0b6871579/crates/ethereum/evm/src/lib.rs>.
//! Receipt construction follows Alloy 0.39.0's Ethereum receipt builder:
//! <https://docs.rs/alloy-evm/0.39.0/src/alloy_evm/eth/receipt_builder.rs.html>.

use alloy_consensus::{Header, Typed2718};
use alloy_eips::Decodable2718;
use alloy_evm::eth::{
    EthBlockExecutionCtx,
    receipt_builder::{ReceiptBuilder, ReceiptBuilderCtx},
    spec::EthExecutorSpec,
};
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::ExecutionData;
use eez_primitives::{Block, EezPrimitives, EezTxEnvelope, EezTxType, Receipt};
use reth_chainspec::{ChainSpec, EthChainSpec, Hardforks};
use reth_evm::{
    ConfigureEngineEvm, ConfigureEvm, Evm, EvmEnv, EvmEnvFor, ExecutableTxIterator,
    ExecutionCtxFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::{EthBlockAssembler, EthEvmConfig};
use reth_primitives_traits::{
    SealedBlock, SealedHeader, SignedTransaction, transaction::error::InvalidTransactionError,
};
use reth_storage_errors::any::AnyError;
use std::{
    borrow::Cow,
    fmt,
    sync::{Arc, OnceLock},
};

mod evm;
pub use evm::EezEvmFactory;

pub mod balances;
pub mod block;
pub mod history;
pub mod profile;
#[cfg(feature = "stateless")]
pub mod stateless;
use block::{EezBlockExecutorFactory, ExecutionLayer};

/// Enforce L2 transaction support during construction and replay. The underlying
/// EVM is also used for L1 simulation, where blob transactions remain valid.
pub fn ensure_supported_transaction(tx: &EezTxEnvelope) -> Result<(), AnyError> {
    if let EezTxEnvelope::System(native) = tx {
        native.validate().map_err(AnyError::new)?;
        if native.to == eez_primitives::EEZL2_ADDRESS
            && ![
                eez_protocol::system_tx::LOAD_EXECUTION_TABLE_SELECTOR,
                eez_protocol::entries::EXECUTE_INCOMING_SELECTOR,
            ]
            .iter()
            .any(|selector| native.input.starts_with(selector))
        {
            return Err(AnyError::new(eez_protocol::l2::ProfileError(
                "unsupported native EEZL2 call selector",
            )));
        }
    }
    if let EezTxEnvelope::History(native) = tx {
        native.validate().map_err(AnyError::new)?;
    }
    if tx.is_eip4844() {
        return Err(AnyError::new(InvalidTransactionError::Eip4844Disabled));
    }
    Ok(())
}

#[derive(Debug)]
struct L2WithdrawalsUnsupported {
    count: usize,
}

impl fmt::Display for L2WithdrawalsUnsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "L2 blocks cannot contain beacon withdrawals (got {})",
            self.count
        )
    }
}

impl std::error::Error for L2WithdrawalsUnsupported {}

/// L2 ETH issuance is authorized only by validated native deposit transactions.
/// Reject Ethereum's beacon-withdrawal mint path everywhere this configuration
/// constructs or replays an L2 block.
fn ensure_no_withdrawals(count: usize) -> Result<(), AnyError> {
    if count != 0 {
        return Err(AnyError::new(L2WithdrawalsUnsupported { count }));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EezReceiptBuilder;

impl ReceiptBuilder for EezReceiptBuilder {
    type Transaction = EezTxEnvelope;
    type Receipt = Receipt;

    /// Preserve native types `0x45` and `0x46` in the receipt root; Ethereum's receipt builder
    /// only accepts its own type enum. Status, gas, and logs are unchanged.
    fn build_receipt<E: Evm>(&self, ctx: ReceiptBuilderCtx<'_, EezTxType, E>) -> Receipt {
        Receipt {
            tx_type: ctx.tx_type,
            success: ctx.result.is_success(),
            cumulative_gas_used: ctx.cumulative_gas_used,
            logs: ctx.result.into_logs(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EezEvmConfig<C = ChainSpec> {
    ethereum: EthEvmConfig<C>,
    executor: EezBlockExecutorFactory<C>,
    layer: ExecutionLayer,
    // The chain spec is immutable. Cache decoding (including errors), while
    // retaining per-block validation and keeping L1 simulation profile-free.
    schedule: Arc<OnceLock<Result<eez_protocol::l2::Schedule, eez_protocol::l2::ProfileError>>>,
}

impl<C: EthChainSpec<Header = Header>> EezEvmConfig<C> {
    pub fn new(chain_spec: Arc<C>) -> Self {
        Self::with_layer(chain_spec, ExecutionLayer::Rollup)
    }

    /// Source-chain simulation uses the L1's rules, including beacon/request
    /// processing. Never use this constructor for L2 assembly or replay.
    pub fn for_l1_simulation(chain_spec: Arc<C>) -> Self {
        Self::with_layer(chain_spec, ExecutionLayer::L1Simulation)
    }

    fn with_layer(chain_spec: Arc<C>, layer: ExecutionLayer) -> Self {
        Self {
            ethereum: EthEvmConfig::new(chain_spec.clone()),
            executor: EezBlockExecutorFactory::new(chain_spec, layer),
            layer,
            schedule: Arc::new(OnceLock::new()),
        }
    }

    pub fn chain_spec(&self) -> &Arc<C> {
        self.ethereum.chain_spec()
    }
}

impl<C: EthChainSpec<Header = Header>> EezEvmConfig<C> {
    /// Install ready checked L1 evidence for history validation. No
    /// execution path falls back to an RPC source when evidence is absent.
    pub fn with_l1_history(
        mut self,
        source: Arc<dyn history::L1HistorySource>,
    ) -> Result<Self, eez_protocol::l2::ProfileError> {
        let schedule = self.schedule()?;
        self.executor.history = Some(history::HistoryConfig {
            schedule: schedule.history()?,
            bootstrap: schedule.config().l1_reference,
            source,
            l2_chain_id: self.chain_spec().chain().id(),
        });
        Ok(self)
    }

    /// Ready checked evidence for a scheduled L2 timestamp. The source performs
    /// no network or blocking I/O here; absence is an execution/preparation error.
    pub fn l1_headers(
        &self,
        timestamp: u64,
    ) -> Result<Arc<eez_protocol::l1_history::HeaderChain>, eez_protocol::l2::ProfileError> {
        let history = self
            .executor
            .history
            .as_ref()
            .ok_or(eez_protocol::l2::ProfileError(
                "L1 history source is not configured",
            ))?;
        history
            .source
            .headers(history.schedule.source_cutoff(timestamp)?)
    }

    /// Construct an exact history transaction from local parent state and ready
    /// evidence. Call from a blocking builder/worker, never perform remote reads.
    pub fn prepare_history<DB: revm::Database>(
        &self,
        db: &mut DB,
        parent: &Header,
        timestamp: u64,
    ) -> Result<
        (
            eez_protocol::l1_history::HistoryExecution,
            Option<EezTxEnvelope>,
        ),
        alloy_evm::block::BlockExecutionError,
    > {
        let history = self.executor.history.as_ref().ok_or_else(|| {
            alloy_evm::block::BlockExecutionError::msg("L1 history source is not configured")
        })?;
        let (_, expected_timestamp) = self
            .schedule()
            .and_then(|schedule| schedule.next(parent))
            .map_err(alloy_evm::block::BlockExecutionError::other)?;
        if timestamp != expected_timestamp {
            return Err(alloy_evm::block::BlockExecutionError::msg(
                "invalid history block position",
            ));
        }
        crate::history::prepare(db, history, timestamp)
    }

    /// Read and validate the fixed predeploy and inherited history state.
    pub fn history_state<DB: revm::Database>(
        &self,
        db: &mut DB,
    ) -> Result<eez_protocol::l1_history::HistoryState, alloy_evm::block::BlockExecutionError> {
        let history = self.executor.history.as_ref().ok_or_else(|| {
            alloy_evm::block::BlockExecutionError::msg("L1 history source is not configured")
        })?;
        crate::history::state(db, history)
    }

    /// Independently check recorded history inputs against local authenticated
    /// parent state. Does not execute or commit the transaction.
    pub fn validate_history_input<DB: revm::Database>(
        &self,
        db: &mut DB,
        parent: &Header,
        timestamp: u64,
        tx: Option<&EezTxEnvelope>,
    ) -> Result<eez_protocol::l1_history::HistoryExecution, alloy_evm::block::BlockExecutionError>
    {
        let history = self.executor.history.as_ref().ok_or_else(|| {
            alloy_evm::block::BlockExecutionError::msg("L1 history source is not configured")
        })?;
        let (_, expected_timestamp) = self
            .schedule()
            .and_then(|schedule| schedule.next(parent))
            .map_err(alloy_evm::block::BlockExecutionError::other)?;
        if timestamp != expected_timestamp {
            return Err(alloy_evm::block::BlockExecutionError::msg(
                "invalid history block position",
            ));
        }
        crate::history::transition(db, history, timestamp, tx)
    }

    /// Derive the block environment from its public transaction input. Omitted
    /// updates inherit the parent's randomness. Execution
    /// independently binds the inherited endpoint to authenticated contract state.
    pub fn history_prev_randao(
        &self,
        parent: &Header,
        timestamp: u64,
        transaction: Option<&EezTxEnvelope>,
    ) -> Result<alloy_primitives::B256, eez_protocol::l2::ProfileError> {
        let Some(EezTxEnvelope::History(tx)) = transaction.filter(|tx| tx.is_history()) else {
            return Ok(parent.mix_hash);
        };
        tx.validate()
            .map_err(|_| eez_protocol::l2::ProfileError("invalid history transaction"))?;
        let Some(block) = tx.blocks.last() else {
            return Ok(parent.mix_hash);
        };
        let chain = self.l1_headers(timestamp)?;
        let header = chain.at(block.number)?;
        if header.hash() != block.block_hash {
            return Err(eez_protocol::l2::ProfileError(
                "history transaction differs from authenticated headers",
            ));
        }
        Ok(header.header().mix_hash)
    }

    fn schedule(&self) -> Result<eez_protocol::l2::Schedule, eez_protocol::l2::ProfileError> {
        *self
            .schedule
            .get_or_init(|| profile::schedule(self.chain_spec().as_ref()))
    }
}

impl<C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static> ConfigureEvm
    for EezEvmConfig<C>
{
    type Primitives = EezPrimitives;
    type Error = AnyError;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = EezBlockExecutorFactory<C>;
    type BlockAssembler = EthBlockAssembler<C>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.ethereum.block_assembler
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv, Self::Error> {
        if self.layer == ExecutionLayer::Rollup {
            eez_protocol::l2::validate_header(header).map_err(AnyError::new)?;
            self.schedule()
                .and_then(|schedule| schedule.validate_position(header))
                .map_err(AnyError::new)?;
        }
        self.ethereum.evm_env(header).map_err(AnyError::new)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        if self.layer == ExecutionLayer::Rollup {
            profile::validate_next(parent, attributes).map_err(AnyError::new)?;
            let (_, timestamp) = self
                .schedule()
                .and_then(|schedule| schedule.next(parent))
                .map_err(AnyError::new)?;
            if attributes.timestamp != timestamp {
                return Err(AnyError::new(eez_protocol::l2::ProfileError(
                    "next block timestamp differs from its genesis-derived position",
                )));
            }
        }
        self.ethereum
            .next_evm_env(parent, attributes)
            .map_err(AnyError::new)
    }

    /// Reject blobs on block import and stateless proof replay, then reuse
    /// Ethereum's context fields with EEZ transaction types.
    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        if self.layer == ExecutionLayer::Rollup {
            eez_protocol::l2::validate_header(block.header()).map_err(AnyError::new)?;
            self.schedule()
                .and_then(|schedule| schedule.validate_position(block.header()))
                .map_err(AnyError::new)?;
            if block.body().withdrawals.is_none() {
                return Err(AnyError::new(eez_protocol::l2::ProfileError(
                    "withdrawals list must be present and empty",
                )));
            }
            ensure_no_withdrawals(
                block
                    .body()
                    .withdrawals
                    .as_ref()
                    .map_or(0, |withdrawals| withdrawals.len()),
            )?;
            block
                .body()
                .transactions
                .iter()
                .try_for_each(ensure_supported_transaction)?;
        }
        Ok(EthBlockExecutionCtx {
            tx_count_hint: Some(block.transaction_count()),
            parent_hash: block.header().parent_hash,
            parent_beacon_block_root: block.header().parent_beacon_block_root,
            ommers: &block.body().ommers,
            withdrawals: block
                .body()
                .withdrawals
                .as_ref()
                .map(|w| Cow::Borrowed(w.as_slice())),
            extra_data: block.header().extra_data.clone(),
            slot_number: block.header().slot_number,
        })
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EthBlockExecutionCtx<'_>, Self::Error> {
        if self.layer == ExecutionLayer::Rollup {
            ensure_no_withdrawals(
                attributes
                    .withdrawals
                    .as_ref()
                    .map_or(0, |withdrawals| withdrawals.len()),
            )?;
        }
        self.ethereum
            .context_for_next_block(parent, attributes)
            .map_err(AnyError::new)
    }
}

impl<C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static>
    ConfigureEngineEvm<ExecutionData> for EezEvmConfig<C>
{
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        if self.layer == ExecutionLayer::Rollup {
            profile::validate_payload(payload).map_err(AnyError::new)?;
            self.schedule()
                .and_then(|schedule| {
                    schedule.validate_coordinates(
                        payload.payload.block_number(),
                        payload.payload.timestamp(),
                    )
                })
                .map_err(AnyError::new)?;
        }
        self.ethereum
            .evm_env_for_payload(payload)
            .map_err(AnyError::new)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        if self.layer == ExecutionLayer::Rollup {
            ensure_no_withdrawals(payload.payload.withdrawals().map_or(0, Vec::len))?;
        }
        self.ethereum
            .context_for_payload(payload)
            .map_err(AnyError::new)
    }

    /// Decode native envelopes and reject blobs before Engine replay. Native
    /// recovery assigns the protocol sender; derivation and proof checks
    /// establish the call's authority.
    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let rollup = self.layer == ExecutionLayer::Rollup;
        let convert = move |raw: Bytes| -> Result<_, AnyError> {
            let tx = EezTxEnvelope::decode_2718_exact(&raw).map_err(AnyError::new)?;
            if rollup {
                ensure_supported_transaction(&tx)?;
            }
            let signer = tx.try_recover().map_err(AnyError::new)?;
            Ok(tx.with_signer(signer))
        };
        Ok((payload.payload.transactions().clone(), convert))
    }
}

#[cfg(test)]
mod tests;
