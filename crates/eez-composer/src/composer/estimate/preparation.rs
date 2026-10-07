use super::*;
use alloy_sol_types::SolCall;
use eez_protocol::{
    abi::{
        EtherDeltaSol, ExecutionEntrySol, ExpectedRootPerRollupSol, RollupIdWithProofSystemsSol,
        RootUpdateSol, rollupsCall,
    },
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
fn system_nonce(e: &GasEstimator, s: &Snapshot) -> Result<u64, EstimateError> {
    e.l2.chain_provider()
        .provider
        .state_by_block_hash(s.l2_parent.hash())
        .map_err(unavailable)?
        .account_nonce(&eez_primitives::SYSTEM_ADDRESS)
        .map(Option::unwrap_or_default)
        .map_err(unavailable)
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
    s: &Snapshot,
    c: &Composition,
    mut tx: TxEnv,
) -> Result<Probe, EstimateError> {
    let callbacks = sidecars(c, e.rollup_id)?;
    let counts = &c.source.callback_entry_counts;
    if counts.len() != c.source.batch.entries.len()
        || counts.iter().sum::<usize>() != callbacks.len()
    {
        return Err(unsupported("outbound callback partition"));
    }
    let mut cursor = 0;
    let mut effects = Vec::new();
    for (entry, count) in c.source.batch.entries.iter().zip(counts) {
        let end = cursor + count;
        effects.push(OutboundSyncEffect {
            source_entry: OutboundSourceEntry::Mutable(entry.clone()),
            callback_entries: callbacks[cursor..end].to_vec(),
        });
        cursor = end;
    }
    effects.extend(
        c.source
            .l2_static_entries
            .iter()
            .cloned()
            .map(|entry| OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Static(entry),
                callback_entries: vec![],
            }),
    );
    // The builder only carries these bytes through; unsigned execution below
    // never decodes this empty placeholder as a user transaction.
    let group = OutboundSyncGroup {
        effects,
        user_tx: Bytes::new(),
    };
    let pair = build_outbound_group(&group, &system_context(e), system_nonce(e, s)?)
        .map_err(unsupported)?;
    let mut draft = e.l2_draft(s)?;
    let setup = draft.execute_tx(&pair.system_tx).map_err(unavailable)?;
    if !setup.success {
        return Err(unsupported("L2 table setup reverted"));
    }
    let mut fork = draft.fork().map_err(unavailable)?;
    let mut observer = super::super::posting::EffectInspector::for_actions(
        e.l2.manager_address(),
        e.rollup_id,
        0,
        eez_protocol::EEZL2_AUTHORIZED_PROXIES_SLOT,
        &c.actions,
    )
    .map_err(unavailable)?;
    tx.gas_limit = tx.gas_limit.min(fork.remaining_gas());
    let limit = tx.gas_limit;
    let user = fork
        .execute_unsigned(tx, &mut observer)
        .map_err(unavailable)?;
    if !user.success {
        return Err(EstimateError::Revert(user.output));
    }
    let mut logs = user.logs.clone();
    for raw in &pair.followup_system_txs {
        let output = fork
            .execute_committed_inspected(raw, &mut observer)
            .map_err(unavailable)?;
        if !output.success {
            return Err(EstimateError::Effects(
                "L2 callback delivery reverted".into(),
            ));
        }
        logs.extend(output.logs);
    }
    observer
        .verify()
        .map_err(|e| EstimateError::Effects(e.to_string()))?;
    let observed =
        eez_protocol::outbound_gate::observations_from_logs(&logs, e.l2.manager_address());
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
    })
}

/// Prepare the same FIFO root trajectory as settlement, using actual native
/// delivery-prefix hashes. The fixture is private and contains no proof or DA.
pub(super) fn inbound(
    e: &GasEstimator,
    s: &Snapshot,
    c: &Composition,
) -> Result<(eez_protocol::EvmBatch, B256), EstimateError> {
    let sidecars = sidecars(c, e.rollup_id)?;
    let txs = build_inbound_system_txs(&sidecars, &system_context(e), system_nonce(e, s)?)
        .map_err(unsupported)?;
    let chain = e.l2.chain_provider();
    let mut hashes = crate::local::build::SyncPrefixHashes::new(
        chain.provider.as_ref(),
        &chain.evm_config,
        &s.l2_parent,
        s.l2_timestamp,
        s.l2_randao,
        e.recipient,
    );
    let prefix = hashes.prefix(&[]).map_err(unavailable)?;
    let checkpoints = hashes
        .at_positions(&txs, &(0..txs.len()).collect::<Vec<_>>())
        .map_err(unavailable)?;
    // Native hash construction does not by itself assert success.
    let mut draft = e.l2_draft(s)?;
    for tx in &txs {
        if !draft.execute_tx(tx).map_err(unavailable)?.success {
            return Err(unsupported("L2 delivery reverted"));
        }
    }
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
            let inflow = eez_protocol::entries::entry_flat_value_sum(target.as_abi_entry())
                .ok_or_else(|| unsupported("inbound value shape"))?;
            let outflow = eez_protocol::entries::entry_flat_value_sum(entry)
                .ok_or_else(|| unsupported("callback value shape"))?;
            ether_delta(inflow, outflow)?
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
    batch.proofSystems = vec![e.verifier];
    batch.rollupIdsWithProofSystems = vec![RollupIdWithProofSystemsSol {
        rollupId: e.rollup_id,
        proofSystemIndexes: vec![0],
    }];
    batch.proofs = vec![Bytes::new()];
    batch.blobIndices.clear();
    let registry = read_call(
        e,
        s,
        e.l1.manager_address(),
        rollupsCall {
            rollupId: e.rollup_id,
        },
    )?;
    super::deployment::validate(e, s, registry.rollupContract)?;
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
            proofSystems: vec![e.verifier],
        },
    )?;
    let [vkey] = keys.as_slice() else {
        return Err(unsupported("single proof-system profile required"));
    };
    let context = eez_protocol::public_inputs::L1PostingContext {
        chain_id: s.l1_env.cfg_env.chain_id,
        parent_number: s.l1.anchor.number,
        parent_hash: s.l1.anchor.hash(),
        target_timestamp: s.l1_attributes.timestamp,
    };
    let digests = eez_protocol::public_inputs::public_inputs_hashes(&batch, *vkey, context)
        .map_err(unsupported)?;
    let [digest] = digests.as_slice() else {
        return Err(unsupported("single proof digest required"));
    };
    Ok((batch, *digest))
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
    let (mut state, mut env) = s.l1.fork_state(&e.l1).map_err(unavailable)?;
    env.cfg_env.disable_nonce_check = true;
    env.cfg_env.disable_base_fee = true;
    env.cfg_env.disable_eip3607 = true;
    let chain_id = env.cfg_env.chain_id;
    let evm =
        e.l1.chain_provider()
            .evm_config
            .evm_with_env(&mut state, env);
    let output = execute_user(
        evm,
        TxEnv {
            kind: alloy_primitives::TxKind::Call(target),
            data: call.abi_encode().into(),
            gas_limit: 1_000_000,
            chain_id: Some(chain_id),
            ..Default::default()
        },
    )?;
    if !output.success {
        return Err(unsupported("deployment lookup reverted"));
    }
    C::abi_decode_returns(&output.output).map_err(unsupported)
}
