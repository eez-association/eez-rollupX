**Static verdict: nothing blocking, but the correction rests on a wrong premise from my own prior review, and the record needs three small fixes.** I executed nothing. The corrected test's rerun had no result when I read its log, so I make no claim about it.

What this change does: it relaxes one wait in the paired-genesis test from "two log lines" to "one", adds a disclosure that the anchor fence is not genesis-specific, relabels one table row, and clarifies the tooling count. Production code is untouched.

## Should fix

1. **The two anchor lines are two rejections, not one duplicated rejection** (`README.md:212-214`, `evidence.json:526`)
   - **What this is:** the record says the old `>= 2` counted "two log lines emitted for the same rejected attempt". That wording came from finding 1 of my prior review.
   - **Problem:** it is wrong. One worker pass hits the fence twice, on two production paths:
     - The sync-slot candidate checks an in-memory terminal witness (`composer.rs:7589`), logs the ERROR at `:6140` and swallows it.
     - `settle_retained` (`sequencer.rs:1059-1065`, `composer.rs:1952-2040`) then prepares a second candidate from the stored witness (`:6349-6357`, `:7606`) and logs the WARN.
     - The retained paired-node log shows two separate policy evaluations and two sizing lines (110-117, then 119-127).
   - **Fix:** reword both places to "two fence rejections from one slot opportunity: sync-slot candidate and retained-ordinary chunk". Add a dated disposition that finding 1's mechanism was mistaken; leave `fable-review.md` unaltered.
   - **If we skip it:** the audit states a production mechanism that the retained log contradicts.

2. **`> 0` lets the negative checks run before the second path finishes** (`l1_identity.rs:528-552`)
   - **What this is:** after the wait, the test asserts zero attestations, dispatches, evictions and batches.
   - **Problem:** the wait can now return on the ERROR alone. The WARN came 195 ms later, and the poll interval is 500 ms. A regression that dropped the fence on the stored-witness path and signed there could pass when the poll lands in that gap. The old `>= 2` covered both paths in the observed run.
   - **Fix:** wait for one line from each emitter, each also containing the anchor text. Use a line filter like the drift leg at `:877-884`, because `log_count_matching` ORs its patterns (`eez-testkit/src/lib.rs:1781-1784`). The final run already satisfied this condition.
   - **If we skip it:** one fence hit still proves a pre-signing boundary exists, but then say in the record that the negatives cover only the first rejection. Taking the fix means one more rerun; the in-flight one tests `> 0`.

3. **"Target height" is the wrong height** (`README.md:86`, `evidence.json:469`)
   - **What this is:** the new disclosure says the fence compares hashes at the target height.
   - **Problem:** the target is the next, unbuilt L1 block. The fence compares at the terminal witness's selected L1 height and at the proof anchor, which is the target's parent (`composer.rs:485-491`, `:7396`, `:7562-7563`). In the retained log the target is 3 and the anchor is 2.
   - **Fix:** write "at the proof-anchor height (the target's parent) and the terminal witness's selected L1 height".
   - **If we skip it:** the disclosure names a height where no hash exists.

4. **The record does not say whether the corrected test ran** (`README.md:51`, `:112`; `evidence.json:523-529`)
   - **What this is:** the 7-pass matrix ran the pre-correction file (`59ee…`); HEAD holds `a606…`.
   - **Problem:** `post_review_test_correction` has no execution status. `artifacts/issue145/logs/review-correction.log` showed the test started (line 84) and the runtime build at line 162, with no result line.
   - **Fix:** when it finishes, record the command, exit code, summary line, log digest, and that run's own test and runtime executable digests (both were rebuilt). Add one README sentence that only this test was rerun. Otherwise state "not re-executed".
   - **If we skip it:** older results appear to validate a newer test binary.

## Previous corrections

| Correction | Status |
| --- | --- |
| `>= 2` → `> 0` | Applied at `l1_identity.rs:531-533` and matches the patch. Premise wrong; see 1 and 2. |
| Non-genesis-specific disclosure | Present (`README.md:86-88`, `limits.anchor_fence`). Substance right; wording per 3. |
| Independent canonical Anvil label | Satisfied (`README.md:58`). |
| Tooling count | Satisfied. Log shows `Ran 607`, `OK (skipped=9)`, 598 ok lines and 9 skip lines. |
| Original report retained | `fable-review.md` reads line-for-line the same as `final.md`. `final.json` shows only `claude-fable-5-2`, first-party, standard speed, zero spawned agents. |

The pre-review `code_source_sha256` entry for `l1_identity.rs` is acceptable as disclosed.

## Source and reading

- **Source:** HEAD `9f58ce89de95041aa66aa4174e3a211ad66c282f`, read from the worktree ref; base and Core pin taken from the record.
- **Contracts:**
  - `VALIDATION_IDENTITY.md` and `L1_HISTORY_SPEC.md` in full.
  - `l1-origin-and-offline-proving.md:1-484`. It has no section called "bootstrap scope"; the relevant text is the genesis-reference inheritance at `:223-229` and `:282-283`.
  - RPC `SPEC.md` §6.1-6.2 and §7-8 to line 656.
  - Signer `SPEC.md` §1-2 and the §6 history rules.
  - `review-SKILL.md`.
- **Change:** `correction.patch`, `final.md`, the audit README, `evidence.json` and `fable-review.md`, all in full, plus the current `l1_identity.rs`.
- **Production:**
  - `composer.rs:446-504`, `:1952-2040`, `:6088-6147`, `:6277-6405`, `:7375-7611`.
  - `sequencer.rs:1004-1068`.
  - `eez-follower/src/lib.rs:150-281`: readiness, then authentication, then catch-up, with the watcher last.
  - `posting.rs:254-288` and `submitter.rs:239-272`.
- **Drift legs:** unchanged. No contract requires a genesis-specific anchor check.

Verdict: fix 1, 3 and 4 in the record first; 2 is your call.

Not checked: any SHA-256, rustfmt output (the new indentation fits rustfmt's short-last-line rule as I recall it; no post-correction fmt log is retained), the rerun result, and which fence term fired. The Rust-skills hook format was not applied: this is a review and you limited me to Read/Grep/Glob.
