# Claude review 1

Model reported by CLI: `claude-opus-5-5`. Completed successfully in read-only mode. This is reviewer output, not an accepted specification; subsequent dispositions are in [README.md](README.md).

# Review: start-after-position settlement scheduling

Baseline: `6b1e281`, protocol gitlink `855fe06`. I only read source; I made no edits and ran nothing.

**Verdict:** the design works, but it is missing one piece it can't do without: a **single published slot plan** that both the Sequencer and the engine payload job's DA limit read. With that piece in place, two things in the design can be dropped:
- the proof-time guard asked about in item 4;
- most of the "segment recovery" machinery.

## How the code works today (what the design must change)

- **Every Sync-position block is forced empty or composed.** Any non-Sync commit at the grid residue goes through `SyncSlotMode::Empty` (`crates/eez-driver/src/sequencer.rs:1007-1034`). The DA limit returns only the empty-item size when `next == boundary` (`crates/eez-composer/src/composer/sequencing.rs:165-169`). LiveTicks stop at `live_region_end` (`sequencer.rs:621-627`). If only one of these three changes, either the feature does nothing or the Sequencer and the payload builder disagree.
- **Ordinary admission rejects an unsettled, unowned nonempty boundary.** It errors with "preceding boundary is nonempty and has no settlement ownership" (`sequencing.rs:209-216`), and DA errors "stop production, never remove a bound" (`crates/eez-driver/src/slot.rs:178-181`).
- **The interval budget is fixed at K blocks.** It uses `item_limit(..., total_blocks = k)` (`sequencing.rs:224-229`).
- **The protocol, signer and deriver accept nonempty intermediate boundaries:**
  - `Schedule::candidate` only constrains the parent and the endpoint (`crates/eez-protocol/src/l2/derivation.rs:148-177`).
  - The signer's intermediate gate rejects only *native* transactions (`crates/eez-proof-signer/src/settlement/tests/blocks.rs:455`).
  - I found no boundary-emptiness check in the driver, follower or L1 derivation code.
  - Only anchor-only *terminals* must be empty: control-rpc SPEC §3.2 and `empty_chunk_boundaries` (`crates/eez-composer/src/composer.rs:5677-5724`).
- **Startup sizing already tolerates nonempty boundaries.** `validate_retained_byte_segments` resets only at empty, on-grid blocks (`crates/eez-composer/src/composer/policy.rs:212-216, 864-900`).

## Hard blockers (must be in the design)

### B1. One plan record, keyed by target and branch, failing closed to empty

Who owns the decision isn't enough; the design also has to say how other components read it. `ordinary_block_da_limit` is called from the payload job with only `(parent, timestamp)` (`composer.rs:1679-1686`). Today the only shared state is `set_sequencing_target` (`policy.rs:128-140`).

Replace it with a single `set_sequencing_plan(rollup, target_height, BoundaryMode::{Ordinary, Empty}, interval_start_hash)`:
- Publish it from the existing `prepare_next_slot` hook (`sequencer.rs:685-687`), after `recover_failed` and `replace_timed_out_attempt`. Ordering matters: an active replacement (`composer.rs:2005-2069`) must force a settle decision.
- Readers treat a missing plan, a different target, or a parent that doesn't descend from `interval_start_hash` as **Empty**.

That fail-closed default is why the design needs no extra state on restart, reorg, catch-up or missing heads. The invariant is: *a boundary is nonempty only under a current NoSettle plan for that exact target and branch.* Catch-up, capped, misaligned, deferred-late, EndTick and settle-planned boundaries all stay empty or composed, never pool-filled. The existing test `guarded_catchup_preserves_every_intermediate_empty_boundary` (`sequencer.rs:261-324`) must keep passing unchanged.

### B2. The early due check must not call `PolicyState::evaluate`

`evaluate` mutates `drain` (`crates/eez-composer/src/settlement_policy/mod.rs:261-282`). `evaluate_settlement_policy` also needs a built terminal for its drain-coverage check and terminal size (`policy.rs:735-757`).

Add a pure `would_be_due(&self, …)` that takes the target height and treats any existing drain as due. The useful property is **monotonicity**: GENERAL, WINDOW_LIMIT, PURE_INTERVAL and the predecessor are exact at P because they depend on the target height or target L1 timestamp, not on the wall clock. Fullness only grows. So:
- "not due at P" can turn into "due" only through fullness, and that defers to the next interval (soft rule; the hard DA bound still holds).
- An obligation is never lost, because the next interval recomputes it deterministically.

State this argument in the design instead of adding drain bookkeeping.

### B3. Downgrade can wedge an old binary

Once an unsettled nonempty boundary exists, the old `sequencing_da_limit` returns `Err` for every Live and Future payload (`sequencing.rs:209-216`). The head then can't reach the next Sync, so it can't settle past the boundary. Rollback below the "tolerant reader" release therefore requires `cursor ≥ last nonempty boundary`, or a discarded speculative suffix. This is a rollout constraint, not a design flaw. It goes in the runbook and in `docs/configurable-settlement-policy.md`.

## Item 4: are bytes plus block counts enough?

- **L1 gas: yes, for anchor-only batches.** Gas depends on span only through blobs. The projection uses `entries.len()` (one entry) and fixed-size calldata with a maximum-size proof (`composer.rs:6672-6688`). The ceiling comes from the L1 header and target (`composer.rs:6669-6671`). A segment that fits bytes and the window therefore fits gas exactly when a K-interval would. Cross-chain terminals scale with entries, which existing admission already checks.
- **Proof time: no new risk class.**
  - No current path splits on proof time. `OverBudget` constraints are only `window_limit`, `blob_limit` and `l1_tx_gas_budget` (`composer.rs:806, 6614, 6715`).
  - Prediction just defers through the shared deadline (`crates/eez-composer/src/prover_retry.rs:160-193, 316-334`).
  - Multi-interval ranges already exist whenever the policy defers.
- **The span is already bounded.** With NoSettle at B, GENERAL is not due, so `B − cursor < N` (`EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS`, required). WINDOW_LIMIT is not due either, so `B − cursor + K ≤ max_blocks` (`policy.rs:763-765`, with `max_blocks` already intersected with the signer limit, `crates/eez-composer/src/composer/config.rs:32-41`).
  - So the smallest extended segment is at most `min(N + K, max_blocks)`, the same worst case today's policy already proves.
  - **No additional guard is needed**, as long as the early check evaluates GENERAL and WINDOW_LIMIT exactly (B2).
  - Don't add a hardware-independent proof-time bound.
- **Unverified:** I didn't check whether `PrepareOrdinary` prewarm/retention ranges assume an empty boundary at each K. Signer SPEC §4 only says cached ranges can't cover the terminal. Add a test (T5).

## Incorrect or overstated assumptions

1. **"Reserve empty continuations through B+K" doesn't need a forward reservation.** The existing per-block limit already computes `capacity − previous − reserved_empties` (`sequencing.rs:219-229`). Generalize three things:
   - `base` = max(cursor, the last empty grid block or owned optimistic boundary);
   - `boundary` = B+K when B's plan is Ordinary, otherwise B;
   - `total_blocks` = segment length.

   At B itself, `limit < empty` already means "close B empty", provided the boundary caller maps that to Empty instead of an error. The invariant then holds inductively, with no extra state.
2. **"Recover segment state from canonical ancestry" already happens on every call** through the ancestor walk (`sequencing.rs:170-191`). The real issue is cost: that walk becomes O(segment), up to N+K blocks, on every payload job. Cache the cumulative sizer per `(base hash, parent hash)` next to `block_rlps`. Treat this as an implementation detail, not a new recovery path.
3. **"Invalidated/unavailable evidence defers admission"** means an L2 stall at ordinary positions today. At a *boundary*, unavailable evidence should produce an **empty boundary**, not an error. An empty block always fits the reservation.
4. **RANDAO and missed heads:** the design introduces no new dependency.
   - `seed_for(n)` reads only anchors with endpoint < n (`crates/eez-l1/src/derivation.rs:373-380`), and a Live anchor for 106 can only be 106 itself (`derivation.rs:181-203`). So an ordinary 106 at wall clock depends on the same seeds as 101–105.
   - The race between reconciling the anchor at 100 and building 101 already exists. It stays covered by the `stale_context` check (`sequencer.rs:963-969`) and by `commit_sequenced` re-reading the context.
   - With missing heads, keep today's behaviour: LiveTicks hold at `live_region_end` of the latest target (`slot.rs:410-413`, `sequencer.rs:621-627`), and the replacement trigger makes a fresh decision for target+K. Don't add head-independent wall-clock production; it isn't needed for the stated goal.
5. **Failed settle terminal:** an `EndTick` (`sequencer.rs:784-787`) today leaves 105 as head until the next trigger's catch-up, roughly a 6 s stall. Optionally let the LiveTick at ts(106) commit 106 through Empty mode. It must never use the pool.

## Item 5: timing configuration

- **Two trigger sites read `proof_window_open`:** `PendingSyncSlot::new` (`slot.rs:245`) and the replacement window (`composer.rs:2033`). Derive both from one `trigger_offset() = P·L2`, and set `future_count = K − P − 1`.
- **Validation for P:**
  - `1 ≤ P ≤ K−1`;
  - `proof_seed + slack ≤ (K−P)·L2`. Without this, the ordinary-only gate `now + proof > deadline` (`sequencer.rs:72-86`) defers every ordinary-only settlement to Empty.
  - Reject P combined with `EEZ_PREPARATION_TIME_MS`.
  - Keep `EEZ_PROOF_TIME_MS` as the prediction and bootstrap seed (`composer.rs:6993`; `crates/eez-composer/src/composer/admission.rs:157`).
- **The legacy mapping is worth keeping because it's exact and tiny:** when preparation is unset, `proof_window_open = live_count·L2` (`crates/eez-driver/src/timing.rs:304-305`), so `P := live_count()` reproduces both the trigger and the layout. On the Chiado preset (prep 500, proof 750, slack 700) it maps to P=3: the same layout, triggering 50 ms earlier, which is safe.
- **Use "P explicitly set" as the opt-in for Ordinary boundaries.** The legacy mapping keeps all-empty boundaries. That gives a staged rollout with no new feature flag.

## Recommended sequence

1. **Tolerant reader (no behaviour change):**
   - segment-based DA base and `total_blocks`;
   - accept unowned nonempty boundaries below the last empty base;
   - plan plumbing with every plan = Empty;
   - P parsing with the legacy mapping.

   Ship this first; it becomes the rollback floor.
2. **Producer:** add the pure `would_be_due` and publish the plan from `prepare_next_slot`. Make the LiveTick and `commit_one` boundary routing plan-aware, enabled only when P is set explicitly.
3. **Specs and docs:**
   - control-rpc SPEC §3.2: change "previously committed empty Sync block" to "Sync block (ordinary transactions permitted; native forbidden)";
   - `docs/configurable-settlement-policy.md:455-469, 567-595`: boundary and trigger text;
   - the rollback constraint from B3.
4. Later, remove `EEZ_PREPARATION_TIME_MS` and the legacy mapping, and update Compose, Kurtosis and `test_chiado_config.py`.

## Decisive tests

- **T1 (timing):** across the presets in `timing.rs:478-494`, the legacy mapping gives the same `PendingSyncSlot` and `per_trigger_composition` results. P out of range, P with preparation, and `proof + slack > (K−P)·L2` are all rejected through `from_env` subprocess cases (the pattern at `timing.rs:667-716`).
- **T2 (sequencer, `PolicyComposer` fake):**
  - a NoSettle plan pool-fills 104–106 one per tick, with no Empty compose call;
  - a Settle plan bursts 104/105 and then calls compose;
  - stale-target, stale-branch, EndTick and catch-up cases keep boundaries empty and never touch the mempool.
- **T3 (`sequencing.rs`):**
  - a segment spanning a nonempty boundary;
  - a limit at B that can't fit K empties returns the empty size;
  - a restart walk reproduces the same limit;
  - a window reduced through `with_prover_block_limit(2K+1)` (`config.rs:205`) forces Empty.
- **T4 (policy):** `would_be_due` leaves `PolicyState` unchanged, and "not due at P" implies a final evaluation that is not due except through fullness, using the same inputs.
- **T5 (composer and signer):**
  - `emit_historical_chunk` skips the nonempty B and settles B+K within the window;
  - a `Prove` request covering a nonempty intermediate boundary is accepted, including through cached ordinary ranges.
- **T6 (e2e):**
  - `settlement_policy` and `queue_liveness`: a cross-chain transaction arriving after P in a NoSettle interval settles in the next interval;
  - `queue_reorg`: recovery of a failed composed terminal below an ordinary boundary.

## Implementation details (not blockers)

- the LiveTick catch-up-to-wall-clock loop;
- event and metric names;
- caching the segment sizer;
- whether the boundary uses `SlotKind::Live` or a new label.
