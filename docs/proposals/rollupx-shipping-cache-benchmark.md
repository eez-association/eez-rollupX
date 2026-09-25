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

The initial branch-only [workflow](../../.github/workflows/rollupx-cache-benchmark.yml)
used two sequential homelab jobs. Do not open a PR from its execution branch
`test/rollupx-cache-benchmark`; normal CI is unchanged.

1. Build a tools-only image from the same pinned Rust 1.94.0 Bookworm base and
   native package list as the runtime Dockerfile. No Rust compilation in BuildKit.
2. Copy only tracked shipping-build source inputs to a fresh owned directory.
   Fetch dependencies into an empty private Cargo home, then build offline with
   the exact runtime package/binary/example selection, release profile and mold
   flags. Keep `CARGO_INCREMENTAL=0`; no sccache or production caches are used.
3. Record artifact freshness, phase times, cgroup peak memory, seven executable
   hashes, Cargo home/target size and archive preparation time. Persist build
   measurements before CLI checks. Check help for all three shipping binaries,
   short help/version and node help for composer/follower, and rejection of an
   unknown argument for all three. These are CLI checks, not semantic validation.
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

## First homelab attempt and retry

[Run 36173369573](https://github.com/eduadiez/rollupX/actions/runs/36173369573)
compiled the real shipping selection successfully in 1690.625 seconds (28m11s),
with a compilation-container peak of 10,615,058,432 bytes (about 9.9 GiB).
It then failed because `eez-composer --help` returned exit 1. No transport archive
was uploaded, and restore was skipped. The diagnostic artifact upload also hit
GitHub's artifact storage quota; that is distinct from the Actions cache quota.

Composer and follower both propagated Clap's help/version display requests as
application errors. The retry branch lets Clap print and exit at the CLI
entrypoint: help/version return 0, argument errors return 2, and non-Clap
application errors remain fallible. Signer parsing is unchanged, including its
lack of a version flag. No protocol or runtime execution rules are changed.

The benchmark now records its partial result before CLI checks, emits bounded,
escaped log tails directly into the job log on failures/timeouts, and uses
60-second CLI-check deadlines. Invalid-argument checks require exit 2 and the
specific diagnostic; crashes and help failures cannot pass as expected errors.
Failed CLI checks still prevent archive creation and cache upload. The retry
remains branch-only, without merging or changing production CI.

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

Retry preflight (same date):

- All 16 node-common library tests passed with `l1-startup`, including subprocess
  exit-code/output-stream regressions and existing chain-document parser tests.
- Rebuilt the actual composer and follower in the local dev profile with default
  shipping features. All 14 CLI checks passed: long/short help and version, node
  help, and invalid root/node arguments, checking exit codes and output streams.
- Workspace/all-target Clippy (`--locked`, warnings denied), Rust formatting,
  and whitespace checks passed. No dependency or lockfile change.
- Full tooling run: 275 tests, 270 passed and the same five opt-in skips as above.
  After adding the partial-report regression and tightening short-version checks,
  all 12 benchmark tests passed with the bounded real Docker fixture enabled.
  Unchanged fixture restore was all-fresh with identical executable hashes.
- Logs: `artifacts/benchmark-validation/{tooling-cli-fix,clippy-cli-fix,production-cli-fix,docker-fixture-cli-fix-final}.log`.
  Fixture evidence: `artifacts/shipping-cache-fixture-r9slt3l4/`.
  These checks do not claim full release/Kurtosis CI or a successful real
  shipping-cache transport; the isolated homelab retry must establish the latter.

## Successful shipping cache result

[Run 36182979151](https://github.com/eduadiez/rollupX/actions/runs/36182979151),
commit `5a097a19057a86dd3f8188fa0c0eb72903b5ae75`, passed both homelab jobs:

| Measurement | Result |
| --- | ---: |
| Cold shipping compilation | 1695.192 s |
| Unchanged compilation after fresh-job restoration | 1.483 s |
| Entire restore job, including setup and transfer | 206 s |
| Fresh Cargo artifacts on restore | 1157 / 1157 |
| Shipping Cargo target | 4,179,048,050 bytes |
| Cargo home | 1,483,317,392 bytes |
| Transport archive | 1,945,870,732 bytes |
| Archive creation | 71.359 s |

All seven shipping executable hashes matched; CLI checks passed. This establishes
real shipping-state transport and unchanged reuse, not full-CI speedup.

## Next gate: source changes and release-test state

The workflow now has three **serial** jobs using
[`rollupx-cache-followup.py`](../../scripts/ci/rollupx-cache-followup.py). The first
two restore only the immutable cache key `rollupx-shipping-probe-v1-36182979151-1`;
there is no fallback or cold rebuild on a cache miss. Shipping inputs, protocol
pin, build command, toolchain/native package inventory and executable hashes must
still match the successful seed. A different instrumentation commit is allowed
only against that explicit seed commit, with unchanged shipping source hashes.
This deliberately retains the measured baseline rather than rebasing to a
different main revision during the comparison.

1. **Mutations:** warm the unchanged shipping build, then add a failing test to
   Composer’s existing test-only `settlement_policy/tests.rs`. Shipping must
   remain entirely fresh with identical hashes. Change the signer's CLI help
   description; require recompilation, changed signer bytes and the new text
   from actual execution. These edits occur only in the disposable source copy;
   neither modified sources nor mutated Cargo state are uploaded.
2. **Archive seed:** restore a separate clean shipping baseline. Freeze the
   shipping binaries before building native tests and then Composer unit tests,
   using the selections imported from the production native-test producer. A
   `cargo test --no-run --message-format=json` records freshness immediately
   before each matching Nextest archive command. This adds an instrumentation
   invocation; report packaging and compile times separately. Extract both
   archives, verify test executable hashes and included frozen runtime hashes,
   measure target size, and package the combined Cargo state. Retain the existing
   **4 GiB compressed cap**: exceeding it fails before upload; do not raise the
   budget or delete other caches to force a pass.
3. **Archive restore:** restore only this run's combined state in a new job.
   Repeat shipping → native archive → Composer archive, reporting actual reuse
   rather than assuming switching feature variants is free. Require matching
   shipping and test executable hashes. Finally add the new failing Composer
   test, rebuild its archive, extract it and execute the exact new test. Require
   a real assertion failure (exit 101, one failed test, the expected panic marker)
   so a stale archive or zero-test selection cannot pass.

The tools-only image additionally installs the same checksum-pinned Nextest
0.9.143 as production. Compilation stays offline inside the same four-CPU,
20-GiB/no-swap containers. Require 50 GiB free disk, preserve partial reports and
failure log tails, and upload at most one additional capped cache. Test execution
here is only the invalidation canary: the full unit, native E2E and Kurtosis
suites, runtime packaging, and deployment remain outside this experiment.

Local follow-up preflight:

- The bounded real Docker/Nextest fixture passed all three modes, including
  archive extraction/hash checks and execution of the newly failing test.
- The first fixture attempt failed because the copied Nextest configuration
  referenced `eez-node-e2e`, absent from the tiny workspace. Only the fixture's
  package filter was corrected; production selections/configuration are unchanged.
  Both attempts are preserved in `artifacts/benchmark-validation/followup-fixture-{first,second}.log`.
- The tiny fixture uses two identical small selections to exercise orchestration;
  it does not establish real Tokio feature-variant reuse or release-test size.
- Full tooling suite: 280 tests in 122.126s, 274 passed and six opt-in skips.
  The follow-up Docker test was run separately as above; the remaining skips
  cover the existing shipping fixture, Docker payload/runtime, native relocation
  and Kurtosis probes. An additional mutation-anchor check passed separately.
  All 16 production workflow-contract tests, Python compilation and whitespace
  checks passed. No production Rust was edited, so Rust suites were not rerun.
  Logs: `artifacts/benchmark-validation/followup-tooling.log`; successful local
  fixture state: `artifacts/cache-followup-fixture-l3aqx5ec/`.
