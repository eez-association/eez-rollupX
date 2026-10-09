//! Maintenance-only pool view. Never hand this view to RPC or networking.
//!
//! Pinned Reth reinjects an orphaned block's transactions through
//! `add_external_transactions`, losing their original admission origin. That
//! applies the external sender cap to already executed local transactions and
//! can strand a nonce suffix. Composer opts into local treatment for that call;
//! followers retain upstream behavior. Origin cannot be recovered from a block,
//! so this selects all local policies, including the RPC fee cap, backup and
//! eviction/propagation rules, not just a sender-cap exemption. Validation,
//! replacement rules, and explicit no-local exemptions still apply.

use std::sync::Arc;

use alloy_consensus::transaction::Recovered;
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
    eip7594::{BlobCellMask, BlobTransactionSidecarVariant},
};
use alloy_primitives::{Address, B256, TxHash, map::AddressSet};
use reth_eth_wire_types::HandleMempoolData;
use reth_execution_types::ChangedAccount;
use reth_transaction_pool::{
    AddedTransactionOutcome, AllPoolTransactions, AllTransactionsEvents, BestTransactions,
    BestTransactionsAttributes, BlobStore, BlobStoreError, BlockInfo, CanonicalStateUpdate,
    GetPooledTransactionLimit, NewBlobSidecar, NewTransactionEvent, PoolResult, PoolSize,
    PoolTransaction, PropagatedTransactions, TransactionEvents, TransactionListenerKind,
    TransactionOrigin, TransactionPool, TransactionPoolExt, ValidPoolTransaction,
};
use tokio::sync::mpsc::Receiver;

#[derive(Clone, Debug)]
pub(super) struct RecoveryPool<P>(pub(super) P);

// Keep the adapter limited to one overridden operation. All reads, canonical
// updates, backups, and other insertions retain the underlying pool's behavior.
// Re-audit trait defaults and reinsertion entry points when updating pinned Reth.
macro_rules! forward {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) $(-> $ret:ty)?;)*) => {
        $(fn $name(&self $(, $arg: $ty)*) $(-> $ret)? { self.0.$name($($arg),*) })*
    };
}

impl<P: TransactionPool> TransactionPool for RecoveryPool<P> {
    type Transaction = P::Transaction;

    async fn add_external_transactions(
        &self,
        transactions: Vec<Self::Transaction>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        let results = self
            .0
            .add_transactions(TransactionOrigin::Local, transactions)
            .await;
        // Upstream maintenance ignores the returned errors. Preserve visibility
        // of real validation/replacement failures, without pretending they were
        // recovered or retrying invalid transactions indefinitely.
        for error in results.iter().filter_map(|result| result.as_ref().err()) {
            tracing::warn!(name: "eez.node.pool.recovery_rejected",
                event_name = "eez.node.pool.recovery_rejected",
                tx_hash = %error.hash,
                error = %error, "displaced transaction could not reenter the pool");
        }
        results
    }

    forward! {
        fn pool_size(&self) -> PoolSize;
        fn block_info(&self) -> BlockInfo;
        fn add_transaction_and_subscribe(&self, origin: TransactionOrigin, transaction: Self::Transaction) -> impl Future<Output = PoolResult<TransactionEvents>> + Send;
        fn add_transaction(&self, origin: TransactionOrigin, transaction: Self::Transaction) -> impl Future<Output = PoolResult<AddedTransactionOutcome>> + Send;
        fn add_transactions(&self, origin: TransactionOrigin, transactions: Vec<Self::Transaction>) -> impl Future<Output = Vec<PoolResult<AddedTransactionOutcome>>> + Send;
        fn add_transactions_with_origins(&self, transactions: Vec<(TransactionOrigin, Self::Transaction)>) -> impl Future<Output = Vec<PoolResult<AddedTransactionOutcome>>> + Send;
        fn transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents>;
        fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction>;
        fn pending_transactions_listener_for(&self, kind: TransactionListenerKind) -> Receiver<TxHash>;
        fn blob_transaction_sidecars_listener(&self) -> Receiver<NewBlobSidecar>;
        fn new_transactions_listener_for(&self, kind: TransactionListenerKind) -> Receiver<NewTransactionEvent<Self::Transaction>>;
        fn pooled_transaction_hashes(&self) -> Vec<TxHash>;
        fn pooled_transaction_hashes_max(&self, max: usize) -> Vec<TxHash>;
        fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn pooled_transactions_max(&self, max: usize) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_pooled_transaction_elements(&self, tx_hashes: Vec<TxHash>, limit: GetPooledTransactionLimit) -> Vec<<Self::Transaction as PoolTransaction>::Pooled>;
        fn get_pooled_transaction_element(&self, tx_hash: TxHash) -> Option<Recovered<<Self::Transaction as PoolTransaction>::Pooled>>;
        fn best_transactions(&self) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;
        fn best_transactions_with_attributes(&self, best_transactions_attributes: BestTransactionsAttributes) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;
        fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn pending_transactions_max(&self, max: usize) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn pending_and_queued_txn_count(&self) -> (usize, usize);
        fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction>;
        fn all_transactions_by_sender(&self, sender: Address) -> AllPoolTransactions<Self::Transaction>;
        fn all_transaction_hashes(&self) -> Vec<TxHash>;
        fn remove_transactions(&self, hashes: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn remove_transactions_and_descendants(&self, hashes: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn remove_transactions_by_sender(&self, sender: Address) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn prune_transactions(&self, hashes: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_all(&self, txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn on_propagated(&self, txs: PropagatedTransactions);
        fn get_transactions_by_sender(&self, sender: Address) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_pending_transactions_by_sender(&self, sender: Address) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_queued_transactions_by_sender(&self, sender: Address) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_highest_transaction_by_sender(&self, sender: Address) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_highest_consecutive_transaction_by_sender(&self, sender: Address, on_chain_nonce: u64) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_transaction_by_sender_and_nonce(&self, sender: Address, nonce: u64) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_transactions_by_origin(&self, origin: TransactionOrigin) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_pending_transactions_by_origin(&self, origin: TransactionOrigin) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn unique_senders(&self) -> AddressSet;
        fn get_blob(&self, tx_hash: TxHash) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;
        fn get_all_blobs(&self, tx_hashes: Vec<TxHash>) -> Result<Vec<(TxHash, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError>;
        fn get_all_blobs_exact(&self, tx_hashes: Vec<TxHash>) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v1(&self, versioned_hashes: &[B256]) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v2(&self, versioned_hashes: &[B256]) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v3(&self, versioned_hashes: &[B256]) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v4(&self, versioned_hashes: &[B256], cell_mask: BlobCellMask) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError>;
        fn has_blobs_for_versioned_hashes(&self, versioned_hashes: &[B256]) -> Result<Vec<bool>, BlobStoreError>;
        fn blob_store(&self) -> Box<dyn BlobStore>;
    }

    fn retain_unknown<A: HandleMempoolData>(&self, announcement: &mut A) {
        self.0.retain_unknown(announcement);
    }
    fn retain_contains<A: HandleMempoolData>(&self, announcement: &mut A) {
        self.0.retain_contains(announcement);
    }
    fn get_pending_transactions_with_predicate(
        &self,
        predicate: impl FnMut(&ValidPoolTransaction<Self::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.0.get_pending_transactions_with_predicate(predicate)
    }
}

impl<P: TransactionPoolExt> TransactionPoolExt for RecoveryPool<P> {
    type Block = P::Block;
    forward! {
        fn set_block_info(&self, info: BlockInfo);
        fn on_canonical_state_change(&self, update: CanonicalStateUpdate<'_, Self::Block>);
        fn update_accounts(&self, accounts: Vec<ChangedAccount>);
        fn delete_blob(&self, tx: B256);
        fn delete_blobs(&self, txs: Vec<B256>);
        fn cleanup_blobs(&self);
    }
}
