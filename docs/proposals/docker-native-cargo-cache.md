# Docker-owned Cargo state in CI

Status: proposed in [PR #40](https://github.com/eduadiez/rollupX/pull/40), which tracks
the current CI results and merge readiness. Retained state remains opt-in.
Alternative to [PR #39](https://github.com/eduadiez/rollupX/pull/39), based on
`7c90266a`; protocol pin unchanged (`9d13218a`).

## Proposal

Keep one build recipe in `Dockerfile.runtime`. Two small entry steps select online
compiler-cache access or offline retained-state compilation; neither duplicates
the shipping/archive commands. Carry source, Cargo downloads
and target together in a separate, flattened OCI image layout. GitHub Actions
stores that layout using its existing cache service; Docker handles its layers.
The existing Bash producer orchestrates preparation, builds and optional export.
No new production Python helper, persistent builder service or cluster change.

The two caches have different responsibilities:

| Cache | Contents | Writer |
| --- | --- | --- |
| Existing registry cache | Flat executable/test output (`mode=min`), never Cargo state | Existing ref-scoped producer |
| Actions OCI cache | Source, downloads and Cargo target, no verified artifact status | Default-branch pushes after exact-candidate unit success |

Node and signer images copy from the same flat executable-output stage. State
export does not extend previous image ancestry. Cache eviction is normal; no new
GHCR state-image retention policy is needed.

## Guarantees and limits

- Compatibility covers the pinned Rust base, installed native packages/toolchain,
  Dockerfile and archive configuration. Exact keys also cover tracked build
  inputs; compatible-prefix restores let Cargo reconcile a source change.
- Validate the OCI layout/config and Docker-restored state before invoking Cargo.
  Limit transport to 4 GiB and expanded state to 32 GiB; reject future timestamps
  and links outside state. Rejected or missing cache falls back before compiling.
  Build/test failures are never retried as cache misses.
  The expanded-size check runs after Docker unpacks the layers, not as an
  extraction-time disk quota; trusted exports enforce it before upload.
- Checksum rsync applies the authoritative checkout, deletes removed inputs and
  preserves unchanged mtimes. Cached data never overwrites that checkout.
  Cached programs are still trusted CI inputs, not a security boundary against
  a malicious repository maintainer or cache writer.
- Fetch dependencies separately, then compile retained state offline, without
  compiler-cache credentials. Enforce four CPUs, 20 GiB RAM and zero swap inside
  the build step. Require a 24-GiB Docker host and 50 GiB free workspace space;
  these checks do not reserve node resources or impose a PID limit.
- Preserve all seven shipping exports, frozen shipping bytes, the separate
  Composer archive, exact-revision sealing, all E2E shards, Kurtosis and promotion
  gates. Cached compilation never marks tests passed.
- Restore/export/save are optional and time-bounded. Save only after unit success
  on default-branch pushes, and do not re-upload exact cache hits.

## Rollout and rollback

Initially opt-in for `homelab-rust`: `CI_CARGO_STATE_CACHE=true`, or the manual
`ci.yml` boolean `cargo-state-cache=true`. Hosted runners keep the ordinary
Docker/optional compiler-cache path. Unset the variable and leave the manual input
false to disable retained state. No artifact format or consumer changes.

The opt-in job installs checksum-pinned Buildx v0.37.1 in its private Docker
configuration. It does not replace the runner's client or restart its daemon.
The existing homelab daemon successfully enforced limits in a
[real build-step probe](https://github.com/eduadiez/rollupX/actions/runs/36238870311).

## Validation

Require real producer cold/transported-warm builds, changed shipping source,
changed build-script inputs, removed source and an executed new failing test.
Check corrupt/incompatible/missing state, failure propagation, fresh-runner OCI
transport and ordinary registry-output reuse. Compare total build, transfer and
packaging time against PR #39 under matched workloads/resources. Then pass the
normal CI matrix with the proposed producer; fixture results alone are insufficient.

Local validation: 263 tooling tests completed (four opt-in integrations skipped);
the real Docker producer fixture separately passed cold/export/warm, changed
shipping source, changed build-script input, source deletion and new failing-test
execution. The new failure prevents cache export.
Additional real-Docker checks passed: corrupt layer, future timestamp and escaping
symlink rejection; and `mode=min` executable/archive reuse in an empty, bounded
BuildKit builder without invoking Cargo. These extend the existing opt-in Docker
test (`EEZ_TEST_DOCKER_CARGO_STATE=1` and `EEZ_TEST_DOCKER_FLAT_CACHE=1`, both with
`EEZ_TEST_DOCKER_RUNTIME=1`); they are not new production helpers.

The full-workspace prototype's release archives executed 960 + 233 tests
successfully, with three existing skips. The actual candidate Dockerfile also
restored this identical Rust workload in an empty BuildKit builder, with no
recompiled crates and all seven shipping hashes matching.

### Local comparison

One sample per implementation, same Intel Core Ultra 9 185H host, identical Rust
source/lockfile/release selections, compilation capped at four CPUs and 20 GiB,
and no compiler cache. Baseline: PR #39 at `4502043e`; candidate: `6053d7cf`.
Tools were prepared separately. This measures state restoration, Cargo/archive
execution and output export, **not** network cache transfer or image packaging.

| Measurement | PR #39 (gzip/helper) | Docker-native (OCI, fresh builder) |
| --- | ---: | ---: |
| Restore, build and export | 68.93 s | 54.09 s |
| Reported Cargo/archive phases, summed | 6 s | 6 s |
| Transport bytes | 3,170,683,477 | 2,584,678,956 |
| Recompiled crates | 0 | 0 |
| Shipping hashes versus each seed | All seven match | All seven match |

This supports no observed warm-path regression, not a general speedup guarantee.
Storage/page-cache state and concurrent local work were not controlled; cold
compile times are not used to claim a speedup. Local-only logs are retained under
ignored benchmark artifacts; the CI runs linked from PR #40 provide the retrievable
end-to-end evidence. Temporary benchmark helpers are not part of the proposal.

The isolated homelab cold-seed run is
[36241181032](https://github.com/eduadiez/rollupX/actions/runs/36241181032).
Its first attempt, [36241113812](https://github.com/eduadiez/rollupX/actions/runs/36241113812),
stopped before compilation because the temporary workflow used an unavailable
host `/usr/bin/time`; it now uses Bash's built-in timing. This is not a passing run
or a production-code failure.

The PR's CI evidence must cover genuine cold export, transported warm execution,
ordinary output-cache reuse and the complete validation matrix before merging.
No fixture or local timing alone establishes merge readiness.
