//! Network pricing and final ordered L1 execution for the settlement policy.
//! Scheduling remains in `settlement_policy`; this module cannot make work due.

use std::sync::Arc;

use alloy_consensus::{Header, SignableTransaction, Signed, Transaction, TxEip4844WithSidecar};
use alloy_eips::{
    BlockNumberOrTag, Decodable2718, Encodable2718, eip7594::BlobTransactionSidecarVariant,
};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider as _;
use alloy_sol_types::{SolCall, SolEvent};
use eez_l1::{
    BundleTarget, L1BlockRef,
    posting_fees::{
        FeeQuote, FeeSample, PRIORITY_FEE_HISTORY_BLOCKS, PostingFeeConfig, PostingFeeError,
        PriorityProfile, estimate_next_block_priority_fee,
    },
};
use eez_protocol::{CallMode, ExecutionOutcome, blob_semantics::SemanticTransaction};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{
    ConfigureEvm, EvmEnv, NextBlockEnvAttributes, block::BlockExecutor, eth::EthTxResult,
};
use reth_primitives_traits::{Recovered, SealedHeader, SignerRecoverable};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::EvmStateProviderAdapter;
use revm::{
    Database, Inspector,
    context::Journal,
    context_interface::{Cfg as _, ContextTr},
    interpreter::{CallInputs, CallOutcome, CallScheme},
};
use tracing::{Level, event};

use super::CrossChainExecCtx;
use super::gas_trace;
use crate::local::provider::ChainProvider;

#[derive(Debug, thiserror::Error)]
pub(super) enum PostingError {
    #[error("posting inputs unavailable: {0}")]
    Unavailable(String),
    #[error("ordered L1 bundle exceeds capacity: {0}")]
    Capacity(String),
    #[error("ordered L1 bundle does not complete the intended effects: {0}")]
    Effects(String),
    #[error("invalid ordered L1 transaction: {0}")]
    Transaction(String),
    #[error(transparent)]
    Fee(#[from] PostingFeeError),
}

pub(super) struct PostingRuntime {
    pub fees: PostingFeeConfig,
    pub enforce_l2_fee_tariff: bool,
    pub shadow_fee_policy: Option<super::fees::pricing::Policy>,
    pub chain: ChainProvider,
    pub ctx: Arc<CrossChainExecCtx>,
    pub registry: Address,
    /// Destination of the postBatch transaction: the registry, or a wrapper
    /// validated at startup to forward `postAndVerifyBatch` to it. Preflight
    /// still observes registry calls and logs, which the wrapper performs.
    pub post_batch_target: Address,
    pub operator_gas_cap: u64,
    pub operator_blob_cap: Option<u64>,
    pub costs: std::sync::Mutex<super::costs::CostLedger>,
    pub gas_trace: Option<gas_trace::Config>,
}

/// Price a bundle as cross-chain when any of its users executes a cross-chain
/// call on L1. Only cross-chain users carry semantic transactions; ordinary L2
/// transactions travel in the blob without one.
pub(super) fn priority_profile(semantics: &[SemanticTransaction]) -> PriorityProfile {
    if semantics.iter().any(|tx| !tx.actions.is_empty()) {
        PriorityProfile::CrossChain
    } else {
        PriorityProfile::Base
    }
}

#[derive(Debug)]
pub(super) struct PostingQuote {
    pub quote: FeeQuote,
    pub parent: Header,
    pub(super) attributes: NextBlockEnvAttributes,
    pub env: EvmEnv,
    pub gas_budget: u64,
}

#[derive(Debug)]
pub(super) struct BundleExecution {
    /// Receipt gas, after refunds and the calldata floor, for fee accounting.
    pub gas_used: Vec<u64>,
    /// Gas needed before refunds, respecting the calldata floor, for limit sizing.
    pub gas_required: Vec<u64>,
    pub gas_limits: Vec<u64>,
    pub transaction_hashes: Vec<B256>,
    /// Physical blobs in each validated envelope, including user companions.
    pub blob_counts: Vec<u64>,
}

impl PostingRuntime {
    pub fn calldata_floor_active(&self, target: BundleTarget) -> bool {
        match target {
            BundleTarget::Exact { timestamp, .. } => self
                .chain
                .evm_config
                .chain_spec()
                .is_prague_active_at_timestamp(timestamp),
            BundleTarget::NextBlock => true,
        }
    }

    pub fn native_currency(&self) -> &'static str {
        if self.chain.gnosis_evm_config.is_some() {
            "xDAI"
        } else {
            "ETH"
        }
    }

    fn environment(
        &self,
        parent: &Header,
        target: BundleTarget,
    ) -> Result<(EvmEnv, NextBlockEnvAttributes), PostingError> {
        let BundleTarget::Exact { block, timestamp } = target else {
            return Err(PostingError::Unavailable("unresolved target".into()));
        };
        if block <= parent.number
            || timestamp <= parent.timestamp
            || parent.base_fee_per_gas.is_none()
        {
            return Err(PostingError::Unavailable(
                "target or fee-sample header is invalid".into(),
            ));
        }
        let spec = self.chain.evm_config.chain_spec();
        let attributes = NextBlockEnvAttributes {
            timestamp,
            suggested_fee_recipient: parent.beneficiary,
            prev_randao: parent.mix_hash,
            gas_limit: parent.gas_limit,
            parent_beacon_block_root: spec
                .is_cancun_active_at_timestamp(timestamp)
                .then_some(B256::ZERO),
            withdrawals: spec
                .is_shanghai_active_at_timestamp(timestamp)
                .then(Default::default),
            extra_data: parent.extra_data.clone(),
            slot_number: None,
        };
        let mut env = if let Some(config) = &self.chain.gnosis_evm_config {
            config.next_evm_env(&parent.clone().into(), &attributes)
        } else {
            reth_evm_ethereum::EthEvmConfig::new(Arc::clone(self.chain.evm_config.chain_spec()))
                .next_evm_env(parent, &attributes)
        }
        .map_err(|error| PostingError::Unavailable(error.to_string()))?;
        // Conservative planning may be more than one block ahead. Final
        // execution below requires the exact next parent and rebuilds the env.
        env.block_env.number = U256::from(block);
        if env.block_env.blob_excess_gas_and_price.is_none() {
            return Err(PostingError::Unavailable(
                "target fork has no blob pricing rules".into(),
            ));
        }
        Ok((env, attributes))
    }

    pub fn gas_budget(&self, parent: &Header, target: BundleTarget) -> Result<u64, PostingError> {
        let (env, _) = self.environment(parent, target)?;
        Ok(env
            .cfg_env
            .tx_gas_limit_cap()
            .min(parent.gas_limit)
            .min(self.operator_gas_cap))
    }

    pub fn transaction_gas_cap(
        &self,
        parent: &Header,
        target: BundleTarget,
    ) -> Result<u64, PostingError> {
        Ok(self
            .environment(parent, target)?
            .0
            .cfg_env
            .tx_gas_limit_cap())
    }

    /// Sample only for selected posting work. An unset override uses recent
    /// canonical fee-history rewards at `profile`'s percentile and ceiling;
    /// unavailable data is never replaced by a guessed tip.
    pub async fn quote(
        &self,
        target: BundleTarget,
        final_parent: bool,
        profile: PriorityProfile,
    ) -> Result<PostingQuote, PostingError> {
        self.quote_sample(Some(target), final_parent, profile, None, 0)
            .await
    }

    /// Price a hypothetical next block from a completed scan. Unlike submission
    /// pricing, ordinary head advancement is allowed; the sampled block must
    /// remain canonical. This result cannot select a real submission target.
    pub(super) async fn advisory_quote(
        &self,
        sample: L1BlockRef,
        slot_seconds: u64,
    ) -> Result<PostingQuote, PostingError> {
        self.quote_sample(
            None,
            false,
            PriorityProfile::CrossChain,
            Some(sample),
            slot_seconds,
        )
        .await
    }

    async fn quote_sample(
        &self,
        target: Option<BundleTarget>,
        final_parent: bool,
        profile: PriorityProfile,
        pinned: Option<L1BlockRef>,
        slot_seconds: u64,
    ) -> Result<PostingQuote, PostingError> {
        let percentile = self.fees.priority_percentile_for(profile);
        let provider = &self.ctx.l1_provider;
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| PostingError::Unavailable(e.to_string()))?;
        if chain_id != self.ctx.l1_chain_id
            || chain_id != self.chain.evm_config.chain_spec().chain().id()
        {
            return Err(PostingError::Unavailable(
                "submission RPC and selected L1 profile have different chain IDs".into(),
            ));
        }
        let head = provider
            .get_block_by_number(pinned.map_or(BlockNumberOrTag::Latest, |b| b.number.into()))
            .await
            .map_err(|e| PostingError::Unavailable(e.to_string()))?
            .ok_or_else(|| PostingError::Unavailable("canonical fee sample is missing".into()))?;
        let sample = L1BlockRef {
            number: head.header.number,
            hash: head.header.hash,
        };
        if pinned.is_some_and(|expected| expected != sample) {
            return Err(PostingError::Unavailable(
                "advisory fee sample was reorged".into(),
            ));
        }
        let parent = head.header.inner;
        let target =
            match target {
                Some(target) => target,
                None => BundleTarget::Exact {
                    block: parent.number.checked_add(1).ok_or_else(|| {
                        PostingError::Unavailable("sample height overflow".into())
                    })?,
                    timestamp: parent.timestamp.checked_add(slot_seconds).ok_or_else(|| {
                        PostingError::Unavailable("sample timestamp overflow".into())
                    })?,
                },
            };
        let (env, attributes) = self.environment(&parent, target)?;
        if final_parent && sample.number.checked_add(1) != Some(env.block_env.number.to::<u64>()) {
            return Err(PostingError::Unavailable(
                "target's canonical parent is not yet available".into(),
            ));
        }
        let estimated_priority_fee = if self.fees.priority_override.is_none() {
            let history = provider
                .get_fee_history(
                    PRIORITY_FEE_HISTORY_BLOCKS,
                    sample.number.into(),
                    &[f64::from(percentile)],
                )
                .await
                .map_err(|e| PostingError::Unavailable(format!("priority fee history: {e}")))?;
            let rewards = history.reward.as_deref();
            Some(estimate_next_block_priority_fee(
                history.oldest_block,
                sample.number,
                rewards,
                percentile,
            )?)
        } else {
            None
        };
        let quote = self.fees.quote(FeeSample {
            chain_id,
            block: sample,
            target,
            execution_base_fee: Some(u128::from(env.block_env.basefee)),
            blob_base_fee: env
                .block_env
                .blob_excess_gas_and_price
                .map(|blob| blob.blob_gasprice),
            profile,
            estimated_priority_fee,
        })?;
        let check = provider
            .get_block_by_number(pinned.map_or(BlockNumberOrTag::Latest, |b| b.number.into()))
            .await
            .map_err(|e| PostingError::Unavailable(e.to_string()))?
            .ok_or_else(|| PostingError::Unavailable("fee sample disappeared".into()))?;
        if check.header.hash != sample.hash || check.header.number != sample.number {
            return Err(PostingError::Unavailable(
                "canonical head changed during fee estimation".into(),
            ));
        }
        let gas_budget = env
            .cfg_env
            .tx_gas_limit_cap()
            .min(parent.gas_limit)
            .min(self.operator_gas_cap);
        event!(name: "eez.composer.postbatch.fee_quote", Level::INFO,
            event_name = "eez.composer.postbatch.fee_quote", chain_id, sample_block = sample.number,
            sample_hash = %sample.hash, target = ?target, gas_budget,
            network_transaction_gas_max = env.cfg_env.tx_gas_limit_cap(), block_gas = parent.gas_limit,
            operator_gas_cap = self.operator_gas_cap, priority_source = if self.fees.priority_override.is_some() { "override" } else { "fee_history" },
            priority_history_blocks = PRIORITY_FEE_HISTORY_BLOCKS,
            priority_profile = ?profile, priority_reward_percentile = percentile,
            execution_base_fee = env.block_env.basefee, blob_base_fee = quote.estimated_blob_gas_price,
            estimated_priority_fee,
            priority_min = self.fees.priority_min, priority_max = self.fees.priority_max_for(profile),
            priority_fee = quote.max_priority_fee_per_gas, execution_fee_cap = quote.max_fee_per_gas,
            blob_fee_cap = quote.max_fee_per_blob_gas, "selected network posting quote");
        Ok(PostingQuote {
            quote,
            parent,
            attributes,
            env,
            gas_budget,
        })
    }

    /// Execute the exact ordered signed transactions with the node's block
    /// executor. This includes pre-block system changes, real intrinsic/floor
    /// gas, balances, fees, nonces, forwarding, and cumulative block gas.
    pub fn simulate(
        &self,
        raw: &[Bytes],
        quote: &PostingQuote,
        rollup_id: u64,
        endpoint: B256,
        semantics: &[SemanticTransaction],
        stage: &'static str,
    ) -> Result<BundleExecution, PostingError> {
        use reth_chainspec::EthChainSpec as _;
        let mut transactions = Vec::with_capacity(raw.len());
        let mut blob_counts = Vec::with_capacity(raw.len());
        for bytes in raw {
            let (transaction, blobs) = decode_transaction(bytes)?;
            if transaction.gas_limit() > quote.env.cfg_env.tx_gas_limit_cap() {
                return Err(PostingError::Capacity(
                    "companion or poster exceeds active per-transaction gas limit".into(),
                ));
            }
            transactions.push(transaction);
            blob_counts.push(blobs);
        }
        let params = self
            .chain
            .evm_config
            .chain_spec()
            .blob_params_at_timestamp(quote.attributes.timestamp)
            .ok_or_else(|| PostingError::Unavailable("target blob limits missing".into()))?;
        let limits = eez_blob_codec::BlobLimits::from_params(params)
            .map_err(|e| PostingError::Unavailable(e.to_string()))?;
        let companions = blob_counts
            .iter()
            .skip(1)
            .try_fold(0_u64, |sum, count| sum.checked_add(*count))
            .ok_or_else(|| PostingError::Capacity("companion blob count overflow".into()))?;
        let allowance = limits
            .outgoing(companions, u64::MAX, self.operator_blob_cap)
            .map_err(|e| PostingError::Capacity(e.to_string()))?;
        if blob_counts
            .first()
            .is_none_or(|count| *count == 0 || *count > allowance)
            || blob_counts
                .iter()
                .any(|count| *count > params.max_blobs_per_tx)
        {
            return Err(PostingError::Capacity(
                "whole ordered bundle exceeds target blob rules".into(),
            ));
        }
        if transactions[0].gas_limit() > quote.gas_budget {
            return Err(PostingError::Capacity(
                "poster exceeds its resolved gas budget".into(),
            ));
        }
        let local_header = self
            .chain
            .headers
            .header_by_number(quote.parent.number)
            .map_err(|e| PostingError::Unavailable(e.to_string()))?
            .ok_or_else(|| {
                PostingError::Unavailable("canonical L1 state is not locally available".into())
            })?;
        if local_header.hash_slow() != quote.quote.sample.hash {
            return Err(PostingError::Unavailable(
                "local L1 state and canonical fee sample disagree".into(),
            ));
        }
        let provider = self
            .chain
            .provider
            .state_by_block_hash(quote.quote.sample.hash)
            .map_err(|e| PostingError::Unavailable(e.to_string()))?;
        let mut state = State::builder()
            .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
                provider,
            )))
            .with_bundle_update()
            .build();
        let selected = self.gas_trace.as_ref().and_then(|config| {
            semantics
                .iter()
                .map(|semantic| alloy_primitives::keccak256(&semantic.tx_data))
                .find(|hash| config.selects(*hash))
                .map(|hash| (config, hash))
        });
        let _trace = selected.and_then(|(config, hash)| {
            match config.begin(stage, quote.quote.sample.hash, hash) {
                Ok(scope) => Some(scope),
                Err(error) => {
                    event!(Level::WARN, %error, "cannot start selected final gas trace");
                    None
                }
            }
        });
        if gas_trace::active() {
            let source_hashes = semantics
                .iter()
                .map(|semantic| alloy_primitives::keccak256(&semantic.tx_data))
                .collect::<Vec<_>>();
            gas_trace::emit(serde_json::json!({
                "kind": "preflight_context", "endpoint": endpoint,
                "target_timestamp": quote.attributes.timestamp,
                "transaction_count": transactions.len(),
                "source_hashes": source_hashes,
            }));
        }
        let mut inspector = EffectInspector::new(self.registry, rollup_id, semantics)?;
        let output = if let Some(config) = &self.chain.gnosis_evm_config {
            let parent = SealedHeader::new(quote.parent.clone().into(), quote.quote.sample.hash);
            let context = config
                .context_for_next_block(&parent, quote.attributes.clone())
                .map_err(|e| PostingError::Unavailable(e.to_string()))?;
            let traced = gas_trace::maybe_trace(&mut inspector, self.ctx.l1_chain_id);
            let evm = config.evm_with_env_and_inspector(&mut state, quote.env.clone(), traced);
            run_bundle(
                config.create_executor(evm, context),
                transactions,
                self.registry,
                rollup_id,
                endpoint,
            )?
        } else {
            let parent = SealedHeader::new(quote.parent.clone(), quote.quote.sample.hash);
            // L1 posting uses its network rules, independently of the native L2 profile.
            let config = reth_evm_ethereum::EthEvmConfig::new(Arc::clone(
                self.chain.evm_config.chain_spec(),
            ));
            let context = config
                .context_for_next_block(&parent, quote.attributes.clone())
                .map_err(|e| PostingError::Unavailable(e.to_string()))?;
            let traced = gas_trace::maybe_trace(&mut inspector, self.ctx.l1_chain_id);
            let evm = config.evm_with_env_and_inspector(&mut state, quote.env.clone(), traced);
            run_bundle(
                config.create_executor(evm, context),
                transactions,
                self.registry,
                rollup_id,
                endpoint,
            )?
        };
        inspector.verify()?;
        if gas_trace::active() {
            gas_trace::emit(serde_json::json!({
                "kind": "preflight_result", "gas_used": output.gas_used,
                "gas_required": output.gas_required,
            }));
        }
        Ok(output)
    }

    /// Reprice a never-dispatched poster, measure execution, and validate again
    /// at the actual declared gas. Companion bytes are never changed.
    pub async fn finalize(
        &self,
        mut raw: Vec<Bytes>,
        target: BundleTarget,
        rollup_id: u64,
        endpoint: B256,
        semantics: &[SemanticTransaction],
    ) -> Result<(Vec<Bytes>, FeeQuote, BundleExecution), PostingError> {
        let quote = self
            .quote(target, true, priority_profile(semantics))
            .await?;
        let (_, blobs) = decode_transaction(
            raw.first()
                .ok_or_else(|| PostingError::Transaction("empty bundle".into()))?,
        )?;
        let balance = self
            .ctx
            .l1_provider
            .get_balance(self.ctx.l1_poster_signer.address())
            .block_id(quote.quote.sample.hash.into())
            .await
            .map_err(|e| PostingError::Unavailable(e.to_string()))?;
        let blob_reserve = quote.quote.reserve(0, blobs)?.blobs;
        let affordable = balance
            .checked_sub(blob_reserve)
            .and_then(|funds| {
                if quote.quote.max_fee_per_gas == 0 {
                    Some(U256::MAX)
                } else {
                    funds.checked_div(U256::from(quote.quote.max_fee_per_gas))
                }
            })
            .ok_or_else(|| {
                PostingError::Transaction(
                    "poster balance cannot reserve blob and execution fees".into(),
                )
            })?;
        let probe_gas = quote
            .gas_budget
            .min(affordable.try_into().unwrap_or(u64::MAX));
        let reprice = |bytes: &Bytes, gas: u64| {
            eez_blob_codec::reprice_blob_transaction(
                bytes,
                gas,
                quote.quote.max_fee_per_gas,
                quote.quote.max_priority_fee_per_gas,
                quote.quote.max_fee_per_blob_gas,
                &self.ctx.l1_poster_signer,
            )
            .map(|signed| signed.pooled_transaction)
            .map_err(|e| PostingError::Transaction(e.to_string()))
        };
        raw[0] = reprice(&raw[0], probe_gas)?;
        let probe = self.simulate(
            &raw,
            &quote,
            rollup_id,
            endpoint,
            semantics,
            "preflight_probe",
        )?;
        let selected_gas = self.fees.declared_gas(probe.gas_required[0], probe_gas)?;
        raw[0] = reprice(&raw[0], selected_gas)?;
        let execution = match self.simulate(
            &raw,
            &quote,
            rollup_id,
            endpoint,
            semantics,
            "preflight_final",
        ) {
            Ok(execution) => execution,
            Err(PostingError::Effects(_)) if selected_gas < probe_gas => {
                // EIP-150 may need more headroom than consumed gas plus margin.
                // The larger value was validated, but changing gas invalidates
                // the prior simulation, so execute the exact final bytes again.
                raw[0] = reprice(&raw[0], probe_gas)?;
                self.simulate(
                    &raw,
                    &quote,
                    rollup_id,
                    endpoint,
                    semantics,
                    "preflight_fallback",
                )?
            }
            Err(error) => return Err(error),
        };
        let (poster, _) = decode_transaction(&raw[0])?;
        let estimate = quote.quote.estimate(execution.gas_used[0], blobs)?;
        let reserve = quote.quote.reserve(poster.gas_limit(), blobs)?;
        event!(name: "eez.composer.postbatch.preflight", Level::INFO,
            event_name = "eez.composer.postbatch.preflight", rollup_id, target = ?target,
            poster_gas_used = execution.gas_used[0], declared_gas = poster.gas_limit(),
            companion_gas_used = ?execution.gas_used.get(1..), blobs,
            estimated_execution_cost = %estimate.execution, estimated_blob_cost = %estimate.blobs,
            estimated_post_cost = %estimate.total, fee_cap_balance_reservation = %reserve.total,
            "ordered bundle completes all intended effects at final signed gas and fees");
        Ok((raw, quote.quote, execution))
    }
}

fn decode_transaction(raw: &[u8]) -> Result<(Recovered<TransactionSigned>, u64), PostingError> {
    let mut input = raw;
    let (canonical, blobs) = if raw.first() == Some(&3) {
        let signed =
            Signed::<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>::decode_2718(&mut input)
                .map_err(|e| {
                    PostingError::Transaction(format!(
                        "blob transaction requires its full sidecar: {e}"
                    ))
                })?;
        signed
            .tx()
            .tx
            .validate_blob(
                &signed.tx().sidecar,
                alloy_eips::eip4844::env_settings::EnvKzgSettings::Default.get(),
            )
            .map_err(|e| PostingError::Transaction(format!("blob sidecar: {e}")))?;
        let blobs = u64::try_from(signed.tx().tx.blob_versioned_hashes.len())
            .map_err(|e| PostingError::Transaction(e.to_string()))?;
        let canonical = signed
            .tx()
            .tx
            .clone()
            .into_signed(*signed.signature())
            .encoded_2718();
        (canonical, blobs)
    } else {
        input = &[];
        (raw.to_vec(), 0)
    };
    if !input.is_empty() {
        return Err(PostingError::Transaction(
            "trailing pooled transaction bytes".into(),
        ));
    }
    let mut input = canonical.as_slice();
    let transaction = TransactionSigned::decode_2718(&mut input)
        .map_err(|e| PostingError::Transaction(e.to_string()))?;
    if !input.is_empty() {
        return Err(PostingError::Transaction(
            "trailing transaction bytes".into(),
        ));
    }
    let sender = transaction
        .recover_signer()
        .map_err(|e| PostingError::Transaction(e.to_string()))?;
    Ok((Recovered::new_unchecked(transaction, sender), blobs))
}

fn run_bundle<E>(
    mut executor: E,
    transactions: Vec<Recovered<TransactionSigned>>,
    registry: Address,
    rollup_id: u64,
    endpoint: B256,
) -> Result<BundleExecution, PostingError>
where
    E: BlockExecutor<
            Transaction = TransactionSigned,
            Result = EthTxResult<revm::context::result::HaltReason, alloy_consensus::TxType>,
        >,
    E::Evm: reth_evm::Evm<Tx: reth_evm::FromRecoveredTx<TransactionSigned>>,
{
    executor
        .apply_pre_execution_changes()
        .map_err(|e| PostingError::Unavailable(e.to_string()))?;
    let mut gas_used = Vec::with_capacity(transactions.len());
    let mut gas_required = Vec::with_capacity(transactions.len());
    let gas_limits = transactions.iter().map(|tx| tx.gas_limit()).collect();
    let blob_counts = transactions
        .iter()
        .map(|tx| {
            tx.blob_versioned_hashes()
                .map_or(0, |hashes| hashes.len() as u64)
        })
        .collect();
    let transaction_hashes = transactions
        .iter()
        .map(|tx| {
            use alloy_consensus::transaction::TxHashRef as _;
            *tx.tx_hash()
        })
        .collect();
    let mut final_root = None;
    for (index, transaction) in transactions.into_iter().enumerate() {
        gas_trace::set_tx_index(index);
        let result = executor
            .execute_transaction_without_commit(transaction)
            .map_err(|e| PostingError::Transaction(format!("transaction {index}: {e}")))?;
        if !result.result.result.is_success() {
            return Err(PostingError::Effects(format!(
                "transaction {index} failed: {}",
                result.result.result
            )));
        }
        for log in result
            .result
            .result
            .logs()
            .iter()
            .filter(|log| log.address == registry)
        {
            if log.data.topics().first() == Some(&eez_protocol::abi::L2TxSkipped::SIGNATURE_HASH) {
                return Err(PostingError::Effects(format!(
                    "transaction {index} skipped an intended immediate entry"
                )));
            }
            if log.data.topics().first()
                == Some(&eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH)
            {
                let event = eez_protocol::abi::L2ExecutionPerformed::decode_log(log)
                    .map_err(|e| PostingError::Effects(e.to_string()))?;
                if event.rollupId == rollup_id {
                    final_root = Some(event.newRoot);
                }
            }
        }
        let gas = result.result.result.gas();
        if gas_trace::active() {
            gas_trace::emit(serde_json::json!({
                "kind": "transaction_result", "tx_index": index,
                "gas_used": gas.tx_gas_used(),
                "gas_required": gas.total_gas_spent().max(gas.floor_gas()),
            }));
        }
        gas_used.push(gas.tx_gas_used());
        // Refunds reduce the receipt charge, but cannot fund execution. The
        // calldata floor can also exceed execution consumption.
        gas_required.push(gas.total_gas_spent().max(gas.floor_gas()));
        executor.commit_transaction(result);
    }
    if final_root != Some(endpoint) {
        return Err(PostingError::Effects(format!(
            "final applied root {final_root:?} differs from {endpoint}"
        )));
    }
    Ok(BundleExecution {
        gas_used,
        gas_required,
        gas_limits,
        transaction_hashes,
        blob_counts,
    })
}

alloy_sol_types::sol! {
    function executeCrossChainCall(address sourceAddress, bytes callData) external payable returns (bytes);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedCall {
    source: Address,
    target: Address,
    data: Bytes,
    value: U256,
    mode: CallMode,
    success: bool,
    output: Bytes,
}

/// Observe the actual manager path without intercepting execution, warming
/// accounts, or replacing proven lookups with simulation results. Root checks
/// alone miss a caught static lookup failure with no state change.
pub(super) struct EffectInspector {
    registry: Address,
    rollup_id: u64,
    proxy_slot: u8,
    expected: Vec<ObservedCall>,
    frames: Vec<Option<usize>>,
    observed: Vec<Option<ObservedCall>>,
}

impl EffectInspector {
    pub(super) fn new(
        registry: Address,
        rollup_id: u64,
        semantics: &[SemanticTransaction],
    ) -> Result<Self, PostingError> {
        let actions = [true, false].into_iter().flat_map(|outbound| {
            semantics
                .iter()
                .filter(move |tx| (tx.origin_chain.0 != 0) == outbound)
                .flat_map(|tx| &tx.actions)
        });
        Self::for_actions(
            registry,
            0,
            rollup_id,
            eez_protocol::EEZ_AUTHORIZED_PROXIES_SLOT,
            actions,
        )
    }

    pub(super) fn for_actions<'a>(
        registry: Address,
        source_rollup: u64,
        rollup_id: u64,
        proxy_slot: u8,
        actions: impl IntoIterator<Item = &'a eez_protocol::ExecutedAction>,
    ) -> Result<Self, PostingError> {
        let mut expected = Vec::new();
        for action in actions
            .into_iter()
            .filter(|a| a.source_rollup_id.0 == source_rollup && a.target_rollup_id.0 == rollup_id)
        {
            let ExecutionOutcome::Resolved {
                return_data,
                success,
                ..
            } = &action.outcome
            else {
                return Err(PostingError::Effects(
                    "unresolved intended cross-chain outcome".into(),
                ));
            };
            expected.push(ObservedCall {
                source: action.source_address,
                target: action.target_address,
                data: action.data.clone(),
                value: action.value,
                mode: action.call_mode,
                success: *success,
                output: return_data.clone().into(),
            });
        }
        Ok(Self {
            registry,
            rollup_id,
            proxy_slot,
            expected,
            frames: Vec::new(),
            observed: Vec::new(),
        })
    }

    pub(super) fn verify(self) -> Result<(), PostingError> {
        let observed: Vec<_> = self.observed.into_iter().flatten().collect();
        if observed != self.expected {
            return Err(PostingError::Effects(format!(
                "actual cross-chain calls/outcomes differ (expected {}, observed {})",
                self.expected.len(),
                observed.len()
            )));
        }
        Ok(())
    }
}

impl<DB: Database, CTX: ContextTr<Db = DB, Journal = Journal<DB>>> Inspector<CTX>
    for EffectInspector
{
    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let data = inputs.input.as_bytes(context);
        // The registry is a transparent proxy: its DELEGATECALL into the
        // implementation repeats the target (storage context), caller and
        // calldata of the CALL that entered it. Only the entering frame is
        // the cross-chain call, so recording both would count it twice.
        let recognized = matches!(inputs.scheme, CallScheme::Call | CallScheme::StaticCall)
            && inputs.target_address == self.registry
            && (data.starts_with(&executeCrossChainCallCall::SELECTOR)
                || data.starts_with(&eez_protocol::abi::staticCrossChainCallCall::SELECTOR));
        self.frames.push(recognized.then(|| {
            self.observed.push(None);
            self.observed.len() - 1
        }));
        None
    }

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        let Some(index) = self.frames.pop().flatten() else {
            return;
        };
        // The real manager has already read this slot. Inspect the journal
        // directly so observation cannot change cold/warm gas accounting.
        let key = eez_protocol::proxy_mapping_key(inputs.caller, self.proxy_slot);
        let proxy = context
            .journal()
            .state
            .get(&self.registry)
            .and_then(|account| account.storage.get(&U256::from_be_bytes(key.0)))
            .and_then(|slot| eez_protocol::decode_proxy_value(slot.present_value));
        let Some(proxy) = proxy.filter(|proxy| proxy.original_rollup_id.0 == self.rollup_id) else {
            return;
        };
        let data = inputs.input.as_bytes(context);
        let (source, data, mode) = if data.starts_with(&executeCrossChainCallCall::SELECTOR) {
            let Ok(call) = executeCrossChainCallCall::abi_decode(&data) else {
                return;
            };
            (call.sourceAddress, call.callData, CallMode::Mutable)
        } else {
            let Ok(call) = eez_protocol::abi::staticCrossChainCallCall::abi_decode(&data) else {
                return;
            };
            (call.sourceAddress, call.callData, CallMode::Static)
        };
        let success = outcome.result.result.is_ok();
        let output = if success {
            let Ok(output) = executeCrossChainCallCall::abi_decode_returns(&outcome.result.output)
            else {
                return;
            };
            output
        } else {
            outcome.result.output.clone()
        };
        self.observed[index] = Some(ObservedCall {
            source,
            target: proxy.original_address,
            data,
            value: inputs.call_value(),
            mode,
            success,
            output,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxEip1559;
    use alloy_network::TxSignerSync;
    use alloy_primitives::TxKind;
    use alloy_signer_local::PrivateKeySigner;
    use reth_chainspec::ChainSpecBuilder;
    use reth_evm::Evm as _;
    use reth_evm_ethereum::EthEvmConfig;
    use revm::{
        database::InMemoryDB,
        state::{AccountInfo, Bytecode},
    };

    const REGISTRY: Address = Address::repeat_byte(0x11);
    const PROXY: Address = Address::repeat_byte(0x22);
    const ENDPOINT: B256 = B256::repeat_byte(0x33);

    fn signer() -> PrivateKeySigner {
        B256::with_last_byte(7).to_string().parse().unwrap()
    }

    fn tx(nonce: u64, to: Address, gas: u64, data: Bytes) -> Recovered<TransactionSigned> {
        let mut tx = TxEip1559 {
            chain_id: 1,
            nonce,
            gas_limit: gas,
            max_fee_per_gas: 100,
            max_priority_fee_per_gas: 2,
            to: TxKind::Call(to),
            input: data,
            ..Default::default()
        };
        let signature = signer().sign_transaction_sync(&mut tx).unwrap();
        let raw = tx.into_signed(signature).encoded_2718();
        decode_transaction(&raw).unwrap().0
    }

    fn configuration(gas_limit: u64) -> (EthEvmConfig, Header, NextBlockEnvAttributes) {
        let config = EthEvmConfig::new(Arc::new(
            ChainSpecBuilder::mainnet().prague_activated().build(),
        ));
        let parent = Header {
            number: 1,
            timestamp: 1,
            gas_limit,
            gas_used: gas_limit / 2,
            base_fee_per_gas: Some(10),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            ..Default::default()
        };
        let attributes = NextBlockEnvAttributes {
            timestamp: 2,
            suggested_fee_recipient: Address::repeat_byte(9),
            prev_randao: B256::ZERO,
            gas_limit,
            parent_beacon_block_root: Some(B256::ZERO),
            withdrawals: Some(Default::default()),
            extra_data: Bytes::new(),
            slot_number: None,
        };
        (config, parent, attributes)
    }

    fn database(code: Vec<u8>) -> InMemoryDB {
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            signer().address(),
            AccountInfo {
                balance: U256::from(10).pow(U256::from(24)),
                ..Default::default()
            },
        );
        db.insert_account_info(
            REGISTRY,
            AccountInfo {
                code: Some(Bytecode::new_raw(code.into())),
                ..Default::default()
            },
        );
        db
    }

    fn push32(code: &mut Vec<u8>, value: B256) {
        code.push(0x7f);
        code.extend(value.0);
    }

    fn emit_root(skipped: bool) -> Vec<u8> {
        let mut code = Vec::new();
        push32(&mut code, ENDPOINT);
        code.extend([0x5f, 0x52]); // MSTORE(0, endpoint)
        push32(&mut code, B256::with_last_byte(1)); // indexed rollup/index
        push32(
            &mut code,
            if skipped {
                eez_protocol::abi::L2TxSkipped::SIGNATURE_HASH
            } else {
                eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH
            },
        );
        // The pinned settlement event includes newRoot and etherBalance;
        // untouched memory supplies the zero balance word in this fixture.
        let data_length = if skipped { 0x20 } else { 0x40 };
        code.extend([0x60, data_length, 0x5f, 0xa2, 0x00]); // LOG2, STOP
        code
    }

    fn execute(
        code: Vec<u8>,
        txs: Vec<Recovered<TransactionSigned>>,
        gas_limit: u64,
    ) -> Result<BundleExecution, PostingError> {
        let (config, parent, attributes) = configuration(gas_limit);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let mut state = State::builder()
            .with_database(database(code))
            .with_bundle_update()
            .build();
        let parent = SealedHeader::seal_slow(parent);
        let context = config.context_for_next_block(&parent, attributes).unwrap();
        let evm = config.evm_with_env(&mut state, env);
        run_bundle(
            config.create_executor(evm, context),
            txs,
            REGISTRY,
            1,
            ENDPOINT,
        )
    }

    #[test]
    fn full_bundle_uses_actual_cumulative_gas_and_checks_declared_gas_for_every_companion() {
        let run = |second_gas| {
            execute(
                emit_root(false),
                vec![
                    tx(0, REGISTRY, 90_000, Bytes::new()),
                    tx(1, Address::repeat_byte(0x44), second_gas, Bytes::new()),
                ],
                120_000,
            )
        };
        let valid = run(90_000).unwrap();
        // Declared limits total 180k, but the second transaction fits the
        // remaining 120k block gas after the first one's actual execution.
        assert_eq!(valid.gas_used.len(), 2);
        assert_eq!(valid.gas_required, valid.gas_used);
        assert!(valid.gas_used.iter().sum::<u64>() < 90_000);
        assert!(matches!(run(100_000), Err(PostingError::Transaction(_))));
        // A missing nonce cannot be repaired by the first transaction passing.
        assert!(
            execute(
                emit_root(false),
                vec![
                    tx(0, REGISTRY, 90_000, Bytes::new()),
                    tx(2, Address::repeat_byte(0x44), 90_000, Bytes::new())
                ],
                120_000
            )
            .is_err()
        );
    }

    #[test]
    fn posting_limit_covers_execution_before_storage_refunds() {
        // Set then clear four slots, earning enough refund to hit the 20% cap.
        // A 20% margin on the resulting receipt charge is less than the gas
        // execution needs before those refunds become available.
        let mut code = Vec::new();
        for slot in 0..4u8 {
            code.extend([0x60, 1, 0x60, slot, 0x55]); // SSTORE(slot, 1)
            code.extend([0x5f, 0x60, slot, 0x55]); // SSTORE(slot, 0)
        }
        code.extend(emit_root(false));
        let run = |gas| {
            execute(
                code.clone(),
                vec![tx(0, REGISTRY, gas, Bytes::new())],
                1_000_000,
            )
        };
        let fees = PostingFeeConfig::parse(|name| {
            Ok(match name {
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS" | "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS" => {
                    Some("100".into())
                }
                "EEZ_L1_POSTBATCH_PRIORITY_FEE" => Some("2".into()),
                _ => None,
            })
        })
        .unwrap();
        let probe = run(900_000).unwrap();
        let old_limit = fees.declared_gas(probe.gas_used[0], 900_000).unwrap();
        assert!(old_limit < probe.gas_required[0]);
        assert!(matches!(run(old_limit), Err(PostingError::Effects(_))));
        let limit = fees.declared_gas(probe.gas_required[0], 900_000).unwrap();
        assert!(limit < 900_000);
        let final_execution = run(limit).unwrap();
        assert_eq!(final_execution.gas_used, probe.gas_used);
        assert_eq!(final_execution.gas_required, probe.gas_required);
        assert!(fees.declared_gas(probe.gas_required[0], limit - 1).is_err());
    }

    #[test]
    fn posting_limit_respects_calldata_floor() {
        let data = Bytes::from(vec![1; 1024]);
        let run = |gas| {
            execute(
                emit_root(false),
                vec![tx(0, REGISTRY, gas, data.clone())],
                120_000,
            )
        };
        let probe = run(90_000).unwrap();
        // Prague calldata floor: 21k + 10 gas per token (4 per nonzero byte).
        assert_eq!(probe.gas_required, vec![21_000 + 40 * 1024]);
        assert_eq!(probe.gas_required, probe.gas_used);
        let final_execution = run(probe.gas_required[0]).unwrap();
        assert_eq!(final_execution.gas_used, probe.gas_used);
        assert!(matches!(
            run(probe.gas_required[0] - 1),
            Err(PostingError::Transaction(_))
        ));
    }

    #[test]
    fn outer_success_without_complete_effects_is_rejected() {
        let mut truncated = emit_root(false);
        let length_index = truncated.len() - 4;
        truncated[length_index] = 0x20;
        assert!(matches!(
            execute(
                truncated,
                vec![tx(0, REGISTRY, 90_000, Bytes::new())],
                120_000
            ),
            Err(PostingError::Effects(_))
        ));
        assert!(
            execute(
                emit_root(false),
                vec![tx(0, REGISTRY, 90_000, Bytes::new())],
                120_000
            )
            .is_ok()
        );
        assert!(
            matches!(execute(emit_root(true), vec![tx(0, REGISTRY, 90_000, Bytes::new())], 120_000),
            Err(PostingError::Effects(message)) if message.contains("skipped"))
        );
        assert!(
            matches!(execute(vec![0x00], vec![tx(0, REGISTRY, 90_000, Bytes::new())], 120_000),
            Err(PostingError::Effects(message)) if message.contains("final applied root"))
        );
        assert!(
            matches!(execute(vec![0x5f, 0x5f, 0xfd], vec![tx(0, REGISTRY, 90_000, Bytes::new())], 120_000),
            Err(PostingError::Effects(message)) if message.contains("failed"))
        );
    }

    #[test]
    fn final_gas_and_fee_changes_require_complete_effect_revalidation() {
        // Both branches return outer success. Only sufficient forwarding gas
        // completes the entry; the smaller declaration emits a skipped entry.
        let mut code = vec![0x61, 0xc3, 0x50, 0x5a, 0x11, 0x60, 0, 0x57];
        code.extend(emit_root(true));
        code[6] = u8::try_from(code.len()).unwrap();
        code.push(0x5b);
        code.extend(emit_root(false));
        assert!(
            execute(
                code.clone(),
                vec![tx(0, REGISTRY, 90_000, Bytes::new())],
                120_000
            )
            .is_ok()
        );
        assert!(matches!(
            execute(code, vec![tx(0, REGISTRY, 50_000, Bytes::new())], 120_000),
            Err(PostingError::Effects(_))
        ));

        // The same state transition now derives its applied value from the
        // actual GASPRICE. A new fee invalidates the old expected commitment.
        let mut priced = vec![0x3a];
        priced.extend_from_slice(&emit_root(false)[33..]);
        let run = |tip, expected| {
            let (config, parent, attrs) = configuration(120_000);
            let env = config.next_evm_env(&parent, &attrs).unwrap();
            let mut state = State::builder()
                .with_database(database(priced.clone()))
                .with_bundle_update()
                .build();
            let context = config
                .context_for_next_block(&SealedHeader::seal_slow(parent), attrs)
                .unwrap();
            let mut transaction = TxEip1559 {
                chain_id: 1,
                nonce: 0,
                gas_limit: 90_000,
                max_fee_per_gas: 100,
                max_priority_fee_per_gas: tip,
                to: TxKind::Call(REGISTRY),
                ..Default::default()
            };
            let signature = signer().sign_transaction_sync(&mut transaction).unwrap();
            let raw = transaction.into_signed(signature).encoded_2718();
            run_bundle(
                config.create_executor(config.evm_with_env(&mut state, env), context),
                vec![decode_transaction(&raw).unwrap().0],
                REGISTRY,
                1,
                B256::with_last_byte(expected),
            )
        };
        assert!(run(2, 12).is_ok());
        assert!(matches!(run(3, 12), Err(PostingError::Effects(_))));
        assert!(
            run(3, 13).is_ok(),
            "a newly composed expected outcome must validate at the final fees"
        );
    }

    #[test]
    fn selected_preflight_trace_records_storage_gas_without_changing_execution() {
        let directory = std::env::temp_dir().join(format!(
            "eez-preflight-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let trace = gas_trace::Config::parse(
            Some(B256::ZERO.to_string()),
            Some(directory.to_str().unwrap().into()),
        )
        .unwrap()
        .unwrap();
        let run = |record: bool| {
            let (config, parent, attrs) = configuration(200_000);
            let env = config.next_evm_env(&parent, &attrs).unwrap();
            let mut code = vec![0x60, 1, 0x60, 0, 0x55]; // SSTORE(0, 1)
            code.extend(emit_root(false));
            let mut state = State::builder()
                .with_database(database(code))
                .with_bundle_update()
                .build();
            let context = config
                .context_for_next_block(&SealedHeader::seal_slow(parent), attrs)
                .unwrap();
            let mut inspector = EffectInspector::new(REGISTRY, 1, &[]).unwrap();
            let _scope = record.then(|| {
                trace
                    .begin("preflight_final", B256::ZERO, B256::ZERO)
                    .unwrap()
            });
            let traced = gas_trace::maybe_trace(&mut inspector, 1);
            let evm = config.evm_with_env_and_inspector(&mut state, env, traced);
            let execution = run_bundle(
                config.create_executor(evm, context),
                vec![tx(0, REGISTRY, 120_000, Bytes::new())],
                REGISTRY,
                1,
                ENDPOINT,
            )
            .unwrap();
            inspector.verify().unwrap();
            (execution.gas_used[0], execution.gas_required[0])
        };
        let ordinary = run(false);
        assert!(!directory.exists(), "ordinary execution wrote a gas trace");
        let selected = run(true);
        assert_eq!(ordinary, selected);
        let files = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1);
        let values = std::fs::read_to_string(&files[0])
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(values.iter().any(|value| {
            value["kind"] == "step"
                && value["op"] == 0x55
                && value["gas_cost"].as_u64().is_some_and(|gas| gas > 0)
                && value["storage_key"] == "0x0"
        }));
        assert!(values.iter().any(|value| {
            value["kind"] == "transaction_result"
                && value["gas_used"] == ordinary.0
                && value["gas_required"] == ordinary.1
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn static_call_database(returned: u8) -> InMemoryDB {
        let key = eez_protocol::proxy_mapping_key(PROXY, eez_protocol::EEZ_AUTHORIZED_PROXIES_SLOT);
        let mut code = Vec::new();
        push32(&mut code, key);
        code.extend([0x54, 0x50]); // real manager reads authorization
        // Return ABI bytes containing one byte.
        for (offset, word) in [
            (0, B256::with_last_byte(32)),
            (32, B256::with_last_byte(1)),
            (64, B256::from([returned; 32])),
        ] {
            push32(&mut code, word);
            code.extend([0x60, offset, 0x52]);
        }
        code.extend([0x60, 96, 0x5f, 0xf3]);
        let mut db = database(code);
        // Same Solidity packed layout as the production proxy mapping.
        let value =
            (U256::from_be_slice(PROXY.as_slice()) << 8) | (U256::from(1) << 168) | U256::from(1);
        db.insert_account_storage(REGISTRY, U256::from_be_bytes(key.0), value)
            .unwrap();
        // Copy the calldata, STATICCALL the actual manager, ignore success,
        // and return. This deliberately permits a caller to catch a bad lookup.
        let mut proxy = vec![0x36, 0x5f, 0x5f, 0x37, 0x5f, 0x5f, 0x36, 0x5f, 0x73];
        proxy.extend(REGISTRY.0);
        proxy.extend([0x5a, 0xfa, 0x50, 0x00]);
        db.insert_account_info(
            PROXY,
            AccountInfo {
                code: Some(Bytecode::new_raw(proxy.into())),
                ..Default::default()
            },
        );
        db
    }

    fn static_semantics() -> Vec<SemanticTransaction> {
        vec![SemanticTransaction {
            origin_chain: eez_protocol::RollupId(0),
            tx_data: Bytes::new(),
            actions: vec![eez_protocol::ExecutedAction {
                parent_index: None,
                call_mode: CallMode::Static,
                source_address: signer().address(),
                target_address: PROXY,
                source_rollup_id: eez_protocol::RollupId(0),
                target_rollup_id: eez_protocol::RollupId(1),
                data: Bytes::from_static(b"lookup"),
                value: U256::ZERO,
                outcome: ExecutionOutcome::Resolved {
                    return_data: vec![42],
                    gas_used: 0,
                    success: true,
                },
                revert_span: None,
            }],
        }]
    }

    #[test]
    fn only_bundles_with_cross_chain_calls_use_the_cross_chain_priority() {
        assert_eq!(priority_profile(&[]), PriorityProfile::Base);
        let mut without_calls = static_semantics();
        without_calls[0].actions.clear();
        assert_eq!(priority_profile(&without_calls), PriorityProfile::Base);
        assert_eq!(
            priority_profile(&static_semantics()),
            PriorityProfile::CrossChain
        );
        without_calls.extend(static_semantics());
        assert_eq!(
            priority_profile(&without_calls),
            PriorityProfile::CrossChain
        );
    }

    #[test]
    fn actual_static_lookup_observation_preserves_gas_and_detects_caught_wrong_results() {
        let (config, parent, attributes) = configuration(1_000_000);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let data = eez_protocol::abi::staticCrossChainCallCall {
            sourceAddress: signer().address(),
            callData: Bytes::from_static(b"lookup"),
        }
        .abi_encode();
        let transaction = tx(0, PROXY, 200_000, data.into());
        let tx_env = config.tx_env(&transaction);
        let mut plain = static_call_database(42);
        let baseline = config
            .evm_with_env(&mut plain, env.clone())
            .transact(tx_env.clone())
            .unwrap();
        assert!(baseline.result.is_success());
        for (returned, valid) in [(42, true), (43, false)] {
            let mut state = static_call_database(returned);
            let mut inspector = EffectInspector::new(REGISTRY, 1, &static_semantics()).unwrap();
            let actual = config
                .evm_with_env_and_inspector(&mut state, env.clone(), &mut inspector)
                .transact(tx_env.clone())
                .unwrap();
            assert!(
                actual.result.is_success(),
                "source catches the lookup result"
            );
            assert_eq!(
                actual.result.tx_gas_used(),
                baseline.result.tx_gas_used(),
                "observation must not warm storage"
            );
            assert_eq!(inspector.verify().is_ok(), valid);
        }
        let mut state = static_call_database(42);
        let mut twice = static_semantics();
        let duplicate = twice[0].actions[0].clone();
        twice[0].actions.push(duplicate);
        let mut inspector = EffectInspector::new(REGISTRY, 1, &twice).unwrap();
        config
            .evm_with_env_and_inspector(&mut state, env, &mut inspector)
            .transact(tx_env)
            .unwrap();
        assert!(
            inspector.verify().is_err(),
            "one lookup cannot satisfy two expected calls"
        );
    }

    #[test]
    fn malformed_or_trailing_transactions_never_reach_execution() {
        assert!(decode_transaction(&[]).is_err());
        let signed = tx(0, REGISTRY, 100_000, Bytes::new());
        let mut raw = signed.encoded_2718();
        raw.push(0);
        assert!(decode_transaction(&raw).is_err());
    }

    #[test]
    fn chiado_native_executor_charges_its_blob_price_and_credits_its_fee_collector() {
        use reth_gnosis::{
            evm_config::{GnosisEvmConfig, HeaderLookup},
            spec::{chains::CHIADO_GENESIS, gnosis_spec::GnosisChainSpec},
        };
        #[derive(Debug)]
        struct Headers;
        impl HeaderLookup for Headers {
            fn header_by_hash(&self, _: &B256) -> Option<gnosis_primitives::header::GnosisHeader> {
                None
            }
        }
        let spec = GnosisChainSpec::from(CHIADO_GENESIS.clone());
        let collector: Address = spec.genesis().config.extra_fields["eip1559collector"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let fork_times = [
            (spec.genesis().config.cancun_time.unwrap(), false),
            (spec.genesis().config.prague_time.unwrap(), true),
        ];
        let config = GnosisEvmConfig::new(Arc::new(spec), Headers);
        for (timestamp, collects_blob_fees) in fork_times {
            let (_, mut parent, mut attributes) = configuration(30_000_000);
            parent.timestamp = timestamp;
            attributes.timestamp = timestamp + 5;
            let env = config.next_evm_env(&parent.into(), &attributes).unwrap();
            assert_eq!(env.cfg_env.chain_id, 10200);
            assert_eq!(
                env.cfg_env.tx_gas_limit_cap(),
                u64::MAX,
                "Osaka's cap must not be imposed at Cancun"
            );
            assert_eq!(
                env.block_env
                    .blob_excess_gas_and_price
                    .unwrap()
                    .blob_gasprice,
                1_000_000_000
            );
            let tx_env = revm::context::TxEnv {
                tx_type: 3,
                caller: signer().address(),
                kind: TxKind::Call(Address::repeat_byte(0x44)),
                gas_limit: 100_000,
                gas_price: 100,
                gas_priority_fee: Some(2),
                chain_id: Some(10200),
                max_fee_per_blob_gas: 2_000_000_000,
                blob_hashes: vec![B256::from([1; 32])],
                ..Default::default()
            };
            let initial = U256::from(10).pow(U256::from(24));
            let result = config
                .evm_with_env(database(vec![0]), env.clone())
                .transact(tx_env)
                .unwrap();
            assert!(result.result.is_success());
            let gas = result.result.tx_gas_used();
            let blob_cost =
                U256::from(alloy_eips::eip4844::DATA_GAS_PER_BLOB) * U256::from(1_000_000_000_u64);
            assert_eq!(
                result.state[&collector].info.balance,
                U256::from(gas * 10)
                    + if collects_blob_fees {
                        blob_cost
                    } else {
                        U256::ZERO
                    }
            );
            assert_eq!(
                initial - result.state[&signer().address()].info.balance,
                U256::from(gas * 12) + blob_cost
            );
        }
    }
}
