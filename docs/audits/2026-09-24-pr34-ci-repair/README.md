# PR #34 CI failure investigation and repair

| Field | Value |
| --- | --- |
| Date | 2026-09-24 UTC |
| Baseline / PR head | `f688387f54e6c26afbccc43b0d21480b37a1d07c` |
| PR base at investigation | `3f1c8cba78c604e80c4c2b67b3104e6ad16544b2` |
| Protocol gitlink | `820e50f749f63165be6a5797d4c5fe52cbbbbe34`, unchanged, clean |
| Failed CI | [35925811402](https://github.com/eduadiez/rollupX/actions/runs/35925811402) |
| Scope | Independent CI evidence verification, deterministic recovery-test setup, test inventory |
| Operations | Read-only canonical RPC queries; isolated native test processes; no live deployment, funding, restart, or database copy |

## Findings

1. The outbound causal example passed only mutable rows to the group verifier.
   Both new read/write cases include static rows, so no complete L1 host matched.
   The repair supplies every static row from the same canonical source load to
   `verify_outbound_source_group`. A mixed group requires one complete
   `loadExecutionTable`, with no filtered mutable rows or concatenated pools.
   The host must be unique; positions, results, rolling hashes, and exact
   retained-event multiplicities remain checked. No signer profile or runtime
   authorization rule changes.
2. The lookup trace checker expected exactly one registry frame. The pinned
   `CrossChainProxy._fallback` first reads `STATIC_CHECK_GAS()` before its
   actual lookup. The checker now requires those two exact ordered registry
   frames, including a successful, zero-value STATICCALL returning an ABI word.
   The actual lookup still requires the exact caller, proxy, input, success or
   revert, and output. Additional registry frames remain errors.
3. The Gnosis recovery test's fixed 12-second outage did not guarantee any
   missing historical boundary. A diagnostic baseline run recorded the empty
   window `(1790233933, 1790233933]`, making its assertion impossible. The node
   did subsequently construct an empty Sync, outside that empty interval.
   The test now keeps the outage until direct canonical observations show L1
   two slots ahead of L2. It then checks a rebuilt boundary inside that window,
   its actual canonical empty block, existing blob budgets, eventual settlement,
   follower replay, and a subsequent cross-chain request. The timeout is still
   bounded; no Composer timeout or retry policy was relaxed.

4. Once recovery progressed, the first candidate passed Ethereum and reached
   settlement, follower replay, and the follow-up cross-chain call on Gnosis,
   then failed an older incidental retarget assertion. Gnosis recorded four
   101–104 ms preparation samples, but all `policy.target_selected` events
   preceded them: later exact targets still fit and required no retarget.
   Requiring a subsequently enlarged budget in that event was therefore not a
   valid workload invariant. The final test checks that multi-blob preparation
   records a sample, and that every actual selection reserves at least the
   configured proof budget, stays on the correct L1 slot/height, and leaves
   strictly enough preparation and submission time. Existing controlled tests
   `measured_preparation_reserves_time_without_changing_proof_or_network_budgets`
   and `policy_target_uses_the_first_slot_with_a_complete_preparation_budget`
   cover reserve calculation and slot choice using injected durations. The
   final process check is rerun on both profiles; this intermediate Gnosis
   failure is retained rather than described as a passing recovery test.

The pinned `STATIC_ENTRY.md` still describes top-level L2 selection as hash-only.
Its implementation (`EEZL2.sol`, `staticCrossChainCall`) checks both the hash and
`expectedEntryIndex`; the fork's accepted events integration already accounts
for that position. This repair follows that existing supported profile and does
not edit the submodule or silently redefine the older upstream prose.

## Reproduction and evidence

- Baseline native Gnosis test: failed with the same recovery assertion as CI.
  A second diagnostic run changed only the assertion message to print the window
  and failed with the empty interval above. Neither is counted as a pass.
- Both mixed cases: the baseline shipping-example executable rejects retained
  canonical calldata/receipts with `expected one L1 host ... found 0`.
  The [committed fixtures](../../../crates/eez-protocol/tests/fixtures/events-causal/README.md)
  retain the full posted batch, exact per-user table loads, and receipts.
  Selection uses the scenario's expected runtime hashes, not the upstream
  summary's first-load hint (which belongs to another user in the same block).
- Both lookup cases: the baseline jq checker exits 4 on actual canonical
  Chiado traces; the corrected checker exits 0 on those identical inputs.
  [Committed trace fixtures](../../../scripts/ci/tests/fixtures/protocol-lookup/README.md)
  also retain independently encoded expected lookup calldata/results.
- `causal_evidence` includes the shipping verifier source directly and runs the
  same verification function. CI archives/selects this target in its release
  unit lane. Tests cover both successful mixed cases, missing/extra/wrong static
  rows, positions, results, keys, success flags, load boundaries, receipt
  multiplicity, and ambiguous hosts. Existing mutable/static selection tests
  remain included.

The first full Python tooling run lacked Foundry on `PATH` and failed three
raw-transaction shell checks. Re-running with the pinned Foundry directory
selected ran 252 tests: 248 passed and four optional infrastructure checks were
skipped. This is an environment correction, not a diagnostic retry pass of
the same environment.

The first candidate Rust checks caught test-only compilation mistakes (the
wrong error wrapper for `wait_for`, and generated ABI call structs without a
`Default` implementation). These were corrected before process validation;
the failed compile logs are retained separately and are not runtime test passes.

Validation results and local artifact hashes are recorded in `validation.json`.
Ignored `artifacts/pr34-ci-repair/` logs and datadirs are local-only and have no
retention guarantee. This offline verifier evidence is not a new live staged
run or Beacon-blob replay; the new full CI run must establish the complete
Kurtosis result at the published revision.

## Diagnostic-upload limitation

The failed run also exhausted GitHub artifact storage. A read-only repository
inventory returned 51 records, all marked expired, totaling 2,363,556,857 bytes
of historical artifact sizes. No active repository artifact was available for
cleanup. Historical sizes do not establish current account-wide billable usage.
An attempted account-wide billing-usage query was blocked by automatic approval
review pending separate user authorization. No artifacts were deleted, billing
settings changed, or unrelated repositories inspected. Job logs and the local
baseline evidence were preserved; artifact uploads remain unverified.
