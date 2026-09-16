# Native E2E job duration

| Field | Value |
| --- | --- |
| Date/status | 2026-09-16 UTC; historical timeout confirmed, partitioned CI execution implemented; full candidate CI pending |
| Source | `eduadiez/rollupX`, base `5469c88860408daf776ee812156dd280c1a9e262`, with this change |
| Protocol | `3c0cdf4955941a5c23639346932b55b33babebbd`, unchanged |
| Scope | Complete native E2E validation within each job's execution deadline; preserve artifact identity and release gating |

## Observed baseline

The completed [main run 35087350683](https://github.com/eduadiez/rollupX/actions/runs/35087350683)
at `ddf40bc2eb5fac8f7973b112eb383256cd8aac74` contains two native E2E
jobs cancelled at their 90-minute deadlines. Each executes tests serially using
the already-built release test payload and runtime binaries.

| Job | Evidence from retained GitHub job log |
| --- | --- |
| [Embedded E2E](https://github.com/eduadiez/rollupX/actions/runs/35087350683/job/104778464703) | 51 tests selected; cancellation before suite completion. Recorded completed attempts consumed 5,102 seconds, including retries. |
| [Cross-chain E2E](https://github.com/eduadiez/rollupX/actions/runs/35087350683/job/104778464924) | 60 tests selected; reached 57/60 before cancellation. Recorded completed attempts consumed 5,333 seconds, including retries. |
| [Kurtosis E2E](https://github.com/eduadiez/rollupX/actions/runs/35087350683/job/104778464812) | Completed in approximately 35 minutes with 27 scenarios passed and `bridgeL2` failed. This was not a 90-minute timeout. Diagnostic artifact upload also failed because storage quota was exhausted. |

These are historical observations, not a candidate performance benchmark. Full
logs were downloaded locally under `/tmp/rollupx-e2e-investigation`; they are not
committed. Linked GitHub logs remain subject to repository retention.

The native logs also contain failures that extra time alone would not resolve:

- Ethereum/Gnosis ordinary-burst tests did not always observe the expected
  Catchup signal. Ethereum passed only on retry; Gnosis failed all three attempts.
- The fullness overshoot test observed three blobs instead of two, then passed
  on retry.
- Ethereum forty-request and short-result forty-five-request admission tests
  failed the assertion that each wave settles in one L1 block on all attempts.

The configured `flaky-result = "fail"` correctly treats retry-only passes as
failures. This change does not relax those assertions or the retry policy.

## Change and validation

Both native suites now run as four isolated CI jobs using Nextest
[`slice:1/4` through `slice:4/4`](https://www.nexte.st/docs/ci-features/partitioning/).
Each job retains serial execution, verifies the same immutable payload, and
does not rebuild Rust. The matrix runs every partition even after a failure;
the existing final gate requires both complete matrices and every other lane
to succeed before promoting the exact tested image digests.

The job timeout stays at 90 minutes. This addresses cumulative serial execution
inside one job. Total elapsed time still depends on runner capacity and queueing;
eight native jobs plus Kurtosis can become runnable after the producer finishes.
The no-publication fallback remains unpartitioned in the producer job.

Validation commands run from the candidate worktree:

```bash
python3 -m unittest discover -s scripts/ci/tests -v
EEZ_TEST_NATIVE_ARCHIVE=1 CARGO_NET_OFFLINE=true \
  python3 -m unittest discover -s scripts/ci/tests \
  -p test_native_artifacts.py -k NativeRelocationTest -v
actionlint -shellcheck= -pyflakes= .github/workflows/ci.yml
bash -n scripts/ci/run-native-suite.sh
git diff --check
```

The real tiny-workspace fixture uses Rust 1.94.0 and Nextest 0.9.143. It builds
and seals artifacts, removes the producer checkout from its original location,
blocks Cargo/rustc in consumers, and executes both full suites and all eight
partitions. It verifies nonempty, disjoint partition results with exactly the
same union as each full suite, and rejects an empty partition with exit code 4.
The fixture passed in 20.9 seconds; this is tooling validation, not a measurement
of the real node E2E suites. The initial fixture attempt lacked `mold`; the
successful run used the official checksum-verified linker under `/tmp`.

The final full CI tooling run selected 238 tests: 234 passed and four opt-in
checks were skipped. The native relocation check was also run separately with
its opt-in enabled and passed.

Mocked regressions additionally cover partition forwarding, serial execution,
failure propagation, rejection of partitioned unit certification, unique
diagnostic artifact names, and failed/cancelled/skipped matrix results blocking
promotion. Workflow syntax and shell syntax validation passed.

## Remaining validation

- Run the complete candidate CI and record each partition's wall time on the
  actual runner pool. No fourfold speedup or passing release is claimed locally.
- Diagnose any reproduced assertion failures and the Kurtosis `bridgeL2`
  failure using exact-revision process/network evidence. Historical logs identify
  failures but do not establish their causes.
- Real release-image execution, network semantics, and independent follower
  replay were not run locally for this workflow-only change.
