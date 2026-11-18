use super::{
    Address, B256, Bytes, Composition, ConfigureEvm, EstimateError, EvmFeatures, GasEstimator,
    Probe, Recovered, RecoveredTxEnvelope, RollupId, Snapshot, Transaction, TxEnvelope, TxLegacy,
    U256, set_source_gas_limit, unavailable, unsupported,
};
use alloy_sol_types::SolCall;
use eez_protocol::{
    abi::{EtherDeltaSol, ExecutionEntrySol, ExpectedRootPerRollupSol, RootUpdateSol, rollupsCall},
    entries::{EntryTrace, InboundSidecar},
    system_tx::{
        OutboundSourceEntry, OutboundSyncEffect, OutboundSyncGroup, SystemTxContext,
        build_inbound_system_txs, build_outbound_group,
    },
};

fn system_context(e: &GasEstimator) -> SystemTxContext {
    SystemTxContext {
        eezl2_address: e.l2.manager_address(),
        l2_chain_id: e.l2.chain_provider().evm_config.chain_spec().chain().id(),
        this_rollup_id: e.rollup_id,
    }
}

fn sidecars(c: &Composition, rid: u64) -> Result<Vec<InboundSidecar>, EstimateError> {
    c.targets
        .iter()
        .filter(|t| t.rollup_id == RollupId(rid))
        .flat_map(|t| &t.batch.entries)
        .map(|e| InboundSidecar::for_rollup(e, RollupId(rid)).map_err(unsupported))
        .collect()
}

pub(super) fn outbound(
    e: &GasEstimator,
    c: &Composition,
    mut tx: RecoveredTxEnvelope,
    mut fork: crate::local::build::SyncBlockFork,
) -> Result<Probe, EstimateError> {
    let mut effects = c
        .source
        .batch
        .entries
        .iter()
        .cloned()
        .map(|entry| OutboundSyncEffect {
            source_entry: OutboundSourceEntry::Mutable(entry),
        })
        .collect::<Vec<_>>();
    effects.extend(
        c.source
            .l2_static_entries
            .iter()
            .cloned()
            .map(|entry| OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Static(entry),
            }),
    );
    // The builder only carries these bytes through; unsigned execution below
    // never decodes this empty placeholder as a user transaction.
    let group = OutboundSyncGroup {
        effects,
        user_tx: Bytes::new(),
    };
    // Unsigned advisory execution has no signed origin occurrence. This
    // placeholder never enters a body, DA, proof request or settlement.
    let pair = build_outbound_group(&group, &system_context(e)).map_err(unsupported)?;
    let raw = fork
        .encode_plan(
            &eez_protocol::system_tx::PlannedTransaction::Native(pair.system_tx),
            B256::ZERO,
        )
        .map_err(unavailable)?;
    let setup = fork.execute_committed(&raw).map_err(unavailable)?;
    if !setup.success {
        return Err(unsupported("L2 table setup reverted"));
    }
    let mut observer = super::super::posting::EffectInspector::for_actions(
        e.l2.manager_address(),
        e.rollup_id,
        0,
        eez_protocol::EEZL2_AUTHORIZED_PROXIES_SLOT,
        &c.actions,
    )
    .map_err(unavailable)?;
    let limit = tx.gas_limit().min(fork.remaining_gas());
    set_source_gas_limit(&mut tx, limit)?;
    let user = fork
        .execute_unsigned(tx.map(eez_evm::EezEvmTx::Ethereum), &mut observer)
        .map_err(unavailable)?;
    if !user.success {
        return Err(EstimateError::Revert(user.output));
    }
    observer
        .verify()
        .map_err(|e| EstimateError::Effects(e.to_string()))?;
    let observed =
        eez_protocol::outbound_gate::observations_from_logs(&user.logs, e.l2.manager_address());
    eez_protocol::outbound_gate::verify_expected_outbound_hashes(
        &c.source.outbound_event_hashes,
        &observed,
    )
    .map_err(EstimateError::Effects)?;
    Ok(Probe {
        required: user.gas_required,
        limit,
        source_gas: user.gas_used,
        output: user.output,
        actions: c.actions.clone(),
        settlement: None,
        post_required: 0,
        post_limit: 0,
        post_funding: None,
        retained_l1: None,
        retained_l2: Vec::new(),
        retained_root: None,
    })
}

/// Prepare the same FIFO root trajectory as settlement, using actual native
/// delivery-prefix hashes. The fixture is private and contains no proof or DA.
pub(super) fn inbound(
    e: &GasEstimator,
    s: &Snapshot,
    c: &Composition,
    blob_hashes: &[B256],
) -> Result<(eez_protocol::EvmBatch, Vec<B256>, Vec<Bytes>, B256), EstimateError> {
    let sidecars = sidecars(c, e.rollup_id)?;
    let source_context = eez_protocol::native_source::LiveSourceContext::new(
        system_context(e).l2_chain_id,
        s.l1.anchor.hash(),
    )
    .map_err(unsupported)?;
    let semantics = eez_protocol::blob_semantics::SemanticTransaction {
        operations_version: Default::default(),
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::new(),
        actions: c.actions.clone(),
    };
    let actions = eez_protocol::blob_semantics::ReconstructionActions::from_semantics(
        &[semantics],
        RollupId(e.rollup_id),
        system_context(e).l2_chain_id,
    )
    .map_err(unsupported)?;
    let plans = build_inbound_system_txs(&sidecars, &system_context(e))
        .map_err(unsupported)?
        .into_iter()
        .map(eez_protocol::system_tx::PlannedTransaction::Native)
        .collect::<Vec<_>>();
    let chain = e.l2.chain_provider();
    let mut hashes = crate::local::build::SyncPrefixHashes::new(
        chain.provider.as_ref(),
        &chain.evm_config,
        &s.l2_parent,
        s.l2_timestamp,
        s.l2_randao,
        e.recipient,
    );
    let prefix = hashes.prefix(&s.l2_prefix).map_err(unavailable)?;
    let txs = e
        .l2_draft(s)?
        .prepare_transactions(
            &plans,
            &actions
                .source_entries(&source_context)
                .map_err(unsupported)?,
        )
        .map_err(|(_, error)| unavailable(error))?;
    let mut full = s.l2_prefix.clone();
    full.extend(txs.iter().cloned());
    let checkpoints = hashes
        .at_positions(&full, &(s.l2_prefix.len()..full.len()).collect::<Vec<_>>())
        .map_err(unavailable)?;
    let actions = eez_protocol::settlement::composition_fifo_actions(c).map_err(unsupported)?;
    let plan = eez_protocol::settlement::fifo_transaction_root_plan(prefix, &actions, &checkpoints)
        .map_err(unsupported)?;
    eez_protocol::settlement::fifo_observation_claims(c, &plan).map_err(unsupported)?;
    let mut batch = c.source.batch.clone();
    let mut traces = c.source.entry_traces.clone();
    let mut live = prefix;
    let mut source_index = 0;
    let mut target_index = 0;
    for action in &actions {
        let eez_protocol::settlement::FifoSourceAction::Mutable { applies } = action else {
            continue;
        };
        let entry = batch
            .entries
            .get_mut(source_index)
            .ok_or_else(|| unsupported("missing inbound entry"))?;
        let new_root = *plan
            .mutable_new_roots
            .get(source_index)
            .ok_or_else(|| unsupported("missing inbound root checkpoint"))?;
        let delta = if *applies {
            let target = sidecars
                .get(target_index)
                .ok_or_else(|| unsupported("missing inbound sidecar"))?;
            target_index += 1;
            let value = eez_protocol::entries::project_inbound_value(
                entry,
                &traces[source_index],
                target.as_abi_entry(),
                eez_protocol::RollupId(e.rollup_id),
            )
            .map_err(unsupported)?;
            ether_delta(value.l1_to_l2_value, value.l2_to_l1_value)?
        } else {
            EtherDeltaSol::ZERO
        };
        entry.rootUpdates = vec![RootUpdateSol {
            rollupId: e.rollup_id,
            currentRoot: live,
            newRoot: new_root,
            etherDelta: delta,
        }];
        live = new_root;
        source_index += 1;
    }
    if target_index != sidecars.len()
        || source_index != batch.entries.len()
        || plan.static_roots.len() != batch.staticEntries.len()
    {
        return Err(unsupported("inbound FIFO coverage"));
    }
    for (entry, root) in batch.staticEntries.iter_mut().zip(&plan.static_roots) {
        entry.expectedRoots = vec![ExpectedRootPerRollupSol {
            rollupId: e.rollup_id,
            root: *root,
        }];
    }
    batch.entries.insert(
        0,
        ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: e.rollup_id,
                currentRoot: s.settled_root,
                newRoot: prefix,
                etherDelta: EtherDeltaSol::ZERO,
            }],
            destinationRollupId: e.rollup_id,
            success: true,
            ..Default::default()
        },
    );
    traces.insert(0, EntryTrace::default());
    eez_protocol::entries::finalize_l1_entry_traces(&mut batch, &traces).map_err(unsupported)?;
    batch.immediateEntryCount = U256::from(1);
    batch.blockNumber = u64::MAX;
    batch.blobIndices = (0..blob_hashes.len()).map(U256::from).collect();
    let registry = read_call(
        e,
        s,
        e.l1.manager_address(),
        rollupsCall {
            rollupId: e.rollup_id,
        },
    )?;
    let members = match &e.quorum {
        Some(quorum) => {
            let chain = e.l1.chain_provider();
            let state = chain
                .provider
                .state_by_block_hash(s.l1.anchor.hash())
                .map_err(unavailable)?;
            quorum
                .usable_members(
                    registry.rollupContract,
                    |system| super::deployment::known_verifier(state.as_ref(), system),
                    |to, input| read_bytes(e, s, to, input).map_err(|error| error.to_string()),
                )
                .map_err(unavailable)?
        }
        None => Vec::new(),
    };
    let verifiers = if e.quorum.is_some() {
        members
            .iter()
            .map(|(system, _)| *system)
            .collect::<Vec<_>>()
    } else {
        e.verifiers.clone()
    };
    crate::attestation_quorum::assign_proof_systems(&mut batch, e.rollup_id, verifiers.clone());
    batch.proofs = vec![
        if blob_hashes.is_empty() {
            Bytes::new()
        } else {
            super::super::fees::settlement::placeholder_proof()
        };
        verifiers.len()
    ];
    super::deployment::validate(e, s, registry.rollupContract, &verifiers)?;
    if registry.root != s.settled_root {
        return Err(unavailable("settlement root changed"));
    }
    alloy_sol_types::sol! {
        function checkProofSystemsAndGetVkeys(address[] proofSystems) external view returns (bytes32[]);
    }
    let keys = read_call(
        e,
        s,
        registry.rollupContract,
        checkProofSystemsAndGetVkeysCall {
            proofSystems: verifiers.clone(),
        },
    )?;
    if keys.len() != verifiers.len() || keys.is_empty() {
        return Err(unsupported("invalid verification-key coverage"));
    }
    batch.expectedRootPerRollup = vec![ExpectedRootPerRollupSol {
        rollupId: e.rollup_id,
        root: s.settled_root,
    }];
    let context = eez_protocol::l1_history::ProofContext::new(
        U256::from(system_context(e).l2_chain_id),
        e.rollup_id,
        eez_protocol::l1_history::EvidenceAnchor::ActualParent {
            number: s.l1.anchor.number,
            hash: s.l1.anchor.hash(),
            target_timestamp: s.l1_attributes.timestamp,
        },
        s.l1.anchor.timestamp,
    )
    .map_err(unsupported)?;
    let digests = crate::attestation_quorum::member_digests(&batch, &keys, blob_hashes, context)
        .map_err(unsupported)?;
    Ok((batch, digests, txs, live))
}

fn ether_delta(inflow: U256, outflow: U256) -> Result<EtherDeltaSol, EstimateError> {
    let inflow = alloy_primitives::I256::try_from(inflow).map_err(unsupported)?;
    let outflow = alloy_primitives::I256::try_from(outflow).map_err(unsupported)?;
    let delta = inflow
        .checked_sub(outflow)
        .ok_or_else(|| unsupported("ether delta overflow"))?;
    if delta < alloy_primitives::I256::from(EtherDeltaSol::MIN)
        || delta > alloy_primitives::I256::from(EtherDeltaSol::MAX)
    {
        return Err(unsupported("ether delta exceeds int192"));
    }
    Ok(EtherDeltaSol::from(delta))
}

fn read_call<C: SolCall>(
    e: &GasEstimator,
    s: &Snapshot,
    target: Address,
    call: C,
) -> Result<C::Return, EstimateError> {
    let output = read_bytes(e, s, target, call.abi_encode())?;
    C::abi_decode_returns(&output).map_err(unsupported)
}

fn read_bytes(
    e: &GasEstimator,
    s: &Snapshot,
    to: Address,
    calldata: Vec<u8>,
) -> Result<Bytes, EstimateError> {
    let (mut state, mut env) = s.l1.fork_state(&e.l1).map_err(unavailable)?;
    env.version
        .features
        .remove(EvmFeatures::NONCE_CHECK | EvmFeatures::BASE_FEE_CHECK | EvmFeatures::EIP3607);
    let chain_id = env.version.chain_id;
    let tx = Recovered::new_unchecked(
        eez_evm::EezEvmTx::Ethereum(TxEnvelope::Legacy(TxLegacy {
            to: alloy_primitives::TxKind::Call(to),
            input: calldata.into(),
            gas_limit: 1_000_000,
            chain_id: Some(chain_id),
            ..Default::default()
        })),
        Address::ZERO,
    );
    let mut evm =
        e.l1.chain_provider()
            .evm_config
            .evm_with_env(&mut state.db, env);
    let output = evm.transact(&tx).map_err(unavailable)?.result;
    if !output.is_success() {
        return Err(unsupported("deployment lookup reverted"));
    }
    Ok(output.output)
}
