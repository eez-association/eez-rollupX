//! Sendable session state; each EVM borrows it only during synchronous execution.
use alloy_primitives::{Address, map::AddressMap};
use eez_evm::{EezEvm, EezEvmConfig, EezEvmTypes};
use eez_evm_inspector::OverlaySnapshot;
use evm2::{
    block::bundle_state::AccountStatus,
    evm::{CacheDB, Db, PendingState},
};
use reth_evm::{ConfigureEvm, EvmEnvFor, database::StateProviderDatabase};
use reth_storage_api::{EvmStateProviderAdapter, StateProviderBox};

type ProviderDb = Db<StateProviderDatabase<EvmStateProviderAdapter<StateProviderBox>>>;

/// Retains only database/cache values and lifecycle metadata across calls or awaits.
/// evm2's interpreter and rollback journal remain local to each executing thread.
pub struct SimState {
    pub db: CacheDB<ProviderDb>,
    statuses: AddressMap<AccountStatus>,
}

impl std::fmt::Debug for SimState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimState")
            .field("cached_accounts", &self.db.cache.accounts.len())
            .field("lifecycle_accounts", &self.statuses.len())
            .finish_non_exhaustive()
    }
}

impl SimState {
    pub fn new(provider: StateProviderBox, snapshot: Option<OverlaySnapshot>) -> Self {
        let snapshot = snapshot.unwrap_or_default();
        let mut db = CacheDB::new(Db::new(StateProviderDatabase::new(
            EvmStateProviderAdapter(provider),
        )));
        db.cache = snapshot.cache;
        Self {
            db,
            statuses: snapshot.statuses,
        }
    }

    pub fn snapshot(&self) -> OverlaySnapshot {
        OverlaySnapshot {
            cache: self.db.cache.clone(),
            statuses: self.statuses.clone(),
        }
    }

    pub fn take_snapshot(&mut self) -> OverlaySnapshot {
        OverlaySnapshot {
            cache: std::mem::take(&mut self.db.cache),
            statuses: std::mem::take(&mut self.statuses),
        }
    }

    pub fn restore(&mut self, snapshot: OverlaySnapshot) {
        self.db.cache = snapshot.cache;
        self.statuses = snapshot.statuses;
    }

    pub fn evm<'a>(
        &'a mut self,
        config: &EezEvmConfig,
        env: EvmEnvFor<EezEvmConfig>,
    ) -> EezEvm<'a> {
        config.evm_with_env(&mut self.db, env)
    }

    pub fn transact(
        &mut self,
        config: &EezEvmConfig,
        env: &EvmEnvFor<EezEvmConfig>,
        tx: &alloy_consensus::transaction::Recovered<eez_evm::EezEvmTx>,
        inspector: Option<&mut dyn evm2::Inspector<EezEvmTypes>>,
    ) -> evm2::registry::HandlerResult<evm2::TxResultWithState<EezEvmTypes>> {
        let mut evm = self.evm(config, env.clone());
        match inspector {
            Some(inspector) => evm.transact_with_inspector(tx, inspector),
            None => evm.transact(tx),
        }
    }

    /// Only the caller's accepted pending effects enter the retained session.
    pub fn commit(&mut self, pending: &PendingState) {
        let mut snapshot = OverlaySnapshot {
            cache: Default::default(),
            statuses: std::mem::take(&mut self.statuses),
        };
        snapshot.commit_pending(std::mem::take(&mut self.db.cache), pending);
        self.restore(snapshot);
    }

    /// Accept a synthetic contract/proxy frame without its sender nonce bump.
    /// Retaining that bump would drift contract CREATE nonces and prevent later
    /// CREATE2 deployment at an as-yet undeployed proxy address (EIP-684).
    pub fn commit_frame(&mut self, pending: PendingState, caller: Address) {
        let mut snapshot = OverlaySnapshot {
            cache: Default::default(),
            statuses: std::mem::take(&mut self.statuses),
        };
        snapshot.commit_pending_resetting_nonce(
            std::mem::take(&mut self.db.cache),
            &pending,
            caller,
        );
        self.restore(snapshot);
    }
}
