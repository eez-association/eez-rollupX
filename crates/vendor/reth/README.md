# Reth shutdown backport

These two crates are copied from Reth **v2.7.0**, commit
`3d592ece6de8c4559987416a544fc215fd6d6921`. They retain their upstream
`MIT OR Apache-2.0` license; both license texts are included here.

The only Rust changes are the engine and launcher changes from
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

The crate manifests are unchanged from v2.7.0. This directory's workspace
manifest expands their inherited package metadata, lints and dependency entries
from the same revision; upstream path dependencies point to the existing
`v2.7.0` Git source. The root patches only these two packages. No dependency
version or feature selection is changed. Keeping the backport under `crates/`
makes it part of every existing Docker producer and source-content cache key.

The debug unit CI lane also runs the cleanup regression in this separate
workspace with `--locked` and `--no-tests=fail`. Its lockfile starts from the
root lockfile and adds only the four dependencies needed by upstream tests;
common package versions and source identities match. Its local patches ensure
both backported packages are used. Reproduce the check from the repository root:

```bash
cargo +1.95.0 nextest run --locked \
  --manifest-path crates/vendor/reth/Cargo.toml \
  -p reth-engine-tree --lib \
  -E 'test(test_engine_termination_waits_for_cleanup)' \
  --no-tests=fail --retries 0 --flaky-result fail
```

To review or regenerate, check out the exact base revision above, apply
`shutdown.patch`, and compare/copy `crates/engine/tree` and `crates/node/builder`
to the corresponding directories here. Keep the inherited dependency settings
identical to the base workspace. Do not edit the global Cargo cache.

Remove these patches only after adopting and validating a compatible Reth
release containing the fix. The backport changes shutdown ordering, not the
database format, protocol, ABI or queue durability.
The lockfile source changes still alter the prover validation identity: rebuild
and restart Composer and signer together. Mixing the backport with an old
signer fails the existing startup identity check.
