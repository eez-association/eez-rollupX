# Native E2E job duration

| Field | Value |
| --- | --- |
| Date/status | 2026-09-16 UTC; historical timeout confirmed, partitioned CI execution implemented; full candidate CI pending |
| Source | `eduadiez/rollupX`, base `5469c88860408daf776ee812156dd280c1a9e262`, with this change |
| Protocol | `3c0cdf4955941a5c23639346932b55b33babebbd`, unchanged |
| Scope | Reduce native E2E execution cost and complete validation within each job's deadline; preserve coverage, artifact identity and release gating |

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
failures. The initial partitioning change preserved assertions and retries;
the cost follow-up below removes automatic diagnostic retries.

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

## Initial remaining validation

- Run the complete candidate CI and record each partition's wall time on the
  actual runner pool. No fourfold speedup or passing release is claimed locally.
- Diagnose any reproduced assertion failures and the Kurtosis `bridgeL2`
  failure using exact-revision process/network evidence. Historical logs identify
  failures but do not establish their causes.
- Real release-image execution, network semantics, and independent follower
  replay were not run locally for this workflow-only change.

## Follow-up: recovery assertion in embedded shard 4

The [embedded shard 4 run at `e9be01a`](https://github.com/eduadiez/rollupX/actions/runs/35143239173/job/104966547612)
completed in approximately 24 minutes: 11 tests passed and
`gnosis_ordinary_burst_is_budgeted_across_sync_intervals` failed all three
attempts. Its assertion required a `Catchup`-mode Composer invocation after
the injected canonical-reader outage. Diagnostic upload again hit the storage
quota; the job console retained the relevant node log tail.

The first attempt's log shows recovery at 20:43:46 UTC rebuilding the 20:43:38
and 20:43:43 boundaries in `Empty` mode, then preparing the 20:43:48 terminal
in `Steady` mode. This agrees with the
[documented scheduler contract](../../configurable-settlement-policy.md#network-profiles-and-execution):
once catch-up reaches a usable future terminal, it may compose in that same
opportunity. A `Catchup`-mode terminal is therefore not a reliable witness that
historical boundaries were rebuilt.

The test now snapshots the stalled L2 timestamp and current L1 timestamp before
restoring RPC. It requires a subsequent `Empty`-mode invocation for a boundary
strictly newer than the stalled L2 head and no later than that L1 snapshot.
This still requires actual historical recovery; an unrelated empty boundary
or a standalone `Steady` invocation cannot satisfy it. The subsequent checks
for empty boundary bodies, bounded encoded intervals, transaction completion,
settlement timing, independent follower replay and cross-chain progress remain.

Local validation uses the unchanged runtime source revision, fixture-only ports,
and preserved logs/datadirs under `/tmp/rollupx-e2e-investigation`. Cargo rebuilt
the Composer and follower during candidate compilation, changing their executable
hashes; this is semantic validation, not a matched-binary performance comparison.
These are local-only debug-profile checks; the complete CI release-artifact
matrix still needs to run for the fix.

The original Gnosis assertion passed locally both with a cold signer build
(291.70 seconds including compilation) and with prebuilt binaries (186.11
seconds). Its warm-run log contains a late `Catchup` terminal, unlike the CI
failure's `Steady` terminal. The failing baseline evidence is therefore the
three CI attempts, not a claimed local failure-to-pass reproduction.

The corrected Ethereum and Gnosis cases both passed, without retries: two
selected tests, 29 filtered out, 368.29 seconds of execution. Both recovery
traces show two `Empty` boundaries followed by a `Steady` terminal. These runs
also completed the retained on-chain assertions and independent follower replay.
Workspace Clippy with warnings denied, formatting, and `git diff --check` passed.
The integration contracts built successfully with existing Forge lint warnings.

Commands used for the corrected checks (the prebuilt test overrides pointed to
the locally source-built `target/debug` role binaries, avoiding compilation after
the fixture genesis clock starts):

```bash
cargo +1.94.0 test --locked -p eez-node-e2e --test settlement_policy \
  ordinary_burst_is_budgeted_across_sync_intervals -- --test-threads=1
cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
git diff --check
```

## Follow-up: reduce repeated fixture work and failed-test cost

At `f6d6258`, [cross-chain shard 2](https://github.com/eduadiez/rollupX/actions/runs/35152624479/job/105000042359)
failed the short-result forty-five-request single-block assertion on all three
attempts. Its admission diagnostics report `preparation_time`: the 500-ms budget
accepted only 42, 40 and 43 of the 45 requests. This capacity test was also
requiring a particular CPU throughput. Its shared single-wave fixture now starts
preparation 1,500 ms before the deadline, preserving slot cadence, single-block
settlement, gas/blob bounds, nonce ordering, receipts, state checks and independent
follower replay. The separate explicit-preparation mixed-wave tests retain their
500-ms budget.

The shared native fixture now queues protocol CREATE transactions and registration
in signer nonce order before waiting for receipts. It also batches the three L1
proxy creations and overlaps independent L1/L2 setup. Every deployment/proxy
receipt must succeed; predicted CREATE addresses are checked against receipts.
Transaction order within each chain, contract addresses, pinned bytecode and
the final outbound-parent readiness check are retained. This removes repeated
block waits without changing the 5-second L1 or 1-second L2 cadence.

The six Ethereum/Gnosis capacity cases were run serially before and after this
fixture change, using the same frozen Composer, follower and signer executables
from `f6d6258`; their SHA-256 hashes were rechecked after execution. Both runs use
the 1,500-ms capacity fixture and the local optimized debug test profile. All six
passed, covering 400 cross-chain requests plus six warmups and independent replay.

| Local paired sample | Execution time, excluding compilation |
| --- | --- |
| Original fixture | 534.83 seconds |
| Batched fixture | 332.17 seconds |
| Reduction | 202.66 seconds (37.9%) |

This is one local six-case comparison, not a full-suite CI speedup claim. Candidate
setup diagnostics range from 27.2 to 32.8 seconds; the candidate used `--nocapture`
to expose those timings. Logs and datadirs remain under
`/tmp/rollupx-e2e-investigation`, outside the committed evidence.

The CI Nextest profile now runs each case once instead of retrying failures twice.
Since retry-only passes already failed the gate, retries added cost without making
a run releasable. All selected tests still run after a failure; explicit diagnostic
retries still treat flakes as failures. No test is removed or ignored.

[Embedded shard 3](https://github.com/eduadiez/rollupX/actions/runs/35152624479/job/105000042474)
failed before running tests because fetching the pinned protocol hit a TLS handshake
error. Both native jobs now use a small shared checkout helper: at most three
attempts, bounded backoff, the same pinned gitlinks, and a failing job if checkout
still fails. The helper avoids duplicating this error-prone retry block between
jobs; it never substitutes a branch tip or changes the protocol pin.

The complete CI tooling suite selected 242 tests: 238 passed and four opt-in checks
were skipped. The real native relocation fixture was separately enabled and
passed in 21.03 seconds. Besides complete/disjoint partition coverage, it injects
permanent failures and verifies that every selected binary executes, each failed
case executes once, and the consumer returns failure. These are tooling checks,
not real-node performance measurements. Full candidate release-artifact CI remains
required before release; image production and runner allocation are unchanged.

Eleven affected local process tests passed without retries: the six capacity cases,
both explicit 500-ms preparation cases, queue reorg recovery, signer attester
mismatch rejection, and Gnosis ordinary-burst recovery. The two tight-budget cases
and the recovery/reorg cases retained their independent follower replay checks.
Documentation validation resolved 40 local links at the unchanged protocol pin
and syntax-checked all five Bash examples in `AGENTS.md` and this record. Workflow
validation, edited shell syntax, Rust formatting and `git diff --check` passed.
All eight `eez-testkit` unit tests and workspace Clippy with warnings denied passed.
