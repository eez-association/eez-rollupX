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
