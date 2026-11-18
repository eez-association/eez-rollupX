//! Same-thread snapshot exchange for nested callbacks into a suspended EVM.
//! Cache values and account lifecycle travel together; returned changes are journaled
//! so enclosing source reverts undo callbacks. Transient state is not exchanged.

use std::sync::{Arc, Mutex};

use crate::overlay_state::OverlaySnapshot;
use alloy_primitives::{Address, B256, U256};
use evm2::{EvmFeatures, evm::State};

/// Cache exchange for nested dispatches that re-enter a suspended rollup.
///
/// # Why it exists
///
/// During nested dispatch, the caller's revm state remains mutably borrowed.
/// A re-entered session therefore cannot borrow that state directly. This
/// channel transfers cache snapshots across the dispatch boundary in one frame
/// per dispatch:
///
/// - [`open_frame`](Self::open_frame) records this rollup's live cache before a
///   dispatch. A session that re-enters the rollup preloads the innermost
///   frame's snapshot when it opens.
///
/// - [`publish`](Self::publish) records a re-entered session's cumulative cache
///   as the innermost frame's result. Sibling callbacks reuse one session, so a
///   later result replaces an earlier one.
///
/// - [`close_frame`](Self::close_frame) returns the snapshot together with the
///   result published while the frame was open, if any. The suspended inspector
///   applies the difference through [`apply_overlay_diff`].
///
/// A result belongs to the frame that was innermost when it was published. A
/// dispatch that does not re-enter this rollup therefore closes without a
/// result; it must not see a result left by an earlier callback, which may
/// predate state the suspended execution has since changed. Two independent
/// push/pop stacks cannot provide this: publishes happen per re-entry and pops
/// per dispatch, so a dispatch with no re-entry, or with several, unbalances
/// them.
///
/// All access is same-thread; a poisoned lock means a dispatch panicked
/// mid-exchange, so methods panic rather than silently dropping state.
#[derive(Debug, Default)]
pub struct OverlayChannel {
    /// Open dispatch frames, innermost last.
    frames: Mutex<Vec<DispatchFrame>>,
}

/// One dispatch from a suspended rollup, as returned by
/// [`OverlayChannel::close_frame`].
#[derive(Debug)]
pub struct DispatchFrame {
    /// The rollup's live cache when the dispatch began.
    pub pre_dispatch: OverlaySnapshot,
    /// The latest cumulative cache a re-entered session published during the
    /// dispatch, or `None` when the dispatch did not re-enter the rollup.
    pub result: Option<OverlaySnapshot>,
}

/// A poisoned lock means a panic escaped mid-exchange; continuing would
/// silently drop state propagation.
const POISONED: &str = "overlay channel mutex poisoned by a panicked dispatch";

impl OverlayChannel {
    /// Clear all frames at a transaction-composition boundary.
    pub fn reset(&self) {
        self.frames.lock().expect(POISONED).clear();
    }

    /// Open a frame for a dispatch from this rollup.
    pub fn open_frame(&self, pre_dispatch: OverlaySnapshot) {
        self.frames.lock().expect(POISONED).push(DispatchFrame {
            pre_dispatch,
            result: None,
        });
    }

    /// Clone the innermost open frame's snapshot for a re-entered session.
    pub fn peek_pre_snapshot(&self) -> Option<OverlaySnapshot> {
        self.frames
            .lock()
            .expect(POISONED)
            .last()
            .map(|frame| frame.pre_dispatch.clone())
    }

    /// Refresh a retained target session before its next re-entry checkpoint.
    ///
    /// A new dispatch can follow local writes by the resumed source, so a
    /// session left over from an earlier dispatch must use the new snapshot.
    /// After a callback publishes in this frame, keep the session's cumulative
    /// cache instead: sibling callbacks share it, including queued rollbacks
    /// already applied by the composition builder. Reapplying the original
    /// snapshot, or the last published result, would undo those changes.
    /// With no suspended frame this leaves ordinary target-session state intact.
    pub fn refresh_before_reentry(&self, cache: &mut OverlaySnapshot) {
        if let Some(frame) = self.frames.lock().expect(POISONED).last()
            && frame.result.is_none()
        {
            *cache = frame.pre_dispatch.clone();
        }
    }

    /// Record a re-entered session's cumulative cache as the innermost open
    /// frame's result, replacing any earlier result of that frame.
    ///
    /// Returns `false` and drops the cache when no frame is open: no suspended
    /// execution of this rollup exists to apply it.
    pub fn publish(&self, cache: OverlaySnapshot) -> bool {
        match self.frames.lock().expect(POISONED).last_mut() {
            Some(frame) => {
                frame.result = Some(cache);
                true
            }
            None => false,
        }
    }

    /// Close the innermost frame at the end of its dispatch.
    pub fn close_frame(&self) -> Option<DispatchFrame> {
        self.frames.lock().expect(POISONED).pop()
    }
}

/// Shared handle to an [`OverlayChannel`].
pub type OverlayChannelHandle = Arc<OverlayChannel>;

/// Build a fresh, empty [`OverlayChannelHandle`].
#[must_use]
pub fn new_overlay_channel() -> OverlayChannelHandle {
    Arc::new(OverlayChannel::default())
}

/// Errors surfaced by the overlay diff-apply path.
///
/// Destruction remains unsupported by this account-state exchange. A returned
/// cache that introduces or changes a destroyed account is rejected; only an
/// unchanged destruction already in the source snapshot can pass through.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    /// The nested state shows an account in `SelfDestructed` state
    /// that the source state does not. Out of scope for the overlay
    /// diff-apply path.
    #[error("SELFDESTRUCT mutation at {address}: out of scope for overlay diff-apply")]
    Selfdestruct {
        /// Address of the destroyed account.
        address: Address,
    },
    /// Source-state journal returned a database error while loading an
    /// account during diff-apply. Surfaced verbatim (Display).
    #[error("source state load_account({address}) failed: {message}")]
    SourceLoad {
        /// Address whose load failed.
        address: Address,
        /// Underlying database error rendered via Display.
        message: String,
    },
    /// The returned cache must carry newly installed bytecode: it cannot be
    /// recovered from the source chain's pre-execution database.
    #[error("overlay bytecode {hash} is missing for {address}")]
    MissingCode {
        /// Account receiving code.
        address: Address,
        /// Expected runtime code hash.
        hash: B256,
    },
    /// A returned code object does not match the account's code hash.
    #[error("overlay bytecode hash mismatch for {address}: expected {expected}, got {actual}")]
    CodeHashMismatch {
        /// Account receiving code.
        address: Address,
        /// Hash recorded in the returned account.
        expected: B256,
        /// Hash of the returned runtime code.
        actual: B256,
    },
    /// The EVM rejected the journaled creation transition.
    #[error("source state creation checkpoint for {address} failed: {message}")]
    SourceCreation {
        /// New contract address.
        address: Address,
        /// Underlying journal error.
        message: String,
    },
    /// `journal.sstore` failed during diff-apply.
    #[error("source state sstore({address}, {key}) failed: {message}")]
    SourceSstore {
        /// Address whose storage write failed.
        address: Address,
        /// Storage key whose write failed.
        key: U256,
        /// Underlying database error rendered via Display.
        message: String,
    },
}

/// Apply a successful callback through the source journal; discard the composition on error.
pub fn apply_overlay_diff(
    state: &mut State<'_>,
    before: &OverlaySnapshot,
    after: &OverlaySnapshot,
    features: EvmFeatures,
) -> Result<(), OverlayError> {
    after.apply_to(state, before, features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{KECCAK256_EMPTY, bytes};
    use evm2::{
        block::bundle_state::AccountStatus,
        bytecode::Bytecode,
        evm::{AccountInfo, Cache, InMemoryDB},
    };

    fn put(
        snapshot: &mut OverlaySnapshot,
        address: Address,
        info: AccountInfo,
        status: AccountStatus,
        storage: &[(U256, U256)],
    ) {
        snapshot.cache.accounts.insert(address, Some(info));
        snapshot.statuses.insert(address, status);
        snapshot
            .cache
            .storage
            .entry(address)
            .or_default()
            .slots
            .extend(storage.iter().copied());
    }
    fn apply(
        state: &mut State<'_>,
        before: &OverlaySnapshot,
        after: &OverlaySnapshot,
    ) -> Result<(), OverlayError> {
        apply_overlay_diff(state, before, after, EvmFeatures::EIP161)
    }
    fn info(state: &mut State<'_>, address: Address) -> AccountInfo {
        state
            .account(&address)
            .unwrap()
            .get()
            .cloned()
            .unwrap_or_default()
    }
    fn slot(state: &mut State<'_>, address: Address) -> U256 {
        state
            .storage_slot(&address, U256::ZERO)
            .unwrap()
            .get()
            .current
    }
    #[test]
    fn issue208_nested_return_preserves_created_contract_code() {
        let child = Address::with_last_byte(0x42);
        let code = Bytecode::new_raw(bytes!("602a60005260206000f3"));
        let expected = AccountInfo::empty().with_code(code.clone()).with_nonce(1);
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        after.cache.contracts.insert(expected.code_hash, code);
        put(
            &mut after,
            child,
            expected.clone(),
            AccountStatus::InMemoryChange,
            &[(U256::ZERO, U256::from(42))],
        );
        let mut state = State::new(InMemoryDB::default());
        apply(&mut state, &before, &after).unwrap();
        assert_eq!(slot(&mut state, child), U256::from(42));
        assert_eq!(info(&mut state, child).code_hash, expected.code_hash);
        assert_eq!(info(&mut state, child).nonce, 1);
    }
    #[test]
    fn issue208_nested_return_preserves_factory_create_nonce() {
        let factory = Address::with_last_byte(0x43);
        let initial = AccountInfo::empty().with_nonce(1);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&factory, initial.clone());
        let mut before = OverlaySnapshot::default();
        put(
            &mut before,
            factory,
            initial.clone(),
            AccountStatus::Loaded,
            &[],
        );
        let mut after = before.clone();
        put(
            &mut after,
            factory,
            initial.with_nonce(2),
            AccountStatus::Changed,
            &[],
        );
        let mut state = State::new(db);
        apply(&mut state, &before, &after).unwrap();
        assert_eq!(info(&mut state, factory).nonce, 2);
    }
    #[test]
    fn created_account_code_balance_storage_and_flags_revert_together() {
        for (warm, balance) in [(false, 0), (true, 0), (true, 11)] {
            let child = Address::with_last_byte(0x44);
            let initial = AccountInfo::empty().with_balance(U256::from(balance));
            let mut db = InMemoryDB::default();
            if balance != 0 {
                db.insert_account_info(&child, initial.clone());
            }
            let mut state = State::new(db);
            let mut before = OverlaySnapshot::default();
            if warm {
                state.account(&child).unwrap();
                put(&mut before, child, initial, AccountStatus::Loaded, &[]);
            }
            let checkpoint = state.checkpoint();
            let code = Bytecode::new_raw(bytes!("602a60005260206000f3"));
            let mut expected = AccountInfo::empty()
                .with_code(code.clone())
                .with_nonce(2)
                .with_balance(U256::from(17));
            expected.code = None;
            let mut after = before.clone();
            after
                .cache
                .contracts
                .insert(expected.code_hash, code.clone());
            put(
                &mut after,
                child,
                expected,
                AccountStatus::InMemoryChange,
                &[(U256::ZERO, U256::from(42))],
            );
            apply(&mut state, &before, &after).unwrap();
            assert!(state.account(&child).unwrap().is_created());
            let actual = info(&mut state, child);
            assert_eq!(actual.nonce, 2);
            assert_eq!(actual.balance, U256::from(17));
            assert_eq!(actual.code.as_ref(), Some(&code));
            assert_eq!(slot(&mut state, child), U256::from(42));
            state.rollback(checkpoint, EvmFeatures::EIP161);
            assert!(!state.account(&child).unwrap().is_created());
            let actual = info(&mut state, child);
            assert_eq!(actual.code_hash, KECCAK256_EMPTY);
            assert_eq!(actual.nonce, 0);
            assert_eq!(actual.balance, U256::from(balance));
            assert_eq!(slot(&mut state, child), U256::ZERO);
            assert!(
                state
                    .account(&child)
                    .unwrap()
                    .load_code()
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn factory_nonce_updates_accumulate_and_revert() {
        let factory = Address::with_last_byte(0x45);
        let initial = AccountInfo::empty().with_nonce(1);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&factory, initial.clone());
        let mut state = State::new(db);
        let mut before = OverlaySnapshot::default();
        put(
            &mut before,
            factory,
            initial.clone(),
            AccountStatus::Loaded,
            &[],
        );
        let checkpoint = state.checkpoint();
        for nonce in [3, 4] {
            let mut after = before.clone();
            put(
                &mut after,
                factory,
                initial.clone().with_nonce(nonce),
                AccountStatus::Changed,
                &[],
            );
            apply(&mut state, &before, &after).unwrap();
            assert_eq!(info(&mut state, factory).nonce, nonce);
            assert!(!state.account(&factory).unwrap().is_created());
            before = after;
        }
        state.rollback(checkpoint, EvmFeatures::EIP161);
        assert_eq!(info(&mut state, factory).nonce, 1);
    }
    #[test]
    fn callback_can_clear_cold_storage_and_change_a_cold_account_balance() {
        let address = Address::with_last_byte(0x46);
        let initial = AccountInfo::empty()
            .with_nonce(1)
            .with_balance(U256::from(10));
        let mut db = InMemoryDB::default();
        db.insert_account_info(&address, initial.clone());
        db.insert_account_storage(&address, &U256::ZERO, &U256::from(7));
        let mut state = State::new(db);
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        put(
            &mut after,
            address,
            initial.with_nonce(2).with_balance(U256::from(4)),
            AccountStatus::Changed,
            &[(U256::ZERO, U256::ZERO)],
        );
        let checkpoint = state.checkpoint();
        apply(&mut state, &before, &after).unwrap();
        assert_eq!(slot(&mut state, address), U256::ZERO);
        assert_eq!(info(&mut state, address).balance, U256::from(4));
        assert_eq!(info(&mut state, address).nonce, 2);
        assert!(!state.account(&address).unwrap().is_created());
        state.rollback(checkpoint, EvmFeatures::EIP161);
        assert_eq!(slot(&mut state, address), U256::from(7));
        assert_eq!(info(&mut state, address).balance, U256::from(10));
        assert_eq!(info(&mut state, address).nonce, 1);
    }
    #[test]
    fn read_only_cache_entries_do_not_replace_live_source_state() {
        let address = Address::with_last_byte(0x47);
        let mut state = State::new(InMemoryDB::default());
        state.account(&address).unwrap().set_balance(U256::from(99));
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        put(
            &mut after,
            address,
            AccountInfo::empty().with_balance(U256::from(10)),
            AccountStatus::Loaded,
            &[],
        );
        apply(&mut state, &before, &after).unwrap();
        assert_eq!(info(&mut state, address).balance, U256::from(99));
    }
    #[test]
    fn newly_installed_bytecode_must_be_present_and_match_its_hash() {
        let address = Address::with_last_byte(0x48);
        let code = Bytecode::new_raw(bytes!("00"));
        let mut expected = AccountInfo::empty().with_code(code).with_nonce(1);
        let hash = expected.code_hash;
        expected.code = None;
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        put(
            &mut after,
            address,
            expected,
            AccountStatus::InMemoryChange,
            &[],
        );
        let mut state = State::new(InMemoryDB::default());
        assert!(
            matches!(apply(&mut state,&before,&after),Err(OverlayError::MissingCode {address:found,hash:missing}) if found==address && missing==hash)
        );
        after.cache.contracts.insert(hash, Bytecode::default());
        assert!(
            matches!(apply(&mut state,&before,&after),Err(OverlayError::CodeHashMismatch {address:found,..}) if found==address)
        );
    }
    #[test]
    fn callback_selfdestruction_is_rejected_by_the_apply_path() {
        let address = Address::with_last_byte(0x49);
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        after.statuses.insert(address, AccountStatus::Destroyed);
        let mut state = State::new(InMemoryDB::default());
        assert!(
            matches!(apply(&mut state,&before,&after),Err(OverlayError::Selfdestruct{address:found}) if found==address)
        );
    }
    fn marked(byte: u8) -> OverlaySnapshot {
        let mut snapshot = OverlaySnapshot::default();
        snapshot
            .cache
            .accounts
            .insert(Address::with_last_byte(byte), None);
        snapshot
    }
    #[test]
    fn retained_session_refreshes_for_a_new_dispatch_but_preserves_siblings_and_rollbacks() {
        let channel = OverlayChannel::default();
        let mut cache = marked(1);
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(cache, marked(1), "ordinary session has no suspended source");

        channel.open_frame(marked(2));
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(cache, marked(2));
        cache = marked(3);
        assert!(channel.publish(cache.clone()));
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(cache, marked(3), "sibling callback keeps cumulative state");

        // The builder applies a queued rollback before taking the next
        // checkpoint. Neither the old prestate nor the last result can replace it.
        cache = marked(4);
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(cache, marked(4));
        channel.close_frame().unwrap();

        // The original source resumes, writes locally, and dispatches again.
        channel.open_frame(marked(5));
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(
            cache,
            marked(5),
            "new source writes replace stale session state"
        );
        assert!(channel.publish(cache.clone()));
        channel.open_frame(marked(6));
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(cache, marked(6), "recursive reentry uses its own frame");
        channel.close_frame().unwrap();
        cache = marked(7);
        channel.refresh_before_reentry(&mut cache);
        assert_eq!(
            cache,
            marked(7),
            "outer frame still has its own published result"
        );
    }

    #[test]
    fn frames_preload_and_close_innermost_first() {
        let channel = OverlayChannel::default();
        channel.open_frame(marked(1));
        channel.open_frame(marked(2));
        assert_eq!(channel.peek_pre_snapshot(), Some(marked(2)));

        let inner = channel.close_frame().expect("inner frame");
        assert_eq!(inner.pre_dispatch, marked(2));
        assert_eq!(channel.peek_pre_snapshot(), Some(marked(1)));
        let outer = channel.close_frame().expect("outer frame");
        assert_eq!(outer.pre_dispatch, marked(1));
        assert!(channel.close_frame().is_none());
        assert_eq!(channel.peek_pre_snapshot(), None);
    }

    /// The reported failure: an earlier callback's result is pending for an
    /// outer frame when an inner dispatch returns without re-entry. The inner
    /// frame must close empty and leave the outer frame's result in place.
    #[test]
    fn dispatch_without_reentry_does_not_take_an_outer_result() {
        let channel = OverlayChannel::default();
        channel.open_frame(marked(1));
        assert!(channel.publish(marked(10)));
        channel.open_frame(marked(2));

        let inner = channel.close_frame().expect("inner frame");
        assert_eq!(inner.result, None);
        let outer = channel.close_frame().expect("outer frame");
        assert_eq!(outer.result, Some(marked(10)));
    }

    /// Sibling callbacks publish cumulative caches of one reused session; only
    /// the latest belongs to the frame, and none is left for a later dispatch.
    #[test]
    fn later_result_replaces_earlier_result_of_the_same_frame() {
        let channel = OverlayChannel::default();
        channel.open_frame(marked(1));
        assert!(channel.publish(marked(10)));
        assert!(channel.publish(marked(11)));
        assert_eq!(
            channel.close_frame().expect("frame").result,
            Some(marked(11))
        );

        channel.open_frame(marked(2));
        assert_eq!(channel.close_frame().expect("later frame").result, None);
    }

    #[test]
    fn recursive_results_stay_with_their_frames() {
        let channel = OverlayChannel::default();
        channel.open_frame(marked(1));
        channel.open_frame(marked(2));
        assert!(channel.publish(marked(20)));
        assert_eq!(
            channel.close_frame().expect("inner frame").result,
            Some(marked(20))
        );
        assert!(channel.publish(marked(10)));
        assert_eq!(
            channel.close_frame().expect("outer frame").result,
            Some(marked(10))
        );
    }

    #[test]
    fn publish_without_an_open_frame_is_dropped() {
        let channel = OverlayChannel::default();
        assert!(!channel.publish(marked(10)));
        channel.open_frame(marked(1));
        assert_eq!(channel.close_frame().expect("frame").result, None);
    }

    #[test]
    fn reset_clears_open_frames() {
        let channel = OverlayChannel::default();
        channel.open_frame(marked(1));
        assert!(channel.publish(marked(10)));

        channel.reset();

        assert!(channel.close_frame().is_none());
        assert_eq!(channel.peek_pre_snapshot(), None);
    }

    /// Keep the Send checkpoint's complete native cache and lifecycle metadata together.
    #[test]
    fn clone_state_field_parity_default() {
        let mut source = marked(1);
        source
            .cache
            .storage
            .entry(Address::ZERO)
            .or_default()
            .wipe();
        source
            .cache
            .block_hashes
            .insert(U256::ONE, B256::repeat_byte(7));
        source
            .statuses
            .insert(Address::ZERO, AccountStatus::DestroyedAgain);
        let Cache {
            accounts,
            contracts,
            storage,
            block_hashes,
            _non_exhaustive: (),
        } = source.cache.clone();
        let cloned = OverlaySnapshot {
            cache: Cache {
                accounts,
                contracts,
                storage,
                block_hashes,
                _non_exhaustive: (),
            },
            statuses: source.statuses.clone(),
        };
        assert_eq!(source, cloned);
    }
    #[test]
    fn unchanged_destroyed_account_is_not_a_new_selfdestruct() {
        let mut before = OverlaySnapshot::default();
        before
            .statuses
            .insert(Address::ZERO, AccountStatus::Destroyed);
        before.cache.accounts.insert(Address::ZERO, None);
        let after = before.clone();
        apply(&mut State::new(InMemoryDB::default()), &before, &after).unwrap();
    }
    #[test]
    fn new_or_changed_destroyed_account_remains_unsupported() {
        let before = OverlaySnapshot::default();
        let mut after = before.clone();
        after
            .statuses
            .insert(Address::ZERO, AccountStatus::Destroyed);
        let mut state = State::new(InMemoryDB::default());
        assert!(matches!(
            apply(&mut state, &before, &after),
            Err(OverlayError::Selfdestruct { .. })
        ));
        let mut before = after.clone();
        after
            .statuses
            .insert(Address::ZERO, AccountStatus::DestroyedAgain);
        assert!(matches!(
            apply(&mut state, &before, &after),
            Err(OverlayError::Selfdestruct { .. })
        ));
        before.statuses.insert(Address::ZERO, AccountStatus::Loaded);
        assert!(matches!(
            apply(&mut state, &before, &after),
            Err(OverlayError::Selfdestruct { .. })
        ));
    }
}
