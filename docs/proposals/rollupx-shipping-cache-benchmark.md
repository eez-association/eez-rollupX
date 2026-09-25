# Real shipping-build cache experiment

Status: temporary experiment; not production CI and not for merging.
Base: main `7c90266a5fe8cfdb46b249c61079badc2dce71a3`, plus the unchanged
small-probe implementation from draft PR #38. Protocol gitlink remains
`9d13218a355e7d543ca8a9927eea2962600b716d`.

The small probe passed cache transport and all 14 reuse/invalidation cases on
[GitHub-hosted](https://github.com/eduadiez/rollupX/actions/runs/36170864508) and
[homelab](https://github.com/eduadiez/rollupX/actions/runs/36171178574) runners.
That does not establish that transporting a real Cargo target is economical.

## First gate: shipping target size and unchanged reuse

The branch-only [workflow](../../.github/workflows/rollupx-cache-benchmark.yml)
uses two sequential homelab jobs. Do not open a PR from its execution branch
`test/rollupx-cache-benchmark`; normal CI is unchanged.

1. Build a tools-only image from the same pinned Rust 1.94.0 Bookworm base and
   native package list as the runtime Dockerfile. No Rust compilation in BuildKit.
2. Copy only tracked shipping-build source inputs to a fresh owned directory.
   Fetch dependencies into an empty private Cargo home, then build offline with
   the exact runtime package/binary/example selection, release profile and mold
   flags. Keep `CARGO_INCREMENTAL=0`; no sccache or production caches are used.
3. Record artifact freshness, phase times, cgroup peak memory, seven executable
   hashes, Cargo home/target size and archive preparation time. Execute `--help`
   for the three shipping binaries as smoke checks, not semantic validation.
4. Archive sources, dependency downloads and compiled state with PAX timestamps.
   Refuse upload above 4 GiB compressed. The run-specific cache has no fallback
   key and the restore job fails on a miss. No cache budget increase or cleanup
   of existing caches is authorized.
5. Restore in a fresh job, verify commit/source/protocol/build identity and seed
   executable hashes, reconcile the authoritative checkout with checksum rsync,
   then invoke the same offline build. Native package and toolchain identity must
   also match. Report actual freshness (recompilation is a measured result), and
   require all seven final executable hashes to remain unchanged.

All compilation containers have four CPUs, 20 GiB memory, no swap, read-only
roots and no Docker socket, operator credentials or host caches. Only dependency
fetch has networking. At least 30 GiB free disk is required; no automatic host
cleanup. Containers have unique names and only those containers are removed.
The source allowlist matches this shipping stage, not arbitrary workspace/E2E
inputs; it deliberately excludes git metadata and the protocol checkout, as
does the current shipping Docker stage. Fixed container paths are identical
between seed and restore, but differ from the production Docker build path.

Tool installation uses the production package list, not a new dependency pin;
package inventories are compared between jobs to reject drift. This initial
experiment does not compare to the existing sccache/BuildKit baseline. Its cold
build is intentionally uncached. Compare total GitHub job/step time including
tool-image setup, archive compression and cache transfer, not just Cargo time.

## Decision and remaining work

This first gate deliberately excludes release test archives, mutation builds,
runtime Docker packaging, unit/E2E execution, and publication. It cannot replace
the existing CI gate or prove a whole-pipeline speedup. If transfer/reuse is not
promising, stop before spending resources on that larger matrix.

If promising, retain separate native/Composer feature selections and add the two
test archives, fresh-job test-only and production edits, exact runtime packaging
and normal correctness checks. Compare the current cached Docker build and the
candidate on the same source, resources and workloads, including all transfer
costs. Production integration also needs compatibility keys, cold-build fallback
and a reviewed source inventory; none is enabled by this experiment.

## Local preflight evidence (2026-09-25)

- Full tooling suite: 272 tests in 119.884s; 267 passed, five opt-in tests skipped.
  Four skips are the existing Docker payload/runtime, native archive relocation
  and Kurtosis integration probes. The fifth is this new Docker fixture.
- All eight benchmark tests passed separately with the real Docker fixture
  enabled. Seven tiny executables built cold, survived the PAX/gzip round trip,
  and were all fresh with unchanged hashes after source synchronization.
- All 16 production workflow-contract checks passed; no production workflow was
  edited. The shipping command and native package list are checked against
  `Dockerfile.runtime` by the new tests.
- Local-only logs are in `artifacts/benchmark-validation/` and
  `artifacts/shipping-cache-fixture-lma8nrg_/` in this worktree. These validate
  orchestration, not real RollupX cache size or performance.
