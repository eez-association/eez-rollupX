# Pending debug trace correction

Status: correction verified by unit, process and live checks. This follows the independent
[parity review](parity.md), preserving its original failure and source identity.

## Failure

At candidate runtime `319ebc2dbfdace3d5e708a2c9881cc741529aec4`, a read-only
ordinary transfer simulation at a stable parent 12302 returned `-32603` from
`debug_traceCall pending`: the environment-only fallback correctly refused an
unexecuted L1 history import. The same input succeeded with debug `latest` and
`eth_call pending`. No transaction was broadcast. The local record is
`artifacts/extended-validation/kurtosis/pending-debug/comparison.json`, SHA-256
`79b28fc523a3076dd4071d0a3fd298375a6bb718338e667c53a1ae7bb14c9278`.
The exact main runtime `a50c326d` from CI `39776847137` subsequently passed
this same input at stable parent 338 (2 mod 6): both debug tags returned
`failed=false`, gas 21000, and `eth_call pending` returned `0x`. Its local
record is `artifacts/extended-validation/baseline/pending-debug/comparison.json`.
The final candidate passes the same live observation, as recorded below.

## Implementation

The [pending adapter](../../../crates/eez-node-common/src/rpc/pending.rs) now
supplies the existing checked prefix state/environment to Reth's unmodified
`DebugApi::debug_trace_call`. It preserves the selected tracer, overrides,
application failure output, named/positional parameters and historical selectors.
An executed pending payload and calls with `txIndex` use Reth's existing
state lookup and replay;
transaction-index replay is not approximated by applying an empty pending prefix.
The [history specification](../../../contracts/L1_HISTORY_SPEC.md#ethereum-rpc-simulation)
defines this surface and the remaining unadapted simulation methods.

Tracing uses the registry's existing aggregate guard, shared by stock debug,
trace and bundle methods. EthApi's similarly named guard is a different proof
quota and cannot substitute for it. A minimal accessor in the exact vendored
Reth RPC builder exposes a clone of the real guard; it does not create another
pool or limit. The pending context retains its I/O permit; the adapter owns a reference to
the acquired tracing permit through detached blocking execution. Stored-block
and already executed pending routes retain that reference in their delegated
worker callback too. Independent review caught the initial draft's omission of
this ownership on delegated routes before the correction was built or deployed.

Reth's inherent tracing future retains an abstract `TraceExt` boundary with an
explicit `Send` result before entering the async RPC implementation. Rust 1.95
rejects the directly specialized callback lifetimes there. This small helper
does not allocate another future box, create a task or acquire another permit.

No protocol rules, native encoding, history validation or prover API change for
this correction. The added Reth package remains at the same
v2.7 commit; its [vendor record](../../../crates/vendor/reth/README.md) owns the
source provenance and patch. The nested vendor workspace also takes the already
reviewed Tokio 1.53.3 fix so its lifecycle check uses the same scheduler version.

## Required verification

- Existing prefix unit controls cover an advancing import, an empty repeat,
  actual opcode PREVRANDAO, request-local state, malformed storage and frozen
  evidence. The missing-prefix environment path must remain a rejection.
- The existing process test `pending_history_import_requires_executed_state`
  adds debug observations at a stable import boundary. It compares trace output
  with ordinary calls, checks both named spellings, historical/default tags,
  post-prefix overrides, application reverts and real-block `txIndex` behavior.
- The vendor quota regression must prove an exported permit blocks a stock debug
  method under a one-request limit; an independent semaphore would fail it.
- Two adapter tests dispatch the real state callback, suspend it with a channel,
  cancel and await the owning request, and assert that the next trace remains
  blocked until the callback is released. Both cached-prefix and stored-state
  delegation are covered; no external adapter clone can mask premature release.
  Timeouts act only as failure watchdogs. Reth mocks are dev dependencies.
- The affected process test and separate live sample pass on the combined
  shipping candidate, as recorded below. HTTP
  tests do not independently establish WebSocket/IPC transport behavior.

The final [reviewed lock record](pending-debug-lock.json) records the direct
trace edges, the local Reth patch and the test-only mock edges. Full workspace
metadata before and after has identical resolved package features. The portable
identity and validation version remain unchanged. The workflow contract tests
pass **21/21** after adding the explicit vendor quota regression to the debug lane.
These metadata and Python results are not Rust execution coverage.

## Executed local checks

`cargo +1.95.0 test --locked --offline -p eez-node-common --lib --features
l1-startup -- --test-threads=1` passes **33/33**, with no ignored or filtered
tests. This includes both real callback cancellation tests and the existing
import/repeat, PREVRANDAO, malformed-state and frozen-evidence controls. The
initial compilation failures (the test pool constructor and the concrete
tracing future's lifetime inference) remain under
`artifacts/extended-validation/checks/node-common-pending-*`; neither is counted
as runtime coverage.

A temporary negative control removed the tracing permit from
`SimulationApi::clone`. Both cancellation tests then failed at the required
assertion that the next permit acquisition remains pending, after the original
RPC task had been aborted and awaited. The code was restored in `finally`, its
exact hash was checked, and the full **33/33** suite passed again. This control
does not weaken or persistently change either test. The retained result is
`artifacts/extended-validation/checks/trace-quota-mutation.json`, with both
assertion failures and the restored source hash. All checks used one build job
and a memory guard; none was cancelled by the guard.

`cargo +1.95.0 fmt --all --check` and `git diff --check` pass.
`cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings`
passes in 80.02 seconds with one job and at least 20,193,251,328 available host
bytes. The generic tracing helper has a local `manual_async_fn` expectation:
its explicit `Send` return bound is the deliberate compiler boundary described
above. The first lint result and its correction are retained.

The first vendor-quota build was cancelled before tests when the independent
workspace selected a different full-debug profile and began rebuilding the
whole dependency graph. Its replacement explicitly uses the root's existing
local dev settings (`opt-level=1`, `debug=line-tables-only`,
`split-debuginfo=unpacked`) with the same locked sources. This is a semantic unit
check, not a shipping-build or performance comparison. The replacement quota check passes **1/1** (34 tests outside selection), in
0.041 seconds after 511.14 seconds of preparation/compilation. It uses the real
stock debug method under the exported registry guard. Cancellation of the first
build is not counted as test coverage. Its retained result is
`artifacts/extended-validation/checks/reth-rpc-quota-root-profile/result.json`.

The shutdown cleanup regression also passes **1/1** (206 tests outside
selection) with the corrected nested Tokio 1.53.3 lock. It covers successful
and failed persistence, requiring worker-resource cleanup before acknowledging
termination. The test took 0.296 seconds after 862.23 seconds of preparation/
compilation with the same explicit dev profile; the memory guard did not fire
(minimum available memory 16,026,611,712 bytes). Results are retained under
`artifacts/extended-validation/checks/reth-cleanup-root-profile/`.

The final root lock also passes the full **7/7** `eez-control-rpc` library tests
with no filtered/ignored tests, including portable identities, malformed/truncated
wire bytes and the dependency-review guard. The guarded command finishes in
2.00 seconds; its result is in
`artifacts/extended-validation/checks/control-rpc-pending-lock/`.

A separate local debug role build was manually cancelled after 1,234.33 seconds
before producing a verified current role set. The final CI producer was already
building the shipping runtime and matching test archive; the process regression
will consume those exact verified artifacts instead of duplicating another large
local harness build. This cancellation is not successful compilation or test
coverage. The command, first output and explicit reason are preserved under
`artifacts/extended-validation/checks/pending-debug-source-roles/`. No old local
role executable was substituted.

## Exact final shipping process

The final CI `40123208325` merge
`c35cf5bc71548f74b8d72e3fa2a0a66113eab0c8` has the same tree as
`28049ff734ab931cbde01246c5d238cce6ef65bf`. Its existing native consumer
verified the archive identity and all 30 test files, then ran exactly
`e2e::pending_history_import_requires_executed_state`: **1/1 PASS** in
9.540 seconds, with 178 tests outside selection and no retries. Runtime binaries
match the packaged shipping archive; no Rust/runtime executable was rebuilt or
substituted. Foundry 1.7.1 built the exact checkout's contract fixtures first.

All assertions remained intact: matching history/PREVRANDAO between call and
trace, named/positional parameters, default/historical selectors, post-prefix
overrides, real transaction-index replay/bounds, and application revert output.
The [portable process record](pending-debug-process.json) contains the exact
executable hashes, invocation, result and retained local log digests. Logs and
datadirs remain under the final consumer's `artifacts/pending-debug/`; these
ignored files are local-only. The full process took 11 seconds and retained at
least 19,918,147,584 available host bytes; the resource guard did not fire.

The independent CI Embedded 4/4 lane also passes this unchanged process case in
9.697 seconds. Its complete lane passes 10/10; the final process inventory is
tracked separately from the local directed invocation.

## Final live control

At **20:14:53 UTC**, the verified `28049ff7` node on the fresh isolated enclave
passed the same ordinary-transfer input as the original failure and exact main
control. The L2 parent remained **104** (2 mod 6), hash
`0x12e0e16484b5f53f2cce36ea831abbf11c4049f16acbbb808926ef0aa23eea34`,
through the observation. Both `debug_traceCall latest` and `pending` returned
`failed=false`, gas 21000; `eth_call pending` returned `0x`. No transaction was
broadcast. The [candidate network record](kurtosis/candidate-network.json)
contains the input, before/after parent, next cutoff and all responses, alongside
the deployed executable's match to the shipping archive.

This closes the reproduced live regression for that input. It does not expand
the process/unit assertions or the transport coverage described below.

## Final independent source review

A separate reviewer followed the final adapter through node/follower wiring and
the exact Reth `3d592ece` helpers, without modifying source or executing another
process. No material finding remained: the aggregate quota is shared, worker
ownership survives cancellation on both routes, executed pending state bypasses
the prefix, and historical/transaction-index paths retain Reth's behavior. The
reviewed `pending.rs` SHA-256 is
`d2edb17047f0091c910ab40eb86788dce1d66f068a47fabf3c09ae79e125658f`.

The existing HTTP process/unit/mutation results are not rerun coverage from
that review. Dedicated WS/IPC transport probes and forced concurrent appearance/
disappearance of an executed pending payload are not covered by these tests;
the corresponding wiring and bypass were inspected statically. Other methods
outside the specified four public simulation APIs remain outside this correction.
