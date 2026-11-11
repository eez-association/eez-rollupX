//! Overlay primitives for shared-source-state nested dispatch.
//!
//! When a target-session inspector dispatches a nested call back to
//! the entry rollup (e.g. L2→L1 during flash-loan or reentrant), the
//! nested call needs to see source-sim's in-flight state — direct
//! counterpart to how reth's revm executes nested CALL opcodes by
//! sharing the live `State<DB>`.
//!
//! The inspector's synchronous dispatch keeps
//! source-sim's `evm.transact`, target-session execution, and
//! nested-back-to-entry dispatch all on the same OS thread — no
//! Send/Sync requirements, no cross-thread cloning. The re-entered
//! session opens a fresh `State<DB>` preloaded with the published
//! snapshot via `StateBuilder::with_cached_prestate`, runs the nested
//! call against it, and publishes its post-execute cache; the
//! suspended inspector then applies the cache delta as journal
//! entries onto source-sim's live state.
//!
//! Sharing works through `CacheState` because revm's `State<DB>`
//! itself lacks `Clone`: `with_cached_prestate` preloads a fresh
//! `State` with a cloned cache, reading cold data from a fresh
//! database instance. (`clone_state` in this module's tests guards
//! that assumption against revm field drift.)
//!
//! # Crate-layering invariants
//!
//! This module lives in `eez-evm-inspector` because that crate
//! already depends on `revm`. It must not depend on `reth_*` or
//! `eez-composer`. The actual nested-call execution + diff-apply path
//! lives in this crate's `inspector.rs` (which has the necessary
//! `&mut Context` access during the `Inspector::call` hook).

use std::sync::{Arc, Mutex};

use revm::bytecode::Bytecode;
use revm::context_interface::journaled_state::account::JournaledAccountTr;
use revm::context_interface::{Cfg, ContextTr, Host, JournalTr};
use revm::database::{AccountStatus, CacheState, states::CacheAccount};
use revm::primitives::{Address, B256, KECCAK_EMPTY, U256};

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
    pub pre_dispatch: CacheState,
    /// The latest cumulative cache a re-entered session published during the
    /// dispatch, or `None` when the dispatch did not re-enter the rollup.
    pub result: Option<CacheState>,
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
    pub fn open_frame(&self, pre_dispatch: CacheState) {
        self.frames.lock().expect(POISONED).push(DispatchFrame {
            pre_dispatch,
            result: None,
        });
    }

    /// Clone the innermost open frame's snapshot for a re-entered session.
    pub fn peek_pre_snapshot(&self) -> Option<CacheState> {
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
    pub fn refresh_before_reentry(&self, cache: &mut CacheState) {
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
    pub fn publish(&self, cache: CacheState) -> bool {
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
    /// REVM rejected the journaled creation transition.
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
        key: revm::primitives::StorageKey,
        /// Underlying database error rendered via Display.
        message: String,
    },
}

/// Whether the overlay introduced or changed a destroyed account.
fn has_unsupported_destruction(
    before: &CacheState,
    address: &Address,
    after_account: &CacheAccount,
) -> bool {
    after_account.status.was_destroyed() && before.accounts.get(address) != Some(after_account)
}

/// Apply the per-account, per-slot delta between two cache snapshots
/// onto a live revm context's source state, via journal entries so the
/// outer transaction's revert semantics propagate naturally.
///
/// # When this is called
///
/// The inspector hook fires for a cross-chain CALL, snapshots the
/// source cache as `before` in a new overlay-channel frame, and drives
/// the dispatch. A nested dispatch back into this rollup opens its
/// session preloaded with `before` and publishes its post-execute cache
/// into that frame. After a successful dispatch, the inspector closes
/// the frame and calls this function with its result as `after`.
///
/// # Mutation classes
///
/// In scope for the diff-apply path:
///
/// - **Storage writes:** for each account in `after`, for each storage
///   slot whose value differs from `before`, emit
///   `ctx.journal_mut().sstore(addr, key, value)`. The journal entry
///   gives revert-safety (a user-tx revert in source-sim unwinds the
///   diff-applied entries together with source-sim's own writes).
///
/// - **Balance, nonce and code:** apply changed values through journaled
///   account setters. Use the source journal as the baseline when the callback
///   first loaded an account, rather than treating an absent cache entry as an
///   empty account. Newly installed code must be present in the returned cache.
/// - **Creation:** an ordinary contract created in the callback is marked
///   created through a REVM creation checkpoint before its state is applied.
///   Installing code alone loses transaction-local creation and rollback rules.
///
/// Out of scope — surfaced as `Err(OverlayError::*)` or skipped:
///
/// - **SELFDESTRUCT:** new or changed destruction states are rejected via
///   [`OverlayError::Selfdestruct`]; this overlay does not reconstruct their
///   transaction-local destruction history from the returned cache.
///
/// - **Transient storage:** not represented by `CacheState`; exchanging it
///   across synthetic target sessions is outside this account-state overlay.
///
/// # Idempotency
///
/// The inspector closes the frame before this call, so each result is
/// applied at most once. Calling it twice on the same `(before, after)` is
/// well-defined but emits redundant journal entries; callers should not do
/// this.
///
/// # Errors
///
/// - [`OverlayError::Selfdestruct`] if the overlay's cache shows an
///   account whose status flipped to destroyed during overlay execution.
/// - Missing or inconsistent bytecode, or a source database/journal failure.
///   Errors abort composition; callers must not commit a partially applied diff.
pub fn apply_overlay_diff<CTX>(
    ctx: &mut CTX,
    before: &CacheState,
    after: &CacheState,
) -> Result<(), OverlayError>
where
    CTX: ContextTr + Host,
{
    for (addr, after_acc) in &after.accounts {
        // A destroyed account can be carried unchanged from the source
        // snapshot. Only a new or changed destruction is a mutation that
        // this diff-apply path cannot represent.
        if has_unsupported_destruction(before, addr, after_acc) {
            return Err(OverlayError::Selfdestruct { address: *addr });
        }
        if after_acc.status.was_destroyed() {
            continue;
        }

        // Filter: only apply for accounts the overlay actually modified.
        // `Loaded` / `LoadedEmptyEIP161` / `LoadedNotExisting` mark
        // accounts the overlay merely *read* (cold-load to populate
        // the cache); applying their info to source is a no-op at best
        // and a "set_balance to disk value" misstep at worst.
        if !matches!(
            after_acc.status,
            AccountStatus::Changed | AccountStatus::InMemoryChange
        ) {
            continue;
        }

        let Some(after_plain) = after_acc.account.as_ref() else {
            continue;
        };
        let before_plain = before.accounts.get(addr).and_then(|a| a.account.as_ref());

        let source_info = ctx
            .journal_mut()
            .load_account(*addr)
            .map_err(|e| OverlayError::SourceLoad {
                address: *addr,
                message: format!("{e:?}"),
            })?
            .data
            .info
            .clone();
        let new_code = if source_info.code_hash == after_plain.info.code_hash {
            None
        } else {
            let hash = after_plain.info.code_hash;
            let code = if hash == KECCAK_EMPTY {
                Bytecode::default()
            } else {
                after_plain
                    .info
                    .code
                    .clone()
                    .or_else(|| after.contracts.get(&hash).cloned())
                    .ok_or(OverlayError::MissingCode {
                        address: *addr,
                        hash,
                    })?
            };
            let actual = code.hash_slow();
            if actual != hash {
                return Err(OverlayError::CodeHashMismatch {
                    address: *addr,
                    expected: hash,
                    actual,
                });
            }
            Some(code)
        };

        if source_info.has_no_code_and_nonce()
            && after_acc.status == AccountStatus::InMemoryChange
            && after_plain.info.nonce != 0
        {
            // Target sessions execute ordinary CALLs without authorization
            // lists: a new nonce-bearing account here comes from CREATE/CREATE2.
            // Zero value makes the checkpoint's caller unused; balances are
            // applied separately below. Commit this inner checkpoint while
            // retaining its creation entry for an enclosing source-frame revert.
            let spec = ctx.cfg().spec().into();
            ctx.journal_mut()
                .create_account_checkpoint(*addr, *addr, U256::ZERO, spec)
                .map_err(|e| OverlayError::SourceCreation {
                    address: *addr,
                    message: format!("{e:?}"),
                })?;
            ctx.journal_mut().checkpoint_commit();
        }

        {
            let mut loaded = ctx.journal_mut().load_account_mut(*addr).map_err(|e| {
                OverlayError::SourceLoad {
                    address: *addr,
                    message: format!("{e:?}"),
                }
            })?;
            if *loaded.data.balance() != after_plain.info.balance {
                loaded.data.set_balance(after_plain.info.balance);
            }
            if loaded.data.nonce() != after_plain.info.nonce {
                loaded.data.set_nonce(after_plain.info.nonce);
            }
            if let Some(code) = new_code {
                loaded.data.set_code(after_plain.info.code_hash, code);
            }
        }

        // Iterate after's storage; emit sstore for every slot whose
        // value differs from before.
        for (key, after_val) in &after_plain.storage {
            let before_val = before_plain.and_then(|b| b.storage.get(key).copied());
            // A slot absent from `before` may be nonzero in the database.
            // The callback can clear it, so an unknown value is not zero.
            if before_val == Some(*after_val) {
                continue;
            }
            ctx.journal_mut()
                .sstore(*addr, *key, *after_val)
                .map_err(|e| OverlayError::SourceSstore {
                    address: *addr,
                    key: *key,
                    message: format!("{e:?}"),
                })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::MainContext;
    use revm::context::Context;
    use revm::database::CacheDB;
    use revm::database::{EmptyDB, State};
    use revm::primitives::bytes;
    use revm::state::AccountInfo;

    #[test]
    fn issue208_nested_return_preserves_created_contract_code() {
        let child = Address::with_last_byte(0x42);
        let code = Bytecode::new_raw(bytes!("602a60005260206000f3"));
        let expected = AccountInfo::default().with_code(code.clone()).with_nonce(1);
        let before = CacheState::default();
        let mut after = before.clone();
        after.contracts.insert(expected.code_hash, code);
        after.accounts.insert(
            child,
            CacheAccount::new_newly_created(
                expected.clone(),
                [(U256::ZERO, U256::from(42))].into_iter().collect(),
            ),
        );
        let mut context = Context::mainnet().with_db(CacheDB::<EmptyDB>::default());
        apply_overlay_diff(&mut context, &before, &after).unwrap();
        // Prove the production apply path ran and carried the same child's
        // storage before checking that resumed execution can see its code.
        assert_eq!(
            context.sload(child, U256::ZERO).unwrap().data,
            U256::from(42)
        );
        let actual = &context.journal_mut().load_account(child).unwrap().data.info;
        assert_eq!(
            actual.code_hash, expected.code_hash,
            "new code was lost on nested return"
        );
        assert_eq!(actual.nonce, 1);
    }

    #[test]
    fn issue208_nested_return_preserves_factory_create_nonce() {
        let factory = Address::with_last_byte(0x43);
        let initial = AccountInfo::default().with_nonce(1);
        let mut database = CacheDB::<EmptyDB>::default();
        database.insert_account_info(factory, initial.clone());
        let mut before = CacheState::default();
        before.insert_account(factory, initial.clone());
        let mut after = before.clone();
        after.accounts.insert(
            factory,
            CacheAccount::new_changed(initial.with_nonce(2), Default::default()),
        );
        let mut context = Context::mainnet().with_db(database);
        apply_overlay_diff(&mut context, &before, &after).unwrap();
        let actual = context
            .journal_mut()
            .load_account(factory)
            .unwrap()
            .data
            .info
            .nonce;
        assert_eq!(actual, 2, "factory nonce was lost on nested return");
    }

    #[test]
    fn created_account_code_balance_storage_and_flags_revert_together() {
        // Include a source that already observed empty code, and a prefunded
        // CREATE address. Reverting creation must preserve the prior balance.
        for (warm, balance) in [(false, 0), (true, 0), (true, 11)] {
            let child = Address::with_last_byte(0x44);
            let initial = AccountInfo::default().with_balance(U256::from(balance));
            let mut db = CacheDB::<EmptyDB>::default();
            if balance != 0 {
                db.insert_account_info(child, initial.clone());
            }
            let mut context = Context::mainnet().with_db(db);
            let mut before = CacheState::default();
            if warm {
                context.journal_mut().load_account(child).unwrap();
                before.insert_account(child, initial);
            }
            let checkpoint = context.journal_mut().checkpoint();
            let code = Bytecode::new_raw(bytes!("602a60005260206000f3"));
            let mut expected = AccountInfo::default()
                .with_code(code.clone())
                .with_nonce(2)
                .with_balance(U256::from(17));
            // The code may live in CacheState::contracts rather than in info.
            expected.code = None;
            let mut after = before.clone();
            after.contracts.insert(expected.code_hash, code.clone());
            after.accounts.insert(
                child,
                CacheAccount::new_newly_created(
                    expected.clone(),
                    [(U256::ZERO, U256::from(42))].into_iter().collect(),
                ),
            );

            apply_overlay_diff(&mut context, &before, &after).unwrap();
            let account = context.journal_mut().load_account(child).unwrap().data;
            assert!(account.is_created_locally());
            assert!(account.is_created());
            assert_eq!(account.info.nonce, 2);
            assert_eq!(account.info.balance, U256::from(17));
            assert_eq!(account.info.code.as_ref(), Some(&code));
            assert_eq!(
                context.sload(child, U256::ZERO).unwrap().data,
                U256::from(42)
            );

            context.journal_mut().checkpoint_revert(checkpoint);
            let account = context.journal_mut().load_account(child).unwrap().data;
            assert!(!account.is_created_locally());
            assert!(!account.is_created());
            assert_eq!(account.info.code_hash, KECCAK_EMPTY);
            assert_eq!(account.info.nonce, 0);
            assert_eq!(account.info.balance, U256::from(balance));
            assert_eq!(context.sload(child, U256::ZERO).unwrap().data, U256::ZERO);
            assert!(context.journal_mut().code(child).unwrap().data.is_empty());
        }
    }

    #[test]
    fn factory_nonce_updates_accumulate_and_revert() {
        let factory = Address::with_last_byte(0x45);
        let initial = AccountInfo::default().with_nonce(1);
        let mut db = CacheDB::<EmptyDB>::default();
        db.insert_account_info(factory, initial.clone());
        let mut context = Context::mainnet().with_db(db);
        let mut before = CacheState::default();
        before.insert_account(factory, initial.clone());
        let checkpoint = context.journal_mut().checkpoint();
        for nonce in [3, 4] {
            let mut after = before.clone();
            after.accounts.insert(
                factory,
                CacheAccount::new_changed(initial.clone().with_nonce(nonce), Default::default()),
            );
            apply_overlay_diff(&mut context, &before, &after).unwrap();
            let account = context.journal_mut().load_account(factory).unwrap().data;
            assert_eq!(account.info.nonce, nonce);
            assert!(!account.is_created_locally());
            before = after;
        }
        context.journal_mut().checkpoint_revert(checkpoint);
        assert_eq!(
            context
                .journal_mut()
                .load_account(factory)
                .unwrap()
                .data
                .info
                .nonce,
            1
        );
    }

    #[test]
    fn callback_can_clear_cold_storage_and_change_a_cold_account_balance() {
        let address = Address::with_last_byte(0x46);
        let initial = AccountInfo::default()
            .with_nonce(1)
            .with_balance(U256::from(10));
        let mut db = CacheDB::<EmptyDB>::default();
        db.insert_account_info(address, initial.clone());
        db.insert_account_storage(address, U256::ZERO, U256::from(7))
            .unwrap();
        let mut context = Context::mainnet().with_db(db);
        let before = CacheState::default();
        let mut after = before.clone();
        after.accounts.insert(
            address,
            CacheAccount::new_changed(
                initial.with_nonce(2).with_balance(U256::from(4)),
                [(U256::ZERO, U256::ZERO)].into_iter().collect(),
            ),
        );
        let checkpoint = context.journal_mut().checkpoint();
        apply_overlay_diff(&mut context, &before, &after).unwrap();
        assert_eq!(context.sload(address, U256::ZERO).unwrap().data, U256::ZERO);
        let account = context.journal_mut().load_account(address).unwrap().data;
        assert_eq!(account.info.balance, U256::from(4));
        assert_eq!(account.info.nonce, 2);
        assert!(!account.is_created());
        context.journal_mut().checkpoint_revert(checkpoint);
        assert_eq!(
            context.sload(address, U256::ZERO).unwrap().data,
            U256::from(7)
        );
        let account = context.journal_mut().load_account(address).unwrap().data;
        assert_eq!(account.info.balance, U256::from(10));
        assert_eq!(account.info.nonce, 1);
    }

    #[test]
    fn read_only_cache_entries_do_not_replace_live_source_state() {
        let address = Address::with_last_byte(0x47);
        let mut context = Context::mainnet().with_db(CacheDB::<EmptyDB>::default());
        context
            .journal_mut()
            .load_account_mut(address)
            .unwrap()
            .data
            .set_balance(U256::from(99));
        let before = CacheState::default();
        let mut after = before.clone();
        after.insert_account(address, AccountInfo::default().with_balance(U256::from(10)));
        apply_overlay_diff(&mut context, &before, &after).unwrap();
        assert_eq!(
            context
                .journal_mut()
                .load_account(address)
                .unwrap()
                .data
                .info
                .balance,
            U256::from(99)
        );
    }

    #[test]
    fn newly_installed_bytecode_must_be_present_and_match_its_hash() {
        let address = Address::with_last_byte(0x48);
        let code = Bytecode::new_raw(bytes!("00"));
        let mut expected = AccountInfo::default().with_code(code).with_nonce(1);
        let hash = expected.code_hash;
        expected.code = None;
        let before = CacheState::default();
        let mut after = before.clone();
        after.accounts.insert(
            address,
            CacheAccount::new_newly_created(expected, Default::default()),
        );
        let mut context = Context::mainnet().with_db(CacheDB::<EmptyDB>::default());
        assert!(
            matches!(apply_overlay_diff(&mut context, &before, &after), Err(OverlayError::MissingCode { address: found, hash: missing }) if found == address && missing == hash)
        );
        after.contracts.insert(hash, Bytecode::default());
        assert!(
            matches!(apply_overlay_diff(&mut context, &before, &after), Err(OverlayError::CodeHashMismatch { address: found, .. }) if found == address)
        );
    }

    #[test]
    fn callback_selfdestruction_is_rejected_by_the_apply_path() {
        let address = Address::with_last_byte(0x49);
        let before = CacheState::default();
        let mut after = before.clone();
        after
            .accounts
            .insert(address, CacheAccount::new_destroyed());
        let mut context = Context::mainnet().with_db(CacheDB::<EmptyDB>::default());
        assert!(
            matches!(apply_overlay_diff(&mut context, &before, &after), Err(OverlayError::Selfdestruct { address: found }) if found == address)
        );
    }

    /// A cache distinguishable from others by one marker account.
    fn marked(byte: u8) -> CacheState {
        let mut cache = CacheState::default();
        cache.insert_not_existing(Address::with_last_byte(byte));
        cache
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

    /// Field-by-field clone of revm [`State<DB>`], constructed literally so a
    /// field addition in a future revm version breaks this test's compile —
    /// the drift guard for the cache-sharing assumptions in the live overlay
    /// path (`with_cached_prestate`).
    fn clone_state<DB: Clone>(src: &State<DB>) -> State<DB> {
        // Fresh overlay states have no observer hook. A hook owns a non-cloneable
        // callback, so fail if a fixture starts relying on one rather than drop it.
        assert!(src.state_hook.is_none());
        State {
            cache: src.cache.clone(),
            database: src.database.clone(),
            transition_state: src.transition_state.clone(),
            bundle_state: src.bundle_state.clone(),
            use_preloaded_bundle: src.use_preloaded_bundle,
            block_hashes: src.block_hashes.clone(),
            bal_state: src.bal_state.clone(),
            state_hook: None,
        }
    }

    /// Regression: `clone_state` must produce a struct literally
    /// equivalent to the source. Any divergence (a field accidentally
    /// not cloned, a field whose Clone impl drifted) shows up as a
    /// non-equal Debug rendering of a state field.
    #[test]
    fn clone_state_field_parity_default() {
        let src: State<EmptyDB> = State::builder().with_database(EmptyDB::default()).build();
        let cloned = clone_state(&src);

        assert_eq!(format!("{:?}", src.cache), format!("{:?}", cloned.cache));
        assert_eq!(
            format!("{:?}", src.transition_state),
            format!("{:?}", cloned.transition_state),
        );
        assert_eq!(
            format!("{:?}", src.bundle_state),
            format!("{:?}", cloned.bundle_state),
        );
        assert_eq!(src.use_preloaded_bundle, cloned.use_preloaded_bundle);
        assert_eq!(
            format!("{:?}", src.block_hashes),
            format!("{:?}", cloned.block_hashes),
        );
        assert_eq!(
            format!("{:?}", src.bal_state),
            format!("{:?}", cloned.bal_state),
        );
        assert!(src.state_hook.is_none() && cloned.state_hook.is_none());
    }

    #[test]
    fn unchanged_destroyed_account_is_not_a_new_selfdestruct() {
        let address = Address::ZERO;
        let mut before = CacheState::default();
        before
            .accounts
            .insert(address, CacheAccount::new_destroyed());
        let after = before.clone();

        assert!(!has_unsupported_destruction(
            &before,
            &address,
            &after.accounts[&address],
        ));
    }

    #[test]
    fn new_or_changed_destroyed_account_remains_unsupported() {
        let address = Address::ZERO;
        let before = CacheState::default();
        let mut after = CacheState::default();
        after
            .accounts
            .insert(address, CacheAccount::new_destroyed());

        assert!(has_unsupported_destruction(
            &before,
            &address,
            &after.accounts[&address],
        ));

        let mut before = after.clone();
        after.accounts.get_mut(&address).unwrap().status = AccountStatus::DestroyedAgain;
        assert!(has_unsupported_destruction(
            &before,
            &address,
            &after.accounts[&address],
        ));

        before.accounts.get_mut(&address).unwrap().status = AccountStatus::Loaded;
        assert!(has_unsupported_destruction(
            &before,
            &address,
            &after.accounts[&address],
        ));
    }
}
