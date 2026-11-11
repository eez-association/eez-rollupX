# Claude corrective review

Read-only review using `claude-fable-5-1 --effort max`. Original response below;
these are reviewer claims, not all confirmed findings. See README for reconciliation.

**Review of the PR #123 corrective changes: seven findings, none a soundness break in the signer path; nothing was run.**

The checkout changed while I was reading it. It now contains code that is not in `/tmp/pr123-fix-review.diff`: a `next_source_slot` cursor in the materializer, terminal-failed-slot duplicate coverage in the verifier, a new fixture and test, and `SPEC.md:994-996`. Finding 1 is in that newer code. Line numbers are from my last read, and all traces are by hand.

## Findings

**1. Medium — the new source-slot cursor makes the materializer emit tables its own verifier rejects** (not in the diff)
- **Where:** `crates/eez-protocol/src/entries/materialize.rs:221-235, 243-246, 269-274`; `crates/eez-protocol/src/entries/verify.rs:476-491`.
- **Mechanism:** `next_source_slot` is one watermark. A terminal failure of any root advances it past earlier slots that the runtime cursor can still select.
- **Case A — duplicate non-terminal slot.** Roots `[A fails after a callback, C fails with no callbacks, A fails after the same callback]`:
  - The third root runtime-selects slot 0, but the scan from `next_source_slot = 2` finds nothing and pushes a second copy of A.
  - The verifier only lets terminal failed slots share coverage, so slot 2 stays unseen and it returns "did not exercise every source entry".
  - The same happens with `[A ok inside a reverted scope, C terminal fail, A ok]`.
- **Case B — stale slot overwritten.** Roots `[A terminal fail, {B ok, A ok} in a reverted scope, A terminal fail]`:
  - The last root runtime-selects slot 0, but the scan from the restored watermark picks the successful A at slot 2.
  - Lines 243-246 clear only the arrays, not `success`/`returnData`, so the comparison fails with "different callback program".
  - If the stale slot has the same outcome header, its callback program is erased silently and downstream verification fails.
- **Impact:** both fail closed; valid L2-origin retry shapes are evicted as malformed at `crates/eez-composer/src/composer.rs:3617-3635`. The single-cursor version in the diff handled both.
- **Fix direction:** rebuild non-terminal programs in the runtime-selected slot; allocate a fresh slot only for a terminal failure; never reuse a same-hash slot that is not the selected one. Add both sequences as tests.

**2. Medium — inbound projection failure is an uncharged requeue loop on every L1 except chain id 1**
- **Where:** `composer.rs:88-90, 6793-6799, 5811-5824`.
- **Mechanism:** `check_inbound_deposit_limit` returns before projecting unless `l1_chain_id == 1`, so inbound admission never runs the paired verifier elsewhere. The first run is in `prepare_post_batch_raw`, where an error becomes `PreparePostBatchError::Build`. That arm re-queues every survivor without charging an attempt and builds an empty slot.
- **Concrete trigger:** an inbound root whose L2 target makes the same static L1 read twice around a local write, where the first read made callbacks `[flag, rate]` and the second only `[flag]`. Both chains execute this correctly, but `verify.rs:900-903` hard-errors on the earlier candidate's missing `rate` row instead of trying the next candidate.
- **Not new:** the same composition previously stalled at the signer, because `ExecutionTableMismatch` is non-actionable. Mainnet now evicts it only as a side effect of the deposit policy; outbound evicts on all chains.
- **Fix direction:** run `project_inbound_value` for applying roots at inbound admission on every chain and evict on error; keep only the cap comparison chain-gated.

**3. Medium — the escrow gate now counts only surviving outflow, but L1 simulation assumes funding**
- **Where:** `composer.rs:3612-3635`; `crates/eez-composer/src/local/slot.rs:237-241, 256`.
- **Mechanism:** L1 target frames run with `disable_balance_check`, and the comment there makes the composer's escrow check the funding authority. The old flat sum counted every root value outside a revert window, including naturally failed roots. The projection drops them.
- **Consequence:** if a value-bearing root that reverts with non-empty data exceeds the manager's ETH balance, the on-chain `call{value}` returns `(false, "")`. That differs from the simulated revert data, so the host hits `RollingHashMismatch` and is skipped, stranding later entries.
- **Condition:** L2 value not fully escrow-backed, e.g. prefunded genesis on a fresh network.
- **Not new for revert windows:** the old sum also skipped those.
- **Fix direction:** gate on gross executed L2→L1 value; keep the surviving projection for `etherDelta` only.

**4. Medium (policy) — the mainnet deposit cap misses deposits carried by L2-origin transactions**
- **Where:** `composer.rs:83-131`, called only at `4455`; `3617-3628`.
- **Mechanism:** an outbound root's L1 target can call back into L2 with value. That is supported and yields a positive `etherDelta`. The outbound path uses only `l2_to_l1_value` and ignores `l1_to_l2_value`, so the 0.1 ETH cap is avoidable by originating on L2.
- **Decision for you:** if the cap is meant only for L1-origin source transactions, say so in the constant's doc; otherwise apply it to the outbound projection as well.

**5. Medium (characterization, not an exploit) — fix 1 closes callbacks only**
- **Where:** `materialize.rs:1146-1157`; `crates/eez-protocol/src/settlement.rs:53`.
- **Mechanism:** a successful, zero-value, callback-free non-applying root is still admitted. On L1 its entry commits and returns `returnData` to the caller with no L2 transition. Nothing in `EEZ.sol` `_executeEntry` forces the predicted source-frame revert.
- **Why it matters:** this is the same unenforced-rollback premise the SPEC now cites at `SPEC.md:739-741`. Any harm needs an L1 contract that treats the success as applied and a revert that differs between simulation and inclusion; I did not build that.
- **Decision for you:** reject all successful non-applying mutable roots, or document why the return-value case is acceptable.

**6. Low — `crates/eez-proof-signer/SPEC.md` still contradicts the code**
- `SPEC.md:862-864` says construction deduplicates identical static candidates; it no longer does (`materialize.rs:1014`, and `SPEC.md:991-994` says the opposite).
- `SPEC.md:936-937` says host rows in a static group are only for static callbacks, and `967-968` limits nested rows to mutable-only hosts; the verifier and tests accept nested mutable callbacks in mixed hosts.
- The test at `materialize.rs:3061` is named `..._but_reject_failed_and_rollback_sources` but asserts acceptance.

**7. Low — coverage re-encodes rows on every selection**
- **Where:** `verify.rs:1068-1076` and `486-488`.
- **Mechanism:** each static selection ABI-encodes the selected row and every unseen row until a match; the source-slot scan re-encodes `source_entry` per candidate. It was O(1) before. Honest tables stay cheap and table size bounds the worst case, but the projection now runs this several times per candidate.
- **Fix direction:** precompute per-row hashes once in `ChainState::new`.

## Checked, no finding
- **Fix 1 boundaries:** a successful no-op-root entry with callbacks now classifies as an inbound effect and fails the checkpoint/candidate binding; semantic materialization rejects it independently.
- **Fix 2:** duplicate coverage only ever marks a byte-identical copy of a selected, validated row, at most one per selection, so extra or differing rows still fail. Selection matches the forward scan in `EEZ.sol`/`EEZL2.sol`.
- **Fix 3 (as in the diff):** rebuild-and-compare covers entry bytes and trace, so changed callbacks or changed callback results on a retry are rejected.
- **Fix 4:** the placeholder-root projection is self-consistent and yields the same facts the signer recomputes; all five call sites align entries, traces and source partitions.

## Limits
- I did not re-review unrelated main changes.
- I did not check the baseline fixtures' provenance.
- I did not check the baseline layout for terminal failures inside reverted scopes.
