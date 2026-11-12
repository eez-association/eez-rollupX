//! Network pricing and final ordered L1 execution for the settlement policy.
//! Scheduling remains in `settlement_policy`; this module cannot make work due.

use std::sync::Arc;

use alloy_consensus::{Header, SignableTransaction, Signed, Transaction, TxEip4844WithSidecar};
use alloy_eips::{
    BlockNumberOrTag, Decodable2718, Encodable2718, eip7594::BlobTransactionSidecarVariant,
};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider as _;
use alloy_sol_types::{SolCall, SolError, SolEvent};
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
    interpreter::{
        CallInputs, CallOutcome, CallScheme, CreateInputs, CreateOutcome, InstructionResult,
    },
};
use tracing::{Level, event};

use super::CrossChainExecCtx;
use super::gas_trace;
use crate::local::provider::ChainProvider;

mod priority;

#[derive(Debug, thiserror::Error)]
pub(super) enum PostingError {
    #[error("posting inputs unavailable: {0}")]
    Unavailable(String),
    #[error("ordered L1 bundle exceeds capacity: {0}")]
    Capacity(String),
    #[error("ordered L1 bundle does not complete the intended effects: {0}")]
    Effects(String),
    #[error("ordered L1 source {hash} does not complete its intended effects: {reason}")]
    SourceEffects { hash: B256, reason: String },
    #[error("invalid ordered L1 transaction: {0}")]
    Transaction(String),
    #[error(transparent)]
    Fee(#[from] PostingFeeError),
}

pub(super) struct PostingRuntime {
    pub fees: PostingFeeConfig,
    pub collection: super::fees::config::RuntimeFees,
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
    /// Shared bounded canonical observations; never refund/queue ownership.
    pub priority_history: tokio::sync::Mutex<priority::PriorityHistory>,
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

impl PostingQuote {
    pub fn probe_funding(
        &self,
        poster: Address,
        balance: U256,
        blobs: u64,
    ) -> Result<crate::PosterBalanceError, PostingFeeError> {
        Ok(crate::PosterBalanceError {
            poster,
            chain_id: self.quote.chain_id,
            available: balance,
            blob_reserve: self.quote.reserve(0, blobs)?.blobs,
            affordable_gas: self
                .quote
                .affordable_gas(balance, self.gas_budget, blobs)?
                .unwrap_or(0),
            gas_budget: self.gas_budget,
            sample: self.quote.sample,
        })
    }
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
            .map_err(|e| PostingError::Unavailable(eez_l1::diagnostics::rpc_error(&e)))?;
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
            .map_err(|e| PostingError::Unavailable(eez_l1::diagnostics::rpc_error(&e)))?
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
        let parent = head.header.inner.clone();
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
        let mut excluded_priority_transactions = 0;
        let filtered_priority = self.collection.l1_refund.is_some();
        let estimated_priority_fee = if self.fees.priority_override.is_none() && filtered_priority {
            let (priority, excluded) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut history = self.priority_history.lock().await;
                    // Receipt/trace RPC state is large. Keep it out of every
                    // enclosing scheduler/quote future while retaining the
                    // guard and cancellation in this request.
                    Box::pin(history.estimate(
                        provider,
                        head,
                        self.ctx.l1_poster_signer.address(),
                        self.registry,
                        percentile,
                    ))
                    .await
                })
                .await
                .map_err(|_| {
                    PostingError::Unavailable("filtered fee history timed out".into())
                })??;
            excluded_priority_transactions = excluded;
            Some(priority)
        } else if self.fees.priority_override.is_none() {
            let history = provider
                .get_fee_history(
                    PRIORITY_FEE_HISTORY_BLOCKS,
                    sample.number.into(),
                    &[f64::from(percentile)],
                )
                .await
                .map_err(|e| {
                    PostingError::Unavailable(format!(
                        "priority fee history: {}",
                        eez_l1::diagnostics::rpc_error(&e)
                    ))
                })?;
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
            .map_err(|e| PostingError::Unavailable(eez_l1::diagnostics::rpc_error(&e)))?
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
            operator_gas_cap = self.operator_gas_cap, priority_source = if self.fees.priority_override.is_some() { "override" } else if filtered_priority { "filtered_receipts" } else { "fee_history" },
            excluded_priority_transactions,
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
            .map_err(|e| PostingError::Unavailable(eez_l1::diagnostics::rpc_error(&e)))?;
        let probe_gas = quote
            .quote
            .affordable_gas(balance, quote.gas_budget, blobs)?
            .ok_or_else(|| {
                PostingError::Transaction(
                    "poster balance cannot reserve blob and execution fees".into(),
                )
            })?;
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
            Err(PostingError::Effects(_) | PostingError::SourceEffects { .. })
                if selected_gas < probe_gas =>
            {
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
    let transaction_hashes: Vec<B256> = transactions
        .iter()
        .map(|tx| {
            use alloy_consensus::transaction::TxHashRef as _;
            *tx.tx_hash()
        })
        .collect();
    let mut final_root = None;
    for (index, transaction) in transactions.into_iter().enumerate() {
        gas_trace::set_tx_index(index);
        // The bundle starts with our poster. Only subsequent signed companions
        // can identify a source-local execution failure; a posting/root failure
        // alone must never authorize a user's retry penalty.
        let effects = |reason| {
            if index == 0 {
                PostingError::Effects(reason)
            } else {
                PostingError::SourceEffects {
                    hash: transaction_hashes[index],
                    reason,
                }
            }
        };
        let result = executor
            .execute_transaction_without_commit(transaction)
            .map_err(|e| PostingError::Transaction(format!("transaction {index}: {e}")))?;
        if !result.result.result.is_success() {
            let selector = result
                .result
                .result
                .output()
                .and_then(|bytes| bytes.get(..4))
                .map(alloy_primitives::hex::encode);
            return Err(effects(format!(
                "transaction {index} failed: {}; revert selector: {selector:?}",
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
                return Err(effects(format!(
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
#[derive(Clone, PartialEq, Eq)]
pub(super) struct EffectInspector {
    registry: Address,
    rollup_id: u64,
    proxy_slot: u8,
    expected: Vec<ObservedCall>,
    // Only inbound roots authorize standalone L2 deliveries. Nested calls keep
    // their existing grouped validation; they cannot authorize retry folding.
    expected_committed: Vec<Option<bool>>,
    frames: Vec<(Option<usize>, usize)>,
    observed: Vec<Option<ObservedCall>>,
    reverted: Vec<bool>,
    retry_reverted: Vec<bool>,
    transaction_ends: Vec<usize>,
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
        let actions: Vec<_> = actions.into_iter().cloned().collect();
        let applied = eez_protocol::settlement::l1_applied_root_mask(&actions)
            .map_err(PostingError::Effects)?;
        let mut expected = Vec::new();
        let mut expected_committed = Vec::new();
        for (index, action) in actions.iter().enumerate().filter(|(_, a)| {
            a.source_rollup_id.0 == source_rollup && a.target_rollup_id.0 == rollup_id
        }) {
            expected_committed.push(
                (source_rollup == 0
                    && action.parent_index.is_none()
                    && action.call_mode == CallMode::Mutable
                    && action.outcome.is_success())
                .then_some(applied[index]),
            );
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
            expected_committed,
            frames: Vec::new(),
            observed: Vec::new(),
            reverted: Vec::new(),
            retry_reverted: Vec::new(),
            transaction_ends: Vec::new(),
        })
    }

    pub(super) fn verify(self) -> Result<(), PostingError> {
        let actual: Vec<_> = self
            .observed
            .iter()
            .enumerate()
            .filter_map(|(index, call)| call.as_ref().map(|call| (index, call)))
            .collect();
        let mismatch = || {
            PostingError::Effects(format!(
                "actual cross-chain calls/outcomes differ (expected {}, observed {})",
                self.expected.len(),
                actual.len()
            ))
        };
        let mut matched = 0;
        for (position, &(index, call)) in actual.iter().enumerate() {
            let Some(expected) = self.expected.get(matched) else {
                return Err(mismatch());
            };
            let committed = self.expected_committed[matched];
            if call == expected
                && committed.is_none_or(|committed| self.reverted[index] != committed)
            {
                matched += 1;
                continue;
            }
            // Discard an attempt only while matching a committing inbound root.
            // Expected lookup/nested calls must remain in their original slots,
            // even when another source in the same bundle needs retry folding.
            if committed == Some(true)
                && call == expected
                && self.retry_reverted[index]
                && actual.get(position + 1).is_some_and(|&(next, retry)| {
                    call == retry
                        && !self
                            .transaction_ends
                            .iter()
                            .any(|&end| end > index && end <= next)
                })
            {
                continue;
            }
            return Err(mismatch());
        }
        if matched != self.expected.len() {
            return Err(mismatch());
        }
        Ok(())
    }

    fn end_frame(&mut self, result: InstructionResult, source_call: bool) -> Option<usize> {
        let (index, start) = self.frames.pop()?;
        let end = self.observed.len();
        if !result.is_ok() {
            self.reverted[start..end].fill(true);
            // Only explicit source-call reverts establish retry eligibility.
            // Halts/CREATE rollbacks preserve earlier explicit revert evidence;
            // a broader explicit scope overrides single-call eligibility.
            if source_call && index.is_none() && result == InstructionResult::Revert {
                self.retry_reverted[start..end].fill(end - start == 1);
            }
        }
        if self.frames.is_empty() {
            self.transaction_ends.push(end);
        }
        index
    }
}

impl<DB: Database, CTX: ContextTr<Db = DB, Journal = Journal<DB>>> Inspector<CTX>
    for EffectInspector
{
    fn create(&mut self, _context: &mut CTX, _inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        self.frames.push((None, self.observed.len()));
        None
    }

    fn create_end(
        &mut self,
        _context: &mut CTX,
        _inputs: &CreateInputs,
        outcome: &mut CreateOutcome,
    ) {
        self.end_frame(outcome.result.result, false);
    }

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
        let start = self.observed.len();
        let index = recognized.then(|| {
            self.observed.push(None);
            self.reverted.push(false);
            self.retry_reverted.push(false);
            self.observed.len() - 1
        });
        self.frames.push((index, start));
        None
    }

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        let Some(index) = self.end_frame(outcome.result.result, true) else {
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
        // This resolver error occurs before a remote callback is executed.
        // Read only the account already checked by the real manager so this
        // observer does not change gas warming. A matching intended read must
        // still execute: omitting this failed lookup cannot satisfy `verify`.
        if mode == CallMode::Static
            && !success
            && let Ok(failure) = eez_protocol::abi::StaticCallProxyNotDeployed::abi_decode(&output)
            && failure.abi_encode().as_slice() == output.as_ref()
            && context
                .journal()
                .state
                .get(&failure.proxy)
                .is_some_and(|account| account.info.is_empty_code_hash())
            && self.expected.iter().any(|expected| {
                expected.mode == CallMode::Static
                    && expected.source == source
                    && expected.target == proxy.original_address
                    && expected.data == data
                    && expected.value == inputs.call_value()
            })
        {
            return;
        }
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

    #[tokio::test]
    async fn posting_quote_checks_rpc_signing_and_embedded_chain_ids() {
        use alloy_provider::{ProviderBuilder, mock::Asserter};
        use eez_l1::{BuilderRelays, L1ReaderConfig, Submitter, SubmitterConfig};
        use reth_provider::test_utils::MockEthProvider;
        use std::time::Duration;

        for (rpc_chain, signing_chain, embedded_chain) in
            [(1_u64, 1_u64, 1_u64), (2, 1, 1), (1, 2, 1), (1, 1, 2)]
        {
            let matching = rpc_chain == signing_chain && rpc_chain == embedded_chain;
            let replies = Asserter::new();
            replies.push_success(&alloy_primitives::U64::from(rpc_chain));
            let (config, parent, _) = configuration(2_000_000);
            if matching {
                let mut block: alloy_rpc_types_eth::Block = Default::default();
                block.header.inner = parent.clone();
                block.header.hash = parent.hash_slow();
                replies.push_success(&block);
                replies.push_success(&block);
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies.clone());
            let mut spec = (**config.chain_spec()).clone();
            spec.chain = embedded_chain.into();
            spec.genesis.config.chain_id = embedded_chain;
            let local: Arc<MockEthProvider> = Arc::new(MockEthProvider::new());
            // Construction needs a submission handle, but quoting must use
            // only the mocked canonical provider and never contact this URL.
            let unused_url = "http://127.0.0.1:0".parse().unwrap();
            let submitter = Submitter::new(
                SubmitterConfig {
                    reader: L1ReaderConfig {
                        rpc_url: unused_url,
                        beacon_url: None,
                        eez: REGISTRY,
                        rollup_id: 1,
                        deploy_block: 0,
                    },
                    builder_relays: BuilderRelays::new(vec!["http://127.0.0.1:0".parse().unwrap()])
                        .unwrap(),
                    target_rpc_url: None,
                    poster: signer(),
                    bundle_signer: None,
                    bundle_capture_dir: None,
                    bundle_submission_lead: Duration::from_millis(7500),
                },
                Duration::from_secs(12),
                Duration::from_millis(500),
            )
            .unwrap();
            let runtime = PostingRuntime {
                fees: PostingFeeConfig::parse(|name| {
                    Ok(match name {
                        "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS"
                        | "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS" => Some("1000000000".into()),
                        "EEZ_L1_POSTBATCH_PRIORITY_FEE" => Some("2".into()),
                        _ => None,
                    })
                })
                .unwrap(),
                collection: super::super::fees::config::RuntimeFees {
                    l2_to_l1_enabled: false,
                    pricing: None,
                    l1_refund: None,
                    quote_base_fee_percent: 150,
                },
                chain: ChainProvider {
                    provider: local.clone(),
                    headers: local,
                    evm_config: eez_evm::EezEvmConfig::for_l1_simulation(Arc::new(spec)),
                    gnosis_evm_config: None,
                },
                ctx: Arc::new(CrossChainExecCtx {
                    eezl2_address: eez_protocol::EEZL2_ADDRESS,
                    l2_chain_id: 90210,
                    l1_provider: provider.root().clone(),
                    submitter,
                    l1_poster_signer: signer(),
                    l1_chain_id: signing_chain,
                    blob_sidecar_encoding: eez_blob_codec::BlobSidecarEncoding::Eip4844,
                    ecdsa_proof_system_address: Address::repeat_byte(3),
                }),
                registry: REGISTRY,
                post_batch_target: REGISTRY,
                operator_gas_cap: 2_000_000,
                operator_blob_cap: None,
                costs: Default::default(),
                gas_trace: None,
                priority_history: Default::default(),
            };
            let result = runtime
                .quote(
                    BundleTarget::Exact {
                        block: 2,
                        timestamp: 2,
                    },
                    true,
                    PriorityProfile::Base,
                )
                .await;
            if matching {
                let quote = result.unwrap();
                assert_eq!(quote.quote.chain_id, 1);
                assert_eq!(quote.quote.sample.hash, parent.hash_slow());
            } else {
                assert!(
                    matches!(result, Err(PostingError::Unavailable(ref message))
                    if message == "submission RPC and selected L1 profile have different chain IDs"),
                    "rpc={rpc_chain}, signing={signing_chain}, embedded={embedded_chain}: {result:?}"
                );
            }
            assert!(replies.read_q().is_empty());
        }
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
    fn native_companion_failure_identifies_exact_signed_source() {
        let companion = tx(1, REGISTRY, 21_000, Bytes::new());
        use alloy_consensus::transaction::TxHashRef as _;
        let hash = *companion.tx_hash();
        assert!(matches!(execute(emit_root(false), vec![
            tx(0, REGISTRY, 90_000, Bytes::new()), companion,
        ], 120_000), Err(PostingError::SourceEffects { hash: failed, .. }) if failed == hash));
        assert!(matches!(
            execute(
                emit_root(false),
                vec![tx(0, REGISTRY, 21_000, Bytes::new()),],
                120_000
            ),
            Err(PostingError::Effects(_))
        ));
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
            operations_version: Default::default(),
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
                revert_spans: vec![],
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
    fn shared_inbound_row_requires_an_identical_reverted_observation_in_the_same_transaction() {
        let mut inspector = EffectInspector::new(REGISTRY, 1, &static_semantics()).unwrap();
        inspector.expected[0].mode = CallMode::Mutable;
        inspector.expected_committed[0] = Some(true);
        let call = inspector.expected[0].clone();
        inspector.observed = vec![Some(call.clone()), Some(call.clone())];
        inspector.reverted = vec![true, false];
        inspector.retry_reverted = vec![true, false];
        inspector.transaction_ends = vec![2];
        assert!(inspector.clone().verify().is_ok());

        let mut historical = inspector.clone();
        historical.expected.push(call);
        historical.expected_committed = vec![Some(false), Some(true)];
        assert!(
            historical.verify().is_ok(),
            "exact historical traces still match"
        );

        for variant in 0..6 {
            let mut invalid = inspector.clone();
            match variant {
                0 => invalid.retry_reverted[0] = false,
                1 => invalid.reverted[1] = true,
                2 => invalid.transaction_ends = vec![1, 2],
                3 => {
                    invalid.observed[0].as_mut().unwrap().output = Bytes::from_static(b"different")
                }
                4 => invalid.observed[0].as_mut().unwrap().success = false,
                5 => invalid.expected_committed[0] = None,
                _ => unreachable!(),
            }
            assert!(invalid.verify().is_err(), "variant {variant}");
        }
    }

    #[test]
    fn retry_matching_preserves_nested_calls_and_other_sources() {
        let mut inbound = static_semantics().remove(0);
        inbound.actions[0].call_mode = CallMode::Mutable;
        let mut outbound = inbound.clone();
        outbound.origin_chain = eez_protocol::RollupId(1);
        let mut root = inbound.actions[0].clone();
        root.source_rollup_id = eez_protocol::RollupId(1);
        root.target_rollup_id = eez_protocol::RollupId(0);
        let mut nested = inbound.actions[0].clone();
        nested.parent_index = Some(0);
        nested.revert_spans = vec![1];
        let mut retry = nested.clone();
        retry.revert_spans.clear();
        outbound.actions = vec![root, nested, retry];
        let mut inspector =
            EffectInspector::new(REGISTRY, 1, &[outbound, inbound.clone()]).unwrap();
        assert_eq!(inspector.expected_committed, [None, None, Some(true)]);
        let call = inspector.expected[0].clone();
        inspector.observed = vec![Some(call); 4];
        inspector.reverted = vec![true, false, true, false];
        inspector.retry_reverted = inspector.reverted.clone();
        inspector.transaction_ends = vec![2, 4];
        assert!(inspector.verify().is_ok());
        // Two separate inbound companions may each reuse their own row.
        let mut inspector = EffectInspector::new(REGISTRY, 1, &[inbound.clone(), inbound]).unwrap();
        let call = inspector.expected[0].clone();
        inspector.observed = vec![Some(call); 5];
        inspector.reverted = vec![true, true, false, true, false];
        inspector.retry_reverted = inspector.reverted.clone();
        inspector.transaction_ends = vec![3, 5];
        assert!(inspector.verify().is_ok());
    }

    #[test]
    fn inbound_retry_validation_tracks_real_frames_and_commitment() {
        const OUTER: Address = Address::repeat_byte(0x44);
        const INNER: Address = Address::repeat_byte(0x55);
        const NESTED: Address = Address::repeat_byte(0x66);
        fn forward(code: &mut Vec<u8>, target: Address) {
            // CALL(target, calldata), discard success and output.
            code.extend([0x5f, 0x5f, 0x36, 0x5f, 0x5f, 0x73]);
            code.extend(target.0);
            code.extend([0x5a, 0xf1, 0x50]);
        }
        fn install(db: &mut InMemoryDB, address: Address, code: Vec<u8>) {
            db.insert_account_info(
                address,
                AccountInfo {
                    code: Some(Bytecode::new_raw(code.into())),
                    ..Default::default()
                },
            );
        }
        let run = |lookup: bool,
                   calls: usize,
                   halt: bool,
                   nested: bool,
                   retry: bool,
                   separate_transaction: bool| {
            let (config, parent, attributes) = configuration(2_000_000);
            let env = config.next_evm_env(&parent, &attributes).unwrap();
            let mut state = static_call_database(42);
            let mut proxy = vec![0x36, 0x5f, 0x5f, 0x37];
            forward(&mut proxy, REGISTRY);
            // Forward the ABI-wrapped manager result to the source frame.
            proxy.extend([0x3d, 0x5f, 0x5f, 0x3e, 0x3d, 0x5f, 0xf3]);
            install(&mut state, PROXY, proxy);
            let mut inner = vec![0x36, 0x5f, 0x5f, 0x37];
            for index in 0..calls {
                forward(
                    &mut inner,
                    if nested && index == 0 { NESTED } else { PROXY },
                );
            }
            inner.extend(if halt {
                vec![0xfe]
            } else {
                vec![0x5f, 0x5f, 0xfd]
            });
            install(&mut state, INNER, inner);
            let mut child = vec![0x36, 0x5f, 0x5f, 0x37];
            forward(&mut child, PROXY);
            child.extend([0x5f, 0x5f, 0xfd]);
            install(&mut state, NESTED, child);
            let mut outer = vec![0x36, 0x5f, 0x5f, 0x37];
            forward(&mut outer, INNER);
            if retry && !separate_transaction {
                forward(&mut outer, PROXY);
            }
            outer.push(0x00);
            install(&mut state, OUTER, outer);
            let mut semantics = static_semantics();
            let action = &mut semantics[0].actions[0];
            action.call_mode = CallMode::Mutable;
            if lookup {
                action.revert_spans = vec![1];
            }
            let mut inspector = EffectInspector::new(REGISTRY, 1, &semantics).unwrap();
            let data: Bytes = executeCrossChainCallCall {
                sourceAddress: signer().address(),
                callData: Bytes::from_static(b"lookup"),
            }
            .abi_encode()
            .into();
            for target in
                std::iter::once(OUTER).chain((retry && separate_transaction).then_some(PROXY))
            {
                let tx_env = config.tx_env(tx(0, target, 1_000_000, data.clone()));
                let actual = config
                    .evm_with_env_and_inspector(&mut state, env.clone(), &mut inspector)
                    .transact(tx_env)
                    .unwrap();
                assert!(actual.result.is_success());
            }
            inspector
        };
        let valid = run(false, 1, false, false, true, false);
        assert_eq!(valid.reverted, [true, false]);
        assert_eq!(valid.retry_reverted, [true, false]);
        assert!(valid.verify().is_ok());
        assert!(run(true, 1, false, false, false, false).verify().is_ok());
        assert!(run(true, 1, true, false, false, false).verify().is_ok());
        // A committed retry cannot consume lookup-only authorization.
        assert!(run(true, 1, false, false, true, false).verify().is_err());
        assert!(run(false, 1, false, false, false, false).verify().is_err());
        assert!(run(false, 1, true, false, true, false).verify().is_err());
        // An inner explicit revert remains valid evidence under an outer halt.
        assert!(run(false, 1, true, true, true, false).verify().is_ok());
        for nested in [false, true] {
            let broad = run(false, 2, false, nested, true, false);
            assert_eq!(broad.reverted, [true, true, false]);
            assert_eq!(broad.retry_reverted, [false, false, false]);
            assert!(broad.verify().is_err());
        }
        let separate = run(false, 1, false, false, true, true);
        assert_eq!(separate.transaction_ends, [1, 2]);
        assert!(separate.verify().is_err());
        // The exact-match path must enforce the same commitment distinction.
        assert!(run(true, 0, false, false, true, false).verify().is_err());
        assert!(run(false, 0, false, false, true, false).verify().is_ok());
    }

    #[test]
    fn constructor_rollback_is_lookup_evidence_but_not_retry_evidence() {
        let (config, parent, attributes) = configuration(2_000_000);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let data = executeCrossChainCallCall {
            sourceAddress: signer().address(),
            callData: Bytes::from_static(b"lookup"),
        }
        .abi_encode();
        // Copy an appended payload to memory before executing the supplied body.
        fn with_payload(mut body: Vec<u8>, payload: &[u8]) -> Vec<u8> {
            let length = u16::try_from(payload.len()).unwrap().to_be_bytes();
            let offset = u16::try_from(8 + body.len()).unwrap().to_be_bytes();
            let mut code = vec![
                0x61, length[0], length[1], 0x61, offset[0], offset[1], 0x5f, 0x39,
            ];
            code.append(&mut body);
            code.extend(payload);
            code
        }
        let mut state = static_call_database(42);
        // The constructor calls the proxy, then explicitly reverts.
        let mut body = vec![
            0x5f,
            0x5f,
            0x60,
            u8::try_from(data.len()).unwrap(),
            0x5f,
            0x5f,
            0x73,
        ];
        body.extend(PROXY.0);
        body.extend([0x5a, 0xf1, 0x50, 0x5f, 0x5f, 0xfd]);
        let init = with_payload(body, &data);
        let size = u16::try_from(init.len()).unwrap().to_be_bytes();
        let outer = with_payload(
            vec![0x61, size[0], size[1], 0x5f, 0x5f, 0xf0, 0x50, 0x00],
            &init,
        );
        let factory = Address::repeat_byte(0x44);
        state.insert_account_info(
            factory,
            AccountInfo {
                code: Some(Bytecode::new_raw(outer.into())),
                ..Default::default()
            },
        );
        let mut semantics = static_semantics();
        semantics[0].actions[0].call_mode = CallMode::Mutable;
        semantics[0].actions[0].revert_spans = vec![1];
        let mut inspector = EffectInspector::new(REGISTRY, 1, &semantics).unwrap();
        let result = config
            .evm_with_env_and_inspector(&mut state, env, &mut inspector)
            .transact(config.tx_env(tx(0, factory, 1_000_000, Bytes::new())))
            .unwrap();
        assert!(result.result.is_success());
        assert_eq!(inspector.reverted, [true]);
        assert_eq!(inspector.retry_reverted, [false]);
        assert_eq!(inspector.transaction_ends, [1]);
        assert!(inspector.verify().is_ok());
    }

    #[test]
    fn missing_static_proxy_lookup_is_not_a_remote_call_but_retry_is_required() {
        let missing = Address::repeat_byte(0x77);
        let exact = eez_protocol::abi::StaticCallProxyNotDeployed { proxy: missing }.abi_encode();
        let (config, parent, attributes) = configuration(1_000_000);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let data = eez_protocol::abi::staticCrossChainCallCall {
            sourceAddress: signer().address(),
            callData: Bytes::from_static(b"lookup"),
        }
        .abi_encode();
        let tx_env = config.tx_env(tx(0, PROXY, 200_000, data.into()));
        for (proxy_has_code, trailing, valid) in [
            (false, false, true),
            (true, false, false),
            (false, true, false),
        ] {
            let mut state = static_call_database(42);
            let key =
                eez_protocol::proxy_mapping_key(PROXY, eez_protocol::EEZ_AUTHORIZED_PROXIES_SLOT);
            let mut code = Vec::new();
            push32(&mut code, key);
            code.extend([0x54, 0x50, 0x73]);
            code.extend(missing.0);
            code.extend([0x3b, 0x50]); // the resolver checks callback code
            let mut output = exact.clone();
            if trailing {
                output.push(0);
            }
            for (index, chunk) in output.chunks(32).enumerate() {
                let mut word = [0; 32];
                word[..chunk.len()].copy_from_slice(chunk);
                push32(&mut code, B256::from(word));
                code.extend([0x60, (index * 32) as u8, 0x52]);
            }
            code.extend([0x60, output.len() as u8, 0x5f, 0xfd]);
            state.insert_account_info(
                REGISTRY,
                AccountInfo {
                    code: Some(Bytecode::new_raw(code.into())),
                    ..Default::default()
                },
            );
            if proxy_has_code {
                state.insert_account_info(
                    missing,
                    AccountInfo {
                        code: Some(Bytecode::new_raw(vec![0].into())),
                        ..Default::default()
                    },
                );
            }
            let mut inspector = EffectInspector::new(REGISTRY, 1, &static_semantics()).unwrap();
            config
                .evm_with_env_and_inspector(&mut state, env.clone(), &mut inspector)
                .transact(tx_env.clone())
                .unwrap();
            assert!(
                inspector.clone().verify().is_err(),
                "a failed lookup alone cannot satisfy the expected read"
            );
            let mut retry = static_call_database(42);
            config
                .evm_with_env_and_inspector(&mut retry, env.clone(), &mut inspector)
                .transact(tx_env.clone())
                .unwrap();
            assert_eq!(
                inspector.verify().is_ok(),
                valid,
                "only exact missing-proxy lookup failures may precede a required matching retry"
            );
        }
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
