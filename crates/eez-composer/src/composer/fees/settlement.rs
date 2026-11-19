//! Receipt-cost estimation for outbound settlement. No signed transaction or
//! proof leaves this module: all roots, blobs and proof acceptance are private
//! estimation inputs. Actual dispatch still uses independent proof validation
//! and `PostingRuntime::finalize` on the complete signed bundle.

use alloy_consensus::{Transaction, TxEip4844, TxLegacy, transaction::Recovered};
use alloy_primitives::{Address, B256, Bytes, I256, TxKind, U256, b256};
use alloy_sol_types::{SolCall, SolEvent};
use eez_protocol::{
    EvmBatch,
    abi::{EtherDeltaSol, ExecutionEntrySol, RootUpdateSol, postAndVerifyBatchCall, rollupsCall},
    blob_semantics::SemanticTransaction,
    entries::EntryTrace,
    system_tx::OutboundSyncGroup,
};
use evm2::{
    EvmFeatures, EvmTypes, Inspector,
    ethereum::TxEnvelope,
    evm::inspector::InspectorConfig,
    interpreter::{InstrStop, Interpreter, Message, MessageKind, MessageResult},
};
use reth_evm::{
    BlockExecutorFactory, ConfigureEvm, database::StateProviderDatabase,
    execute::BlockExecutor as _,
};
use reth_evm_ethereum::EthEvmEnv;
use reth_primitives_traits::SealedHeader;
use reth_storage_api::{EvmStateProviderAdapter, StateProvider};

use super::super::posting::{EffectInspector, PostingQuote, PostingRuntime};
use super::quote::FeeQuoteError;

alloy_sol_types::sol! {
    function verify(bytes proof, bytes32 publicInputsHash) external view returns (bool);
    function checkProofSystemsAndGetVkeys(address[] proofSystems) external view returns (bytes32[]);
}

pub(in crate::composer) struct SettlementInput<'a> {
    pub rollup_id: u64,
    pub settled_root: B256,
    pub entries: &'a [ExecutionEntrySol],
    pub traces: &'a [EntryTrace],
    pub groups: &'a [OutboundSyncGroup],
    pub semantics: &'a [SemanticTransaction],
    pub blobs: usize,
}

fn unavailable(error: impl std::fmt::Display) -> FeeQuoteError {
    tracing::debug!(%error, "L1 settlement fee simulation unavailable");
    FeeQuoteError::Unavailable("L1 settlement execution")
}

/// Quote and cumulative signed admission use identical receipt-cost arithmetic.
/// Gas-limit headroom is used only to choose an executable transaction limit.
pub(in crate::composer) fn measured_payment(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    input: SettlementInput<'_>,
) -> Result<
    (
        u64,
        eez_l1::posting_fees::PostingCost,
        super::pricing::RequiredPayment,
    ),
    FeeQuoteError,
> {
    Estimator::new(runtime, posting).measured_payment(input)
}

/// Bound to one immutable posting context and one source envelope's pricing
/// loop. Source/native execution always reruns; only identical successful L1
/// replays can share a gas measurement. Never share this across quote requests.
pub(in crate::composer) struct Estimator<'a> {
    runtime: &'a PostingRuntime,
    posting: &'a PostingQuote,
    preparation: PreparationCache,
    cache: ReplayCache,
}

impl<'a> Estimator<'a> {
    pub fn new(runtime: &'a PostingRuntime, posting: &'a PostingQuote) -> Self {
        Self {
            runtime,
            posting,
            preparation: PreparationCache::default(),
            cache: ReplayCache::default(),
        }
    }

    pub fn reuses(&self) -> usize {
        self.cache.reuses
    }

    pub fn measured_payment(
        &mut self,
        input: SettlementInput<'_>,
    ) -> Result<
        (
            u64,
            eez_l1::posting_fees::PostingCost,
            super::pricing::RequiredPayment,
        ),
        FeeQuoteError,
    > {
        let snapshot = self
            .preparation
            .get_or_load(input.rollup_id, input.settled_root, || {
                prepare_snapshot(
                    self.runtime,
                    self.posting,
                    input.rollup_id,
                    input.settled_root,
                )
            })?;
        let (prepared, funding) = prepare(self.runtime, self.posting, snapshot, &input)?;
        let gas = self.cache.measure(prepared, |prepared| {
            estimate(self.runtime, self.posting, prepared, &funding)
        })?;
        let cost = self
            .posting
            .quote
            .estimate(gas, input.blobs as u64)
            .map_err(unavailable)?;
        let payment = self
            .runtime
            .collection
            .pricing
            .as_ref()
            .ok_or(FeeQuoteError::Disabled)?
            .required_payment(cost.total)
            .map_err(FeeQuoteError::Limit)?;
        Ok((gas, cost, payment))
    }
}

/// Validated deployment and account reads at the estimator's pinned L1 state.
/// No simulated state, blob reservation or execution outcome belongs here.
struct SnapshotPreparation {
    members: Vec<(Address, B256)>,
    poster_nonce: u64,
    poster_balance: U256,
}

#[derive(Default)]
struct PreparationCache {
    previous: Option<((u64, B256), SnapshotPreparation)>,
}

impl PreparationCache {
    fn get_or_load(
        &mut self,
        rollup_id: u64,
        settled_root: B256,
        load: impl FnOnce() -> Result<SnapshotPreparation, FeeQuoteError>,
    ) -> Result<&SnapshotPreparation, FeeQuoteError> {
        // The owning estimator fixes the state hash, target and runtime. A
        // different rollup or expected root must still pass deployment/root
        // validation, even if a previous pricing iteration loaded successfully.
        let key = (rollup_id, settled_root);
        if self
            .previous
            .as_ref()
            .is_none_or(|(previous, _)| *previous != key)
        {
            self.previous = Some((key, load()?));
        }
        // Both paths above establish a successful preparation for this key.
        Ok(&self
            .previous
            .as_ref()
            .expect("matching preparation loaded")
            .1)
    }
}

/// Complete variable inputs consumed by the L1 probe and its validation. The
/// enclosing Estimator pins state, block environment, gas policy and deployment.
/// Source fee bytes are absent only because L1 executes this prepared post, not
/// the L2 envelope. Blob count/fees and expected nested results remain bound.
#[derive(Clone, PartialEq, Eq)]
struct PreparedReplay {
    tx: Recovered<TxEip4844>,
    verify_data: Vec<(Address, Bytes)>,
    endpoint: B256,
    rollup_id: u64,
    effects: EffectInspector,
}

#[derive(Default)]
struct ReplayCache {
    previous: Option<(PreparedReplay, u64)>,
    reuses: usize,
}

impl ReplayCache {
    fn measure(
        &mut self,
        prepared: PreparedReplay,
        execute: impl FnOnce(&PreparedReplay) -> Result<u64, FeeQuoteError>,
    ) -> Result<u64, FeeQuoteError> {
        if let Some((previous, gas)) = &self.previous
            && previous == &prepared
        {
            self.reuses += 1;
            return Ok(*gas);
        }
        let gas = execute(&prepared)?;
        self.previous = Some((prepared, gas));
        Ok(gas)
    }
}

/// Full-width hashes bound calldata pricing without pretending to know the
/// unsigned source's final block hash or KZG commitments. Distinct roots keep
/// the real nonzero-to-nonzero storage transition and carrier ordering.
fn placeholder(index: usize) -> Result<B256, FeeQuoteError> {
    let mut hash = B256::repeat_byte(0x11);
    hash[31] = u8::try_from(index + 1).map_err(|_| FeeQuoteError::Limit("settlement entries"))?;
    Ok(hash)
}

fn batch(
    proof_systems: Vec<Address>,
    input: &SettlementInput<'_>,
) -> Result<EvmBatch, FeeQuoteError> {
    if input.entries.is_empty() || input.entries.len() != input.traces.len() {
        return Err(FeeQuoteError::Unsupported("settlement trace coverage"));
    }
    let counts = eez_protocol::system_tx::outbound_settlement_source_counts(
        input.entries,
        input.groups.iter().map(OutboundSyncGroup::source_shape),
    )
    .map_err(unavailable)?;
    let effects = input
        .groups
        .iter()
        .flat_map(|g| &g.effects)
        .collect::<Vec<_>>();
    let prefix = placeholder(0)?;
    let checkpoints = (0..input.entries.len())
        .map(|i| placeholder(i + 1))
        .collect::<Result<Vec<_>, _>>()?;
    let (anchor, carriers) = eez_protocol::settlement::settlement_root_carriers(
        prefix,
        checkpoints
            .iter()
            .copied()
            .zip(input.entries.iter().map(|e| e.success)),
    );
    let mut result = EvmBatch::default();
    result.entries.push(ExecutionEntrySol {
        rootUpdates: vec![RootUpdateSol {
            rollupId: input.rollup_id,
            currentRoot: input.settled_root,
            newRoot: anchor,
            etherDelta: EtherDeltaSol::ZERO,
        }],
        destinationRollupId: input.rollup_id,
        success: true,
        ..Default::default()
    });
    let mut live = anchor;
    let mut cursor = 0;
    for (((entry, trace), count), new_root) in input
        .entries
        .iter()
        .zip(input.traces)
        .zip(counts)
        .zip(carriers)
    {
        let sources = effects[cursor..cursor + count]
            .iter()
            .map(|effect| &effect.source_entry)
            .collect::<Vec<_>>();
        let value = eez_protocol::entries::project_outbound_value(
            entry,
            trace,
            &sources,
            eez_protocol::RollupId(input.rollup_id),
        )
        .map_err(unavailable)?;
        let (inflow, outflow) = (value.l1_to_l2_value, value.l2_to_l1_value);
        let delta = I256::try_from(inflow)
            .ok()
            .and_then(|inflow| {
                I256::try_from(outflow)
                    .ok()
                    .and_then(|outflow| inflow.checked_sub(outflow))
            })
            .filter(|delta| {
                *delta >= I256::from(EtherDeltaSol::MIN) && *delta <= I256::from(EtherDeltaSol::MAX)
            })
            .ok_or(FeeQuoteError::Unsupported("settlement value overflow"))?;
        let mut entry = entry.clone();
        entry.destinationRollupId = input.rollup_id;
        entry.rootUpdates = vec![RootUpdateSol {
            rollupId: input.rollup_id,
            currentRoot: live,
            newRoot: new_root,
            etherDelta: EtherDeltaSol::from(delta),
        }];
        result.entries.push(entry);
        live = new_root;
        cursor += count;
    }
    let mut traces = vec![EntryTrace::default()];
    traces.extend_from_slice(input.traces);
    eez_protocol::entries::finalize_l1_entry_traces(&mut result, &traces).map_err(unavailable)?;
    result.immediateEntryCount = U256::from(result.entries.len());
    result.blockNumber = u64::MAX;
    result.expectedRootPerRollup = vec![eez_protocol::abi::ExpectedRootPerRollupSol {
        rollupId: input.rollup_id,
        root: input.settled_root,
    }];
    crate::attestation_quorum::assign_proof_systems(
        &mut result,
        input.rollup_id,
        proof_systems.clone(),
    );
    result.blobIndices = (0..input.blobs).map(U256::from).collect();
    result.proofs = vec![placeholder_proof(); proof_systems.len()];
    Ok(result)
}

/// Full-width valid curve input executes the verifier's real recovery path.
pub(in crate::composer) fn placeholder_proof() -> Bytes {
    let mut proof =
        b256!("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798").to_vec();
    proof.extend([0x11; 32]);
    proof.push(27);
    proof.into()
}

fn read_call<C: SolCall>(
    read: impl FnOnce(Address, Vec<u8>) -> Result<Bytes, FeeQuoteError>,
    to: Address,
    call: C,
) -> Result<C::Return, FeeQuoteError> {
    C::abi_decode_returns(&read(to, call.abi_encode())?).map_err(unavailable)
}

fn read_view(
    chain: &crate::local::provider::ChainProvider,
    provider: &dyn StateProvider,
    env: &reth_evm::EvmEnvFor<eez_evm::EezEvmConfig>,
    to: Address,
    calldata: Vec<u8>,
) -> Result<Bytes, FeeQuoteError> {
    let state = StateProviderDatabase::new(EvmStateProviderAdapter(provider));
    let mut env = EthEvmEnv::new_with_version(env.spec, env.block, env.version);
    let chain_id = env.version.chain_id;
    env.version
        .features
        .remove(EvmFeatures::NONCE_CHECK | EvmFeatures::BASE_FEE_CHECK);
    eez_evm::disable_fee_charging(&mut env.version);
    let mut evm = chain.evm_config.evm_with_env(state, env);
    let tx = Recovered::new_unchecked(
        eez_evm::EezEvmTx::Ethereum(TxEnvelope::Legacy(TxLegacy {
            to: TxKind::Call(to),
            input: calldata.into(),
            gas_limit: 1_000_000,
            chain_id: Some(chain_id),
            ..Default::default()
        })),
        Address::ZERO,
    );
    let result = evm.transact(&tx).map_err(unavailable)?.result;
    if !result.is_success() {
        return Err(FeeQuoteError::Unsupported("settlement deployment lookup"));
    }
    Ok(result.output)
}

pub(crate) fn proof_registration(
    chain: &crate::local::provider::ChainProvider,
    quorum: &crate::AttestationQuorum,
    registry: Address,
    rollup_id: u64,
    header: &alloy_consensus::Header,
) -> Result<crate::attestation_quorum::QuorumRegistration, eez_prover::ProverError> {
    let missing =
        || eez_prover::ProverError::Backend("local registration state unavailable".into());
    let hash = header.hash_slow();
    let env = chain.evm_config.evm_env(header).map_err(|_| missing())?;
    let state = chain
        .provider
        .state_by_block_hash(hash)
        .map_err(|_| missing())?;
    let read = |to, input| read_view(chain, state.as_ref(), &env, to, input);
    let call = rollupsCall {
        rollupId: rollup_id,
    };
    let output = read(registry, call.abi_encode()).map_err(|_| missing())?;
    let manager = rollupsCall::abi_decode_returns(&output)
        .map_err(|_| eez_prover::ProverError::Backend("invalid rollup registration".into()))?
        .rollupContract;
    quorum
        .local_registration(
            manager,
            |system| super::super::estimate::deployment::known_verifier(state.as_ref(), system),
            |system, input| read(system, input).map_err(|e| e.to_string()),
            |member, reason| {
                tracing::warn!(
                    name: "eez.composer.attestation.member_skipped",
                    event_name = "eez.composer.attestation.member_skipped",
                    proof_system = %member.proof_system,
                    attester = %member.attester,
                    %manager, reason, "an attester is not asked for this batch"
                );
            },
        )
        .map_err(|error| eez_prover::ProverError::Backend(error.to_string()))
}

fn prepare_snapshot(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    rollup_id: u64,
    settled_root: B256,
) -> Result<SnapshotPreparation, FeeQuoteError> {
    let provider = runtime
        .chain
        .provider
        .state_by_block_hash(posting.quote.sample.hash)
        .map_err(unavailable)?;
    let read = |to, input| read_view(&runtime.chain, provider.as_ref(), &posting.env, to, input);
    let registry = read_call(
        read,
        runtime.registry,
        rollupsCall {
            rollupId: rollup_id,
        },
    )?;
    if registry.root != settled_root {
        return Err(FeeQuoteError::Stale);
    }
    let proof_systems = match &runtime.quorum {
        Some(quorum) => quorum
            .usable_proof_systems(
                registry.rollupContract,
                |system| {
                    super::super::estimate::deployment::known_verifier(provider.as_ref(), system)
                },
                |to, input| read(to, input).map_err(|error| error.to_string()),
            )
            .map_err(unavailable)?,
        None => runtime.proof_systems.clone(),
    };
    super::super::estimate::deployment::validate_at(
        provider.as_ref(),
        runtime.registry,
        registry.rollupContract,
        &proof_systems,
        runtime.post_batch_target,
    )
    .map_err(unavailable)?;
    let keys = read_call(
        read,
        registry.rollupContract,
        checkProofSystemsAndGetVkeysCall {
            proofSystems: proof_systems.clone(),
        },
    )?;
    if keys.len() != proof_systems.len() || keys.is_empty() {
        return Err(FeeQuoteError::Unsupported("settlement proof profile"));
    }
    let account = provider
        .basic_account(&runtime.ctx.l1_poster_signer.address())
        .map_err(unavailable)?
        .unwrap_or_default();
    Ok(SnapshotPreparation {
        members: proof_systems.into_iter().zip(keys).collect(),
        poster_nonce: account.nonce,
        poster_balance: account.balance,
    })
}

fn prepare(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    snapshot: &SnapshotPreparation,
    input: &SettlementInput<'_>,
) -> Result<(PreparedReplay, crate::PosterBalanceError), FeeQuoteError> {
    let batch = batch(
        snapshot.members.iter().map(|(system, _)| *system).collect(),
        input,
    )?;
    let hashes = (0..input.blobs)
        .map(|i| {
            let mut hash = placeholder(i)?;
            hash[0] = 1;
            Ok(hash)
        })
        .collect::<Result<Vec<_>, FeeQuoteError>>()?;
    let context = eez_protocol::l1_history::ProofContext::new(
        U256::from(runtime.ctx.l2_chain_id),
        input.rollup_id,
        eez_protocol::l1_history::EvidenceAnchor::ActualParent {
            number: posting.parent.number,
            hash: posting.quote.sample.hash,
            target_timestamp: posting.attributes.timestamp,
        },
        posting.parent.timestamp,
    )
    .map_err(unavailable)?;
    let digests = crate::attestation_quorum::member_digests(
        &batch,
        &snapshot
            .members
            .iter()
            .map(|(_, key)| *key)
            .collect::<Vec<_>>(),
        &hashes,
        context,
    )
    .map_err(unavailable)?;
    let verify_data = batch
        .proofSystems
        .iter()
        .zip(&batch.proofs)
        .zip(digests)
        .map(|((&system, proof), digest)| {
            (
                system,
                verifyCall {
                    proof: proof.clone(),
                    publicInputsHash: digest,
                }
                .abi_encode()
                .into(),
            )
        })
        .collect();
    let poster = runtime.ctx.l1_poster_signer.address();
    // Only the balance is immutable. A fee-dependent call can change the blob
    // count, so reserve blob fees and derive affordable gas for every candidate.
    let funding = posting
        .probe_funding(poster, snapshot.poster_balance, input.blobs as u64)
        .map_err(unavailable)?;
    if funding.affordable_gas < 21_000 {
        return Err(FeeQuoteError::PosterFunds(Box::new(funding)));
    }
    let tx = Recovered::new_unchecked(
        TxEip4844 {
            to: runtime.post_batch_target,
            input: postAndVerifyBatchCall {
                batch: batch.clone(),
            }
            .abi_encode()
            .into(),
            nonce: snapshot.poster_nonce,
            gas_limit: funding.affordable_gas,
            max_fee_per_gas: posting.quote.max_fee_per_gas,
            max_priority_fee_per_gas: posting.quote.max_priority_fee_per_gas,
            max_fee_per_blob_gas: posting.quote.max_fee_per_blob_gas,
            blob_versioned_hashes: hashes,
            chain_id: runtime.ctx.l1_chain_id,
            ..Default::default()
        },
        poster,
    );
    let endpoint = batch
        .entries
        .last()
        .and_then(|entry| entry.rootUpdates.last())
        .ok_or(FeeQuoteError::Unsupported("settlement endpoint"))?
        .newRoot;
    let effects = EffectInspector::new(runtime.registry, input.rollup_id, input.semantics)
        .map_err(unavailable)?;
    Ok((
        PreparedReplay {
            tx,
            verify_data,
            endpoint,
            rollup_id: input.rollup_id,
            effects,
        },
        funding,
    ))
}

fn estimate(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    prepared: &PreparedReplay,
    funding: &crate::PosterBalanceError,
) -> Result<u64, FeeQuoteError> {
    let execute = |tx| replay(runtime, posting, prepared, tx, funding);
    let (used, required) = execute(prepared.tx.clone())?;
    let mut selected = prepared.tx.clone();
    selected.inner_mut().gas_limit = runtime
        .fees
        .declared_gas(required, funding.affordable_gas)
        .map_err(|error| {
            if funding.limits_probe() {
                FeeQuoteError::PosterFunds(Box::new(funding.clone()))
            } else {
                unavailable(error)
            }
        })?;
    if selected.gas_limit() == funding.affordable_gas {
        return Ok(used);
    }
    // As in final preflight, gas-sensitive code may need the larger verified
    // limit. Only a successful replay of all intended effects establishes cost.
    match execute(selected) {
        Ok((gas, _)) => Ok(gas),
        Err(FeeQuoteError::Unsupported(_)) => Ok(used),
        Err(error) => Err(error),
    }
}

fn replay(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    prepared: &PreparedReplay,
    tx: Recovered<TxEip4844>,
    funding: &crate::PosterBalanceError,
) -> Result<(u64, u64), FeeQuoteError> {
    let gas_limit = tx.gas_limit();
    let tx = tx.map(|tx| TxEnvelope::Eip4844(tx.into()));
    let provider = runtime
        .chain
        .provider
        .state_by_block_hash(posting.quote.sample.hash)
        .map_err(unavailable)?;
    let mut state = StateProviderDatabase::new(EvmStateProviderAdapter(provider));
    let mut inspector = MeteredProofSet::new(
        runtime.registry,
        prepared.verify_data.clone(),
        prepared.effects.clone(),
    )
    .map_err(unavailable)?;
    macro_rules! execute {
        ($config:expr, $parent:expr) => {{
            let config = $config;
            let context = config
                .context_for_next_block(&$parent, posting.attributes.clone())
                .map_err(unavailable)?;
            let env = EthEvmEnv::new_with_version(
                posting.env.spec,
                posting.env.block,
                posting.env.version,
            );
            let evm = config.evm_with_env(&mut state, env);
            let mut executor = config
                .block_executor_factory()
                .create_executor(evm, context);
            executor
                .apply_pre_execution_changes()
                .map_err(unavailable)?;
            // Reuse the pre-execution EVM: accepted native changes live in its overlay.
            let mut traced = crate::composer::quote_trace::inspect(
                &mut inspector,
                posting.env.version.chain_id,
                "settlement",
                &tx,
            );
            executor
                .evm_mut()
                .transact_with_inspector(&tx, &mut traced)
                .map_err(|error| {
                    if funding.explains(gas_limit, &error) {
                        FeeQuoteError::PosterFunds(Box::new(funding.clone()))
                    } else {
                        unavailable(error)
                    }
                })?
                .discard()
        }};
    }
    let result = if let Some(config) = &runtime.chain.gnosis_evm_config {
        execute!(
            config,
            SealedHeader::new(posting.parent.clone().into(), posting.quote.sample.hash)
        )
    } else {
        let config =
            reth_evm_ethereum::EthEvmConfig::new(runtime.chain.evm_config.chain_spec().clone());
        execute!(
            &config,
            SealedHeader::new(posting.parent.clone(), posting.quote.sample.hash)
        )
    };
    if funding.limits_probe()
        && gas_limit == funding.affordable_gas
        && result.stop == InstrStop::OutOfGas
    {
        return Err(FeeQuoteError::PosterFunds(Box::new(funding.clone())));
    }
    if !result.is_success() || !inspector.valid() {
        tracing::debug!(
            ?result,
            proofs = inspector.proofs.len(),
            "conditional settlement failed"
        );
        return Err(FeeQuoteError::Unsupported("conditional settlement failed"));
    }
    let mut last = None;
    for log in result
        .logs
        .iter()
        .filter(|log| log.address == runtime.registry)
    {
        if log.data.topics().first() == Some(&eez_protocol::abi::L2TxSkipped::SIGNATURE_HASH) {
            return Err(FeeQuoteError::Unsupported("settlement skipped an entry"));
        }
        if log.data.topics().first()
            == Some(&eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH)
        {
            let event =
                eez_protocol::abi::L2ExecutionPerformed::decode_log(log).map_err(unavailable)?;
            if event.rollupId == prepared.rollup_id {
                last = Some(event.newRoot);
            }
        }
    }
    if last != Some(prepared.endpoint) {
        return Err(FeeQuoteError::Unsupported("settlement endpoint mismatch"));
    }
    inspector.effects.verify().map_err(|error| {
        tracing::debug!(%error, "settlement fee effects mismatch");
        FeeQuoteError::Unsupported("settlement effects mismatch")
    })?;
    let gas = result;
    Ok((gas.tx_gas_used(), gas.total_gas_spent.max(gas.floor_gas)))
}

/// Conditional estimation must observe every configured verifier exactly once.
/// Each verifier still executes its real bytecode and gas path. This inspector
/// is confined to disposable estimation state, never signed preflight.
pub(in crate::composer) struct MeteredProofSet {
    proofs: Vec<MeteredProof>,
    effects: EffectInspector,
}

impl MeteredProofSet {
    pub(in crate::composer) fn new(
        registry: Address,
        calls: Vec<(Address, Bytes)>,
        effects: EffectInspector,
    ) -> Result<Self, String> {
        if calls.is_empty() || calls.len() > crate::attestation_quorum::MAX_ATTESTERS {
            return Err("invalid estimation proof set".into());
        }
        let proofs = calls
            .into_iter()
            .map(|(verifier, calldata)| MeteredProof {
                registry,
                verifier,
                calldata,
                matches: 0,
                completed: 0,
                invalid: false,
            })
            .collect();
        Ok(Self { proofs, effects })
    }

    pub(in crate::composer) fn valid(&self) -> bool {
        self.proofs.iter().all(MeteredProof::valid)
    }
}

/// Execute the actual verifier bytecode and preserve all its gas consumption.
/// Only its final boolean is assumed valid, scoped to one exact registry call.
/// This inspector is never installed on signed preflight or canonical execution.
pub(in crate::composer) struct MeteredProof {
    registry: Address,
    verifier: Address,
    calldata: Bytes,
    matches: usize,
    completed: usize,
    invalid: bool,
}

impl MeteredProof {
    pub(in crate::composer) fn valid(&self) -> bool {
        !self.invalid && self.matches == 1 && self.completed == 1
    }

    fn matches<T: EvmTypes>(&self, context: &Interpreter<'_, '_, T>, inputs: &Message<T>) -> bool {
        inputs.kind == MessageKind::StaticCall
            && inputs.is_static()
            && inputs.caller == self.registry
            && inputs.destination == self.verifier
            && inputs.code_address == self.verifier
            && *inputs.input.to_bytes(context.memory()) == self.calldata.as_ref()
    }
}

impl<T: EvmTypes> Inspector<T> for MeteredProofSet {
    fn inspector_config(&self) -> InspectorConfig {
        InspectorConfig::without_steps()
    }
    fn create(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        self.effects.create(context, inputs)
    }

    fn create_end(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &Message<T>,
        outcome: &mut MessageResult<T>,
    ) {
        self.effects.create_end(context, inputs, outcome);
    }

    fn call(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        if let Some(proof) = self
            .proofs
            .iter_mut()
            .find(|proof| proof.matches(context, inputs))
        {
            proof.matches += 1;
        }
        self.effects.call(context, inputs)
    }

    fn call_end(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &Message<T>,
        outcome: &mut MessageResult<T>,
    ) {
        if let Some(proof) = self
            .proofs
            .iter_mut()
            .find(|proof| proof.matches(context, inputs))
        {
            proof.completed += 1;
            let zero = U256::ZERO.to_be_bytes::<32>();
            let one = U256::from(1).to_be_bytes::<32>();
            if proof.matches != 1
                || proof.completed != 1
                || !outcome.stop.is_success()
                || (outcome.output.as_ref() != zero && outcome.output.as_ref() != one)
            {
                proof.invalid = true;
            } else {
                outcome.output = one.to_vec().into();
            }
        }
        self.effects.call_end(context, inputs, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId};
    use evm2::{
        BaseEvmTypes, SpecId, ethereum::ethereum_tx_registry, evm::InMemoryDB,
        precompiles::Precompiles,
    };

    fn snapshot_fixture(nonce: u64) -> SnapshotPreparation {
        SnapshotPreparation {
            members: vec![(Address::repeat_byte(2), B256::repeat_byte(7))],
            poster_nonce: nonce,
            poster_balance: U256::from(100),
        }
    }

    #[test]
    fn preparation_reuse_is_bound_to_rollup_root_and_estimator() {
        let root = B256::repeat_byte(1);
        let mut cache = PreparationCache::default();
        assert_eq!(
            cache
                .get_or_load(1, root, || Ok(snapshot_fixture(10)))
                .unwrap()
                .poster_nonce,
            10
        );
        let snapshot = cache
            .get_or_load(1, root, || panic!("same snapshot must reuse"))
            .unwrap();
        assert_eq!(
            snapshot.members,
            vec![(Address::repeat_byte(2), B256::repeat_byte(7))]
        );
        assert_eq!(snapshot.poster_balance, U256::from(100));

        // Equal roots do not authorize using another rollup's deployment/vkey.
        assert_eq!(
            cache
                .get_or_load(2, root, || Ok(snapshot_fixture(20)))
                .unwrap()
                .poster_nonce,
            20
        );
        // A changed expected root must reach the lookup and its stale check.
        assert!(matches!(
            cache.get_or_load(2, B256::repeat_byte(2), || Err(FeeQuoteError::Stale)),
            Err(FeeQuoteError::Stale)
        ));
        assert_eq!(
            cache
                .get_or_load(2, root, || panic!(
                    "failed lookup must not replace validated preparation"
                ))
                .unwrap()
                .poster_nonce,
            20
        );

        // A different quote/envelope starts with fresh deployment/account reads,
        // even when the rollup and root match the previous quote.
        assert_eq!(
            PreparationCache::default()
                .get_or_load(2, root, || Ok(snapshot_fixture(30)))
                .unwrap()
                .poster_nonce,
            30
        );
    }

    #[test]
    fn preparation_retries_failed_lookups() {
        let mut cache = PreparationCache::default();
        for _ in 0..2 {
            for error in [
                FeeQuoteError::Stale,
                FeeQuoteError::Unavailable("L1 settlement execution"),
                FeeQuoteError::Unsupported("settlement proof profile"),
            ] {
                assert!(cache.get_or_load(1, B256::ZERO, || Err(error)).is_err());
                assert!(cache.previous.is_none());
            }
        }
        assert_eq!(
            cache
                .get_or_load(1, B256::ZERO, || Ok(snapshot_fixture(1)))
                .unwrap()
                .poster_nonce,
            1
        );
    }

    fn prepared_fixture() -> PreparedReplay {
        PreparedReplay {
            tx: Recovered::new_unchecked(
                TxEip4844 {
                    gas_limit: 1_000_000,
                    input: Bytes::from_static(&[1, 2, 3]),
                    ..Default::default()
                },
                Address::ZERO,
            ),
            verify_data: vec![(Address::repeat_byte(2), Bytes::from_static(&[4, 5, 6]))],
            endpoint: B256::repeat_byte(7),
            rollup_id: 1,
            effects: callback_expectations(1, true),
        }
    }

    fn callback_expectations(output: u8, success: bool) -> EffectInspector {
        EffectInspector::new(
            Address::repeat_byte(1),
            1,
            &[SemanticTransaction {
                operations_version: Default::default(),
                origin_chain: RollupId(0),
                tx_data: Bytes::new(),
                actions: vec![ExecutedAction {
                    parent_index: None,
                    source_rollup_id: RollupId(0),
                    target_rollup_id: RollupId(1),
                    source_address: Address::repeat_byte(2),
                    target_address: Address::repeat_byte(3),
                    call_mode: CallMode::Mutable,
                    data: Bytes::from_static(&[4]),
                    value: U256::ZERO,
                    outcome: ExecutionOutcome::Resolved {
                        return_data: vec![output],
                        success,
                        gas_used: 100,
                    },
                    revert_spans: vec![],
                }],
            }],
        )
        .unwrap()
    }

    #[test]
    fn settlement_reuse_requires_transaction_proof_endpoint_and_callback_identity() {
        let original = prepared_fixture();
        for variant in 0..14 {
            let mut cache = ReplayCache::default();
            assert_eq!(cache.measure(original.clone(), |_| Ok(123)).unwrap(), 123);
            let mut changed = original.clone();
            match variant {
                0 => changed.tx.inner_mut().input = Bytes::from_static(&[9]),
                1 => changed.tx.inner_mut().gas_limit -= 1,
                2 => changed.tx.inner_mut().max_fee_per_gas += 1,
                3 => changed.tx.inner_mut().max_priority_fee_per_gas = 1,
                4 => changed
                    .tx
                    .inner_mut()
                    .blob_versioned_hashes
                    .push(B256::repeat_byte(1)),
                5 => changed.tx.inner_mut().max_fee_per_blob_gas += 1,
                6 => {
                    changed.tx =
                        Recovered::new_unchecked(changed.tx.into_inner(), Address::repeat_byte(9))
                }
                7 => changed.tx.inner_mut().nonce += 1,
                8 => changed.tx.inner_mut().to = Address::repeat_byte(9),
                9 => changed.verify_data[0].1 = Bytes::from_static(&[9]),
                10 => changed.endpoint = B256::repeat_byte(9),
                11 => changed.rollup_id += 1,
                // Even identical posting bytes cannot reuse validation of
                // different expected nested results or success polarity.
                12 => changed.effects = callback_expectations(9, true),
                _ => changed.effects = callback_expectations(1, false),
            }
            let mut executed = false;
            let gas = cache
                .measure(changed.clone(), |_| {
                    executed = true;
                    Ok(456)
                })
                .unwrap();
            assert!(executed, "changed input {variant} reused a measurement");
            assert_eq!(gas, 456);
            assert_eq!(cache.reuses, 0);
            assert_eq!(
                cache
                    .measure(changed, |_| panic!("identical successful input must reuse"))
                    .unwrap(),
                456
            );
            assert_eq!(cache.reuses, 1);
        }
    }

    #[test]
    fn settlement_reuse_never_memoizes_failed_replay() {
        let mut cache = ReplayCache::default();
        for _ in 0..2 {
            let mut executed = false;
            assert!(matches!(
                cache.measure(prepared_fixture(), |_| {
                    executed = true;
                    Err(FeeQuoteError::Unsupported("settlement effects mismatch"))
                }),
                Err(FeeQuoteError::Unsupported("settlement effects mismatch"))
            ));
            assert!(executed);
            assert!(cache.previous.is_none());
        }
        assert_eq!(cache.measure(prepared_fixture(), |_| Ok(123)).unwrap(), 123);
        assert_eq!(cache.reuses, 0);
        // Another quote/envelope owns an empty cache even for equal inputs.
        assert_eq!(
            ReplayCache::default()
                .measure(prepared_fixture(), |_| Ok(456))
                .unwrap(),
            456
        );
    }

    fn fixture() -> (
        MeteredProofSet,
        Message<BaseEvmTypes>,
        MessageResult<BaseEvmTypes>,
    ) {
        let registry = Address::repeat_byte(1);
        let verifier = Address::repeat_byte(2);
        let calldata: Bytes = verifyCall {
            proof: vec![0x11; 65].into(),
            publicInputsHash: B256::repeat_byte(3),
        }
        .abi_encode()
        .into();
        let inspector = MeteredProofSet::new(
            registry,
            vec![(verifier, calldata.clone())],
            EffectInspector::new(registry, 1, &[]).unwrap(),
        )
        .unwrap();
        let input = Message::<BaseEvmTypes> {
            input: calldata.into(),
            gas_limit: 100_000,
            reservoir: 0,
            code_address: verifier,
            destination: verifier,
            call_target: verifier,
            caller: registry,
            value: U256::ZERO,
            kind: MessageKind::StaticCall,
            caller_is_static: true,
            ..Default::default()
        };
        let outcome = MessageResult::<BaseEvmTypes> {
            stop: InstrStop::Return,
            output: U256::ZERO.to_be_bytes::<32>().to_vec().into(),
            gas: evm2::interpreter::GasTracker::new_used_gas(100_000, 4_321, 0),
            ..Default::default()
        };
        (inspector, input, outcome)
    }

    #[test]
    fn conditional_quorum_requires_each_distinct_verifier_once() {
        let mut host = evm2::Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            Default::default(),
            ethereum_tx_registry(SpecId::OSAKA),
            InMemoryDB::default(),
            Precompiles::base(SpecId::OSAKA),
        );
        let frame = Message::<BaseEvmTypes>::default();
        let mut context = Interpreter::<BaseEvmTypes>::new(&frame);
        context.prepare_run(&mut host);
        let (one, mut input, mut outcome) = fixture();
        let other = Address::repeat_byte(3);
        let mut inspector = MeteredProofSet::new(
            one.proofs[0].registry,
            vec![
                (one.proofs[0].verifier, one.proofs[0].calldata.clone()),
                (other, one.proofs[0].calldata.clone()),
            ],
            EffectInspector::new(one.proofs[0].registry, 1, &[]).unwrap(),
        )
        .unwrap();
        assert!(inspector.call(&mut context, &mut input).is_none());
        inspector.call_end(&mut context, &input, &mut outcome);
        assert!(!inspector.valid(), "one proof cannot satisfy the whole set");
        input.destination = other;
        input.code_address = other;
        assert!(inspector.call(&mut context, &mut input).is_none());
        inspector.call_end(&mut context, &input, &mut outcome);
        assert!(
            inspector.valid(),
            "every exact verifier call completed once"
        );
        inspector.call(&mut context, &mut input);
        inspector.call_end(&mut context, &input, &mut outcome);
        assert!(
            !inspector.valid(),
            "duplicate verifier execution must not count"
        );
    }

    #[test]
    fn proof_assumption_runs_verifier_and_preserves_its_metered_gas() {
        let mut host = evm2::Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            Default::default(),
            ethereum_tx_registry(SpecId::OSAKA),
            InMemoryDB::default(),
            Precompiles::base(SpecId::OSAKA),
        );
        let frame = Message::<BaseEvmTypes>::default();
        let mut context = Interpreter::<BaseEvmTypes>::new(&frame);
        context.prepare_run(&mut host);
        let (mut inspector, mut input, mut outcome) = fixture();
        let gas = outcome.gas;
        assert!(
            inspector.call(&mut context, &mut input).is_none(),
            "verifier must execute"
        );
        inspector.call_end(&mut context, &input, &mut outcome);
        assert_eq!(outcome.output.as_ref(), U256::from(1).to_be_bytes::<32>());
        assert_eq!(outcome.gas, gas, "assumption cannot erase verifier cost");
        assert_eq!(
            (
                inspector.proofs[0].matches,
                inspector.proofs[0].completed,
                inspector.proofs[0].invalid
            ),
            (1, 1, false)
        );
        assert!(inspector.call(&mut context, &mut input).is_none());
        inspector.call_end(&mut context, &input, &mut outcome);
        assert!(
            inspector.proofs[0].invalid,
            "repeated proof calls cannot authorize an estimate"
        );
    }

    #[test]
    fn proof_assumption_rejects_wrong_scope_reverts_and_malformed_returns() {
        let mut host = evm2::Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            Default::default(),
            ethereum_tx_registry(SpecId::OSAKA),
            InMemoryDB::default(),
            Precompiles::base(SpecId::OSAKA),
        );
        let frame = Message::<BaseEvmTypes>::default();
        let mut context = Interpreter::<BaseEvmTypes>::new(&frame);
        context.prepare_run(&mut host);
        for variant in 0..7 {
            let (mut inspector, mut input, mut outcome) = fixture();
            match variant {
                0 => input.caller = Address::ZERO,
                1 => input.destination = Address::ZERO,
                2 => input.code_address = Address::ZERO,
                3 => input.kind = MessageKind::Call,
                4 => {
                    input.kind = MessageKind::DelegateCall;
                    input.caller_is_static = false;
                }
                5 => input.input = Bytes::new().into(),
                _ => inspector.proofs[0].calldata = Bytes::new(),
            }
            assert!(inspector.call(&mut context, &mut input).is_none());
            inspector.call_end(&mut context, &input, &mut outcome);
            assert_eq!(inspector.proofs[0].matches, 0);
            assert_eq!(outcome.output.as_ref(), [0; 32]);
        }
        for variant in 0..3 {
            let (mut inspector, mut input, mut outcome) = fixture();
            match variant {
                0 => outcome.stop = InstrStop::Revert,
                1 => outcome.output = vec![1].into(),
                _ => outcome.output = U256::from(2).to_be_bytes::<32>().to_vec().into(),
            }
            let original = outcome.clone();
            inspector.call(&mut context, &mut input);
            inspector.call_end(&mut context, &input, &mut outcome);
            assert!(inspector.proofs[0].invalid);
            assert_eq!(outcome, original);
        }
    }
}
