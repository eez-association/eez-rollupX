//! Wallet gas estimation. All execution state is request-local; estimates never
//! reserve a nonce, enter the held pool, request a proof or authorize posting.
use crate::local::build::{SyncBlockState, TxOutcome};
use crate::{
    Direction,
    local::{InboundL2TargetSession, L1SlotState, L1TargetSession, LocalChainClient},
};
use alloy_consensus::Header;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rpc_types_eth::TransactionRequest;
use eez_protocol::{Composition, CompositionBuilder, RollupId, composition::Rollup};
use reth_evm::{ConfigureEvm, Evm, EvmEnv, NextBlockEnvAttributes, rpc::TryIntoTxEnv};
use reth_primitives_traits::SealedHeader;
use reth_storage_api::StateProvider;
use revm::{
    Database, DatabaseCommit,
    context::{Cfg as _, TxEnv},
};
use std::{collections::HashMap, sync::Arc, time::Instant};

mod budget;
mod classify;
pub(super) mod deployment;
pub use classify::is_dispatch;
mod l1;
mod preparation;

#[derive(Debug, thiserror::Error)]
pub enum EstimateError {
    #[error("invalid estimation request: {0}")]
    Invalid(String),
    #[error(transparent)]
    InsufficientFunds(crate::SourceBalanceError),
    #[error(transparent)]
    PosterFunds(Box<crate::PosterBalanceError>),
    #[error("cross-chain estimation unavailable: {0}")]
    Unavailable(String),
    #[error("cross-chain estimation unavailable: settlement reconciliation pending")]
    SettlementPending,
    #[error("cross-chain estimation unavailable: chain context changed during estimation")]
    ContextChanged,
    #[error("unsupported cross-chain estimate: {0}")]
    Unsupported(String),
    #[error("execution reverted")]
    Revert(Bytes),
    #[error("cross-chain execution differs from composition: {0}")]
    Effects(String),
    #[error("gas estimation deadline exceeded")]
    Timeout,
}
fn unavailable(e: impl std::fmt::Display) -> EstimateError {
    EstimateError::Unavailable(e.to_string())
}
fn source_error(chain_id: u64, error: eez_protocol::ExecutorError) -> EstimateError {
    match crate::SourceBalanceError::from_executor(chain_id, &error) {
        Some(balance) => EstimateError::InsufficientFunds(balance),
        None => unavailable(error),
    }
}
fn unsupported(e: impl std::fmt::Display) -> EstimateError {
    EstimateError::Unsupported(e.to_string())
}

/// Immutable dependencies only. Each probe creates independent execution and
/// overlay sessions; sharing this handle does not share speculative state.
#[derive(Clone, Debug)]
pub struct GasEstimator {
    l1: Arc<LocalChainClient>,
    l2: Arc<LocalChainClient>,
    head: Arc<eez_l1::L1CanonicalHead>,
    optimistic: Arc<crate::optimistic::OptimisticallyIncluded>,
    rollup_id: u64,
    poster: Address,
    post_batch_target: Address,
    verifier: Address,
    offline_profile: eez_protocol::l1_history::OfflineProfile,
    rollup_manager: Address,
    l1_slot_seconds: u64,
    recipient: Address,
}

pub(super) struct Snapshot {
    l1: L1SlotState,
    l1_env: EvmEnv,
    l1_attributes: NextBlockEnvAttributes,
    pub(super) l2_parent: SealedHeader<Header>,
    pub(super) l2_timestamp: u64,
    pub(super) l2_randao: B256,
    l1_history: Arc<eez_protocol::l1_history::HeaderChain>,
    settled_root: B256,
    generation: Option<u64>,
    // Request-local, natively replayed L1 predecessors. These bytes never enter
    // the pool or chain; final settlement still rebuilds the real shared batch.
    l2_prefix: Vec<Bytes>,
    l1_prefix_gas: u64,
    prefix_semantic_bytes: usize,
}
#[derive(Debug)]
pub(super) struct Probe {
    required: u64,
    limit: u64,
    pub(super) source_gas: u64,
    pub(super) settlement: Option<(u64, usize, usize)>,
    post_required: u64,
    post_limit: u64,
    post_funding: Option<crate::PosterBalanceError>,
    output: Bytes,
    actions: Vec<eez_protocol::ExecutedAction>,
    retained_l1: Option<revm::database::CacheState>,
    retained_l2: Vec<Bytes>,
    retained_root: Option<B256>,
}

pub(super) struct InboundPricing<'a> {
    pub runtime: &'a super::posting::PostingRuntime,
    pub posting: &'a super::posting::PostingQuote,
    pub operations: usize,
    pub maximum_blobs: usize,
    pub budget: Arc<super::fees::quote::Budget>,
}

impl<L2> super::Composer<L2>
where
    L2: reth_storage_api::BlockReader<Header = Header> + Send + Sync + 'static,
    <L2 as reth_storage_api::TransactionsProvider>::Transaction: alloy_eips::eip2718::Encodable2718,
{
    /// Build a read-only estimator without exposing signing or submission handles.
    pub fn gas_estimator(&self, rollup_id: u64) -> Result<GasEstimator, EstimateError> {
        let rollup = self
            .inner
            .rollups
            .get(&rollup_id)
            .ok_or_else(|| unsupported("unknown rollup"))?;
        let cc = &self.inner.cross_chain;
        Ok(GasEstimator {
            l1: cc.local.l1_entry.clone(),
            l2: cc.local.l2_entry.clone(),
            head: rollup.l1_head.clone(),
            optimistic: rollup.optimistic.clone(),
            rollup_id,
            poster: cc.exec_ctx.l1_poster_signer.address(),
            post_batch_target: self.inner.posting.post_batch_target,
            verifier: cc.exec_ctx.ecdsa_proof_system_address,
            offline_profile: cc.exec_ctx.offline_profile.clone(),
            rollup_manager: cc.exec_ctx.rollup_manager,
            l1_slot_seconds: self.inner.emission.timing.l1_block_time().as_secs(),
            recipient: self.inner.fee_recipient,
        })
    }
}

impl GasEstimator {
    pub(super) fn source_account(
        &self,
        direction: Direction,
        snapshot: &Snapshot,
        address: Address,
    ) -> Result<revm::state::AccountInfo, EstimateError> {
        if direction == Direction::Inbound {
            let (mut state, _) = snapshot.l1.fork_state(&self.l1).map_err(unavailable)?;
            state
                .basic(address)
                .map(Option::unwrap_or_default)
                .map_err(unavailable)
        } else {
            let mut draft = self.l2_draft(snapshot)?;
            let mut fork = draft.fork().map_err(unavailable)?;
            fork.state_and_env()
                .0
                .basic(address)
                .map(Option::unwrap_or_default)
                .map_err(unavailable)
        }
    }

    /// Replay an owned L1 predecessor, including its real fee fields, source
    /// nonce, balance changes and complete nested effects, into disposable state.
    /// Each predecessor has its own conditional native setup at the same target;
    /// final batching remains authoritative for shared-table gas and proof checks.
    pub(super) fn advance_inbound(
        &self,
        snapshot: &mut Snapshot,
        tx: TxEnv,
        deadline: Instant,
        pricing: InboundPricing<'_>,
    ) -> Result<(), EstimateError> {
        let mut replay = self.priced_inbound(snapshot, tx, deadline, pricing)?;
        let semantic = eez_protocol::blob_semantics::SemanticTransaction {
            origin_chain: RollupId::MAINNET,
            tx_data: Bytes::new(),
            actions: replay.actions,
        };
        let overhead =
            super::policy::semantic_overhead(self.rollup_id, &[semantic]).map_err(unavailable)?;
        snapshot.prefix_semantic_bytes = snapshot
            .prefix_semantic_bytes
            .checked_add(overhead.semantic_bytes)
            .filter(|bytes| *bytes <= 2 * 1024 * 1024)
            .ok_or_else(|| unsupported("pending DA capacity"))?;
        snapshot.l1_prefix_gas = snapshot
            .l1_prefix_gas
            .checked_add(replay.source_gas)
            .ok_or_else(|| unsupported("pending gas overflow"))?;
        snapshot.l1.cache = replay
            .retained_l1
            .take()
            .ok_or_else(|| unavailable("missing predecessor state"))?;
        snapshot.settled_root = replay
            .retained_root
            .ok_or_else(|| unavailable("missing predecessor endpoint"))?;
        snapshot.l2_prefix.extend(replay.retained_l2);
        Ok(())
    }

    /// Blocking bounded-probe work. The RPC worker retains its concurrency permit
    /// until this returns, even if the caller's response deadline has expired.
    pub fn estimate(
        &self,
        direction: Direction,
        request: TransactionRequest,
        deadline: Instant,
    ) -> Result<u64, EstimateError> {
        let snapshot = self.snapshot()?;
        self.estimate_on_snapshot(direction, request, deadline, &snapshot)
    }

    /// Reuse one pinned context for gas discovery and fee variants. The fee
    /// worker owns the aggregate deadline and permit; this acquires neither.
    pub(super) fn estimate_on_snapshot(
        &self,
        direction: Direction,
        request: TransactionRequest,
        deadline: Instant,
        snapshot: &Snapshot,
    ) -> Result<u64, EstimateError> {
        if request.from.is_none() {
            return Err(EstimateError::Invalid("from is required".into()));
        }
        if request.to.is_none_or(|to| to.is_create()) {
            return Err(unsupported("contract creation"));
        }
        if request.minimal_tx_type() as u8 > 2
            || request.transaction_type.is_some_and(|t| t > 2)
            || request.authorization_list.is_some()
            || request.blob_versioned_hashes.is_some()
            || request.sidecar.is_some()
        {
            return Err(unsupported(
                "only legacy, access-list and EIP-1559 source calls are supported",
            ));
        }
        // Every probe starts at this same native prefix. Only forks may execute
        // candidates, so gas search cannot inherit a prior probe's writes,
        // spent nonce or gas. Reopening would replay owned predecessors again.
        let mut draft = self.l2_draft(snapshot)?;
        let mut env = if direction == Direction::Inbound {
            snapshot.l1_env.clone()
        } else {
            draft.evm_env().clone()
        };
        env.cfg_env.disable_base_fee = true;
        if request
            .chain_id
            .is_some_and(|id| id != env.cfg_env.chain_id)
        {
            return Err(EstimateError::Invalid(
                "chainId differs from the source chain".into(),
            ));
        }
        let caller = request
            .from
            .ok_or_else(|| EstimateError::Invalid("from is required".into()))?;
        let account = if direction == Direction::Inbound {
            self.source_account(direction, snapshot, caller)?
        } else {
            let mut fork = draft.fork().map_err(unavailable)?;
            fork.state_and_env()
                .0
                .basic(caller)
                .map_err(unavailable)?
                .unwrap_or_default()
        };
        let nonce = account.nonce;
        if request.nonce.is_some_and(|requested| requested != nonce) {
            return Err(EstimateError::Invalid(format!(
                "nonce must match replayed source state ({nonce})"
            )));
        }
        let mut tx: TxEnv = request
            .clone()
            .try_into_tx_env(&env)
            .map_err(|e| EstimateError::Invalid(format!("{e}")))?;
        tx.nonce = nonce;
        tx.tx_type = request.transaction_type.unwrap_or(tx.tx_type);
        let cap = env.cfg_env.tx_gas_limit_cap().min(env.block_env.gas_limit);
        tx.gas_limit = tx.gas_limit.min(cap);
        let balance = account.balance;
        let available = balance
            .checked_sub(tx.value)
            .ok_or(EstimateError::InsufficientFunds(
                crate::SourceBalanceError {
                    available: balance,
                    required: tx.value,
                    chain_id: env.cfg_env.chain_id,
                },
            ))?;
        if tx.gas_price != 0 {
            tx.gas_limit = tx
                .gas_limit
                .min((available / U256::from(tx.gas_price)).saturating_to());
        }
        let first = self.probe(direction, snapshot, tx.clone(), deadline, &mut draft)?;
        // Native setup consumes block capacity. Its measured remainder can
        // tighten the high bound before the user's first execution.
        let ceiling = first.limit;
        // L1 companions scan a shared table whose preceding rows can increase
        // after this isolated quote. Reserve 20% there (5% for L2), plus the
        // call stipend. This sizes gas, not the fee tariff: native replay still
        // prices actual consumption, and final preflight may shrink the batch.
        let candidate = first
            .required
            .saturating_add(2_300)
            .saturating_mul(if direction == Direction::Inbound {
                120
            } else {
                105
            })
            .div_ceil(100)
            .min(ceiling);
        let selected = if candidate == ceiling && tx.gas_limit == ceiling {
            ceiling
        } else {
            tx.gas_limit = candidate;
            match self.probe(direction, snapshot, tx.clone(), deadline, &mut draft) {
                Ok(ref lower)
                    if lower.limit == candidate
                        && same_actions(&first.actions, &lower.actions)
                        && first.output == lower.output =>
                {
                    candidate
                }
                Ok(_) | Err(EstimateError::Revert(_) | EstimateError::Effects(_)) => {
                    tx.gas_limit = ceiling;
                    let repeated = self.probe(direction, snapshot, tx, deadline, &mut draft)?;
                    if repeated.limit != ceiling
                        || !same_actions(&first.actions, &repeated.actions)
                        || first.output != repeated.output
                    {
                        return Err(EstimateError::Effects(
                            "high-gas replay changed its effects".into(),
                        ));
                    }
                    tracing::debug!(
                        ?direction,
                        ceiling,
                        candidate,
                        "wallet gas estimate uses verified ceiling"
                    );
                    ceiling
                }
                Err(error) => return Err(error),
            }
        };
        if let Some(generation) = snapshot.generation
            && (self.optimistic.pending_attempt().is_some()
                || self.head.policy_anchor_snapshot().generation != generation)
        {
            return Err(EstimateError::ContextChanged);
        }
        tracing::debug!(
            ?direction,
            gas = selected,
            measured = first.source_gas,
            proof_assumed = direction == Direction::Inbound,
            "wallet gas estimate completed"
        );
        Ok(selected)
    }

    /// Freeze the same native history prefix used by block production. Fee
    /// probes must include its exact bytes, gas and selected PREVRANDAO.
    fn history_plan(
        &self,
        parent: &SealedHeader<Header>,
        through: eez_l1::L1BlockRef,
    ) -> Result<
        (
            u64,
            B256,
            Arc<eez_protocol::l1_history::HeaderChain>,
            Vec<Bytes>,
        ),
        EstimateError,
    > {
        use alloy_eips::Encodable2718 as _;
        let (timestamp, headers) = self
            .head
            .estimation_block_context(parent, through)
            .map_err(unavailable)?;
        let chain = self.l2.chain_provider();
        let config = chain
            .evm_config
            .clone()
            .with_l1_history(Arc::new(eez_evm::history::WitnessHistory(Arc::clone(
                &headers,
            ))))
            .map_err(unavailable)?;
        let provider = chain
            .provider
            .state_by_block_hash(parent.hash())
            .map_err(unavailable)?;
        let mut db = reth_revm::database::StateProviderDatabase::new(
            reth_storage_api::EvmStateProviderAdapter(provider),
        );
        let (plan, tx) = config
            .prepare_history(&mut db, parent, timestamp)
            .map_err(unavailable)?;
        Ok((
            timestamp,
            plan.prev_randao,
            headers,
            tx.into_iter().map(|tx| tx.encoded_2718().into()).collect(),
        ))
    }

    /// Gas and pricing must use the exact posting target, including skipped-slot
    /// timestamps and the poster's fee environment, rather than independently
    /// choosing a nominal next slot.
    pub(super) fn quote_snapshot(
        &self,
        posting: &super::posting::PostingQuote,
        l2_parent: SealedHeader<Header>,
        endpoint: eez_l1::L2Endpoint,
    ) -> Result<Snapshot, EstimateError> {
        let (l2_timestamp, l2_randao, l1_history, l2_prefix) = self
            .history_plan(&l2_parent, posting.quote.sample)
            .map_err(unavailable)?;
        Ok(Snapshot {
            l1: L1SlotState {
                anchor: SealedHeader::new(posting.parent.clone(), posting.quote.sample.hash),
                cache: Default::default(),
                posting: Some((
                    posting.env.clone(),
                    self.poster,
                    posting.quote.estimated_effective_gas_price,
                )),
            },
            l1_env: posting.env.clone(),
            l1_attributes: posting.attributes.clone(),
            l2_parent,
            l2_timestamp,
            l2_randao,
            l1_history,
            settled_root: endpoint.hash,
            generation: None,
            l2_prefix,
            l1_prefix_gas: 0,
            prefix_semantic_bytes: 0,
        })
    }

    fn snapshot(&self) -> Result<Snapshot, EstimateError> {
        if self.optimistic.pending_attempt().is_some() {
            return Err(EstimateError::SettlementPending);
        }
        let anchor = self.head.policy_anchor_snapshot();
        let through = anchor
            .through
            .ok_or_else(|| unavailable("canonical scan not ready"))?;
        let endpoint = anchor
            .endpoint
            .ok_or_else(|| unavailable("canonical settlement endpoint missing"))?;
        let l1_provider = self.l1.chain_provider();
        let header = l1_provider
            .headers
            .header_by_number(through.number)
            .map_err(unavailable)?
            .ok_or_else(|| unavailable("L1 parent missing"))?;
        let parent = SealedHeader::seal_slow(header);
        if parent.hash() != through.hash {
            return Err(unavailable("L1 scan parent changed"));
        }
        let timestamp = parent
            .timestamp
            .checked_add(self.l1_slot_seconds)
            .ok_or_else(|| unavailable("L1 timestamp overflow"))?;
        let attributes = NextBlockEnvAttributes {
            timestamp,
            suggested_fee_recipient: parent.beneficiary,
            prev_randao: parent.mix_hash,
            gas_limit: parent.gas_limit,
            parent_beacon_block_root: Some(B256::ZERO),
            withdrawals: Some(Default::default()),
            extra_data: parent.extra_data.clone(),
            slot_number: None,
        };
        let env = if let Some(config) = &l1_provider.gnosis_evm_config {
            config.next_evm_env(&parent.header().clone().into(), &attributes)
        } else {
            reth_evm_ethereum::EthEvmConfig::new(l1_provider.evm_config.chain_spec().clone())
                .next_evm_env(&parent, &attributes)
        }
        .map_err(unavailable)?;
        let l2_provider = self.l2.chain_provider();
        let number = l2_provider
            .headers
            .best_block_number()
            .map_err(unavailable)?;
        let l2_parent = SealedHeader::seal_slow(
            l2_provider
                .headers
                .header_by_number(number)
                .map_err(unavailable)?
                .ok_or_else(|| unavailable("L2 parent missing"))?,
        );
        let settled_header = l2_provider
            .headers
            .header_by_number(endpoint.number)
            .map_err(unavailable)?
            .ok_or_else(|| unavailable("settled L2 parent missing"))?;
        if SealedHeader::seal_slow(settled_header).hash() != endpoint.hash
            || l2_parent.number < endpoint.number
        {
            return Err(unavailable(
                "L2 branch differs from the reconciled endpoint",
            ));
        }
        let (l2_timestamp, l2_randao, l1_history, l2_prefix) = self
            .history_plan(&l2_parent, through)
            .map_err(unavailable)?;
        let l1 = L1SlotState {
            anchor: parent,
            cache: Default::default(),
            posting: Some((env.clone(), self.poster, 0)),
        };
        Ok(Snapshot {
            l1,
            l1_env: env,
            l1_attributes: attributes,
            l2_parent,
            l2_timestamp,
            l2_randao,
            l1_history,
            settled_root: endpoint.hash,
            generation: Some(anchor.generation),
            l2_prefix,
            l1_prefix_gas: 0,
            prefix_semantic_bytes: 0,
        })
    }

    pub(super) fn l2_draft(&self, s: &Snapshot) -> Result<SyncBlockState, EstimateError> {
        let chain = self.l2.chain_provider();
        let config = chain
            .evm_config
            .clone()
            .with_l1_history(Arc::new(eez_evm::history::WitnessHistory(Arc::clone(
                &s.l1_history,
            ))))
            .map_err(unavailable)?;
        SyncBlockState::open(
            chain.provider.clone(),
            &config,
            &s.l2_parent,
            s.l2_timestamp,
            self.recipient,
            &s.l2_prefix,
        )
        .map_err(unavailable)
    }

    fn probe(
        &self,
        direction: Direction,
        s: &Snapshot,
        tx: TxEnv,
        deadline: Instant,
        draft: &mut SyncBlockState,
    ) -> Result<Probe, EstimateError> {
        self.probe_inner(direction, s, tx, deadline, None, draft)
    }

    pub(super) fn priced_inbound(
        &self,
        s: &Snapshot,
        tx: TxEnv,
        deadline: Instant,
        pricing: InboundPricing<'_>,
    ) -> Result<Probe, EstimateError> {
        let limit = tx.gas_limit;
        if Instant::now() >= deadline {
            return Err(EstimateError::Timeout);
        }
        let mut draft = self.l2_draft(s)?;
        let result = self.probe_inner(
            Direction::Inbound,
            s,
            tx,
            deadline,
            Some(pricing),
            &mut draft,
        )?;
        if result.limit != limit {
            return Err(unsupported("quoted gas exceeds native block capacity"));
        }
        Ok(result)
    }

    fn probe_inner(
        &self,
        direction: Direction,
        s: &Snapshot,
        tx: TxEnv,
        deadline: Instant,
        pricing: Option<InboundPricing<'_>>,
        draft: &mut SyncBlockState,
    ) -> Result<Probe, EstimateError> {
        if Instant::now() >= deadline {
            return Err(EstimateError::Timeout);
        }
        let l2_env = draft.evm_env().clone();
        let l1 = self.l1.isolated(s.l1.anchor.hash(), s.l1_env.clone());
        let l2 = self.l2.isolated(s.l2_parent.hash(), l2_env);
        let mut rollups = HashMap::new();
        for (id, client, dialect) in [
            (
                RollupId::MAINNET,
                l1.clone(),
                eez_protocol::ChainDialect::EvmL1Style,
            ),
            (
                RollupId(self.rollup_id),
                l2.clone(),
                eez_protocol::ChainDialect::EvmL2Style,
            ),
        ] {
            rollups.insert(
                id,
                Rollup {
                    client: if let Some(pricing) = &pricing {
                        super::fees::quote::bounded_client(client.clone(), pricing.budget.clone())
                    } else {
                        Arc::new(budget::Client {
                            inner: client.clone(),
                            deadline,
                        })
                    },
                    session: None,
                    config: eez_protocol::TargetConfig {
                        dialect,
                        proxy_lookup: eez_protocol::ProxyLookupConfig {
                            contract_address: client.manager_address(),
                            authorized_proxies_slot: dialect.proxy_lookup_slot(),
                        },
                    },
                },
            );
        }
        let wrap = |session: Box<dyn eez_protocol::TargetExecutionSession + Send>| {
            if let Some(pricing) = &pricing {
                super::fees::quote::bounded_session(session, pricing.budget.clone())
            } else {
                budget::wrap(session, deadline)
            }
        };
        let (entry, mut builder, source) = if direction == Direction::Inbound {
            let session =
                InboundL2TargetSession::new(draft.fork().map_err(unavailable)?, l2.clone());
            let mut builder =
                CompositionBuilder::new(RollupId::MAINNET, rollups).with_sessions(HashMap::from([
                    (RollupId(self.rollup_id), wrap(Box::new(session))),
                ]));
            let (mut state, _) = s.l1.fork_state(&l1).map_err(unavailable)?;
            let mut source_env = s.l1_env.clone();
            source_env.cfg_env.disable_base_fee = true;
            let chain_id = source_env.cfg_env.chain_id;
            let source = l1
                .simulate_source_env_on(tx.clone(), &mut builder, &mut state, source_env, true)
                .map_err(|error| source_error(chain_id, error))?;
            (RollupId::MAINNET, builder, source)
        } else {
            let session = L1TargetSession::new(&s.l1, l1).map_err(unavailable)?;
            let mut builder =
                CompositionBuilder::new(RollupId(self.rollup_id), rollups).with_sessions(
                    HashMap::from([(RollupId::MAINNET, wrap(Box::new(session)))]),
                );
            let mut fork = draft.fork().map_err(unavailable)?;
            let (state, env) = fork.state_and_env();
            let mut source_env = env.clone();
            source_env.cfg_env.disable_base_fee = true;
            let chain_id = source_env.cfg_env.chain_id;
            let source = l2
                .simulate_source_env_on(tx.clone(), &mut builder, state, source_env, true)
                .map_err(|error| source_error(chain_id, error))?;
            (RollupId(self.rollup_id), builder, source)
        };
        if !source.success {
            return Err(EstimateError::Revert(source.output));
        }
        let _sessions = builder.take_sessions();
        // Finalization requires cross-chain entries; ordinary calls must take
        // the native replay path before that protocol-only validation.
        if builder.recorded_count() == 0 {
            if direction == Direction::Outbound {
                let mut fork = draft.fork().map_err(unavailable)?;
                let limit = tx.gas_limit.min(fork.remaining_gas());
                let mut tx = tx;
                tx.gas_limit = limit;
                let user = fork
                    .execute_unsigned(tx, revm::inspector::NoOpInspector)
                    .map_err(unavailable)?;
                if !user.success {
                    return Err(EstimateError::Revert(user.output));
                }
                if user.output != source.output {
                    return Err(EstimateError::Effects(
                        "ordinary replay output changed".into(),
                    ));
                }
                return Ok(Probe {
                    required: user.gas_required,
                    limit,
                    source_gas: user.gas_used,
                    output: user.output,
                    actions: vec![],
                    settlement: None,
                    post_required: 0,
                    post_limit: 0,
                    post_funding: None,
                    retained_l1: None,
                    retained_l2: Vec::new(),
                    retained_root: None,
                });
            }
            return Err(unsupported("request did not produce a cross-chain action"));
        }
        let composition = builder.finalize().map_err(unsupported)?;
        if composition
            .targets
            .iter()
            .any(|t| t.rollup_id != RollupId::MAINNET && t.rollup_id != RollupId(self.rollup_id))
        {
            return Err(unsupported("foreign rollup"));
        }
        if Instant::now() >= deadline {
            return Err(EstimateError::Timeout);
        }
        let replay = if entry == RollupId::MAINNET {
            let (blobs, da_bytes) = if let Some(pricing) = &pricing {
                let semantic = eez_protocol::blob_semantics::SemanticTransaction {
                    origin_chain: RollupId::MAINNET,
                    tx_data: Bytes::new(),
                    actions: composition.actions.clone(),
                };
                let overhead = super::policy::semantic_overhead(self.rollup_id, &[semantic])
                    .map_err(unavailable)?;
                let bytes = eez_blob_codec::native_stream_len(
                    pricing.operations,
                    overhead
                        .semantic_bytes
                        .checked_add(s.prefix_semantic_bytes)
                        .ok_or_else(|| unsupported("pending semantic bytes overflow"))?,
                )
                .map_err(unavailable)?;
                let blobs = bytes.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB).max(1);
                if bytes > 2 * 1024 * 1024 || blobs > pricing.maximum_blobs {
                    return Err(unsupported("quote DA capacity"));
                }
                (blobs, bytes)
            } else {
                (0, 0)
            };
            let hashes = (0..blobs)
                .map(|i| {
                    let mut hash = B256::repeat_byte(0x11);
                    hash[0] = 1;
                    hash[31] = i as u8;
                    hash
                })
                .collect::<Vec<_>>();
            let (batch, digest, pending_txs, pending_root) =
                preparation::inbound(self, s, &composition, &hashes)?;
            let mut replay = l1::replay(
                self,
                s,
                l1::Replay {
                    batch: &batch,
                    composition: &composition,
                    tx: tx.clone(),
                    digest,
                    posting: pricing.as_ref().map(|p| p.posting),
                    blob_hashes: &hashes,
                    gas_limit: None,
                },
            )?;
            if let Some(pricing) = &pricing {
                let used = replay
                    .settlement
                    .ok_or_else(|| unavailable("missing posting receipt"))?
                    .0;
                let gas_limit = pricing
                    .runtime
                    .fees
                    .declared_gas(replay.post_required, replay.post_limit)
                    .map_err(|error| match &replay.post_funding {
                        Some(funding) if funding.limits_probe() => {
                            EstimateError::PosterFunds(Box::new(funding.clone()))
                        }
                        _ => unavailable(error),
                    })?;
                if gas_limit != replay.post_limit {
                    match l1::replay(
                        self,
                        s,
                        l1::Replay {
                            batch: &batch,
                            composition: &composition,
                            tx,
                            digest,
                            posting: Some(pricing.posting),
                            blob_hashes: &hashes,
                            gas_limit: Some(gas_limit),
                        },
                    ) {
                        Ok(selected) => replay = selected,
                        Err(
                            EstimateError::Unsupported(_)
                            | EstimateError::Revert(_)
                            | EstimateError::Effects(_),
                        ) => {}
                        Err(error) => return Err(error),
                    }
                }
                let used = replay.settlement.map_or(used, |v| v.0);
                replay.settlement = Some((used, blobs, da_bytes));
            }
            replay.retained_l2 = pending_txs;
            replay.retained_root = Some(pending_root);
            Ok(replay)
        } else {
            preparation::outbound(self, &composition, tx, draft.fork().map_err(unavailable)?)
        }?;
        if replay.output != source.output {
            return Err(EstimateError::Effects(
                "native source output differs from composition".into(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(EstimateError::Timeout);
        }
        Ok(replay)
    }
}

fn execute_user<E: Evm<Tx = TxEnv, HaltReason = revm::context_interface::result::HaltReason>>(
    mut evm: E,
    tx: TxEnv,
    funding: Option<&crate::PosterBalanceError>,
) -> Result<TxOutcome, EstimateError>
where
    E::DB: DatabaseCommit,
{
    let gas_limit = tx.gas_limit;
    let result = evm.transact(tx).map_err(|error| match funding {
        Some(funding) if funding.explains(gas_limit, &error) => {
            EstimateError::PosterFunds(Box::new(funding.clone()))
        }
        _ => unavailable(error),
    })?;
    if let Some(funding) = funding
        && funding.limits_probe()
        && gas_limit == funding.affordable_gas
        && matches!(
            result.result,
            revm::context_interface::result::ExecutionResult::Halt {
                reason: revm::context_interface::result::HaltReason::OutOfGas(_),
                ..
            }
        )
    {
        return Err(EstimateError::PosterFunds(Box::new(funding.clone())));
    }
    let gas = result.result.gas();
    let output = TxOutcome {
        success: result.result.is_success(),
        gas_used: gas.tx_gas_used(),
        gas_required: gas.total_gas_spent().max(gas.floor_gas()),
        output: result.result.output().cloned().unwrap_or_default(),
        logs: result.result.logs().to_vec(),
    };
    evm.db_mut().commit(result.state);
    Ok(output)
}

fn same_actions(
    left: &[eez_protocol::ExecutedAction],
    right: &[eez_protocol::ExecutedAction],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.parent_index == b.parent_index
                && a.call_mode == b.call_mode
                && a.target_address == b.target_address
                && a.target_rollup_id == b.target_rollup_id
                && a.source_address == b.source_address
                && a.source_rollup_id == b.source_rollup_id
                && a.data == b.data
                && a.value == b.value
                && a.revert_span == b.revert_span
                && a.outcome.is_success() == b.outcome.is_success()
                && a.outcome.return_data() == b.outcome.return_data()
        })
}
