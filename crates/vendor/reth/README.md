# Reth v2.7 patches

These three crates are copied from Reth **v2.7.0**, commit
`3d592ece6de8c4559987416a544fc215fd6d6921`. They retain their upstream
`MIT OR Apache-2.0` license; both license texts are included here.

| Local crate | Upstream directory | Patch |
| --- | --- | --- |
| `reth-engine-tree` | `crates/engine/tree` | `shutdown.patch` |
| `reth-node-builder` | `crates/node/builder` | `shutdown.patch` |
| `reth-rpc-builder` | `crates/rpc/rpc-builder` | `rpc-shared-guard.patch` |

The engine and launcher changes come from
[upstream PR #27609](https://github.com/paradigmxyz/reth/pull/27609), head
`00a894ab7925d9af094689478abcb8aaaa410459`. That proposal was **open and
unmerged** when inspected on 2026-10-12. This is a locally reviewed backport,
not an upstream release or an upgrade to upstream main.

The engine previously acknowledged termination before dropping its persistence
and prewarming workers. Their RocksDB cleanup could then race native process
destructors and abort a successful restart. The backport releases these workers
before acknowledging termination. The launcher also waits for explicit shutdown
completion before treating a closed engine event channel as an error.

`shutdown.patch` records the complete upstream diff, including its unit and
Ethereum process regressions. The latter live outside these two crates and are
retained in the patch as reference coverage, not as tests run by this workspace.
See the [investigation](../../../docs/audits/2026-10-11-public-release-readiness/README.md)
for executed validation and limitations.

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

The crate manifests are unchanged from v2.7.0. This directory's workspace
manifest expands their inherited package metadata, lints and dependency entries
from the same revision; upstream path dependencies point to the existing
`v2.7.0` Git source. Its Tokio minimum is 1.53.3, matching the root workspace's
reviewed security update. The root and this workspace patch only these three
Reth packages; the RPC accessor introduces no dependency or feature change.
Keeping the patches under `crates/` makes them part of every existing Docker
producer and source-content cache key.

The debug unit CI lane selects the cleanup and shared-quota regressions in
separate steps in this workspace, both with `--locked` and `--no-tests=fail`.
Its lockfile starts from the
root lockfile and includes upstream test dependencies; common package versions
and source identities remain aligned. Its local patches select the same three
packages. Reproduce the cleanup check from the repository root:

```bash
cargo +1.95.0 nextest run --locked \
  --manifest-path crates/vendor/reth/Cargo.toml \
  -p reth-engine-tree --lib \
  -E 'test(test_engine_termination_waits_for_cleanup)' \
  --no-tests=fail --retries 0 --flaky-result fail
```

The shared-quota regression uses the same existing test infrastructure:

```bash
cargo +1.95.0 nextest run --locked \
  --manifest-path crates/vendor/reth/Cargo.toml \
  -p reth-rpc-builder --lib \
  -E 'test(test_exported_guard_shares_debug_quota)' \
  --no-tests=fail --retries 0 --flaky-result fail
```

The commands above are reproduction recipes; configuring the new CI selection
does not establish that the shared-quota test has run. Executed validation is recorded in the
[extended branch audit](../../../docs/audits/2026-10-24-extended-validation/README.md).

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
require the separate build-review digest check, not a new protocol identity.
Change the validation-contract version only when the accepted execution,
decoding, evidence or settlement semantics change. Rebuild affected artifacts
to include these implementation fixes; an unchanged identity is not evidence
that a particular binary contains them.
