# PR #123: issue acceptance follow-ups

Status: scoped implementation and local validation complete. Hosted CI for the
new PR head remains required. No deployment or merge.

## Identity and scope

PR base/fork target `ff7a5cdc81ce02ad30c6c267c4554de77cecb753`; reviewed
follow-up baseline `7d1fedbc72f3066401b3b1f198893fe4bfc5d6a9`; unchanged initialized
protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f`. The operator's root checkout
was left unchanged. Work used the existing PR worktree. Final implementation:
`286e32855a5b5f574ff4eb8ec38ee6e9563e35d4`. Tests ran on the matching source tree
before that commit; the subsequent audit commit contains evidence only.

The owner authorized #85 C13 investigation, G02 independent rejection coverage,
relevant #81 regressions, and matched performance measurements. The general
retry-table allocator and unrelated issues remain outside this change. See the
[proposal](../../proposals/full-protocol-e2e.md#issue-follow-ups-2026-11-09).

## Findings and changes

- **C13:** shared reconstruction rejected immediate static entries and treated
  `immediateEntryCount` as the semantic outbound/inbound boundary. Pinned
  [multi-prover rules](../../../eez-core-protocol/docs/MULTI_PROVER_SPEC.md)
  deliberately exclude dispatch counts from the proof. A poster can move inbound
  work into its callback after signing. Reconstruction now derives the boundary
  from complete native semantic groups, validates dispatch bounds separately,
  and retains exact table binding. Only the follower and read-only blob evidence verifier use observed-settlement
  binding; signer admission remains strict. Canonical commitment advancement,
  not the dispatch count, still establishes consumption.
- **G02:** added an explicit signer check against the locally configured executing
  EEZ registry/proxy in all three L1 forwarding arrays. An implementation address
  or separate RollupX manager address is not that runtime identity. A typed error
  is returned before signing; all other gates still apply. The direct signer
  fixture mutates a canonical proposal following real offline execution and
  asserts this exact error, not a later generic table mismatch.
- **#81:** added top-level static local-write cases in both directions alongside
  the existing nested cases, plus an explicit no-callback conflicting-result
  regression. This does not admit general mutable retries or claim exact
  gas-sensitive equivalence.

## Baseline reproduction

The focused `observed_settlement_accepts_retuned_dispatch` check fails on the
follow-up baseline with `Settlement("immediate static entries are unsupported")`.
The [retained output](c13-baseline.log) contains one selected failure. The same
scenario uses the follower-specific binding after the change; its strict signer
counterpart must still reject. The contract/process reproduction was added after
the fix, so it is not a before/after process comparison.

## Issue acceptance map

| Item | Evidence / remaining boundary |
| --- | --- |
| #85 C01 failed frames with children | Existing call-pattern/process and paired verifier coverage; earlier [fix audit](../2026-11-09-pr123-fixes/README.md). Successful non-applying L1 roots with callbacks remain explicitly unsupported. |
| #85 C02/C03 deep static reads and failed callbacks | Existing pinned scenario inventory and native call-pattern suite; only selected validated callback graphs authorize effects. |
| #85 C06/C07 nested rollback and static-first groups | Existing rollback verifier/materializer and native overlay regressions; retain bounded groups and complete rollback. |
| #85 C08 mixed static/mutable work | Existing native mixed/nested-static tests and corrected paired funding projections. |
| #85 G02 independent manager destination rejection | New direct signer pipeline/RPC test plus all-array/runtime-identity unit coverage. |
| #85 C13 alternative scheduling | New exact-binding tests and real-proof native-blob process cases; broader restart/reorg qualification must be reported separately. |
| #81 four A→B→A paths | Existing nested tests plus new top-level tests in `cross_chain/call_patterns.rs`. |
| #81 indistinguishable static outcomes | Existing callback-hash collision check plus new empty-callback result collision test. |
| #81 fixed retry budget / gas-sensitive execution | Pinned Core counterexamples demonstrate documented limitations; no new production budget model or full native fixed-budget rejection test in this change. |
| #81 mutable retry after local write / enclosing revert | Existing explicit builder rejection and historical slot regressions; general table allocation/candidate isolation is outside scope. |

Neither issue is declared fully closed by this audit. Core scenario success alone
is not a production proof, and older full-network results do not qualify these
new binaries.

## Validation and performance

The focused protocol/codec/deriver/RPC/client run has 407 passing tests and no
ignored tests. Integration contract tests passed 153/153 again after the final
meta-hook fixture revision. The pinned
`EEZExpressiveness.t.sol` run passed 11/11, including fixed-budget exhaustion,
indistinguishable static outputs, and mutable retry/rollback counterexamples.
These are contract-limit checks, not production acceptance claims.

Performance compares the original reviewed PR head `8b5cafd3` with the corrective
candidate, using Rust 1.95 release builds and the existing `entry_bench` example.
The shared cases cover nested/failed/rollback paths; added repeated-static and
value-planning phases expose compatibility costs and quote-planning work.
The old value phase uses `entry_flat_value_sum`; the candidate uses the corrected
paired projection. They intentionally perform different validation and must not
be described as equivalent algorithms or a whole-RPC benchmark.

An initial shared-target build left the baseline executable at Cargo's common
example output path. Its identical hash was detected before measurement; the
candidate was rebuilt and received a distinct hash. Those invalid candidate
artifacts are excluded. Final timings and benchmark executable identities are
recorded below. The final primary process run passed all 15 selected tests
with no failures or skips (900.94 seconds).


### Diagnostic failures retained

The first process attempt reached a signed proposal but its retuned posting
reverted. The next diagnostic run separated consumed mutable, consumed static,
and unconsumed static prefixes. All three exhausted the original queue-only
posting gas budget; no follower success is claimed for those runs. The
[reduced call traces](dispatch-gas-failures.json) retain the failing call depth,
selector, gas and error. Full local-only traces/logs/datadirs are under
`artifacts/pr123-followups-diagnostic/`; they are not available from a fresh clone.
The separate meta-hook fixture now adds a bounded posting gas allowance when it changes dispatch;
its proof, native blobs and entry contents remain unchanged. Real posters must
estimate their chosen dispatch path rather than reuse a queue-only estimate.
This changes no Composer production gas policy.

The first signer test compile needed explicit ABI struct fields. After that,
the all-array test passed and the direct fixture reached the intended typed
rejection with zero signing. The RPC part could not bind a local listener inside
the sandbox (`EPERM`); the complete socket-enabled signer suite is reported in the final checks below.
A later queued rerun was cancelled to serialize Cargo configurations. The first
workspace lint run found one boolean-to-integer style issue in the new test;
that was corrected. These are not recorded as successful full checks or flakes. The final signer
suite subsequently passed 340 active tests, including both new destination tests;
its five ignored cases are explicit fixture-generation/inventory utilities. All
20 testkit tests passed. The initial evidence-example command lacked its required
`chain-spec` feature; it did not execute tests and was corrected. With that
feature enabled, all 20 evidence-example tests passed.


### Matched component measurements

Five alternating baseline/candidate runs, one pinned CPU (CPU 0, Intel Core
Ultra 9 185H), release builds, 100 warmup iterations per phase; no concurrent
Rust compilation or test networks during measurement. Existing operator services
remained running. Inputs, rounds and benchmark source are matched, except for
the explicitly different old/new value-planning implementation. Values below are
median microseconds per operation, not RPC latency or throughput.

| Case | Materialize: before → after | Verify: before → after | Corrected value planning |
| --- | ---: | ---: | ---: |
| single | 5.37 → 5.59 | 3.81 → 3.78 | 5.13 |
| many_roots | 292.37 → 309.47 | 194.08 → 192.06 | 236.81 |
| nested | 386.48 → 383.77 | 230.29 → 226.62 | 279.39 |
| large_results | 1675.01 → 1663.95 | 969.35 → 967.18 | 1130.35 |
| failed_frames | 383.77 → 382.23 | 224.81 → 223.76 | 273.52 |
| rollback_roots | 291.43 → 309.72 | 192.34 → 192.28 | 233.94 |
| recursive_static | 211.50 → 151.99 | 121.63 → 125.37 | 144.53 |
| repeated_static | 185.90 → 182.87 | 169.25 → 166.67 | 209.28 |

The old isolated-row sum took about 0.01–0.20 µs, but did not enforce the corrected
rollback/funding semantics. Relative percentages against that incomplete work
are misleading; the absolute cost above is the relevant planning tradeoff.
Materialization increased by 4.2% for one call, 5.8% for many roots, and 6.3% for
root rollback; those sample ranges do not overlap. Recursive-static
materialization decreased by 28.1%. Verification changes ranged from -1.6% to
+3.1%. These are this workload's measurements, not general performance guarantees.

[Raw samples](benchmark-results.csv), [medians and min/max ranges](benchmark-summary.json),
[CPU/build identities and binary hashes](benchmark-environment.json), and
[baseline-only benchmark adaptation](baseline-benchmark.patch) are retained.
The candidate benchmark is `crates/eez-protocol/examples/entry_bench.rs`.
Reproduce with Rust 1.95 `cargo build --locked --release -p eez-protocol --example
entry_bench` on each revision, copy each verified executable to a distinct path,
and alternate `taskset -c 0 <executable>` five times. Do not trust a shared Cargo
example-output path without verifying its identity after switching checkouts.

Only outbound value projection was timed directly. Full quote RPC latency,
inbound projection, signer-wide latency, memory/RSS, and sustained network
throughput were not benchmarked. The new destination guard adds one bounded
linear scan without copying calls; its whole-request impact is not measured.


### Independent Claude review

Claude ran with `claude-fable-5-1 --effort max` and read-only Read/Glob/Grep tools.
Its [original response](claude-review.md) is retained. It identified a real test
fixture regression: changing `EEZPostBatcher` invalidates the Composer's recognized
wallet-estimation bytecode. The original fixture was restored exactly; the new
meta hook now lives in `EEZMetaPostBatcher`, selected only by the testkit's `meta`
fixture option. Production fingerprint validation was not widened. The rebuilt original fixture
normalizes to 4,828 bytes with immutable offsets 157/222/1031/3141 and hash
`b9ede5d6a14473b896fe72b193f500250998ab99c9d3e65ad2c7153df35b61d1`, exactly the
existing production fingerprint. The callback proxy is explicitly deployed for
both added top-level static-read paths; an absent proxy is not a retry result.

The read-only blob evidence verifier now uses the same observed binding as the
follower. The supported outer-calldata/native-blob boundary and follower upgrade
order were added to the owning specification and operator runbook. The v8 review
corrections remain part of the proposed PR contract (target branch uses v7);
pre-release PR builds must be upgraded together. The relay pause is armed before
submission, and the test compares complete original calldata after restoring just
the two counts. Diagnostic output is intentionally retained for gas and endpoint
evidence. Mixed outbound/inbound meta dispatch, partial callback prefixes, and
retuned-specific restart/reorg remain coverage limits; generic partial/restart/reorg
regressions are run separately. No universal-wrapper support is claimed.


The consumed-case test deliberately does not claim that a separate meta call
completes the original queued EOA transaction. Existing reconciliation requires
its own receipt/nonce disposition; matching remote effects alone cannot authorize
dropping outer source actions. C13 is follower compatibility, not a new producer
workflow for rewriting live Composer submissions. The unconsumed-case retry
checks that the original source request can still execute normally.


A requested Claude recheck could not run: the account returned
`You've hit your 5-hour limit` (reset advertised as 16:30 UTC). The completed
review above is the only completed Claude review of this follow-up. An additional
internal read-only agent review of the final fixture separation, observed binding,
evidence-tool call, and four static-path fixture configurations reported no new
concrete findings; it is not a Claude recheck or runtime evidence.

The additional outbound manager-destination process test also passed, making
16 selected process tests with no failures or skips. Final source-built runtime
hashes are retained in [final-runtime-binaries.sha256](final-runtime-binaries.sha256).

### Reproduction commands

All Cargo commands use Rust 1.95.0, the unchanged lockfile, and
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` with `/usr/lib/llvm-22/bin` prepended to
`PATH`. Source process tests unset `EEZ_TEST_PREBUILT`,
`EEZ_TEST_RUNTIME_BIN_DIR`, and `EEZ_TEST_PROOF_SIGNER_BIN`; integration
contracts are built first. The final primary process selection is retained in
[process-selection.txt](process-selection.txt):

```bash
forge build --root contracts
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::retuned_dispatch:: static_local_writes partial_consumption:: \
  post_batch_target_wrapper_settles_both_directions \
  wallet_gas_estimates_post_batch_wrapper \
  native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order \
  --test-threads=1 --nocapture
```

`EEZ_TEST_LOG_DIR` and `EEZ_TEST_DATADIR_DIR` retain final primary diagnostics
under local-only `artifacts/pr123-followups-separated/{logs,datadirs}`.
The additional outbound manager-destination process regression is selected by
its full name, `outbound_call_to_the_l1_manager_is_rejected`; an initial filter
omitted `the` and selected no such case, and the separately selected run passed (1 test, 108.06 seconds).

The three new dispatch cases retain their L1 transaction hashes, canonical
endpoints and gas use in [retuned-dispatch-results.txt](retuned-dispatch-results.txt).
Receipt gas use includes refunds and is not a sufficient transaction gas-limit
estimate. No live deployment, operator service restart, or operator wallet operation was
performed.

### Final check inventory

| Check | Result |
| --- | --- |
| Protocol, blob/payload codecs, deriver, control RPC, prover client | 407 passed, no ignored tests |
| Signer | 340 passed; five explicit fixture generation/inventory utilities ignored |
| Testkit | 20 passed |
| `verify_blob_evidence` example, `--features chain-spec` | 20 passed |
| Primary source-built native process selection | 15 passed, no ignored tests |
| Additional manager-destination source-built process selection | 1 passed, no ignored tests |
| Integration contracts | 153 passed |
| Pinned `EEZExpressiveness.t.sol` | 11 passed |
| Workspace Clippy, all targets, `-D warnings` | Passed |
| Rust and integration Solidity formatting; diff whitespace | Passed |
| Edited documentation local links | Passed |
| Signer all-target check and all-feature Clippy, warnings denied | Passed |
| Signer rustdoc, warnings denied | Passed |
| Staged implementation and audit secret scans | Passed |

The focused Core run used `--match-path 'test/EEZExpressiveness.t.sol'` and retained
`--no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'`.
It is not a repeat of the full Core/64-scenario suite. Full current-revision
release artifacts, network suites and Kurtosis remain hosted-CI qualification;
older successful lanes are not evidence for these new sources. No full-workspace
unit suite, new throughput benchmark, or live Chiado deployment was run for this
follow-up. The protocol pin, dependency graph, compiler configuration, production
wrapper and genesis are unchanged.

The [retained check summary](check-summary.txt) includes nonzero test selections
and final compiler/lint completions. Full logs remain local-only under
`/tmp/pr123-followup-*`; the small committed summaries and source identities are
the portable evidence. Run the Rust checks with:

```bash
cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec -p eez-deriver -p eez-control-rpc -p eez-prover-client
cargo +1.95.0 test --locked -p eez-proof-signer -p eez-testkit
cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --example verify_blob_evidence
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 check --locked -p eez-proof-signer --all-targets
cargo +1.95.0 clippy --locked -p eez-proof-signer --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked -p eez-proof-signer --no-deps
```
