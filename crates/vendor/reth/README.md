# Reth v2.11 patches

These three crates are copied from Reth **v2.11.0**, commit
`b9abbcc1b982755baa9021e6874e2e58936ca65f`. They retain their upstream
`MIT OR Apache-2.0` license; both license texts are included here.

| Local crate | Upstream directory | Patch |
| --- | --- | --- |
| `reth-engine-tree` | `crates/engine/tree` | `shutdown.patch` |
| `reth-node-builder` | `crates/node/builder` | `shutdown.patch` |
| `reth-rpc-builder` | `crates/rpc/rpc-builder` | `rpc-shared-guard.patch` |

The shutdown fix was originally backported from
[upstream PR #27609](https://github.com/paradigmxyz/reth/pull/27609), head
`00a894ab7925d9af094689478abcb8aaaa410459`. That proposal was open when
inspected on 2026-10-12; this directory records a local adaptation to the exact
v2.11.0 release, not an adoption of upstream main.

Reth v2.11.0 already drops the engine before acknowledging termination in
`EngineApiTreeHandler::run`, and its persistence service joins on drop. Those
upstream implementations are unchanged. The local patch retains the missing
prewarming-worker join, waits for every payload-prewarm context to release its provider,
and makes explicit launcher shutdown await completion before event-channel closure can
be treated as fatal. These joins are cooperative and unbounded: an in-flight transaction
or blocked provider read can delay shutdown indefinitely. The original cleanup regression
uses an idle worker and covers successful and failed persistence; on unpatched v2.11.0
it fails because termination is acknowledged before that worker's cleanup.

A disposable payload/Engine regression also exposed RocksDB provider destruction on the
named prewarm worker after process exit began. Each prewarm context now carries a
completion sender as its last field, including clones used by parallel transaction and
BAL prewarming. Processor destruction releases its own sender and waits for disconnection.
This adds no wait to normal block execution. Payload handles must be released first so
their termination/validation signals unblock those contexts.

Validation-path sparse-trie, proof-worker and serial-root fallback tasks retain the
same engine-owned signal. A local task owner drops its captures before its sender,
including on unwind or when never run. Proof workers retain a small read-only factory
wrapper until their worker-local providers exit. This adds no per-block cleanup wait
and no worker-name barrier. It applies to the default strategy; custom strategies must
own their own background lifecycle.

The payload regression covers empty, donation and history blocks, including six ordinary
transactions to engage parallel prewarming. Successful process exit is checked separately;
an immediate MDBX reference count is not a complete ownership oracle. Deterministic probes
check prewarm whole/partial-move drop, dispatcher capture and panic paths, and task-owner
return/unwind/unrun drop. Call-site tests drive `DefaultStateRootStrategy::prepare`
through success, cancellation, blocked proof-worker initialization/failure, and a zero-timeout
serial fallback that outlives the winning sparse result. A separate real proof-worker test
injects a job panic and holds provider replacement; it composes with the `prepare` propagation
test rather than injecting a panic through the full Engine path. A real processor-drop test
holds its own `shutdown_owner()` clone, observes no completion for 100 ms, then releases it
and requires completion. Validator sender identity and full Engine acknowledgement for
prepare-launched work remain source-inspected. A separate deliberately
reordered context makes the prewarm probe fail at the intended lifetime assertion.

Payload-builder sparse tasks still use the unchanged public spawn path without an engine
signal. With builder sharing enabled, their position ahead of validation work on a named
worker FIFO can nevertheless delay the engine's completion signal. Their launcher/job
cancellation ordering remains an explicit gate before extending
this wait. BAL pool provider closures are covered indirectly by blocking `end_block`;
BAL execution, a blocked provider or prewarming-pool thread, and full-node restart still
need validation. The signal does not establish that every RocksDB owner is released before
Engine acknowledgement, and cannot provide bounded shutdown for non-returning work.

`shutdown.patch` is the complete local difference from v2.11.0 for these two
crates, including the unit regression. The original Ethereum process regression
is outside these crates and is not coverage provided by this workspace. Full
node shutdown/restart still needs process validation. The historical
[investigation](../../../docs/audits/2026-10-11-public-release-readiness/README.md)
records the original v2.7 backport, not validation of this upgrade.

`rpc-shared-guard.patch` is a local accessor and regression, not an upstream
backport. `RpcRegistryInner::blocking_pool_guard()` clones the existing guard
shared by the registry's debug, trace, bundle and Reth handlers. An additional
handler can therefore use the same aggregate `max_tracing_requests` quota.
Allocating another guard would increase that aggregate capacity; the Eth API's
separate guard uses `proof_permits` and does not provide this quota. The accessor
does not change the built-in handlers, defaults, execution rules or scheduling.

The regression holds the exported guard's sole permit, polls the stock
`debug_traceCallMany` handler with empty bundles, and verifies that it is blocked.
Releasing the permit exposes that handler's immediate invalid-parameter error.
This checks shared admission without sockets, state lookups or timing assumptions.
Callers remain responsible for retaining their permit until expensive work ends,
including when the client cancels its request.

The crate manifests are unchanged from v2.11.0. This directory's workspace
manifest expands their inherited package metadata, lints and dependency entries
from the same revision; upstream path dependencies point to the existing
`v2.11.0` Git source. Its Tokio minimum is 1.53.3, matching the root workspace's
reviewed security update. The root and this workspace patch only these three
Reth packages; the RPC accessor introduces no dependency or feature change.
Keeping the patches under `crates/` makes them part of every existing Docker
producer and source-content cache key. The root and this workspace select official evm2 at
`e681662b6d9ec59edafd112f7c8d64f49f1e6dd1`, using the official repository's
`DaniPopes/evm2` redirect to override Reth's older `alloy-rs/evm2` revision.
No local engine fork or path dependency is required. RollupX's fee and native
system-call adaptation lives in `eez-evm`; these vendored crates retain only
the recorded shutdown and shared-quota changes. All producer inputs and exact
Git dependencies must be available before frozen Docker/CI qualification.

The debug unit CI lane selects the cleanup and shared-quota regressions in
separate steps in this workspace, both with `--locked` and `--no-tests=fail`.
Its lockfile starts from the
root lockfile and includes upstream test dependencies; common package versions
and source identities remain aligned. Its local patches select the same three
packages. Reproduce the cleanup check from the repository root:

```bash
set -euo pipefail
for test in \
  tree::tests::forkchoice_updated_tests::test_engine_termination_waits_for_cleanup \
  tree::payload_processor::prewarm::tests::prewarm_shutdown_signal_follows_factory_drop_on_every_exit \
  tree::payload_processor::tests::processor_drop_waits_for_background_owner \
  tree::state_root_strategy::tests::shutdown_owned_resources_precede_completion_on_return_unwind_and_unrun_drop \
  tree::state_root_strategy::shutdown_tests::prepare_owns_sparse_root_until_success_or_cancellation \
  tree::state_root_strategy::shutdown_tests::prepare_owns_proof_workers_after_cancellation_or_open_failure \
  tree::state_root_strategy::shutdown_tests::prepare_owns_serial_fallback_after_sparse_root_wins \
  tree::state_root_strategy::shutdown_tests::proof_job_panic_retains_owner_until_provider_replacement_exits \
; do
  cargo +1.96.0 nextest run --locked \
    --manifest-path crates/vendor/reth/Cargo.toml \
    -p reth-engine-tree --lib -E "test(=$test)" \
    --no-tests=fail --retries 0 --flaky-result fail
done
```

The shared-quota regression uses the same existing test infrastructure:

```bash
cargo +1.96.0 nextest run --locked \
  --manifest-path crates/vendor/reth/Cargo.toml \
  -p reth-rpc-builder --lib \
  -E 'test(test_exported_guard_shares_debug_quota)' \
  --no-tests=fail --retries 0 --flaky-result fail
```

These commands require the v2.11 Rust 1.96 / LLVM 23 toolchain. They select
focused package regressions, not full runtime or storage compatibility. The
[extended branch audit](../../../docs/audits/2026-10-24-extended-validation/README.md)
records earlier v2.7 validation only; migration evidence must identify the new
source and executable artifacts.

To review or regenerate, check out the exact base revision above, apply
the two patches, and compare/copy the directories in the table. The RPC patch is
the complete difference from its exact upstream crate, including its regression.
Preserve the inherited dependency settings and the documented Tokio alignment.
Do not edit the global Cargo cache.

Remove these patches only after adopting and validating a compatible Reth
release containing the fix. The backport changes shutdown ordering, not the
database format, protocol, ABI or queue durability.
The [portable validation identity](../../eez-control-rpc/VALIDATION_IDENTITY.md)
does not contain Cargo.lock, source revisions or binary hashes. Lockfile changes
require dependency-impact review and affected behavioral tests.
Change the validation-contract version only when the accepted execution,
decoding, evidence or settlement semantics change. Rebuild affected artifacts
to include these implementation fixes; an unchanged identity is not evidence
that a particular binary contains them.
