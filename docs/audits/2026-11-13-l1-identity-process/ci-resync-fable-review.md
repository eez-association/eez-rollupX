# Final review — PR 171 / #145 at `2a428467`

**What this change does:** It adds process tests that check each node role authenticates its L1 identity, plus small testkit and CI-inventory support for them. Two follow-ups sit on top: a lockfile-only bump of two unused QUIC packages to clear three audit findings, and a test-only change so one outage test tolerates the single resync error its own fault injection causes. No production source changes.

**No actionable code or test findings remain.** One documentation item is open.

## Should fix (docs only)

1. **Audit record is stale at the frozen head** (`docs/audits/2026-11-13-l1-identity-process/quinn-audit-remediation.md:120`)
   - **What this is:** The audit folder is the durable record of what was changed and what evidence exists.
   - **Problem:** Line 120 says "Subsequent closeout changes only the audit metadata", but `2a428467` changes test code. The folder also has no mention of CI run 43460984461, its failure, or the health fix.
   - **Fix:** When you append evidence, correct that sentence and record the run as failing evidence at `dbe3d556`. State that the local baseline did not reproduce it, the regression is a unit observer test, and the corrected process runs are reported only once finished.
   - **If we skip it:** A reader would believe no code changed after the Quinn commit and would not find the retained failure.

## Follow-up 1: Quinn lock bump — sound

- **Lock hunks:** Both lockfiles change only version and checksum of `quinn-proto` (0.11.18→0.11.19) and `quinn-udp` (0.5.14→0.5.16). Dependency lists are untouched.
- **Hand-kept edge is valid:** Cargo's own resolution picked `windows-sys 0.52.0` for `quinn-udp`; the committed lock keeps `0.60.2`. The 0.5.16 manifest allows `>=0.52, <=0.61`, so this is legal and Windows-only.
- **Minimum fixed versions:** The pinned advisory files say `>= 0.11.19` and `>= 0.5.16, < 0.6.0`. Both bumps are the smallest that clear them.
- **Not in the shipping build:** The only lock dependents of `quinn` are the two `reqwest` versions. No workspace manifest names `quinn` or `http3`, and none of the eight saved `.tree` files contains `quinn`, `http3` or `h3-quinn`.
- **Build selection matches:** The evidence build command uses the same packages, bins and examples as `Dockerfile.runtime:65-69`. The profile differs (local dev vs release), as the audit states.
- **Audit counts:** The raw JSON shows root 3→0 and vendor 4→1; the evidence JSON names the remaining vendor finding as `libz-sys`. `deny-after.log:2968` shows all three checks ok.
- **No version bump needed:** `VALIDATION_VERSION` is 9 in `identity.rs:16`, and `VALIDATION_IDENTITY.md:164-168,183` says the lockfile guard was removed. Nothing compiled changes.

## Follow-up 2: CI failure and health fix — correct, and I found no masking path

- **Failure cause confirmed:** In `eez-node-node-4786-2.log:114-118` the watcher fetched L1 block 6, the fault became active, and the next catch-up failed with the numbered-header `-32000` error. Line 118 is the only fatal-class record. `nextest.log:47-50` shows the panic came from the blanket count at `eez-testkit/src/lib.rs:1626`.
- **Runtime behaved to contract:** Ordinary blocks continued through the outage, as the accepted proposal's "Clock, settlement and ownership" section requires.
- **Only one error text is reachable:** The stub fault is sticky once written. After any event fails, the first header read in catch-up is `canonical_l1_hash`, at `deriver.rs:732`, `:213` or `:877`. All three produce exactly the accepted text.
- **Window is closed on both sides:**
  - The pre-fault strict check and cursor come before injection (`settlement_policy.rs:3874-3876`).
  - The snapshot at `:4098` comes after history resumed, safe-prefix convergence and matching receipts.
  - Any fatal record before the cursor or after the snapshot makes the total exceed the allowance and fails.
- **Nothing else relaxed:** The final exit-status and raw-marker checks (`:4206-4223`) equal the old blanket check minus the allowance. The follower keeps the unchanged global check.
- **Waits still work:** The helpers this test uses do not call `progress_failure`, so they do not trip on the allowed record.
- **No sibling exposure:** The only other test injecting this fault (`dynamic_admission.rs:1080`) has no blanket fatal check.
- **Unit regression:** It covers two accepted and seven rejected cases. `health-regression-final.log` shows 1 passed, 221 filtered.

Residual limits, not findings:
- The allowance has no upper count, so a future resync retry storm inside the window would pass.
- The validator matches human-readable text; if the production wording changes, the rare race fails closed again.
- A genuine upstream `-32000` numbered-header error between heal and snapshot would be accepted, but recovery is still proven afterwards.

## Original scope

I read the full code and test portion of `final-pr.patch`: `l1_identity.rs`, `main.rs`, the three testkit files and the three script hunks. I found nothing new. The `settlement_policy.rs` hunk in the PR patch has the same hunk positions and line span as `health-fix.patch`, and the current file at 3717-4225 matches it. The three earlier completed reviews at `20746f3c` stand for that scope.

**Verdict: Ship.** Fix 1 in the docs you are already appending; no code edit or revalidation is required by this review.

**Not checked:**
- I ran nothing. I did not verify any hash, the ELF identity claim, or that the patch files equal `git diff` at HEAD; "byte-unchanged since `20746f3c`" rests on the patch reads and spot checks above.
- `shipping-target-build.log` shows a real recompile, but `runtime-before.json` does not record the command that built the "before" binaries.
- The corrected Ethereum/Gnosis process runs were still in progress, so the fix has no completed process-level result yet.
- I read only the proposal's Clock/settlement/ownership section and the section headings of the RPC spec; the gRPC retry-status policy does not apply to this JSON-RPC fault.
- Outside this PR: `AGENTS.md:117,195,200` still says validation version 8 and "local review guard", while the code is 9 and the guard is removed.
