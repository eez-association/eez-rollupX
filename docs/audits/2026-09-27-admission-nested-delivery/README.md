# Nested-input reserves and relay delivery evidence

| Field | Value |
| --- | --- |
| Date and status | 2026-09-27; implementation and validation in progress |
| Baseline source | `c937ea05d8a024d36227f5c9155338d03367cceb` (`feat/cross-chain-admission-throughput`); runtime `9c11c1aead3c6f959bee4787a65f2a116d6e41f7` |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d`; clean |
| Scope | Evidence-based nested-input gas reserves and additive relay delivery diagnostics; unchanged settlement ownership, target binding and final native validation |
| Related records | [Proposal](../../proposals/cross-chain-admission-throughput.md); [previous verified wave](../2026-09-27-admission-backoff-gas/README.md) |

## Investigation

The prior formula charges eight input words per nested call, even when validated
action evidence supplies a shorter input. Nested resolution also copies the
expected-call table repeatedly, so removing that charge entirely requires
measurement. Complete-prefix queue scans remain necessary when enclosing
reverts restore the cursor. Relay acknowledgement alone cannot explain omitted
bundles; the current delivery event lacks the relay identifier and remaining
acknowledgement window.

Local evidence is retained under `artifacts/admission-nested-delivery/`; it is
ignored and unavailable to another checkout. Small result summaries will be
committed after execution. No new runtime has been deployed at this point.

## Local implementation evidence

The 30 input-boundary measurements match baseline execution exactly. Twenty-seven
reserves decrease, by up to 36,800 gas for four nested calls. The minimum new
reserve/execution ratio is 1.0671; fixture execution includes application work
that production separately adds. This is a measured bounded admission allowance,
not a universal EVM upper bound. See [measurement table](gas-input-comparison.csv).
All 61 gas fixture tests passed on Cancun and Prague; all 80 integration tests
passed on Osaka. Composer passes 269 tests with one existing manual diagnostic
ignored; L1 passes 143 tests, including delivery timeout and optional relay hash
handling. Process and deployment verification remain in progress.

First failures retained locally: the initial combined input-boundary fixture
exceeded Foundry's per-test gas ceiling after eight measurements; splitting it
into ten independent cases passed. The first Composer run had one incorrect
new expected discount because its nested action inherited a 256-byte input;
explicitly using a short input fixed that test. Neither was a runtime failure.
Initial Clippy flagged four Option-return simplifications; these were corrected.

Additional short-data fixtures cover empty/32-/64-byte root results and 4-/36-byte
callback inputs, including maximum matching queue and reverted spans. These four
measurements also fit the reserve. Final fixture selection is 63 tests on Cancun
and Prague and 82 integration tests on Osaka. The candidate runtime remains
`b6fe9f5ca3f21f45bd83d8e43445d6ffe9873f34`; the additional change affects test
fixtures only. Workspace Clippy passes with warnings denied.

The full tooling run selected 264 tests: 259 passed, four opt-in infrastructure
tests skipped, and `test_all_workers_follow_discovered_inventory` failed because
its four completed mock workers reached a measured concurrency peak of three.
The unchanged test requires a peak of four. First-run output is retained in
`tooling-first.log`; a diagnostic retry will be recorded separately, not promoted
to a clean first pass. No protocol network runner code changed in this round.

The first native process run reached canonical settlement but failed the new
parent-hash observation assertion. `Exact` transport targets leave their
optional derived-parent guard empty. Delivery observations now use the
already-validated canonical fee sample when it is the target's immediate
parent. This correction changes diagnostics only; admission and observation
classifications are unchanged. First process logs/datadirs are retained under
`focused-native/` and the original log is preserved before the corrected run.
