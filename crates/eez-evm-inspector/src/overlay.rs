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

use revm::context_interface::journaled_state::account::JournaledAccountTr;
use revm::context_interface::{ContextTr, Host, JournalTr};
use revm::database::{AccountStatus, CacheState, states::CacheAccount};
use revm::primitives::Address;

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

// Note on trait choice:
//
// Reads on the OTHER direction of the overlay flow (source-sim
// inspector snapshotting cache before dispatch) go through `Host::sload`
// / `Host::load_account_info_skip_cold_load` so they pick up mid-tx
// journal writes. The natural symmetry would be `Host::sstore` here.
//
// We can't use it: `Host::sstore_skip_cold_load` delegates to
// `JournalImpl::sstore_skip_cold_load` which calls
// `inner.sstore_assume_account_present(...)`. The "assume present"
// check is stricter than `JournalTr::load_account`'s warm-up — even
// after explicitly loading the account, the journal returns
// `ColdLoadSkipped`. The plain `JournalTr::sstore(addr, key, value)`
// path (which calls `load_account_mut` internally and panics on cold
// load via `unwrap_db_error`) is the only one that actually works
// for our explicitly-warmed flow.
//
// Balance writes go through `JournaledAccountTr::set_balance` —
// `Host` exposes `balance(addr)` for reads but no public setter.

/// Errors surfaced by the overlay diff-apply path.
///
/// SELFDESTRUCT is explicitly out of scope: revm's `JournalInner`
/// exposes no public method to mark an account destroyed from
/// outside, and the in-tree fixtures (`flash-loan`,
/// `reentrantCrossChainCalls`) don't destroy accounts. If the nested
/// state's cache shows a destroyed account that the source state
/// does not, the apply path surfaces this loud error rather than
/// silently miscomposing.
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
/// - **Balance changes:** for each account whose balance differs,
///   emit `ctx.journal_mut().load_account_mut(addr).set_balance(...)`.
///   `set_balance` is the load-bearing primitive — `balance_incr` on
///   `JournalTr` is increment-only (`U256` no-wrap on decrease), and
///   overlay flows commonly involve net decreases on one side and
///   increases on the other. Going through `load_account_mut` reaches
///   `JournaledAccountTr::set_balance` which mutates the journaled
///   account directly.
///
/// Out of scope — surfaced as `Err(OverlayError::*)` or skipped:
///
/// - **SELFDESTRUCT:** loud failure via [`OverlayError::Selfdestruct`].
///   No public revm API to mark an account destroyed from outside, and
///   `flash-loan` / `reentrantCrossChainCalls` don't destroy accounts.
///
/// - **Code installation, nonce changes:** rare in overlay-relevant
///   patterns (`flash-loan`, `reentrant`); deferred. A future commit
///   can extend the function with `set_code` / `set_nonce` calls.
///
/// - **Transient storage:** EIP-1153 auto-clears at end of transaction;
///   no diff-apply needed.
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
/// - [`OverlayError::SourceLoad`] / [`OverlayError::SourceSstore`] if
///   the source state's database or journal surfaces an error during
///   apply.
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

        // Per-account warming. Both sstore and set_balance assume the
        // account is already journaled; outside the EVM's normal CALL/
        // SLOAD path we do the load explicitly. `load_account_mut`
        // also gives us a `JournaledAccount` for the balance set; we
        // bind it for the balance branch and let it drop before sstore.
        // (`load_account` is the lighter-weight version — used here
        // for the storage-only branch to avoid pulling code.)
        let balance_changed =
            before_plain.is_some_and(|b| b.info.balance != after_plain.info.balance);

        if balance_changed {
            let mut loaded = ctx.journal_mut().load_account_mut(*addr).map_err(|e| {
                OverlayError::SourceLoad {
                    address: *addr,
                    message: format!("{e:?}"),
                }
            })?;
            loaded.data.set_balance(after_plain.info.balance);
            // `loaded` borrow ends here so the `sstore` calls below
            // can re-borrow `ctx.journal_mut()`.
        } else {
            // Warm the account so the subsequent `sstore` calls don't
            // hit `JournalLoadError::ColdLoadSkipped` (which the
            // public `sstore` method panics on via
            // `unwrap_db_error`).
            ctx.journal_mut()
                .load_account(*addr)
                .map_err(|e| OverlayError::SourceLoad {
                    address: *addr,
                    message: format!("{e:?}"),
                })?;
        }

        // Iterate after's storage; emit sstore for every slot whose
        // value differs from before.
        for (key, after_val) in &after_plain.storage {
            let before_val = before_plain
                .and_then(|b| b.storage.get(key).copied())
                .unwrap_or_default();
            if before_val == *after_val {
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
    use revm::database::{EmptyDB, State};

    /// A cache distinguishable from others by one marker account.
    fn marked(byte: u8) -> CacheState {
        let mut cache = CacheState::default();
        cache.insert_not_existing(Address::with_last_byte(byte));
        cache
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
