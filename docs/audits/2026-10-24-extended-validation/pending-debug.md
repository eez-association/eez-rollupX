# Pending debug trace correction

Status: implementation and verification in progress. This follows the independent
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
The candidate post-fix live sample is pending.

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

No protocol rules, native encoding, history validation, prover API or dependency
versions change for this correction. The added Reth package remains at the same
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
- The vendor quota/cleanup checks, affected process test and a verified
  final-runtime live sample remain to execute on the combined change. HTTP tests
  do not independently establish WebSocket/IPC transport behavior.

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
check, not a shipping-build or performance comparison. Its result is still
pending; cancellation is not counted as test coverage.
