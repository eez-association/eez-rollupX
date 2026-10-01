# Split the release test archive from workspace unit tests

| Field | Value |
| --- | --- |
| Design status | Draft; implemented on branch `ci/split-test-archive`, not accepted until its CI evidence is reviewed |
| Date | 2026-10-01 |
| Source baseline | `eduadiez/rollupX` `main` at `d0776028` |
| Protocol baseline | `eez-core-protocol` `855fe060` (unchanged) |
| Related work | [Docker-native Cargo state](docker-native-cargo-cache.md); baseline CI run [36859860239](https://github.com/eduadiez/rollupX/actions/runs/36859860239) |

## Problem and scope

The producer built the release candidate, then archived every test target in one
`cargo nextest archive --release --workspace --lib ...` invocation. The workspace
selection activates `eez-composer` and `eez-deriver` dev-dependencies, which enable
Tokio's `test-util`. That feature changes Tokio's identity for the whole graph, so
the archive step recompiled 145 crates (126 third-party) that the shipping build
had just compiled. In the baseline CI run this step took 24.5 minutes of a 4-CPU,
cold-state producer. (`reth-provider/test-utils` is not a difference: `reth_gnosis`
already enables it in shipping bytes.)

Scope: CI test selection, the producer's test-archive guard, a new debug unit lane,
the automatically sized compile cap for the retained-state build, and the affected
tooling tests and contributor guidance. Non-goals: debug-profile E2E execution,
runner/ARC configuration, repository variables, and Rust source changes.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | E2E/process tests execute verified, exact-revision release binaries: the frozen shipping candidate, sealed to the packaged runtime and exported only after the producer suite passes. | Unchanged seal/verify/export checks; `test_native_artifacts`; E2E shards pass in CI. |
| R2 | Test-only features never reach shipping bytes; the producer still freezes the candidate before archiving and rechecks its hashes afterwards. | Frozen-hash check unchanged; local frozen hashes equal the release build's. |
| R3 | The release archive's dependency feature set equals the shipping build's: archiving compiles no non-workspace crate, and the producer fails if it does. | Guard unit tests; real tiny-workspace negative case; real local run: new selection passes, previous selection is rejected. |
| R4 | Every test the previous release unit suite ran still runs, including composer tests that need `tokio/test-util`. | Test-name inventory: debug lane ∪ producer suite ⊇ previous unit suite. |
| R5 | Four-way E2E sharding, partition inventories and the E2E target/module inventory are unchanged. | `test_native_artifacts`, `test_workflow_contract`. |
| R6 | Promotion requires the debug unit lane in both execution modes, alongside every existing lane. | `ci-result` gate tests (`test_workflow_contract`). |
| R7 | Retained Cargo state is saved only after the producer's release tests pass for the exact candidate on a default-branch push. | `test_runtime_artifacts` export-state checks. |
| R8 | The retained-state compile cap is sized from the build's Docker daemon (all CPUs; whole-GiB memory minus 4 GiB headroom), resolved once per job, logged with its source, enforced by Docker, asserted inside the build and equal to `CARGO_BUILD_JOBS`. Runner-environment overrides may only lower it; hosts below 8 GiB of cap are rejected before building. No repository variable selects it. | `test_runtime_artifacts` auto/headroom/override/invalid/too-small/recorded-cap checks; `test_workflow_contract`. |

## Design and alternatives

**Release archive (producer).** One invocation selects
`-p eez-node-e2e -p eez-l1 -p eez-protocol -p eez-blob-codec -p eez-payload-codec`
with `--example verify_blob_evidence`, `--test node_e2e` and the five protocol/codec
vector targets. Explicit target selection activates only those packages'
dev-dependencies, so the graph matches the shipping build's features. The producer
suite runs the vectors and `verify_blob_evidence` tests from this archive before
export, and writes `producer-passed.json` (formerly `unit-passed.json`) only after
they pass. That marker gates the E2E export and the optional Cargo-state export.

**Guard.** `native-tests.py build` requests Cargo's JSON messages
(`--cargo-message-format json-render-diagnostics`) and fails the archive step if any
`compiler-artifact` from a non-workspace package was not fresh. Unknown freshness or
package identity counts as compiled. Diagnostics still render on stderr. A new
dev-only third-party crate in an archived target's graph also fails the guard; move
such tests to the debug lane or deliberately revise the selection.

**Debug unit lane.** `ci.yml`'s `debug-unit-tests` job runs
`cargo nextest run --locked --workspace --lib --bins --profile ci --no-fail-fast
--no-tests=fail` in the ordinary test profile, in parallel with the producer (no
`needs`). It uses Lint's toolchain, LLVM, compiler-cache and runner-selection
pattern (integration pool, not the auxiliary pool), and inherits the workflow's
`RUSTFLAGS=-D warnings` and debug-info settings. `EEZ_TEST_PREBUILT=1` makes a test
that would launch a node or signer fail instead of compiling one. `ci-result`
requires it in both publication modes.

**Compile cap.** The retained-state build sizes its Docker limits and
`CARGO_BUILD_JOBS` from the Docker daemon that runs it; see [Compile cap](#compile-cap).

Alternatives:

- **Literal `-p eez-node-e2e` only** (rejected): without the `eez-l1` example, the
  graph re-resolves `stateless`/alloy features and rebuilt 105 crates (89
  third-party Reth-stack crates).
- **Previous selection minus `eez-composer`/`eez-deriver` ("dm")** (not chosen): no
  third-party rebuilds, but it still compiles and links release test binaries for
  17 workspace crates (3:49 versus 2:34 at 22 threads), and the two excluded
  packages' unit tests would need another lane anyway. Any future dev-dependency
  feature in another package would reintroduce the rebuild.
- **Upstream-style debug E2E** (deferred): would remove the release-binary E2E
  gate and needs its own measurement of debug-E2E runtime and flakiness.

## Compile cap

Update (2026-10-01): the retained-state cap is sized automatically. The first
revision of this draft read `EEZ_CI_BUILD_CPUS`/`EEZ_CI_BUILD_MEMORY` from the
repository variables `CI_BUILD_CPUS`/`CI_BUILD_MEMORY` (default 4 CPUs/`20g`);
workflows no longer pass those variables, and the producer no longer has defaults.

**Resolution.** `build-runtime-artifacts.sh prepare-state` reads `docker info`'s
`NCPU` and `MemTotal` from the daemon that runs the build. The cap is all `NCPU`
CPUs and `MemTotal` rounded down to whole GiB minus a fixed 4 GiB headroom for the
daemon, BuildKit and the runner, so Cargo fails at its own cgroup limit instead of
the node OOM-killing the job. Whole GiB keeps the value identical across nominally
identical nodes whose kernels reserve slightly different memory. A cap below 8 GiB
(or an invalid `docker info` value) fails before any build; any whole CPU count of
at least one is accepted. The values pass, as before, to BuildKit's `--resource`
limits, the build step's cgroup assertions and `CARGO_BUILD_JOBS`, so the enforced
cap equals the job count. Each state step logs
`Cargo state compile cap: <N> CPUs (<source>), <M> GiB (<source>, ...)`, with source
`auto: docker info ...` or `override: ...`.

**DinD and overrides.** Under ARC's per-pod DinD sidecar, `docker info` reports the
node, not the sidecar's Kubernetes limits, and Cargo inside the nested build
container cannot see an outer quota. Auto-sizing is therefore correct only when the
job has the whole node. If the DinD sidecar's limits are below the node, set
`EEZ_CI_BUILD_CPUS` (whole CPUs) and `EEZ_CI_BUILD_MEMORY` (bytes) in the runner
container's environment from the Kubernetes downward API: `valueFrom.resourceFieldRef`
with `containerName: dind` and `resource: limits.cpu` / `limits.memory` (default
divisor). Use whole-CPU limits; the downward API rounds fractional CPUs up.
Overrides replace the detected `NCPU`/`MemTotal`, may only lower them, and receive
the same rounding and headroom. Without them, the auto value oversubscribes such a
sidecar: Cargo runs more jobs than its CPU quota, and the memory cap exceeds the
sidecar's limit, so the sidecar is OOM-killed instead of the build failing cleanly.

**Scheduling assumption.** The cap reserves nothing. One build job per node must be
enforced by the runner scale set, for example with runner-pod resource requests that
cover the node's allocatable CPU and memory, or an equivalent dedicated node pool.
Two auto-sized builds on one node would each claim the whole node.

**Capacity (advice only, not implemented).** With three single-job nodes, the
producer, debug unit lane, eight E2E shards and Kurtosis compete for three slots:
sharding reduces elapsed time only when runners are free, otherwise shards queue.
Consider keeping heavy compilation (producer, debug unit lane) isolated one per
node and running lighter jobs in a separate scale set whose pods share nodes. A
build that shares its node that way must not claim the whole node: give its DinD
sidecar limits below the node and export them through the overrides above.

**Cache identity.**

- The retained Cargo-state key (`cargo-oci-v1-linux-amd64-<compatibility>-<source>`)
  does not include the cap. Cargo's fingerprints and outputs do not depend on the
  job count or cgroup limits (`CARGO_BUILD_JOBS` only schedules; release codegen
  units come from the profile), so state saved on one node size is valid on
  another, and Cargo reconciles it by fingerprint as for any restore.
- The cap reaches the `build-offline` step as build arguments, so it is part of that
  step's BuildKit cache key and of everything derived from it: binaries, node and
  signer images, the state export and the GHCR `mode=min` output cache. In a local
  probe (Docker Engine 29.8.1, Buildx v0.37.1) `--resource` alone did not change the
  key: a step built with `memory=1g` was `CACHED` with `memory=2g`. `prepare-state`
  records the resolved cap in `.ci-cargo-state/plan.json`; the build and export steps
  use that record and never re-detect, so the restore check, compilation, packaging
  and export share one identity. Identical nodes produce identical keys; a different
  node size or override misses the GHCR output cache and compiles from retained
  state, never reusing output built under another cap.
- `Dockerfile.runtime` changed (no cap defaults; required cap arguments), which
  changes the compatibility key: the first state build after merging is cold.

## Measurements

Single local samples at `d0776028`, Intel Core Ultra 9 185H, Rust 1.95.0, mold,
no compiler cache, archive steps in a target seeded by the shipping release build.
"4 CPUs" uses `taskset` on four P-cores with `CARGO_BUILD_JOBS=4`; "22 threads" is
uncapped. They are not CI timings; host noise was not controlled.

| Step | Compiled crates (third-party) | 4 CPUs: wall / CPU-min / peak RSS | 22 threads: wall / peak RSS |
| --- | ---: | ---: | ---: |
| Previous archive selection | 145 (126) | 14:49 / 58 / 5.0 GB | 6:22 / 10.1 GB |
| New release archive (E2E + vectors + example) | 6 (0) | 8:01 / 31 / 2.3 GB | 2:34 / 6.0 GB; implementation rerun 5:03 / 5.4 GB |
| "dm": previous minus composer/deriver | 17 (0) | not measured | 3:49 / not recorded |
| `-p eez-node-e2e --test node_e2e` only | 105 (89) | not measured | 5:05 / not recorded |
| Debug unit build (lib/bin + example + vectors) | cold debug graph | 14:30 / 54 / 12.0 GB | 7:08 / 21.1 GB |
| Debug unit run | — | 0:42 / — / 0.2 GB | 0:17 / 0.2 GB |

The new archive's `eez-node-e2e` composer/follower binaries were byte-identical to
the shipping ones.

Implementation check (same host, 22 threads, single samples, target seeded by a
fresh 8:25 shipping build): the guarded `native-tests.py build` compiled six
workspace crates and passed in 5:03; the previous selection, run through the same
guarded code, compiled 145 crates and was rejected for 126 non-workspace crates.
The new archive was sealed, its producer suite ran 30/30 tests and `export-e2e`
kept only `eez-node-e2e::node_e2e`. The debug lane (`--workspace --lib --bins`,
`EEZ_TEST_PREBUILT=1`, `-D warnings`) built in 6:11 at 11.2 GB peak RSS and ran
1,289 tests, all passing (3 ignored). Test-name inventory: debug lane (1,289 run,
1,292 listed) plus producer suite (30) equals the previous release unit suite
exactly (1,319 run, 1,322 listed, same three ignored tests), with no overlap; the
E2E inventory (115 listed, 111 run) is unchanged. The debug lane trades total CPU (it compiles a separate debug
graph) for a shorter producer critical path; whether that shortens CI wall time
depends on runner capacity. The baseline CI producer spent 30.5 min compiling the
release candidate and 24.5 min archiving tests from cold state.

## Compatibility and recovery

`native-tests.py` and `Dockerfile.runtime` are inputs to the retained-state
compatibility key and to BuildKit layer keys, so the first run after merging
rebuilds from cold state once. The producer marker is renamed; an old
`unit-passed.json` no longer authorizes export. Published E2E payloads keep their
format and verification. Rollback: revert the change; no data migration. Reverting
only the auto-sizing restores the 4-CPU/20-GiB default and the optional repository
variables, again with one cold state build.

## Validation plan

- `python3 -m unittest discover -s scripts/ci/tests -v`, including the opt-in real
  tiny-workspace relocation test (`EEZ_TEST_NATIVE_ARCHIVE=1`), which exercises the
  guard's rejection with real Cargo.
- A real local `native-tests.py build` with the new selection (guard passes) and the
  previous one (guard rejects), followed by `seal`, `run --suite producer` and
  `export-e2e`.
- The test-name inventory comparison for R4.
- The PR's CI: producer, debug lane, all E2E shards, Kurtosis and the gate. Record
  its producer timing against the baseline run, with the cold-state caveat above.

## Implementation and follow-up

Implementation and evidence are tracked in the draft PR from
`ci/split-test-archive`. Unverified until that CI run: real CI timings, the debug
lane's peak memory on the runner pool (it sets no job cap), and the one-time cold
state rebuild. Auto-sizing additionally leaves unverified until CI runs on the
homelab: the resolved values on its nodes, that runner-container environment
variables reach the job steps (the override path), and build timings at the
resolved cap. Separate pre-existing gap: `eez-protocol`'s `e2e-causal-verify`
example contains tests that no CI selection runs, before or after this change.
