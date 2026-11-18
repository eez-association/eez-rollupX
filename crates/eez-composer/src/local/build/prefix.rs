//! Candidate-local canonical prefix construction. Finalizing a block applies
//! post-execution changes, so only the state *before* `finish` is reusable.
use super::{
    Address, B256, BlockBuilder, BuildError, Bytes, ConfigureEvm, EezEvmConfig,
    EvmStateProviderAdapter, Header, Recovered, SealedHeader, StateProviderDatabase,
    TransactionSigned, next_block_attributes, recover_tx,
};
use crate::local::provider::StateSnapshotProvider;
use eez_primitives::EezPrimitives;
use reth_evm::{BlockExecutorFactory, execute::BasicBlockBuilder};

struct Checkpoint {
    cache: evm2::evm::Cache,
    execution: eez_evm::block::EezBlockCheckpoint,
    transactions: Vec<Recovered<TransactionSigned>>,
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
        let state = self
            .provider
            .state_by_block_hash(self.parent.hash())
            .unwrap();
        let mut db = StateProviderDatabase::new(EvmStateProviderAdapter(state));
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
        let rebuilt = builder.executor().execution_checkpoint().unwrap();
        assert_eq!(saved.execution, rebuilt);
        assert_eq!(saved.cache, builder.evm_mut().overlay_db_mut().cache);
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
        let state = self
            .provider
            .state_by_block_hash(self.parent.hash())
            .map_err(|e| BuildError::Provider(format!("prefix state provider: {e}")))?;
        let mut db = StateProviderDatabase::new(EvmStateProviderAdapter(state));
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
        let evm = self.config.evm_with_env(&mut db, env.clone());
        let mut builder: BasicBlockBuilder<
            '_,
            <EezEvmConfig as ConfigureEvm>::BlockExecutorFactory,
            _,
            _,
            EezPrimitives,
        > = BasicBlockBuilder {
            executor: self
                .config
                .block_executor_factory()
                .create_executor(evm, ctx.clone()),
            ctx,
            evm_env: env,
            assembler: self.config.block_assembler(),
            parent: self.parent,
            transactions: Vec::new(),
        };
        if let Some(saved) = checkpoint {
            builder.evm_mut().overlay_db_mut().cache = saved.cache;
            builder
                .executor
                .restore_execution_checkpoint(saved.execution)
                .map_err(|e| BuildError::Builder(e.to_string()))?;
            builder.transactions = saved.transactions;
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

        builder
            .executor
            .validate_prefix()
            .map_err(|e| BuildError::Builder(e.to_string()))?;
        // The copy precedes terminal cleanup. This path installs neither state hooks
        // nor inspectors, and Osaka has no block-access-list builder to retain.
        let next = Checkpoint {
            cache: copy_execution_cache(builder.evm_mut())?,
            execution: builder
                .executor
                .execution_checkpoint()
                .map_err(|e| BuildError::Builder(e.to_string()))?,
            transactions: builder.transactions.clone(),
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

fn copy_execution_cache(evm: &mut eez_evm::EezEvm<'_>) -> Result<evm2::evm::Cache, BuildError> {
    if evm.state().bal_builder().is_some() || evm.inspector().is_some() {
        return Err(BuildError::Builder(
            "prefix checkpoint cannot copy execution observers".into(),
        ));
    }
    Ok(evm.overlay_db_mut().cache.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use evm2::evm::inspector::NoopInspector;

    #[test]
    fn prefix_checkpoint_rejects_execution_observers() {
        let config = EezEvmConfig::for_l1_simulation(reth_chainspec::MAINNET.clone());
        let env = reth_evm_ethereum::EthEvmEnv::new(evm2::SpecId::OSAKA, Default::default(), 1);
        let mut evm = config.evm_with_env(evm2::evm::EmptyDB::default(), env);
        assert!(copy_execution_cache(&mut evm).is_ok());
        evm.set_inspector(NoopInspector::default());
        assert!(
            matches!(copy_execution_cache(&mut evm), Err(BuildError::Builder(message))
            if message == "prefix checkpoint cannot copy execution observers")
        );
        assert!(evm.clear_inspector().is_some());
        evm.state_mut().enable_bal_builder();
        assert!(copy_execution_cache(&mut evm).is_err());
        assert!(evm.state().bal_builder().is_some());
    }
}
