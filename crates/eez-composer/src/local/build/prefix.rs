//! Candidate-local canonical prefix construction. Finalizing a block applies
//! post-execution changes, so only the state *before* `finish` is reusable.
use super::*;
use crate::local::provider::StateSnapshotProvider;
use eez_primitives::{EezPrimitives, Receipt};
use reth_evm::execute::BasicBlockBuilder;

struct Checkpoint {
    state: DraftDb,
    transactions: Vec<Recovered<TransactionSigned>>,
    receipts: Vec<Receipt>,
    cumulative_tx_gas_used: u64,
    block_regular_gas_used: u64,
    block_state_gas_used: u64,
    blob_gas_used: u64,
}

/// Reuses execution only inside one candidate's immutable block environment.
/// Exact raw-prefix equality is required, including when an earlier prospective
/// group was rejected. Final block and signer replay remain independent.
pub(crate) struct SyncPrefixHashes<'a> {
    provider: &'a dyn StateSnapshotProvider,
    config: &'a EezEvmConfig,
    parent: &'a SealedHeader<Header>,
    timestamp: u64,
    prev_randao: B256,
    recipient: Address,
    raw: Vec<Bytes>,
    checkpoint: Option<Checkpoint>,
    // Source preparation can run ahead of effect-hash requests within a bracket.
    // Retain scalar hashes of complete prefixes without rewinding the latest state.
    hashes: std::collections::BTreeMap<usize, B256>,
    pub(crate) executed_transactions: usize,
    pub(crate) elapsed: std::time::Duration,
}

impl<'a> SyncPrefixHashes<'a> {
    #[cfg(test)]
    pub(super) fn assert_state_matches_rebuild(&self) {
        let saved = self.checkpoint.as_ref().expect("checkpoint exists");
        let mut db = open_draft_db(self.provider, self.parent.hash(), None).unwrap();
        let attributes = next_block_attributes(
            self.config,
            self.timestamp,
            self.prev_randao,
            self.recipient,
        );
        let mut builder = self
            .config
            .builder_for_next_block(&mut db, self.parent, attributes)
            .unwrap();
        builder.apply_pre_execution_changes().unwrap();
        for (idx, raw) in self.raw.iter().enumerate() {
            builder
                .execute_transaction(recover_tx(raw, idx).unwrap())
                .unwrap();
        }
        assert_eq!(saved.receipts, builder.executor().receipts);
        assert_eq!(
            saved.cumulative_tx_gas_used,
            builder.executor().cumulative_tx_gas_used
        );
        assert_eq!(
            saved.block_regular_gas_used,
            builder.executor().block_regular_gas_used
        );
        assert_eq!(
            saved.block_state_gas_used,
            builder.executor().block_state_gas_used
        );
        assert_eq!(saved.blob_gas_used, builder.executor().blob_gas_used);
        let state = builder.evm_mut().db_mut();
        assert_eq!(saved.state.cache, state.cache);
        assert_eq!(saved.state.transition_state, state.transition_state);
        assert_eq!(saved.state.bundle_state, state.bundle_state);
        assert_eq!(saved.state.block_hashes, state.block_hashes);
        assert_eq!(saved.state.bal_state, state.bal_state);
    }

    pub(crate) fn new(
        provider: &'a dyn StateSnapshotProvider,
        config: &'a EezEvmConfig,
        parent: &'a SealedHeader<Header>,
        timestamp: u64,
        prev_randao: B256,
        recipient: Address,
    ) -> Self {
        Self {
            provider,
            config,
            parent,
            timestamp,
            prev_randao,
            recipient,
            raw: Vec::new(),
            checkpoint: None,
            hashes: Default::default(),
            executed_transactions: 0,
            elapsed: std::time::Duration::ZERO,
        }
    }

    /// Resolve each native identity from the executed prefix immediately before
    /// its whole group. A rejected candidate leaves the production draft intact;
    /// a later different raw prefix invalidates this cache's speculative suffix.
    pub(crate) fn prepare_transactions(
        &mut self,
        draft: &mut SyncBlockState,
        prefix: &[Bytes],
        plans: &[PlannedTransaction],
        sources: &[eez_protocol::native_source::SourceEntry],
    ) -> Result<Vec<Bytes>, (usize, BuildError)> {
        if plans
            .iter()
            .filter(|plan| matches!(plan, PlannedTransaction::Native(_)))
            .count()
            != sources.len()
        {
            return Err((
                0,
                BuildError::Builder("native source row count differs from plans".into()),
            ));
        }
        let mut sources = sources.iter();
        draft.prepare_transactions(plans, |suffix| {
            let full = prefix.iter().chain(suffix).cloned().collect::<Vec<_>>();
            let hash = self
                .prefix(&full)
                .map_err(|error| BuildError::SourcePrefix(error.to_string()))?;
            // Count equality above and one callback per Native plan establish this.
            let source = sources
                .next()
                .ok_or_else(|| BuildError::Builder("native source row missing".into()))?;
            Ok(source.hash(hash))
        })
    }

    pub(crate) fn at_positions(
        &mut self,
        transactions: &[Bytes],
        positions: &[usize],
    ) -> Result<Vec<B256>, BuildError> {
        if let Some(&position) = positions.iter().find(|&&p| p >= transactions.len()) {
            self.clear();
            return Err(BuildError::InvalidCheckpointPosition {
                position,
                tx_count: transactions.len(),
            });
        }
        let result: Result<Vec<B256>, BuildError> = positions
            .iter()
            .map(|&p| self.prefix(&transactions[..=p]))
            .collect();
        if result.is_err() {
            self.clear();
        }
        result
    }

    fn clear(&mut self) {
        self.checkpoint = None;
        self.raw.clear();
        self.hashes.clear();
    }

    pub(crate) fn prefix(&mut self, transactions: &[Bytes]) -> Result<B256, BuildError> {
        let started = std::time::Instant::now();
        let result = self.prefix_inner(transactions);
        self.elapsed += started.elapsed();
        if result.is_err() {
            self.clear();
        }
        result
    }

    fn prefix_inner(&mut self, transactions: &[Bytes]) -> Result<B256, BuildError> {
        if self.raw.starts_with(transactions)
            && let Some(hash) = self.hashes.get(&transactions.len())
        {
            return Ok(*hash);
        }
        if !transactions.starts_with(&self.raw) {
            self.clear();
        }

        let checkpoint = self.checkpoint.take();
        let mut db = open_draft_db(self.provider, self.parent.hash(), None)?;
        let (prior_transactions, prior_execution) = match checkpoint {
            Some(saved) => {
                db = saved.state;
                (
                    saved.transactions,
                    Some((
                        saved.receipts,
                        saved.cumulative_tx_gas_used,
                        saved.block_regular_gas_used,
                        saved.block_state_gas_used,
                        saved.blob_gas_used,
                    )),
                )
            }
            None => (Vec::new(), None),
        };
        let attributes = next_block_attributes(
            self.config,
            self.timestamp,
            self.prev_randao,
            self.recipient,
        );
        let env = self
            .config
            .next_evm_env(self.parent, &attributes)
            .map_err(|e| BuildError::Builder(format!("prefix next_evm_env: {e}")))?;
        let ctx = self
            .config
            .context_for_next_block(self.parent, attributes)
            .map_err(|e| BuildError::Builder(format!("prefix context: {e}")))?;
        let evm = self.config.evm_with_env(&mut db, env);
        let mut builder: BasicBlockBuilder<
            '_,
            <EezEvmConfig as ConfigureEvm>::BlockExecutorFactory,
            _,
            _,
            EezPrimitives,
        > = BasicBlockBuilder {
            executor: self.config.create_executor(evm, ctx.clone()),
            ctx,
            assembler: self.config.block_assembler(),
            parent: self.parent,
            transactions: prior_transactions,
        };
        if let Some((receipts, cumulative, regular, state, blob)) = prior_execution {
            builder.executor.receipts = receipts;
            builder.executor.cumulative_tx_gas_used = cumulative;
            builder.executor.block_regular_gas_used = regular;
            builder.executor.block_state_gas_used = state;
            builder.executor.blob_gas_used = blob;
        } else {
            builder
                .apply_pre_execution_changes()
                .map_err(|e| BuildError::Builder(format!("prefix pre-execution: {e}")))?;
        }
        for (idx, raw) in transactions
            .iter()
            .enumerate()
            .skip(builder.transactions.len())
        {
            builder
                .execute_transaction(recover_tx(raw, idx)?)
                .map_err(|e| BuildError::ExecuteTx {
                    idx,
                    msg: e.to_string(),
                })?;
            self.executed_transactions += 1;
        }

        // Keep every REVM state component, including transition history and
        // block-access tracking. Finalization mutates only the original db;
        // the saved copy remains at the next transaction's pre-state.
        let state = copy_state(
            builder.evm_mut().db_mut(),
            self.provider,
            self.parent.hash(),
        )?;
        let next = Checkpoint {
            state,
            transactions: builder.transactions.clone(),
            receipts: builder.executor.receipts.clone(),
            cumulative_tx_gas_used: builder.executor.cumulative_tx_gas_used,
            block_regular_gas_used: builder.executor.block_regular_gas_used,
            block_state_gas_used: builder.executor.block_state_gas_used,
            blob_gas_used: builder.executor.blob_gas_used,
        };
        let provider = self
            .provider
            .state_by_block_hash(self.parent.hash())
            .map_err(|e| BuildError::Provider(format!("prefix state provider: {e}")))?;
        let outcome = builder
            .finish(provider.as_ref(), None)
            .map_err(|e| BuildError::Builder(format!("prefix finish: {e}")))?;
        let hash = outcome.block.header().hash_slow();
        self.raw = transactions.to_vec();
        self.checkpoint = Some(next);
        self.hashes.insert(transactions.len(), hash);
        Ok(hash)
    }
}

fn copy_state(
    state: &DraftDb,
    provider: &dyn StateSnapshotProvider,
    parent: B256,
) -> Result<DraftDb, BuildError> {
    // Exhaustive destructuring makes newly added REVM state fields require an
    // explicit decision here when the pinned dependency is upgraded.
    let State {
        cache,
        database: _,
        transition_state,
        bundle_state,
        use_preloaded_bundle,
        block_hashes,
        bal_state,
        state_hook,
    } = state;
    // open_draft_db and this execution path install no hooks. A hook owns
    // non-cloneable observer state; silently dropping it would make reuse
    // differ from an independent rebuild if that invariant ever changes.
    if state_hook.is_some() {
        return Err(BuildError::Builder(
            "prefix checkpoint cannot copy a state hook".into(),
        ));
    }
    let database = provider
        .state_by_block_hash(parent)
        .map_err(|e| BuildError::Provider(format!("prefix snapshot provider: {e}")))?;
    Ok(State {
        cache: cache.clone(),
        database: StateProviderDatabase::new(EvmStateProviderAdapter(database)),
        transition_state: transition_state.clone(),
        bundle_state: bundle_state.clone(),
        use_preloaded_bundle: *use_preloaded_bundle,
        block_hashes: block_hashes.clone(),
        bal_state: bal_state.clone(),
        state_hook: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::test_utils::MockEthProvider;

    #[test]
    fn prefix_checkpoint_rejects_non_cloneable_state_hook() {
        let provider: MockEthProvider = MockEthProvider::new();
        let mut state = open_draft_db(&provider, B256::ZERO, None).unwrap();
        assert!(copy_state(&state, &provider, B256::ZERO).is_ok());
        state.set_state_hook(Some(Box::new(|_| {})));
        assert!(matches!(
            copy_state(&state, &provider, B256::ZERO),
            Err(BuildError::Builder(message))
                if message == "prefix checkpoint cannot copy a state hook"
        ));
        assert!(state.state_hook.is_some());
    }
}
