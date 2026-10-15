//! Unsigned, advisory L2 pricing. All execution uses disposable state. A quote
//! is neither an authenticated transaction nor admission or a funding promise.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alloy_consensus::{Header, Transaction, TxEip1559, TxLegacy, Typed2718, TypedTransaction};
use alloy_eips::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxKind, U256, keccak256};
use eez_l1::{BundleTarget, L2Endpoint};
use eez_payload_codec::v0::{BlockRun, PayloadSizer};
use eez_protocol::{
    ChainClient, CompositionBuilder, ExecutorErrorKind, ExecutorResult, RollupId,
    TargetExecutionSession,
    executor::{ExecutionRequest, SessionSnapshot},
    system_tx::{OutboundSourceEntry, OutboundSyncEffect, OutboundSyncGroup, SystemTxContext},
};
use reth_primitives_traits::{Block, BlockBody, SealedHeader, SignerRecoverable};
use reth_storage_api::{BlockReader, BlockSource, StateProviderFactory};
use revm::context::TxEnv;
use serde_json::{Value, json};

use super::super::{
    Composer, PostBatchGasBudget, l1_target_gas, posting::PostingQuote, projected_tx_l1_gas,
};
use crate::local::{L1SlotState, L1TargetSession, LocalChainClient, build::SyncBlockState};

const MAX_SOURCE_GAS: u64 = 5_000_000;
const MAX_CALLDATA: usize = 12 * 1024;
const MAX_HISTORY: u64 = 256;
const MAX_ITERATIONS: usize = 4;
const MAX_FRAMES: usize = 32;
const MAX_DEPTH: usize = 8;
const MAX_DA_BYTES: usize = 2 * 1024 * 1024;

/// Stable error categories for the application quote APIs. Details are fixed
/// strings: backend/provider errors stay in local diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum FeeQuoteError {
    #[error("invalid intent: {0}")]
    Invalid(&'static str),
    #[error("unsupported quote: {0}")]
    Unsupported(&'static str),
    #[error("quote unavailable: {0}")]
    Unavailable(&'static str),
    #[error("quote work limit: {0}")]
    Limit(&'static str),
    #[error("quote context expired or changed")]
    Stale,
    #[error("L2 fee quote policy is not configured")]
    Disabled,
    #[error("signed L2 priority payment does not cover the configured settlement tariff")]
    Underfunded,
}

impl FeeQuoteError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_intent",
            Self::Unsupported(_) => "unsupported_intent",
            Self::Unavailable(_) => "quote_unavailable",
            Self::Limit(_) => "quote_limit",
            Self::Stale => "stale_context",
            Self::Disabled => "quotes_disabled",
            Self::Underfunded => "l2_tip_underfunded",
        }
    }
}

fn unavailable(context: &'static str, error: impl std::fmt::Display) -> FeeQuoteError {
    tracing::debug!(%error, context, "advisory fee quote unavailable");
    FeeQuoteError::Unavailable(context)
}

/// Explicit legacy or type-2 call intent. No fee fields are accepted: the service finds
/// and replays them. The gas limit is fixed throughout this search.
#[derive(Clone, Debug)]
pub struct L2FeeRequest {
    sender: Address,
    tx: TypedTransaction,
    signed_fees: bool,
}

impl L2FeeRequest {
    /// Recover a signed source and retain its exact fees for one admission replay.
    /// The unsigned quote profile also bounds signed admission; no placeholder
    /// signature is accepted as authorization or forwarded to the queue.
    pub fn from_signed(envelope: &alloy_consensus::TxEnvelope) -> Result<Self, FeeQuoteError> {
        match envelope {
            alloy_consensus::TxEnvelope::Legacy(_) if envelope.chain_id().is_none() => {
                return Err(FeeQuoteError::Unsupported(
                    "legacy source requires EIP-155 replay protection",
                ));
            }
            alloy_consensus::TxEnvelope::Legacy(_) | alloy_consensus::TxEnvelope::Eip1559(_) => {}
            _ => {
                return Err(FeeQuoteError::Unsupported(
                    "tariff admission requires legacy or type 2",
                ));
            }
        }
        if envelope.access_list().is_some_and(|list| !list.is_empty()) {
            return Err(FeeQuoteError::Unsupported(
                "access lists are outside the quote profile",
            ));
        }
        let TxKind::Call(to) = envelope.kind() else {
            return Err(FeeQuoteError::Unsupported("contract creation"));
        };
        let sender = envelope
            .recover_signer()
            .map_err(|_| FeeQuoteError::Invalid("signature recovery"))?;
        let mut request = Self::parse(&json!({
            "type":format!("{:#x}",envelope.ty()), "chainId":format!("{:#x}",envelope.chain_id().expect("admitted envelope is replay protected")),
            "from":sender,"to":to,"nonce":format!("{:#x}",envelope.nonce()),
            "gas":format!("{:#x}",envelope.gas_limit()),"value":format!("{:#x}",envelope.value()),"data":envelope.input()
        }))?;
        request.tx = envelope.clone().into();
        request.signed_fees = true;
        Ok(request)
    }

    /// Parse the strict intent schema. All numbers are canonical JSON-RPC
    /// hex quantities, avoiding JavaScript precision loss and ambiguous defaults.
    pub fn parse(value: &Value) -> Result<Self, FeeQuoteError> {
        const FIELDS: &[&str] = &[
            "type", "chainId", "from", "to", "nonce", "gas", "value", "data",
        ];
        let fields = value
            .as_object()
            .ok_or(FeeQuoteError::Invalid("expected object"))?;
        if fields.len() != FIELDS.len() || fields.keys().any(|k| !FIELDS.contains(&k.as_str())) {
            return Err(FeeQuoteError::Invalid(
                "all eight intent fields are required; unknown fields are rejected",
            ));
        }
        let quantity = |key: &str| -> Result<U256, FeeQuoteError> {
            let raw = value[key]
                .as_str()
                .and_then(|s| s.strip_prefix("0x"))
                .ok_or(FeeQuoteError::Invalid("expected hex quantity"))?;
            if raw.is_empty()
                || raw.len() > 64
                || (raw.len() > 1 && raw.starts_with('0'))
                || !raw.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(FeeQuoteError::Invalid("noncanonical hex quantity"));
            }
            U256::from_str_radix(raw, 16).map_err(|_| FeeQuoteError::Invalid("quantity overflow"))
        };
        let small = |key| {
            u64::try_from(quantity(key)?)
                .map_err(|_| FeeQuoteError::Invalid("quantity exceeds u64"))
        };
        let tx_type = small("type")?;
        if !matches!(tx_type, 0 | 2) {
            return Err(FeeQuoteError::Unsupported(
                "only legacy or type 2 contract calls",
            ));
        }
        let address = |key: &str| {
            value[key]
                .as_str()
                .ok_or(FeeQuoteError::Invalid("expected address"))?
                .parse::<Address>()
                .map_err(|_| FeeQuoteError::Invalid("invalid address"))
        };
        let sender = address("from")?;
        if sender.is_zero() || sender == eez_primitives::SYSTEM_ADDRESS {
            return Err(FeeQuoteError::Unsupported("reserved sender"));
        }
        let data = value["data"]
            .as_str()
            .filter(|s| s.starts_with("0x") && s.len() <= 2 + 2 * MAX_CALLDATA)
            .ok_or(FeeQuoteError::Invalid(
                "calldata must be hex and at most 12 KiB",
            ))?
            .parse::<Bytes>()
            .map_err(|_| FeeQuoteError::Invalid("invalid calldata"))?;
        let gas_limit = small("gas")?;
        if !(21_000..=MAX_SOURCE_GAS).contains(&gas_limit) {
            return Err(FeeQuoteError::Invalid("gas must be 21000..5000000"));
        }
        let chain_id = small("chainId")?;
        if chain_id == 0 {
            return Err(FeeQuoteError::Invalid("chainId must be positive"));
        }
        Ok(Self {
            sender,
            signed_fees: false,
            tx: if tx_type == 0 {
                TxLegacy {
                    chain_id: Some(chain_id),
                    nonce: small("nonce")?,
                    gas_limit,
                    to: TxKind::Call(address("to")?),
                    value: quantity("value")?,
                    input: data,
                    ..Default::default()
                }
                .into()
            } else {
                TxEip1559 {
                    chain_id,
                    nonce: small("nonce")?,
                    gas_limit,
                    to: TxKind::Call(address("to")?),
                    value: quantity("value")?,
                    input: data,
                    ..Default::default()
                }
                .into()
            },
        })
    }

    fn fields(&self, tip: u128, cap: u128) -> Value {
        let mut fields = json!({"type":format!("{:#x}",self.tx.ty()), "chainId":format!("{:#x}",self.tx.chain_id().expect("parser requires chain ID")), "from":self.sender,
            "to":self.tx.to().expect("parser admits calls only"), "nonce":format!("{:#x}",self.tx.nonce()),
            "gas":format!("{:#x}",self.tx.gas_limit()), "value":format!("{:#x}",self.tx.value()), "data":self.tx.input()});
        if self.tx.ty() == 0 {
            fields["gasPrice"] = json!(format!("{cap:#x}"));
        } else {
            fields["maxPriorityFeePerGas"] = json!(format!("{tip:#x}"));
            fields["maxFeePerGas"] = json!(format!("{cap:#x}"));
        }
        fields
    }

    fn env(&self, tip: u128, cap: u128) -> TxEnv {
        TxEnv {
            tx_type: self.tx.ty(),
            caller: self.sender,
            gas_limit: self.tx.gas_limit(),
            gas_price: cap,
            gas_priority_fee: self.tx.max_priority_fee_per_gas().map(|_| tip),
            kind: self.tx.kind(),
            value: self.tx.value(),
            data: self.tx.input().clone(),
            nonce: self.tx.nonce(),
            chain_id: self.tx.chain_id(),
            ..Default::default()
        }
    }

    /// Signature bytes are solely an upper bound for DA encoding length. This
    /// envelope is never executed, recovered, proved, queued or submitted.
    fn sizing_bytes(&self, tip: u128, cap: u128) -> Result<Bytes, FeeQuoteError> {
        let mut tx = self.tx.clone();
        match &mut tx {
            TypedTransaction::Legacy(tx) => tx.gas_price = cap,
            TypedTransaction::Eip1559(tx) => {
                tx.max_priority_fee_per_gas = tip;
                tx.max_fee_per_gas = cap;
            }
            _ => return Err(FeeQuoteError::Unsupported("unsupported sizing envelope")),
        }
        Ok(tx
            .into_envelope(Signature::new(U256::MAX, U256::MAX, true))
            .encoded_2718()
            .into())
    }
}

/// Prepared network inputs; opaque to the HTTP front. Expensive synchronous
/// work must run through its bounded blocking worker, never on an async thread.
#[derive(Debug)]
pub struct PreparedL2FeeQuote {
    request: L2FeeRequest,
    rollup_id: u64,
    posting: PostingQuote,
    escrow: U256,
}

impl<L2> Composer<L2>
where
    L2: BlockReader<Block = eez_primitives::Block, Header = Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
{
    /// Whether new outbound admission and final dispatch enforce the tariff.
    /// This does not attest to treasury realization of the configured conversion.
    pub fn enforces_l2_fee_tariff(&self) -> bool {
        self.inner.posting.enforce_l2_fee_tariff
    }

    /// Pin the target and fee sample without signing or invoking a prover.
    pub async fn prepare_l2_fee_quote(
        &self,
        rollup_id: u64,
        request: L2FeeRequest,
    ) -> Result<PreparedL2FeeQuote, FeeQuoteError> {
        let posting = &self.inner.posting;
        let policy = posting
            .shadow_fee_policy
            .as_ref()
            .ok_or(FeeQuoteError::Disabled)?;
        if request.tx.chain_id() != Some(posting.ctx.l2_chain_id)
            || !self.inner.rollups.contains_key(&rollup_id)
        {
            return Err(FeeQuoteError::Invalid("wrong chain or rollup"));
        }
        let target = self
            .inner
            .submitter
            .next_policy_target()
            .await
            .map_err(|e| unavailable("L1 target", e))?;
        let BundleTarget::Exact { timestamp, .. } = target else {
            return Err(FeeQuoteError::Unavailable("unresolved target"));
        };
        if !policy.quote_eligible(
            request.sender,
            posting.ctx.l1_poster_signer.address(),
            timestamp,
        ) {
            return Err(FeeQuoteError::Unsupported(
                "expired policy or excluded sender",
            ));
        }
        let quote = posting
            .quote(
                target,
                true,
                eez_l1::posting_fees::PriorityProfile::CrossChain,
            )
            .await
            .map_err(|e| unavailable("L1 pricing", e))?;
        let escrow = super::super::IEEZReader::new(posting.registry, &posting.ctx.l1_provider)
            .rollups(rollup_id)
            .block(quote.quote.sample.hash.into())
            .call()
            .await
            .map_err(|e| unavailable("L1 escrow", e))?
            .etherBalance;
        Ok(PreparedL2FeeQuote {
            request,
            rollup_id,
            posting: quote,
            escrow,
        })
    }

    /// Simulate a single unsigned intent and price its whole settlement. The
    /// caller owns cancellation and must retain its work permit until return.
    /// Only disposable caches are mutated; no queue or chain writes occur.
    pub fn quote_l2_fee(
        &self,
        prepared: PreparedL2FeeQuote,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Value, FeeQuoteError> {
        let PreparedL2FeeQuote {
            request,
            rollup_id,
            posting,
            escrow,
        } = prepared;
        let budget = Arc::new(Budget::new(cancelled));
        let runtime = &self.inner.posting;
        let policy = runtime
            .shadow_fee_policy
            .as_ref()
            .ok_or(FeeQuoteError::Disabled)?;
        let rollup = self
            .inner
            .rollups
            .get(&rollup_id)
            .ok_or(FeeQuoteError::Invalid("unknown rollup"))?;
        let cc = &self.inner.cross_chain;
        let source = cc.local.l2_entry.chain_provider();
        let parent_number = source
            .headers
            .best_block_number()
            .map_err(|e| unavailable("L2 head", e))?;
        let parent = SealedHeader::seal_slow(
            source
                .headers
                .header_by_number(parent_number)
                .map_err(|e| unavailable("L2 header", e))?
                .ok_or(FeeQuoteError::Unavailable("L2 header"))?,
        );
        let anchor = rollup.l1_head.policy_anchor_snapshot();
        if anchor.through != Some(posting.quote.sample) {
            return Err(FeeQuoteError::Unavailable(
                "settlement scan has not reached the selected L1 parent",
            ));
        }
        let endpoint = anchor
            .endpoint
            .filter(|_| anchor.through.is_some())
            .ok_or(FeeQuoteError::Unavailable("validated settlement anchor"))?;
        let terminal_number = parent
            .number
            .checked_add(1)
            .ok_or(FeeQuoteError::Limit("L2 height overflow"))?;
        self.inner
            .emission
            .checked_window_span(endpoint.number, terminal_number)
            .map_err(|e| unavailable("settlement history capacity", e))?;
        let operations = history_size(
            rollup.l2_provider.as_ref(),
            &parent,
            endpoint,
            self.inner.fee_recipient,
            &budget,
        )?;
        let (timestamp, randao) = rollup
            .l1_head
            .next_block_context(&parent)
            .map_err(|e| unavailable("L2 context", e))?;
        let state = source
            .provider
            .state_by_block_hash(parent.hash())
            .map_err(|e| unavailable("L2 state", e))?;
        let system_nonce = state
            .account_nonce(&eez_primitives::SYSTEM_ADDRESS)
            .map_err(|e| unavailable("system nonce", e))?
            .unwrap_or_default();
        let maximum_blobs = self
            .remaining_posting_blob_allowance(posting.quote.target, 0)
            .map_err(|e| unavailable("blob allowance", e))?;
        let BundleTarget::Exact {
            block: target_block,
            timestamp: target_timestamp,
        } = posting.quote.target
        else {
            return Err(FeeQuoteError::Unavailable("unresolved target"));
        };
        let issued = unix_now()?;
        let expiry = target_timestamp.min(issued.saturating_add(10));
        if issued >= expiry {
            return Err(FeeQuoteError::Stale);
        }
        let mut tip = if request.signed_fees {
            request
                .tx
                .max_priority_fee_per_gas()
                .unwrap_or_else(|| request.tx.max_fee_per_gas())
        } else {
            0
        };
        for iteration in 1..=MAX_ITERATIONS {
            budget.check()?;
            let mut draft = SyncBlockState::open(
                Arc::clone(&source.provider),
                &self.inner.evm_config,
                &parent,
                timestamp,
                randao,
                self.inner.fee_recipient,
                &[],
            )
            .map_err(|e| unavailable("L2 draft", e))?;
            let base = draft.base_fee();
            let cap = if request.signed_fees {
                request.tx.max_fee_per_gas()
            } else {
                u128::from(base)
                    .checked_add(tip)
                    .ok_or(FeeQuoteError::Limit("fee overflow"))?
            };
            let effective_tip = tip.min(
                cap.checked_sub(u128::from(base))
                    .ok_or(FeeQuoteError::Underfunded)?,
            );
            let tx_env = request.env(tip, cap);
            let mut fork = draft
                .fork()
                .map_err(|e| unavailable("L2 simulation state", e))?;
            let (sim_state, sim_env) = fork.state_and_env();
            let sim_env = sim_env.clone();
            let l2 = cc.local.l2_entry.isolated(parent.hash(), sim_env.clone());
            let l1 = cc
                .local
                .l1_entry
                .isolated(posting.quote.sample.hash, posting.env.clone());
            let slot = L1SlotState {
                anchor: SealedHeader::new(posting.parent.clone(), posting.quote.sample.hash),
                cache: Default::default(),
                posting: Some((
                    posting.env.clone(),
                    runtime.ctx.l1_poster_signer.address(),
                    posting.quote.estimated_effective_gas_price,
                )),
            };
            let session = L1TargetSession::new(&slot, Arc::clone(&l1))
                .map_err(|e| unavailable("L1 session", e))?;
            let mut rollups = HashMap::new();
            for (id, client) in [
                (RollupId::MAINNET, l1),
                (RollupId(rollup_id), Arc::clone(&l2)),
            ] {
                let (_, config) = cc
                    .rollups
                    .get(&id)
                    .ok_or(FeeQuoteError::Unavailable("rollup wiring"))?;
                rollups.insert(
                    id,
                    eez_protocol::composition::Rollup {
                        client: Arc::new(BoundedClient {
                            client,
                            budget: Arc::clone(&budget),
                        }),
                        session: None,
                        config: config.clone(),
                    },
                );
            }
            let mut builder = CompositionBuilder::new(RollupId(rollup_id), rollups).with_sessions(
                HashMap::from([(
                    RollupId::MAINNET,
                    Box::new(BoundedSession {
                        inner: Box::new(session),
                        budget: Arc::clone(&budget),
                    }) as Box<dyn TargetExecutionSession + Send>,
                )]),
            );
            let simulated =
                l2.simulate_source_env_on(tx_env.clone(), &mut builder, sim_state, sim_env, false);
            budget.check()?;
            if !simulated
                .map_err(|e| unavailable("source simulation", e))?
                .success
            {
                return Err(FeeQuoteError::Unsupported(
                    "source validation or execution failed",
                ));
            }
            let composition = builder
                .finalize()
                .map_err(|e| unavailable("unsupported composition", e))?;
            let raw_size = request.sizing_bytes(tip, cap)?;
            let (group, l1_entries) = outbound_group(&composition, rollup_id, raw_size.clone())?;
            let withdrawal = l1_entries
                .iter()
                .try_fold(U256::ZERO, |total, entry| {
                    eez_protocol::entries::outbound_ether_out(entry)
                        .and_then(|value| total.checked_add(value))
                })
                .ok_or(FeeQuoteError::Unsupported("withdrawal shape"))?;
            if withdrawal > escrow {
                return Err(FeeQuoteError::Unavailable("insufficient L1 escrow"));
            }
            let pairs = eez_protocol::system_tx::build_cross_chain_sync_groups(
                &[group],
                &[],
                &SystemTxContext {
                    eezl2_address: runtime.ctx.eezl2_address,
                    l2_chain_id: runtime.ctx.l2_chain_id,
                    this_rollup_id: rollup_id,
                },
                system_nonce,
            )
            .map_err(|e| unavailable("native group", e))?;
            let mut source_gas = None;
            for raw in eez_protocol::system_tx::interleave_sync_block_txs(&pairs) {
                budget.check()?;
                let is_source = raw == raw_size;
                let outcome = if is_source {
                    if source_gas.is_some() {
                        return Err(FeeQuoteError::Unsupported("duplicate source"));
                    }
                    draft.preview_unsigned(tx_env.clone())
                } else if raw.first() == Some(&eez_primitives::SYSTEM_TX_TYPE) {
                    draft.execute_tx(&raw)
                } else {
                    return Err(FeeQuoteError::Unsupported("unexpected group member"));
                }
                .map_err(|e| unavailable("native L2 replay", e))?;
                if !outcome.success {
                    return Err(FeeQuoteError::Unsupported("native L2 replay reverted"));
                }
                if is_source {
                    source_gas = Some(outcome.gas_used);
                }
            }
            let gas = source_gas
                .filter(|gas| *gas > 0)
                .ok_or(FeeQuoteError::Unavailable("source gas"))?;
            let semantic = eez_protocol::blob_semantics::SemanticTransaction {
                origin_chain: RollupId(rollup_id),
                tx_data: raw_size,
                actions: composition.actions.clone(),
            };
            let semantics = Self::cross_chain_da_overhead(rollup_id, &[semantic])
                .map_err(|e| unavailable("semantic DA", e))?
                .semantic_bytes;
            let da_bytes = eez_blob_codec::native_stream_len(operations, semantics)
                .map_err(|e| unavailable("native DA", e))?;
            if da_bytes > MAX_DA_BYTES {
                return Err(FeeQuoteError::Limit("DA bytes"));
            }
            let blobs = da_bytes
                .div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB)
                .max(1);
            if blobs > maximum_blobs {
                return Err(FeeQuoteError::Limit("blob allowance"));
            }
            let (l1_gas, cost, payment) = projected_payment(
                runtime,
                &posting,
                l1_entries,
                l1_target_gas(&composition.actions),
                blobs,
            )?;
            let next_tip = required_tip(payment.l2, gas)?;
            if next_tip > effective_tip {
                if request.signed_fees {
                    return Err(FeeQuoteError::Underfunded);
                }
                tip = next_tip;
                continue;
            }
            budget.check()?;
            if unix_now()? >= expiry
                || !rollup.l1_head.generation_is(anchor.generation)
                || source
                    .headers
                    .header_by_number(parent.number)
                    .map_err(|e| unavailable("L2 freshness", e))?
                    .is_none_or(|h| h.hash_slow() != parent.hash())
                || cc
                    .local
                    .l1_entry
                    .chain_provider()
                    .headers
                    .header_by_number(posting.quote.sample.number)
                    .map_err(|e| unavailable("L1 freshness", e))?
                    .is_none_or(|h| h.hash_slow() != posting.quote.sample.hash)
                || cc
                    .local
                    .l1_entry
                    .chain_provider()
                    .headers
                    .best_block_number()
                    .map_err(|e| unavailable("L1 freshness", e))?
                    != posting.quote.sample.number
            {
                return Err(FeeQuoteError::Stale);
            }
            let transaction = request.fields(tip, cap);
            let mut intent = transaction.clone();
            intent
                .as_object_mut()
                .expect("constructed object")
                .remove("maxFeePerGas");
            intent
                .as_object_mut()
                .expect("constructed object")
                .remove("maxPriorityFeePerGas");
            intent
                .as_object_mut()
                .expect("constructed object")
                .remove("gasPrice");
            return Ok(json!({
                "version":1, "mode":"advisory", "intent":intent, "intentHash":keccak256(intent.to_string()), "transaction":transaction,
                "issuedAt":issued, "expiresAt":expiry, "policy":policy.details(),
                "context":{"rollupId":rollup_id, "l1ChainId":runtime.ctx.l1_chain_id, "l2ChainId":runtime.ctx.l2_chain_id,
                    "l1ParentHash":posting.quote.sample.hash, "l1ParentNumber":posting.quote.sample.number,
                    "l1TargetNumber":target_block, "l1TargetTimestamp":target_timestamp,
                    "l2ParentHash":parent.hash(), "l2ParentNumber":parent.number, "l2Timestamp":timestamp,
                    "settledL2Hash":endpoint.hash, "settledL2Number":endpoint.number},
                "estimate":{"basis":"isolated_native_l2_replay_and_projected_l1_post", "iterations":iteration,
                    "sourceGasUsed":gas, "nativeL2GasUsed":draft.gas_used()-gas, "l2BaseFeePerGas":base.to_string(),
                    "l1Gas":l1_gas, "l1TargetGas":l1_target_gas(&composition.actions), "blobs":blobs, "daBytes":da_bytes,
                    "l1ExecutionPrice":posting.quote.estimated_effective_gas_price.to_string(), "l1BlobPrice":posting.quote.estimated_blob_gas_price.to_string(),
                    "l1ExecutionCost":cost.execution.to_string(), "l1BlobCost":cost.blobs.to_string(), "l1PostingCost":cost.total.to_string(),
                    "l1Subtotal":payment.subtotal.to_string(), "l1Risk":payment.risk.to_string(), "l1Profit":payment.profit.to_string(),
                    "requiredL1":payment.l1.to_string(), "requiredL2Tip":payment.l2.to_string(), "grossL2Tip":(U256::from(effective_tip)*U256::from(gas)).to_string(),
                    "qualifiedRecoveryL1":null}
            }));
        }
        Err(FeeQuoteError::Limit(
            "fee-dependent execution did not converge in four simulations",
        ))
    }
}

fn unix_now() -> Result<u64, FeeQuoteError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| FeeQuoteError::Unavailable("clock"))
}

/// Quote and composition use the same whole-post projection. Quotes price one
/// source; composition checks cumulative receipts against the growing prefix.
pub(in crate::composer) fn projected_payment(
    runtime: &super::super::posting::PostingRuntime,
    posting: &PostingQuote,
    entries: &[eez_protocol::abi::ExecutionEntrySol],
    target_gas: u64,
    blobs: usize,
) -> Result<
    (
        u64,
        eez_l1::posting_fees::PostingCost,
        super::pricing::RequiredPayment,
    ),
    FeeQuoteError,
> {
    let mut budget = PostBatchGasBudget::with_calldata_limits(
        runtime.fees.estimate_budget(posting.gas_budget),
        blobs,
        runtime.calldata_floor_active(posting.quote.target),
    );
    if !budget.try_accept(projected_tx_l1_gas(entries, target_gas)) {
        return Err(FeeQuoteError::Limit("L1 gas estimate"));
    }
    let gas = runtime
        .fees
        .declared_gas(budget.full_estimate(), posting.gas_budget)
        .map_err(|e| unavailable("L1 gas margin", e))?;
    let cost = posting
        .quote
        .estimate(gas, blobs as u64)
        .map_err(|e| unavailable("L1 costs", e))?;
    let payment = runtime
        .shadow_fee_policy
        .as_ref()
        .ok_or(FeeQuoteError::Disabled)?
        .required_payment(cost.total)
        .map_err(FeeQuoteError::Limit)?;
    Ok((gas, cost, payment))
}

pub(in crate::composer) fn tariff_history_size<L2: BlockReader<Header = Header>>(
    provider: &L2,
    parent: &SealedHeader<Header>,
    endpoint: L2Endpoint,
    beneficiary: Address,
) -> Result<usize, FeeQuoteError> {
    history_size(
        provider,
        parent,
        endpoint,
        beneficiary,
        &Budget::new(Arc::new(AtomicBool::new(false))),
    )
}

fn required_tip(cost: U256, gas: u64) -> Result<u128, FeeQuoteError> {
    if gas == 0 {
        return Err(FeeQuoteError::Unavailable("zero source gas"));
    }
    u128::try_from(cost.div_ceil(U256::from(gas)))
        .map_err(|_| FeeQuoteError::Limit("priority fee exceeds u128"))
}

fn history_size<L2: BlockReader<Header = Header>>(
    provider: &L2,
    parent: &SealedHeader<Header>,
    endpoint: L2Endpoint,
    beneficiary: Address,
    budget: &Budget,
) -> Result<usize, FeeQuoteError> {
    if endpoint.number > parent.number || parent.number - endpoint.number > MAX_HISTORY {
        return Err(FeeQuoteError::Limit("unsettled history exceeds 256 blocks"));
    }
    let mut hash = parent.hash();
    let mut history = Vec::new();
    let mut raw_bytes = 0_usize;
    for number in (endpoint.number + 1..=parent.number).rev() {
        budget.check()?;
        let block = provider
            .find_block_by_hash(hash, BlockSource::Any)
            .map_err(|e| unavailable("history lookup", e))?
            .ok_or(FeeQuoteError::Unavailable("missing history"))?;
        if block.header().number != number || block.header().hash_slow() != hash {
            return Err(FeeQuoteError::Stale);
        }
        let mut txs = Vec::new();
        for tx in block.body().transactions() {
            if tx.ty() == eez_primitives::SYSTEM_TX_TYPE
                || tx
                    .recover_signer()
                    .map_err(|e| unavailable("history signer", e))?
                    == eez_primitives::SYSTEM_ADDRESS
            {
                return Err(FeeQuoteError::Unavailable(
                    "unsettled native history requires reconciliation",
                ));
            }
            let raw = tx.encoded_2718();
            raw_bytes = raw_bytes
                .checked_add(raw.len())
                .ok_or(FeeQuoteError::Limit("history bytes"))?;
            if raw_bytes > MAX_DA_BYTES {
                return Err(FeeQuoteError::Limit("history bytes"));
            }
            txs.push(Bytes::from(raw));
        }
        history.push(
            PayloadSizer::run(&BlockRun {
                blocks: 1,
                beneficiary: block.header().beneficiary,
                extra_data: &block.header().extra_data,
                transactions: &txs,
            })
            .map_err(|e| unavailable("history encoding", e))?,
        );
        hash = block.header().parent_hash;
    }
    if hash != endpoint.hash {
        return Err(FeeQuoteError::Stale);
    }
    let mut size = PayloadSizer::default();
    for entry in history.iter().rev() {
        size = size
            .combine(entry)
            .map_err(|e| unavailable("history sizing", e))?;
    }
    let terminal = PayloadSizer::run(&BlockRun {
        blocks: 1,
        beneficiary,
        extra_data: eez_driver::BUILDER_EXTRA_DATA,
        transactions: &[],
    })
    .map_err(|e| unavailable("terminal sizing", e))?;
    size.combine(&terminal)
        .and_then(|size| size.encoded_len())
        .map_err(|e| unavailable("operations sizing", e))
}

fn outbound_group(
    composition: &eez_protocol::Composition,
    rollup_id: u64,
    raw: Bytes,
) -> Result<(OutboundSyncGroup, &[eez_protocol::abi::ExecutionEntrySol]), FeeQuoteError> {
    if composition
        .targets
        .iter()
        .any(|t| !t.rollup_id.is_mainnet() && t.rollup_id.0 != rollup_id)
    {
        return Err(FeeQuoteError::Unsupported("foreign rollup"));
    }
    let entries = &composition
        .targets
        .iter()
        .find(|t| t.rollup_id.is_mainnet())
        .ok_or(FeeQuoteError::Unsupported("no L1 execution"))?
        .batch
        .entries;
    if entries.is_empty() {
        return Err(FeeQuoteError::Unsupported("no L1 settlement entries"));
    }
    let source = &composition.source;
    let roots = eez_protocol::system_tx::outbound_settlement_root_counts(entries)
        .map_err(|e| unavailable("settlement roots", e))?;
    if roots.iter().sum::<usize>() != source.batch.entries.len() + source.l2_static_entries.len() {
        return Err(FeeQuoteError::Unsupported("incomplete settlement roots"));
    }
    let callbacks = composition
        .targets
        .iter()
        .filter(|t| t.rollup_id.0 == rollup_id)
        .flat_map(|t| t.batch.entries.iter())
        .map(|entry| eez_protocol::entries::InboundSidecar::for_rollup(entry, RollupId(rollup_id)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| unavailable("callback sidecars", e))?;
    if source.callback_entry_counts.len() != source.batch.entries.len()
        || source.callback_entry_counts.iter().sum::<usize>() != callbacks.len()
    {
        return Err(FeeQuoteError::Unsupported("incomplete callbacks"));
    }
    let mut cursor = 0;
    let mut effects = Vec::new();
    for (entry, count) in source
        .batch
        .entries
        .iter()
        .zip(&source.callback_entry_counts)
    {
        effects.push(OutboundSyncEffect {
            source_entry: OutboundSourceEntry::Mutable(entry.clone()),
            callback_entries: callbacks[cursor..cursor + count].to_vec(),
        });
        cursor += count;
    }
    effects.extend(
        source
            .l2_static_entries
            .iter()
            .cloned()
            .map(|entry| OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Static(entry),
                callback_entries: vec![],
            }),
    );
    Ok((
        OutboundSyncGroup {
            effects,
            user_tx: raw,
        },
        entries,
    ))
}

struct Budget {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    frames: AtomicUsize,
    depth: AtomicUsize,
    exhausted: AtomicBool,
}
impl Budget {
    fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            deadline: Instant::now() + Duration::from_secs(10),
            frames: AtomicUsize::new(0),
            depth: AtomicUsize::new(0),
            exhausted: AtomicBool::new(false),
        }
    }
    fn check(&self) -> Result<(), FeeQuoteError> {
        if self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            return Err(FeeQuoteError::Limit("cancelled or timed out"));
        }
        if self.exhausted.load(Ordering::Relaxed) {
            return Err(FeeQuoteError::Limit("nested frame budget"));
        }
        Ok(())
    }
    fn executor_check(&self) -> ExecutorResult<()> {
        self.check()
            .map_err(|e| ExecutorErrorKind::Unavailable(e.to_string()).into())
    }
}
struct BoundedClient {
    client: Arc<LocalChainClient>,
    budget: Arc<Budget>,
}
impl ChainClient for BoundedClient {
    fn begin_execution_session(&self) -> ExecutorResult<Box<dyn TargetExecutionSession + Send>> {
        self.budget.executor_check()?;
        Ok(Box::new(BoundedSession {
            inner: self.client.begin_execution_session()?,
            budget: Arc::clone(&self.budget),
        }))
    }
}
struct BoundedSession {
    inner: Box<dyn TargetExecutionSession + Send>,
    budget: Arc<Budget>,
}
impl TargetExecutionSession for BoundedSession {
    fn execute(
        &mut self,
        req: ExecutionRequest,
        dispatcher: &mut CompositionBuilder,
    ) -> ExecutorResult<eez_protocol::ExecutionOutcome> {
        self.budget.executor_check()?;
        if self.budget.frames.fetch_add(1, Ordering::Relaxed) >= MAX_FRAMES
            || self.budget.depth.load(Ordering::Relaxed) >= MAX_DEPTH
        {
            self.budget.exhausted.store(true, Ordering::Relaxed);
            return Err(ExecutorErrorKind::Unavailable("quote nested frame budget".into()).into());
        }
        self.budget.depth.fetch_add(1, Ordering::Relaxed);
        let result = self.inner.execute(req, dispatcher);
        self.budget.depth.fetch_sub(1, Ordering::Relaxed);
        self.budget.executor_check()?;
        result
    }
    fn checkpoint(&mut self) -> ExecutorResult<SessionSnapshot> {
        self.budget.executor_check()?;
        self.inner.checkpoint()
    }
    fn rollback(&mut self, snapshot: SessionSnapshot) -> ExecutorResult<()> {
        self.inner.rollback(snapshot)
    }
}

#[cfg(test)]
mod tests;
