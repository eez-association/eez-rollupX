//! [`EezPayloadBuilder`] — drop-in replacement for
//! [`reth_node_ethereum::EthereumPayloadBuilder`] that pins the payload
//! builder's `gas_limit` and `extra_data` to the shared
//! [`eez_driver::BUILDER_GAS_LIMIT`] / [`eez_driver::BUILDER_EXTRA_DATA`]
//! constants instead of reading reth's `--builder.gaslimit` /
//! `--builder.extradata` CLI flags. The Deriver's `execute_block`
//! reads the same constants, so the two block-construction paths can't
//! drift via operator misconfiguration.

use std::sync::{Arc, OnceLock, Weak};

use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use eez_driver::{BUILDER_EXTRA_DATA, BUILDER_GAS_LIMIT};
use eez_payload_codec::BlockDaSizer;
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthPayloadAttributes};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{ConfigureEvm, NextBlockEnvAttributes};
use reth_node_api::{FullNodeTypes, NodeTypes, PrimitivesTy, TxTy};
use reth_node_builder::{BuilderContext, PayloadTypes, components::PayloadBuilderBuilder};
use reth_payload_primitives::PayloadBuilderError;
use reth_primitives_traits::SealedHeader;
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    BestTransactions, PoolTransaction, TransactionPool, ValidPoolTransaction,
    error::InvalidPoolTransactionError,
};

fn data_build_error(message: impl Into<String>) -> PayloadBuilderError {
    PayloadBuilderError::other(std::io::Error::other(message.into()))
}

/// Installed before RPC/payload startup and bound once the composer exists.
/// Weak ownership avoids a cycle through the composer's payload-builder handle.
pub struct SequencingDataGuard {
    rollup_id: u64,
    composer: OnceLock<Weak<dyn eez_driver::SyncSlotComposer>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SequencingDataError {
    #[error("sequencing data limits unavailable: {0}")]
    Unavailable(String),
    #[error(
        "ordinary transaction cannot fit a settlement interval: encoded block bound {size} exceeds network-derived allowance {limit}"
    )]
    TooLarge { size: usize, limit: usize },
}

impl std::fmt::Debug for SequencingDataGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SequencingDataGuard")
            .field("rollup_id", &self.rollup_id)
            .finish_non_exhaustive()
    }
}

impl SequencingDataGuard {
    pub fn new(rollup_id: u64) -> Self {
        Self {
            rollup_id,
            composer: OnceLock::new(),
        }
    }

    pub fn bind(&self, composer: &eez_driver::SyncSlotComposerHandle) -> eyre::Result<()> {
        self.composer
            .set(Arc::downgrade(composer))
            .map_err(|_| eyre::eyre!("sequencing data guard already bound"))
    }

    fn composer(&self) -> Result<Arc<dyn eez_driver::SyncSlotComposer>, String> {
        self.composer
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "sequencing data limits unavailable before composer readiness".into())
    }

    fn block_limit(
        &self,
        parent: &SealedHeader<alloy_consensus::Header>,
        timestamp: u64,
    ) -> Result<usize, String> {
        self.composer()?
            .ordinary_block_da_limit(self.rollup_id, parent, timestamp)
    }

    pub(crate) fn validate_transaction(
        &self,
        tx: &TransactionSigned,
    ) -> Result<(), SequencingDataError> {
        let limit = self
            .composer()
            .map_err(SequencingDataError::Unavailable)?
            .ordinary_transaction_da_limit(self.rollup_id)
            .map_err(SequencingDataError::Unavailable)?;
        let size = BlockDaSizer::new(BUILDER_EXTRA_DATA)
            .with_transaction(&alloy_rlp::encode(tx))
            .and_then(|s| s.item_bytes())
            .map_err(|e| SequencingDataError::Unavailable(e.to_string()))?;
        if size > limit {
            return Err(SequencingDataError::TooLarge { size, limit });
        }
        Ok(())
    }
}

/// Marker type — mirrors `EthereumPayloadBuilder` but uses our constants.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct EezPayloadBuilder {
    data_guard: Option<Arc<SequencingDataGuard>>,
}

impl EezPayloadBuilder {
    #[must_use]
    pub fn with_data_guard(mut self, guard: Option<Arc<SequencingDataGuard>>) -> Self {
        self.data_guard = guard;
        self
    }
}

impl<Types, Node, Pool, Evm> PayloadBuilderBuilder<Node, Pool, Evm> for EezPayloadBuilder
where
    Types: NodeTypes<ChainSpec: reth_chainspec::EthereumHardforks, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    Evm: ConfigureEvm<Primitives = PrimitivesTy<Types>, NextBlockEnvCtx = NextBlockEnvAttributes>
        + 'static,
    Types::Payload:
        PayloadTypes<BuiltPayload = EthBuiltPayload, PayloadAttributes = EthPayloadAttributes>,
{
    type PayloadBuilder = EezRuntimePayloadBuilder<Pool, Node::Provider, Evm>;

    fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: Evm,
    ) -> impl Future<Output = eyre::Result<Self::PayloadBuilder>> {
        std::future::ready(Ok(EezRuntimePayloadBuilder {
            client: ctx.provider().clone(),
            pool,
            evm_config,
            data_guard: self.data_guard,
            config: EthereumBuilderConfig::new()
                .with_gas_limit(BUILDER_GAS_LIMIT)
                .with_extra_data(Bytes::from_static(BUILDER_EXTRA_DATA)),
        }))
    }
}

#[derive(Clone, Debug)]
pub struct EezRuntimePayloadBuilder<Pool, Client, Evm> {
    pool: Pool,
    client: Client,
    evm_config: Evm,
    config: EthereumBuilderConfig,
    data_guard: Option<Arc<SequencingDataGuard>>,
}

impl<Pool, Client, Evm> PayloadBuilder for EezRuntimePayloadBuilder<Pool, Client, Evm>
where
    Evm: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    type Attributes = EthPayloadAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        mut args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let limit = self
            .data_guard
            .as_ref()
            .map(|guard| {
                guard.block_limit(&args.config.parent_header, args.config.attributes.timestamp)
            })
            .transpose()
            .map_err(data_build_error)?;
        if limit.is_some()
            && args
                .config
                .attributes
                .withdrawals
                .as_ref()
                .is_some_and(|w| !w.is_empty())
        {
            return Err(data_build_error(
                "ordinary EEZ data budgeting requires empty withdrawals",
            ));
        }
        if let Some(limit) = limit {
            // A changed network allowance must not retain an older oversized
            // cached candidate merely because it paid a higher execution tip.
            args.best_payload = args
                .best_payload
                .filter(|p| alloy_rlp::encode(p.block()).length() <= limit);
        }
        let outcome = reth_ethereum_payload_builder::default_ethereum_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.config.clone(),
            args,
            |attrs| {
                let transactions = self.pool.best_transactions_with_attributes(attrs);
                match limit {
                    None => transactions,
                    Some(limit) => Box::new(DaTransactions {
                        inner: transactions,
                        size: BlockDaSizer::new(BUILDER_EXTRA_DATA),
                        limit,
                        previous: None,
                    })
                        as Box<
                            dyn BestTransactions<
                                Item = Arc<ValidPoolTransaction<Pool::Transaction>>,
                            >,
                        >,
                }
            },
        )?;
        if let (Some(limit), Some(payload)) = (limit, outcome.payload()) {
            let actual = alloy_rlp::encode(payload.block()).length();
            if actual > limit {
                return Err(data_build_error(format!(
                    "sequencing DA invariant: actual encoded block {actual} exceeds {limit}"
                )));
            }
        }
        Ok(outcome)
    }

    fn on_missing_payload(
        &self,
        _: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        if self.data_guard.is_some() {
            MissingPayloadBehaviour::AwaitInProgress
        } else {
            MissingPayloadBehaviour::RaceEmptyPayload
        }
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let limit = self
            .data_guard
            .as_ref()
            .map(|g| g.block_limit(&config.parent_header, config.attributes.timestamp))
            .transpose()
            .map_err(data_build_error)?;
        let inner = reth_ethereum_payload_builder::EthereumPayloadBuilder::new(
            self.client.clone(),
            self.pool.clone(),
            self.evm_config.clone(),
            self.config.clone(),
        );
        let payload = inner.build_empty_payload(config)?;
        if limit.is_some_and(|n| alloy_rlp::encode(payload.block()).length() > n) {
            return Err(data_build_error(
                "empty block exceeds sequencing data budget",
            ));
        }
        Ok(payload)
    }
}

struct DaTransactions<T: PoolTransaction> {
    inner: Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<T>>>>,
    size: BlockDaSizer,
    limit: usize,
    previous: Option<(alloy_primitives::B256, BlockDaSizer)>,
}

impl<T: PoolTransaction> Iterator for DaTransactions<T> {
    type Item = Arc<ValidPoolTransaction<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let tx = self.inner.next()?;
            let encoded = alloy_rlp::encode(tx.transaction.consensus_ref().inner());
            let next = self.size.with_transaction(&encoded);
            let bytes = next
                .as_ref()
                .ok()
                .and_then(|s| s.item_bytes().ok())
                .unwrap_or(usize::MAX);
            if let Ok(next) = next
                && bytes <= self.limit
            {
                self.previous = Some((*tx.hash(), self.size));
                self.size = next;
                return Some(tx);
            }
            tracing::event!(name: "eez.payload.da_deferred", tracing::Level::DEBUG,
                tx_hash = %tx.hash(), encoded_block_bound = bytes, maximum = self.limit,
                "ordinary transaction retained for a later settlement interval");
            self.inner.mark_invalid(
                &tx,
                InvalidPoolTransactionError::OversizedData {
                    size: bytes,
                    limit: self.limit,
                },
            );
        }
    }
}

impl<T: PoolTransaction> BestTransactions for DaTransactions<T> {
    fn mark_invalid(&mut self, tx: &Self::Item, kind: InvalidPoolTransactionError) {
        if let Some((hash, size)) = self.previous.take()
            && hash == *tx.hash()
        {
            self.size = size;
        }
        self.inner.mark_invalid(tx, kind);
    }
    fn no_updates(&mut self) {
        self.inner.no_updates();
    }
    fn set_skip_blobs(&mut self, skip: bool) {
        self.inner.set_skip_blobs(skip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{TxLegacy, transaction::Recovered};
    use alloy_primitives::{Address, Signature, TxKind, U256};
    use reth_ethereum_primitives::Transaction;
    use reth_transaction_pool::{
        EthPooledTransaction, TransactionOrigin, identifier::TransactionId,
    };
    use std::{
        collections::{HashSet, VecDeque},
        sync::Mutex,
        time::Instant,
    };

    type Tx = Arc<ValidPoolTransaction<EthPooledTransaction>>;

    fn tx(sender: u64, nonce: u64, bytes: usize) -> Tx {
        let transaction = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy {
                chain_id: Some(sender),
                nonce,
                input: vec![0xff; bytes].into(),
                to: TxKind::Call(Address::ZERO),
                gas_limit: 1_000_000,
                ..Default::default()
            }),
            Signature::new(U256::ONE, U256::ONE, false),
        );
        Arc::new(ValidPoolTransaction {
            transaction: EthPooledTransaction::new(
                Recovered::new_unchecked(
                    transaction,
                    Address::from_word(U256::from(sender).into()),
                ),
                bytes,
            ),
            transaction_id: TransactionId::new(sender.into(), nonce),
            propagate: true,
            timestamp: Instant::now(),
            origin: TransactionOrigin::Local,
            authority_ids: None,
        })
    }

    struct Candidates {
        queue: VecDeque<Tx>,
        blocked: HashSet<Address>,
        excluded: Arc<Mutex<Vec<alloy_primitives::B256>>>,
    }
    impl Iterator for Candidates {
        type Item = Tx;
        fn next(&mut self) -> Option<Tx> {
            while let Some(tx) = self.queue.pop_front() {
                if !self.blocked.contains(&tx.sender()) {
                    return Some(tx);
                }
            }
            None
        }
    }
    impl BestTransactions for Candidates {
        fn mark_invalid(&mut self, tx: &Tx, _: InvalidPoolTransactionError) {
            self.blocked.insert(tx.sender());
            self.excluded.lock().unwrap().push(*tx.hash());
        }
        fn no_updates(&mut self) {}
        fn set_skip_blobs(&mut self, _: bool) {}
    }

    fn candidates(txs: Vec<Tx>, limit: usize) -> DaTransactions<EthPooledTransaction> {
        DaTransactions {
            inner: Box::new(Candidates {
                queue: txs.into(),
                blocked: HashSet::new(),
                excluded: Arc::default(),
            }),
            size: BlockDaSizer::new(BUILDER_EXTRA_DATA),
            limit,
            previous: None,
        }
    }

    fn size(txs: &[Tx]) -> usize {
        txs.iter()
            .fold(BlockDaSizer::new(BUILDER_EXTRA_DATA), |s, tx| {
                s.with_transaction(&alloy_rlp::encode(tx.transaction.consensus_ref().inner()))
                    .unwrap()
            })
            .item_bytes()
            .unwrap()
    }

    #[test]
    fn capacity_defers_overflow_and_descendants_but_allows_other_senders() {
        let (a, b, child, c) = (tx(1, 0, 1000), tx(2, 0, 900), tx(2, 1, 1), tx(3, 0, 10));
        let txs = vec![a.clone(), b.clone(), child, c.clone()];
        let limit = size(&[a.clone(), c.clone()]);
        assert_eq!(
            candidates(txs.clone(), limit)
                .map(|tx| *tx.hash())
                .collect::<Vec<_>>(),
            [*a.hash(), *c.hash()]
        );
        // Iteration never mutates the underlying pool; a new interval can select
        // the previously deferred sender and its nonce descendants.
        assert_eq!(candidates(txs.clone(), size(&txs)).count(), txs.len());
        assert_eq!(
            candidates(vec![b.clone()], size(std::slice::from_ref(&b)) - 1).count(),
            0
        );
        assert_eq!(candidates(vec![b.clone()], size(&[b])).count(), 1);
    }

    #[test]
    fn execution_rejection_returns_reserved_bytes_and_empty_boundary_excludes_all_data() {
        let (a, b) = (tx(1, 0, 1000), tx(2, 0, 1000));
        let mut iter = candidates(vec![a.clone(), b.clone()], size(std::slice::from_ref(&a)));
        assert_eq!(iter.next().unwrap().hash(), a.hash());
        iter.mark_invalid(&a, InvalidPoolTransactionError::ExceedsGasLimit(1, 0));
        assert_eq!(iter.next().unwrap().hash(), b.hash());
        assert!(iter.next().is_none());
        let mut empty = candidates(
            vec![a, b],
            BlockDaSizer::new(BUILDER_EXTRA_DATA).item_bytes().unwrap(),
        );
        assert!(empty.next().is_none());
    }
}
