//! L2 pool that refuses SYSTEM_ADDRESS txs and, on composers, known cross-chain
//! sources. They ride in Sync blocks over the engine API. This gate covers RPC
//! and Reth's reorg reinjection without duplicating composer ownership.

use std::{any::Any, fmt, sync::Arc};

use crate::pool_transaction::EezPooledTransaction;
use alloy_primitives::{Address, B256};
use eez_primitives::EezTxEnvelope as TransactionSigned;
use reth_chainspec::EthereumHardforks;
use reth_evm::ConfigureEvm;
use reth_node_api::{NodePrimitives, PrimitivesTy};
use reth_node_builder::{
    BuilderContext,
    components::{PoolBuilder, TxPoolBuilder},
    node::{FullNodeTypes, NodeTypes},
};
use reth_primitives_traits::SealedBlock;
use reth_transaction_pool::{
    CoinbaseTipOrdering, Pool, PoolTransaction, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidationTaskExecutor, TransactionValidator,
    blobstore::NoopBlobStore,
    error::{InvalidPoolTransactionError, PoolTransactionError},
    validate::EthTransactionValidator,
};
use tracing::{Level, event};

mod recovery;

/// reth's Ethereum pool with [`SystemAddressGate`] around its validator.
pub type EezTransactionPool<Client, S, Evm> = Pool<
    TransactionValidationTaskExecutor<
        SystemAddressGate<EthTransactionValidator<Client, EezPooledTransaction, Evm>>,
    >,
    CoinbaseTipOrdering<EezPooledTransaction>,
    S,
>;

#[derive(Debug, thiserror::Error)]
#[error("SYSTEM_ADDRESS transactions are not accepted via the mempool")]
struct SystemAddressRejected;

type CrossChainFilter = Arc<dyn Fn(B256) -> bool + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[error("cross-chain source must be executed in a composer Sync block")]
struct CrossChainSourceRejected;

impl PoolTransactionError for crate::payload::SequencingDataError {
    fn is_bad_transaction(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PoolTransactionError for CrossChainSourceRejected {
    fn is_bad_transaction(&self) -> bool {
        // A valid source reinserted after a reorg is not peer misbehavior.
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PoolTransactionError for SystemAddressRejected {
    fn is_bad_transaction(&self) -> bool {
        // Never valid from anyone, so the peer that sent it earns a penalty.
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Rejects L2 system transactions and optionally composer-owned sources that
/// Reth may reinject from an orphaned Sync. Ordinary users retain validation.
pub struct SystemAddressGate<V> {
    inner: V,
    system_address: Address,
    cross_chain: Option<CrossChainFilter>,
    data_guard: Option<Arc<crate::SequencingDataGuard>>,
}

impl<V: fmt::Debug> fmt::Debug for SystemAddressGate<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemAddressGate")
            .field("inner", &self.inner)
            .field("system_address", &self.system_address)
            .field("cross_chain_filter", &self.cross_chain.is_some())
            .finish()
    }
}

impl<V> SystemAddressGate<V> {
    pub const fn new(inner: V, system_address: Address) -> Self {
        Self {
            inner,
            system_address,
            cross_chain: None,
            data_guard: None,
        }
    }
}

impl<V> SystemAddressGate<V>
where
    V: TransactionValidator,
{
    /// The pool tx carries its recovered sender, so this is a compare, not an
    /// ECDSA recovery.
    fn is_system_sender(&self, tx: &V::Transaction) -> bool {
        self.system_address == tx.sender()
    }

    fn reject(tx: V::Transaction) -> TransactionValidationOutcome<V::Transaction> {
        event!(
            name: "eez.node.pool.system_tx_rejected",
            Level::WARN,
            tx_hash = %tx.hash(),
            sender = %tx.sender(),
            "rejected a SYSTEM_ADDRESS transaction offered to the L2 pool",
        );
        TransactionValidationOutcome::Invalid(
            tx,
            InvalidPoolTransactionError::other(SystemAddressRejected),
        )
    }

    fn is_cross_chain_source(&self, tx: &V::Transaction) -> bool {
        self.cross_chain
            .as_ref()
            .is_some_and(|filter| filter(*tx.hash()))
    }

    fn reject_cross_chain(tx: V::Transaction) -> TransactionValidationOutcome<V::Transaction> {
        event!(name: "eez.node.pool.cross_chain_source_rejected", Level::INFO,
            event_name = "eez.node.pool.cross_chain_source_rejected",
            tx_hash = %tx.hash(), sender = %tx.sender(),
            "cross-chain source remains under composer ownership after reorg");
        TransactionValidationOutcome::Invalid(
            tx,
            InvalidPoolTransactionError::other(CrossChainSourceRejected),
        )
    }

    fn reject_data(
        tx: V::Transaction,
        error: crate::payload::SequencingDataError,
    ) -> TransactionValidationOutcome<V::Transaction> {
        if matches!(error, crate::payload::SequencingDataError::Unavailable(_)) {
            TransactionValidationOutcome::Error(*tx.hash(), Box::new(error))
        } else {
            TransactionValidationOutcome::Invalid(tx, InvalidPoolTransactionError::other(error))
        }
    }
}

impl<V> TransactionValidator for SystemAddressGate<V>
where
    V: TransactionValidator,
    V::Transaction: PoolTransaction<Consensus = TransactionSigned>,
{
    type Transaction = V::Transaction;
    type Block = V::Block;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        if self.is_system_sender(&transaction) {
            return Self::reject(transaction);
        }
        if self.is_cross_chain_source(&transaction) {
            return Self::reject_cross_chain(transaction);
        }
        if let Some(guard) = &self.data_guard
            && let Err(error) = guard.validate_transaction(transaction.consensus_ref().inner())
        {
            return Self::reject_data(transaction, error);
        }
        self.inner.validate_transaction(origin, transaction).await
    }

    async fn validate_transactions(
        &self,
        transactions: impl IntoIterator<Item = (TransactionOrigin, Self::Transaction), IntoIter: Send>
        + Send,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        // One batch keeps the inner validator on a single state provider;
        // `slots` puts the results back in input order.
        let mut slots = Vec::new();
        let mut forwarded = Vec::new();
        for (origin, tx) in transactions {
            if self.is_system_sender(&tx) {
                slots.push(Some(Self::reject(tx)));
            } else if self.is_cross_chain_source(&tx) {
                slots.push(Some(Self::reject_cross_chain(tx)));
            } else if let Some(guard) = &self.data_guard
                && let Err(error) = guard.validate_transaction(tx.consensus_ref().inner())
            {
                slots.push(Some(Self::reject_data(tx, error)));
            } else {
                slots.push(None);
                forwarded.push((origin, tx));
            }
        }
        let mut inner = self
            .inner
            .validate_transactions(forwarded)
            .await
            .into_iter();
        // One outcome per input, in input order. A short inner result would
        // silently drop txs, so panic instead (invariant 7).
        slots
            .into_iter()
            .map(|slot| {
                slot.unwrap_or_else(|| {
                    inner
                        .next()
                        .expect("inner validator returned fewer outcomes than transactions")
                })
            })
            .collect()
    }

    fn on_new_head_block(&self, new_tip_block: &SealedBlock<Self::Block>) {
        self.inner.on_new_head_block(new_tip_block);
    }
}

/// Copy of [`reth_node_ethereum::node::EthereumPoolBuilder`] that wraps the
/// Ethereum validator in [`SystemAddressGate`].
#[derive(Clone)]
pub struct EezPoolBuilder {
    system_address: Address,
    cross_chain: Option<CrossChainFilter>,
    data_guard: Option<Arc<crate::SequencingDataGuard>>,
}

impl fmt::Debug for EezPoolBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EezPoolBuilder")
            .field("system_address", &self.system_address)
            .field("cross_chain_filter", &self.cross_chain.is_some())
            .finish()
    }
}

impl EezPoolBuilder {
    pub const fn new(system_address: Address) -> Self {
        Self {
            system_address,
            cross_chain: None,
            data_guard: None,
        }
    }

    /// Consult the composer's existing source records during pool validation,
    /// including Reth's asynchronous reorg reinjection. Followers omit this.
    #[must_use]
    pub fn with_cross_chain_filter(
        mut self,
        filter: impl Fn(B256) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.cross_chain = Some(Arc::new(filter));
        self
    }

    #[must_use]
    pub fn with_data_guard(mut self, guard: Option<Arc<crate::SequencingDataGuard>>) -> Self {
        self.data_guard = guard;
        self
    }
}

impl<Types, Node, Evm> PoolBuilder<Node, Evm> for EezPoolBuilder
where
    Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Primitives: NodePrimitives<SignedTx = TransactionSigned>,
        >,
    Node: FullNodeTypes<Types = Types>,
    Evm: ConfigureEvm<Primitives = PrimitivesTy<Types>> + Clone + 'static,
{
    type Pool = EezTransactionPool<Node::Provider, NoopBlobStore, Evm>;

    // Nothing here awaits, so `async fn` would build a state machine for no
    // reason; an `async move` body instead would just trip `manual_async_fn`.
    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
        evm_config: Evm,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> + Send {
        // Closure only so the body keeps using `?`.
        let build = move || -> eyre::Result<Self::Pool> {
            let pool_config = ctx.pool_config();

            let blob_store = NoopBlobStore::default();

            let eth_validator =
                TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone(), evm_config)
                    .no_eip4844()
                    .with_max_tx_input_bytes(ctx.config().txpool.max_tx_input_bytes)
                    .with_local_transactions_config(pool_config.local_transactions_config.clone())
                    .set_tx_fee_cap(ctx.config().rpc.rpc_tx_fee_cap)
                    .with_max_tx_gas_limit(ctx.config().txpool.max_tx_gas_limit)
                    .with_minimum_priority_fee(ctx.config().txpool.minimum_priority_fee)
                    .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
                    .build_with_tasks(ctx.task_executor().clone(), blob_store);

            let system_address = self.system_address;
            let validator = eth_validator.map(|inner| {
                let mut gate = SystemAddressGate::new(inner, system_address);
                gate.cross_chain = self.cross_chain.clone();
                gate.data_guard = self.data_guard.clone();
                gate
            });

            let transaction_pool = TxPoolBuilder::new(ctx)
                .with_validator(validator)
                .build(blob_store, pool_config.clone());
            // Only Reth's maintenance task receives the recovery view. Public
            // ingress retains the original pool and its external sender limits.
            reth_node_builder::components::spawn_maintenance_tasks(
                ctx,
                recovery::RecoveryPool(transaction_pool.clone()),
                &pool_config,
            )?;

            event!(
                name: "eez.node.pool.ready",
                Level::INFO,
                %system_address,
                "L2 transaction pool initialized with the SYSTEM_ADDRESS gate",
            );

            Ok(transaction_pool)
        };
        std::future::ready(build())
    }
}

#[cfg(test)]
mod tests {
    use alloy_consensus::{TxLegacy, transaction::Recovered};
    use alloy_primitives::{Signature, TxKind, U256, address};
    use reth_ethereum_primitives::Transaction;
    use reth_transaction_pool::validate::ValidTransaction;

    use super::*;

    const SYSTEM: Address = address!("1111111111111111111111111111111111111111");
    const USER: Address = address!("2222222222222222222222222222222222222222");

    #[derive(Debug)]
    struct AcceptAll;

    impl TransactionValidator for AcceptAll {
        type Transaction = EezPooledTransaction;
        type Block = eez_primitives::Block;

        fn validate_transaction(
            &self,
            _origin: TransactionOrigin,
            transaction: Self::Transaction,
        ) -> impl Future<Output = TransactionValidationOutcome<Self::Transaction>> + Send {
            std::future::ready(TransactionValidationOutcome::Valid {
                balance: U256::MAX,
                state_nonce: 0,
                bytecode_hash: None,
                transaction: ValidTransaction::Valid(transaction),
                propagate: false,
                authorities: None,
            })
        }
    }

    fn pooled_tx(sender: Address, nonce: u64) -> EezPooledTransaction {
        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: 1_000_000_000,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Default::default(),
        });
        // The sender is asserted, so the signature never has to be valid.
        let signed = reth_ethereum_primitives::TransactionSigned::new_unhashed(
            tx,
            Signature::new(U256::ONE, U256::ONE, false),
        );
        EezPooledTransaction::try_from_consensus(Recovered::new_unchecked(signed.into(), sender))
            .unwrap()
    }

    #[tokio::test]
    async fn unavailable_da_limits_are_retryable_and_never_admit_ordinary_data() {
        let mut gate = SystemAddressGate::new(AcceptAll, SYSTEM);
        gate.data_guard = Some(Arc::new(crate::SequencingDataGuard::new(1)));
        assert!(
            gate.validate_transaction(TransactionOrigin::Local, pooled_tx(USER, 0))
                .await
                .is_error()
        );
        let outcomes = gate
            .validate_transactions([
                (TransactionOrigin::Local, pooled_tx(USER, 0)),
                (TransactionOrigin::External, pooled_tx(SYSTEM, 0)),
                (TransactionOrigin::External, pooled_tx(USER, 1)),
            ])
            .await;
        assert!(outcomes[0].is_error());
        assert!(outcomes[1].is_invalid());
        assert!(outcomes[2].is_error());
        let too_large = crate::payload::SequencingDataError::TooLarge { size: 2, limit: 1 };
        assert!(!too_large.is_bad_transaction());
        assert!(
            SystemAddressGate::<AcceptAll>::reject_data(pooled_tx(USER, 0), too_large).is_invalid()
        );
    }

    #[tokio::test]
    async fn rejects_system_sender_and_admits_others() {
        let gate = SystemAddressGate::new(AcceptAll, SYSTEM);

        let out = gate
            .validate_transaction(TransactionOrigin::External, pooled_tx(SYSTEM, 0))
            .await;
        assert!(matches!(
            out.as_invalid(),
            Some(InvalidPoolTransactionError::Other(_))
        ));

        let out = gate
            .validate_transaction(TransactionOrigin::Local, pooled_tx(USER, 0))
            .await;
        assert!(out.is_valid());
    }

    #[tokio::test]
    async fn batch_keeps_input_order() {
        let gate = SystemAddressGate::new(AcceptAll, SYSTEM);
        let batch = vec![
            (TransactionOrigin::External, pooled_tx(USER, 0)),
            (TransactionOrigin::External, pooled_tx(SYSTEM, 1)),
            (TransactionOrigin::External, pooled_tx(USER, 2)),
        ];
        let hashes: Vec<_> = batch.iter().map(|(_, tx)| *tx.hash()).collect();

        let out = gate.validate_transactions(batch).await;

        assert_eq!(
            out.iter()
                .map(TransactionValidationOutcome::tx_hash)
                .collect::<Vec<_>>(),
            hashes
        );
        assert!(out[0].is_valid());
        assert!(out[1].is_invalid());
        assert!(out[2].is_valid());
    }

    #[tokio::test]
    async fn cross_chain_filter_covers_reinjection_and_local_submission_without_blocking_ordinary_users()
     {
        let reserved = *pooled_tx(USER, 1).hash();
        let mut gate = SystemAddressGate::new(AcceptAll, SYSTEM);
        gate.cross_chain = Some(Arc::new(move |hash| hash == reserved));
        for origin in [TransactionOrigin::External, TransactionOrigin::Local] {
            let out = gate.validate_transaction(origin, pooled_tx(USER, 1)).await;
            assert!(out.is_invalid());
        }
        let out = gate
            .validate_transactions([
                (TransactionOrigin::External, pooled_tx(USER, 0)),
                (TransactionOrigin::External, pooled_tx(USER, 1)),
                (TransactionOrigin::External, pooled_tx(SYSTEM, 2)),
                (TransactionOrigin::External, pooled_tx(USER, 3)),
            ])
            .await;
        assert_eq!(
            out.iter()
                .map(TransactionValidationOutcome::is_valid)
                .collect::<Vec<_>>(),
            [true, false, false, true]
        );
        assert!(!CrossChainSourceRejected.is_bad_transaction());
    }

    #[tokio::test]
    async fn recovery_preserves_large_sender_suffix_without_relaxing_public_admission() {
        use reth_transaction_pool::{PoolConfig, TransactionPool};
        for count in [16, 17, 20] {
            let pool = Pool::new(
                SystemAddressGate::new(AcceptAll, SYSTEM),
                CoinbaseTipOrdering::default(),
                NoopBlobStore::default(),
                PoolConfig::default(),
            );
            let txs: Vec<_> = (0..count).map(|nonce| pooled_tx(USER, nonce)).collect();
            let hashes: Vec<_> = txs.iter().map(|tx| *tx.hash()).collect();
            assert!(
                pool.add_transactions(TransactionOrigin::Local, txs.clone())
                    .await
                    .iter()
                    .all(Result::is_ok)
            );
            assert_eq!(
                pool.remove_transactions(hashes.clone()).len(),
                count as usize
            );

            // Baseline mechanism: the unwrapped public pool still applies its
            // per-sender external limit. This is not a global cap increase.
            let external = pool.add_external_transactions(txs.clone()).await;
            assert_eq!(
                external.iter().filter(|r| r.is_ok()).count(),
                (count as usize).min(16)
            );
            assert!(
                external
                    .iter()
                    .filter_map(|r| r.as_ref().err())
                    .all(|error| matches!(
                        error.kind,
                        reth_transaction_pool::error::PoolErrorKind::SpammerExceededCapacity(_)
                    ))
            );
            assert_eq!(
                pool.remove_transactions(hashes.clone()).len(),
                (count as usize).min(16)
            );

            let recovery = recovery::RecoveryPool(pool.clone());
            assert!(
                recovery
                    .add_external_transactions(txs)
                    .await
                    .iter()
                    .all(Result::is_ok)
            );
            assert!(hashes.iter().all(|hash| pool.contains(hash)));
            // No nonce gap remains; every restored predecessor is executable.
            assert_eq!(pool.pending_transactions().len(), count as usize);
        }
    }

    #[tokio::test]
    async fn recovery_revalidates_and_preserves_composer_ownership() {
        use reth_transaction_pool::{PoolConfig, TransactionPool};
        let cross_chain = *pooled_tx(USER, 1).hash();
        let mut gate = SystemAddressGate::new(AcceptAll, SYSTEM);
        gate.cross_chain = Some(Arc::new(move |hash| hash == cross_chain));
        let pool = Pool::new(
            gate,
            CoinbaseTipOrdering::default(),
            NoopBlobStore::default(),
            PoolConfig::default(),
        );
        let recovery = recovery::RecoveryPool(pool.clone());
        let result = recovery
            .add_external_transactions(vec![
                pooled_tx(USER, 0),
                pooled_tx(USER, 1),
                pooled_tx(SYSTEM, 0),
            ])
            .await;
        assert!(result[0].is_ok());
        assert!(matches!(
            &result[1].as_ref().unwrap_err().kind,
            reth_transaction_pool::error::PoolErrorKind::InvalidTransaction(error)
                if error.is_other::<CrossChainSourceRejected>()
        ));
        assert!(matches!(
            &result[2].as_ref().unwrap_err().kind,
            reth_transaction_pool::error::PoolErrorKind::InvalidTransaction(error)
                if error.is_other::<SystemAddressRejected>()
        ));
        assert!(!pool.contains(&cross_chain));
        assert!(pool.get_transactions_by_sender(SYSTEM).is_empty());

        // Repeated notifications do not replace an already-owned transaction.
        let duplicate = recovery
            .add_external_transactions(vec![pooled_tx(USER, 0)])
            .await;
        assert!(matches!(
            duplicate[0].as_ref().unwrap_err().kind,
            reth_transaction_pool::error::PoolErrorKind::AlreadyImported
        ));
        assert_eq!(pool.pool_size().total, 1);
    }

    #[tokio::test]
    async fn recovery_respects_explicit_no_local_exemptions() {
        use reth_transaction_pool::{LocalTransactionConfig, PoolConfig, TransactionPool};
        let config = PoolConfig {
            local_transactions_config: LocalTransactionConfig {
                no_exemptions: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let pool = Pool::new(
            SystemAddressGate::new(AcceptAll, SYSTEM),
            CoinbaseTipOrdering::default(),
            NoopBlobStore::default(),
            config,
        );
        let results = recovery::RecoveryPool(pool)
            .add_external_transactions((0..20).map(|nonce| pooled_tx(USER, nonce)).collect())
            .await;
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 16);
        assert!(
            results
                .iter()
                .filter_map(|r| r.as_ref().err())
                .all(|error| matches!(
                    error.kind,
                    reth_transaction_pool::error::PoolErrorKind::SpammerExceededCapacity(_)
                ))
        );
    }

    #[tokio::test]
    async fn recovery_unblocks_successors_accepted_while_predecessors_were_mined() {
        use reth_transaction_pool::{PoolConfig, TransactionPool};
        let pool = Pool::new(
            SystemAddressGate::new(AcceptAll, SYSTEM),
            CoinbaseTipOrdering::default(),
            NoopBlobStore::default(),
            PoolConfig::default(),
        );
        // The rolled-back state needs nonce zero, but newer local arrivals are
        // already queued. Recovery must not compete with them for 16 slots.
        assert!(
            pool.add_transactions(
                TransactionOrigin::Local,
                (20..64).map(|nonce| pooled_tx(USER, nonce)).collect(),
            )
            .await
            .iter()
            .all(Result::is_ok)
        );
        assert_eq!(pool.queued_transactions().len(), 44);
        let results = recovery::RecoveryPool(pool.clone())
            .add_external_transactions((0..20).map(|nonce| pooled_tx(USER, nonce)).collect())
            .await;
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(pool.pending_transactions().len(), 64);
        assert!(pool.queued_transactions().is_empty());
    }
}
