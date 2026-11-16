//! Native cache snapshots retain account lifecycle separately from cached values.
//!
//! A cache cannot distinguish cold reads from callback mutations or creation from
//! destruction. The overlay needs those distinctions when a suspended EVM resumes.
use crate::overlay::OverlayError;
use alloy_primitives::{Address, KECCAK256_EMPTY, U256, map::AddressMap};
use evm2::{
    EvmFeatures,
    block::bundle_state::AccountStatus,
    bytecode::Bytecode,
    evm::{AccountInfo, Cache, CacheDB, DbResult, EmptyDB, PendingState, State},
};

/// Sendable state exchanged between source simulation and retained callback sessions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlaySnapshot {
    /// Values served to the next synthetic execution, including explicit deletion/wipes.
    pub cache: Cache,
    /// Cumulative lifecycle for the overlay's destruction and read-only gates.
    pub statuses: AddressMap<AccountStatus>,
}

impl OverlaySnapshot {
    /// Project live writes without committing the source journal or sharing transient state.
    ///
    /// Empty-account normalization matches the accepted REVM cache boundary: touched
    /// empty accounts disappear, except that creation precedes empty-account cleanup.
    pub fn capture(&self, state: &mut State<'_>) -> DbResult<Self> {
        // The live EVM can borrow a session's cache as its database. Its own
        // overlay then contains only this transaction's reads; retain untouched
        // prior effects as well as the current journal's values.
        let mut snapshot = self.clone();
        snapshot.cache.merge(
            state
                .committed_copy(EmptyDB::default())
                .overlay_db()
                .cache
                .clone(),
        );
        let addresses: Vec<_> = state
            .loaded_accounts()
            .map(|(address, _)| *address)
            .collect();
        for address in addresses {
            let account = state.account(&address)?;
            snapshot.record_lifecycle(
                address,
                account.original(),
                account.get(),
                account.is_touched(),
                account.is_created(),
                account.is_destructed(),
            );
        }
        Ok(snapshot)
    }

    /// Accept a finalized transaction, retaining reads and cumulative lifecycle metadata.
    ///
    /// `cache` is the executing EVM's accepted overlay before committing `pending`.
    /// The owning composition checkpoint rolls back this whole snapshot.
    pub fn commit_pending(&mut self, cache: Cache, pending: &PendingState) {
        self.commit_pending_inner(cache, pending, None);
    }

    /// Commit a synthetic frame without retaining its transaction-sender nonce bump.
    /// Lifecycle and storage-discard metadata still come from the native pending state.
    pub fn commit_pending_resetting_nonce(
        &mut self,
        cache: Cache,
        pending: &PendingState,
        caller: Address,
    ) {
        self.commit_pending_inner(cache, pending, Some(caller));
    }

    fn commit_pending_inner(
        &mut self,
        cache: Cache,
        pending: &PendingState,
        reset: Option<Address>,
    ) {
        let mut db = CacheDB::new(EmptyDB::default());
        db.cache = cache;
        db.commit_pending(pending);
        self.cache = db.cache;
        for account in pending.account_changes() {
            let adjusted = account
                .current
                .filter(|_| reset == Some(account.address))
                .map(|info| {
                    let mut info = info.clone();
                    info.nonce = account.original.map_or(0, |original| original.nonce);
                    info
                });
            if let (Some(info), Some(Some(cached))) =
                (&adjusted, self.cache.accounts.get_mut(&account.address))
            {
                cached.nonce = info.nonce;
            }
            self.record_lifecycle(
                account.address,
                account.original,
                adjusted.as_ref().or(account.current),
                account.touched,
                account.created,
                account.selfdestructed,
            );
        }
    }

    /// Apply a successful callback's delta through the source's rollback journal.
    ///
    /// New/changed destruction and unavailable bytecode fail closed. A caller must discard
    /// the composition after an error because earlier accounts may already have been applied.
    pub fn apply_to(
        &self,
        state: &mut State<'_>,
        before: &Self,
        features: EvmFeatures,
    ) -> Result<(), OverlayError> {
        for (address, status) in &self.statuses {
            if status.was_destroyed() {
                if before.statuses.get(address) != Some(status)
                    || before.cache.accounts.get(address) != self.cache.accounts.get(address)
                    || before.cache.storage.get(address) != self.cache.storage.get(address)
                {
                    return Err(OverlayError::Selfdestruct { address: *address });
                }
                continue;
            }
            if !status.is_modified_and_not_destroyed() {
                continue;
            }
            let Some(Some(after_info)) = self.cache.accounts.get(address) else {
                continue;
            };
            let source = {
                let mut account = state
                    .account(address)
                    .map_err(|e| OverlayError::SourceLoad {
                        address: *address,
                        message: format!("{e:?}"),
                    })?;
                // REVM's journal load warmed this account; native state loads alone do not.
                // Journal the warmth too, so resumed CALL gas and enclosing rollback agree.
                account.warm();
                account.get().cloned().unwrap_or_default()
            };
            let mut desired = after_info.clone();
            desired.code = if source.code_hash == desired.code_hash {
                source.code.clone()
            } else {
                let hash = desired.code_hash;
                let code = if hash == KECCAK256_EMPTY {
                    Bytecode::default()
                } else {
                    desired
                        .code
                        .clone()
                        .or_else(|| self.cache.contracts.get(&hash).cloned())
                        .ok_or(OverlayError::MissingCode {
                            address: *address,
                            hash,
                        })?
                };
                let actual = code.hash_slow();
                if actual != hash {
                    return Err(OverlayError::CodeHashMismatch {
                        address: *address,
                        expected: hash,
                        actual,
                    });
                }
                Some(code)
            };
            if source.nonce == 0
                && source.is_empty_code_hash()
                && *status == AccountStatus::InMemoryChange
                && desired.nonce != 0
            {
                state
                    .create_account(address, address, &U256::ZERO, features)
                    .map_err(|e| OverlayError::SourceCreation {
                        address: *address,
                        message: format!("{e:?}"),
                    })?
                    .map_err(|e| OverlayError::SourceCreation {
                        address: *address,
                        message: format!("{e:?}"),
                    })?;
            }
            if source != desired {
                let mut account = state
                    .account(address)
                    .map_err(|e| OverlayError::SourceLoad {
                        address: *address,
                        message: format!("{e:?}"),
                    })?;
                account.touch();
                // This handle records a rollback snapshot; copying the full info also retains
                // chain-specific account extensions without a lossy REVM type conversion.
                *account.get_or_insert() = desired;
            }
            if let Some(storage) = self.cache.storage.get(address) {
                for (key, value) in &storage.slots {
                    if before
                        .cache
                        .storage
                        .get(address)
                        .and_then(|s| s.slots.get(key))
                        == Some(value)
                    {
                        continue;
                    }
                    // Match journal.sstore: a cold account touched only by this write must be
                    // included in the next live snapshot, even when its info is unchanged.
                    state
                        .account(address)
                        .map_err(|e| OverlayError::SourceLoad {
                            address: *address,
                            message: format!("{e:?}"),
                        })?
                        .touch();
                    let mut slot = state.storage_slot(address, *key).map_err(|e| {
                        OverlayError::SourceSstore {
                            address: *address,
                            key: *key,
                            message: format!("{e:?}"),
                        }
                    })?;
                    // journal.sstore also warms its slot even when the value is unchanged.
                    slot.warm();
                    slot.set(*value);
                }
            }
        }
        Ok(())
    }

    fn record_lifecycle(
        &mut self,
        address: Address,
        original: Option<&AccountInfo>,
        current: Option<&AccountInfo>,
        touched: bool,
        created: bool,
        selfdestructed: bool,
    ) {
        let status = self
            .statuses
            .entry(address)
            .or_insert_with(|| match original {
                None => AccountStatus::LoadedNotExisting,
                Some(info) if info.is_empty() => AccountStatus::LoadedEmptyEIP161,
                Some(_) => AccountStatus::Loaded,
            });
        if !touched {
            return;
        }
        let deleted = selfdestructed || (!created && current.is_none_or(AccountInfo::is_empty));
        *status = if selfdestructed {
            status.on_selfdestructed()
        } else if created {
            status.on_created()
        } else if deleted {
            status.on_touched_empty_post_eip161()
        } else {
            status.on_changed(
                original.is_none_or(|info| info.nonce == 0 && info.is_empty_code_hash()),
            )
        };
        if deleted {
            self.cache.accounts.insert(address, None);
            self.cache.storage.entry(address).or_default().wipe();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::bytes;
    use evm2::evm::InMemoryDB;

    #[test]
    fn frame_commit_restores_only_sender_nonce_and_empty_lifecycle() {
        use evm2::{
            BaseEvmTypes, Evm, SpecId, ethereum::ethereum_tx_registry, precompiles::Precompiles,
        };
        let caller = Address::repeat_byte(0x71);
        let recipient = Address::repeat_byte(0x72);
        for spec in [SpecId::SHANGHAI, SpecId::CANCUN, SpecId::OSAKA] {
            for original in [
                None,
                Some(AccountInfo::empty()),
                Some(AccountInfo::empty().with_nonce(37)),
                Some(AccountInfo::empty().with_balance(U256::from(20))),
                Some(AccountInfo::empty().with_balance(U256::from(7))),
            ] {
                let mut db = InMemoryDB::default();
                if let Some(info) = &original {
                    db.insert_account_info(&caller, info.clone());
                }
                let value = if original.as_ref().is_some_and(|i| !i.balance.is_zero()) {
                    7
                } else {
                    0
                };
                let mut evm: Evm<'_, BaseEvmTypes> = Evm::new(
                    spec,
                    Default::default(),
                    ethereum_tx_registry(spec),
                    db,
                    Precompiles::base(spec),
                );
                let tx = evm2::ethereum::RecoveredTxEnvelope::new_unchecked(
                    evm2::ethereum::TxEnvelope::Legacy(alloy_consensus::TxLegacy {
                        nonce: original.as_ref().map_or(0, |i| i.nonce),
                        to: alloy_primitives::TxKind::Call(recipient),
                        gas_limit: 100_000,
                        value: U256::from(value),
                        ..Default::default()
                    }),
                    caller,
                );
                let result = evm.transact(&tx).unwrap().detach();
                assert!(result.result.is_success());
                assert_eq!(result.result.tx_gas_used(), 21_000);
                assert!(result.result.output.is_empty());
                let cache = evm.overlay_db_mut().cache.clone();
                let mut actual = OverlaySnapshot::default();
                actual.commit_pending_resetting_nonce(cache.clone(), &result.pending_state, caller);
                let mut expected = OverlaySnapshot::default();
                expected.commit_pending(cache, &result.pending_state);
                let empty = original.as_ref().is_none_or(|info| {
                    info.nonce == 0
                        && info.is_empty_code_hash()
                        && info.balance == U256::from(value)
                });
                if empty {
                    expected.cache.accounts.insert(caller, None);
                    expected.cache.storage.entry(caller).or_default().wipe();
                    expected.statuses.insert(
                        caller,
                        if original.is_none() {
                            AccountStatus::LoadedNotExisting
                        } else {
                            AccountStatus::Destroyed
                        },
                    );
                    let mut next = State::new(InMemoryDB {
                        cache: actual.cache.clone(),
                        ..Default::default()
                    });
                    assert!(
                        next.create_account(&caller, &recipient, &U256::ZERO, EvmFeatures::EIP161)
                            .unwrap()
                            .is_ok(),
                        "a synthetic caller must remain deployable under EIP-684"
                    );
                } else {
                    let info = expected
                        .cache
                        .accounts
                        .get_mut(&caller)
                        .unwrap()
                        .as_mut()
                        .unwrap();
                    info.nonce = original.as_ref().unwrap().nonce;
                    assert_eq!(
                        info.balance,
                        original.as_ref().unwrap().balance - U256::from(value)
                    );
                }
                assert_eq!(actual, expected, "spec={spec:?} original={original:?}");
                assert_eq!(
                    result.pending_state.account_info(&caller).unwrap().nonce,
                    original.as_ref().map_or(0, |i| i.nonce) + 1
                );
            }
        }
    }

    #[test]
    fn frame_commit_retains_child_creation_and_storage_discard() {
        use evm2::{
            BaseEvmTypes, Evm, SpecId, ethereum::ethereum_tx_registry, precompiles::Precompiles,
        };
        let caller = Address::repeat_byte(0x73);
        let child = caller.create(0);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&caller, AccountInfo::empty().with_balance(U256::from(20)));
        db.insert_account_storage(&child, &U256::ZERO, &U256::from(99));
        let mut evm: Evm<'_, BaseEvmTypes> = Evm::new(
            SpecId::OSAKA,
            Default::default(),
            ethereum_tx_registry(SpecId::OSAKA),
            db,
            Precompiles::base(SpecId::OSAKA),
        );
        let tx = evm2::ethereum::RecoveredTxEnvelope::new_unchecked(
            evm2::ethereum::TxEnvelope::Legacy(alloy_consensus::TxLegacy {
                to: alloy_primitives::TxKind::Create,
                gas_limit: 100_000,
                input: bytes!("60006000f3"),
                ..Default::default()
            }),
            caller,
        );
        let result = evm.transact(&tx).unwrap().detach();
        assert!(result.result.is_success());
        assert!(
            result
                .pending_state
                .account_changes_for(&child)
                .unwrap()
                .created
        );
        let mut snapshot = OverlaySnapshot::default();
        snapshot.commit_pending_resetting_nonce(
            evm.overlay_db_mut().cache.clone(),
            &result.pending_state,
            caller,
        );
        assert_eq!(snapshot.cache.accounts[&caller].as_ref().unwrap().nonce, 0);
        assert_eq!(snapshot.cache.accounts[&child].as_ref().unwrap().nonce, 1);
        assert_eq!(snapshot.statuses[&child], AccountStatus::InMemoryChange);
        assert!(snapshot.cache.storage[&child].wiped);
        assert!(
            snapshot.cache.storage[&child]
                .slots
                .values()
                .all(U256::is_zero)
        );
    }

    #[test]
    fn live_creation_snapshot_keeps_code_storage_and_source_rollback() {
        let child = Address::with_last_byte(0x42);
        let mut state = State::new(EmptyDB::default());
        let checkpoint = state.checkpoint();
        state
            .create_account(&child, &child, &U256::ZERO, EvmFeatures::EIP161)
            .unwrap()
            .unwrap();
        let code = Bytecode::new_raw(bytes!("602a60005260206000f3"));
        state.account(&child).unwrap().set_code_slow(code.clone());
        state
            .storage_slot(&child, U256::ZERO)
            .unwrap()
            .set(U256::from(42));
        let snapshot = OverlaySnapshot::default().capture(&mut state).unwrap();
        assert_eq!(snapshot.statuses[&child], AccountStatus::InMemoryChange);
        assert_eq!(
            snapshot.cache.accounts[&child].as_ref().unwrap().code_hash,
            code.hash_slow()
        );
        assert_eq!(
            snapshot.cache.storage[&child].slots[&U256::ZERO],
            U256::from(42)
        );
        state.rollback(checkpoint, EvmFeatures::EIP161);
        assert!(state.account(&child).unwrap().get().is_none());
        assert_eq!(
            snapshot.cache.storage[&child].slots[&U256::ZERO],
            U256::from(42)
        );
    }

    #[test]
    fn read_only_accounts_stay_read_only_and_empty_touch_is_projected() {
        let read = Address::with_last_byte(1);
        let empty = Address::with_last_byte(2);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&read, AccountInfo::empty().with_balance(U256::from(3)));
        db.insert_account_info(&empty, AccountInfo::empty());
        let mut state = State::new(db);
        state.account(&read).unwrap();
        state.account(&empty).unwrap().set_balance(U256::ZERO);
        let snapshot = OverlaySnapshot::default().capture(&mut state).unwrap();
        assert_eq!(snapshot.statuses[&read], AccountStatus::Loaded);
        assert_eq!(snapshot.statuses[&empty], AccountStatus::Destroyed);
        assert_eq!(snapshot.cache.accounts[&empty], None);
        assert!(state.account(&empty).unwrap().get().is_some());
    }
    #[test]
    fn callback_creation_applies_code_nonce_storage_and_outer_rollback() {
        let child = Address::with_last_byte(0x42);
        let mut callback = State::new(EmptyDB::default());
        callback
            .create_account(&child, &child, &U256::ZERO, EvmFeatures::EIP161)
            .unwrap()
            .unwrap();
        let code = Bytecode::new_raw(bytes!("602a5f5260205ff3"));
        callback
            .account(&child)
            .unwrap()
            .set_code_slow(code.clone());
        callback
            .account(&child)
            .unwrap()
            .set_balance(U256::from(17));
        callback
            .storage_slot(&child, U256::ZERO)
            .unwrap()
            .set(U256::from(42));
        let before = OverlaySnapshot::default();
        let after = before.capture(&mut callback).unwrap();
        let mut source = State::new(EmptyDB::default());
        assert!(source.account(&child).unwrap().get().is_none());
        let checkpoint = source.checkpoint();
        after
            .apply_to(&mut source, &before, EvmFeatures::EIP161)
            .unwrap();
        let account = source.account(&child).unwrap();
        assert!(account.is_created());
        assert_eq!(account.nonce(), 1);
        assert_eq!(account.balance(), U256::from(17));
        assert_eq!(account.code_hash(), code.hash_slow());
        drop(account);
        assert_eq!(
            source
                .storage_slot(&child, U256::ZERO)
                .unwrap()
                .get()
                .current,
            U256::from(42)
        );
        source.rollback(checkpoint, EvmFeatures::EIP161);
        assert!(source.account(&child).unwrap().get().is_none());
        assert_eq!(
            source
                .storage_slot(&child, U256::ZERO)
                .unwrap()
                .get()
                .current,
            U256::ZERO
        );
    }

    #[test]
    fn callback_cold_slot_clear_survives_snapshot_and_reverts() {
        let address = Address::with_last_byte(3);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&address, AccountInfo::empty().with_nonce(1));
        db.insert_account_storage(&address, &U256::ZERO, &U256::from(9));
        let mut source = State::new(db);
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        after
            .cache
            .accounts
            .insert(address, Some(AccountInfo::empty().with_nonce(1)));
        after.statuses.insert(address, AccountStatus::Changed);
        after
            .cache
            .storage
            .entry(address)
            .or_default()
            .slots
            .insert(U256::ZERO, U256::ZERO);
        let checkpoint = source.checkpoint();
        after
            .apply_to(&mut source, &before, EvmFeatures::EIP161)
            .unwrap();
        assert!(source.account(&address).unwrap().is_warm());
        assert!(source.storage_slot(&address, U256::ZERO).unwrap().is_warm());
        let next = before.capture(&mut source).unwrap();
        assert_eq!(next.statuses[&address], AccountStatus::Changed);
        assert_eq!(next.cache.storage[&address].slots[&U256::ZERO], U256::ZERO);
        source.rollback(checkpoint, EvmFeatures::EIP161);
        assert!(!source.account(&address).unwrap().is_warm());
        assert!(!source.storage_slot(&address, U256::ZERO).unwrap().is_warm());
        assert_eq!(
            source
                .storage_slot(&address, U256::ZERO)
                .unwrap()
                .get()
                .current,
            U256::from(9)
        );
    }

    #[test]
    fn snapshot_keeps_untouched_prior_effects_with_a_borrowed_cache_database() {
        let retained = Address::with_last_byte(8);
        let changed = Address::with_last_byte(9);
        let mut seed = OverlaySnapshot::default();
        seed.cache.accounts.insert(
            retained,
            Some(AccountInfo::empty().with_balance(U256::from(11))),
        );
        seed.statuses.insert(retained, AccountStatus::Changed);
        let mut db = InMemoryDB {
            cache: seed.cache.clone(),
            ..Default::default()
        };
        let mut state = State::new(&mut db);
        state.account(&changed).unwrap().set_balance(U256::from(17));
        let snapshot = seed.capture(&mut state).unwrap();
        assert_eq!(
            snapshot.cache.accounts[&retained],
            seed.cache.accounts[&retained]
        );
        assert_eq!(snapshot.statuses[&retained], AccountStatus::Changed);
        assert_eq!(
            snapshot.cache.accounts[&changed].as_ref().unwrap().balance,
            U256::from(17)
        );
        assert!(!seed.cache.accounts.contains_key(&changed));
    }

    #[test]
    fn callback_rejects_new_destruction_and_missing_code() {
        let address = Address::with_last_byte(4);
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        after.statuses.insert(address, AccountStatus::Destroyed);
        let mut source = State::new(EmptyDB::default());
        assert!(matches!(
            after.apply_to(&mut source, &before, EvmFeatures::EIP161),
            Err(OverlayError::Selfdestruct { .. })
        ));
        after
            .statuses
            .insert(address, AccountStatus::InMemoryChange);
        let mut info = AccountInfo::empty().with_nonce(1);
        info.code_hash = alloy_primitives::B256::repeat_byte(0x77);
        info.code = None;
        after.cache.accounts.insert(address, Some(info));
        assert!(matches!(
            after.apply_to(&mut source, &before, EvmFeatures::EIP161),
            Err(OverlayError::MissingCode { .. })
        ));
    }
}
