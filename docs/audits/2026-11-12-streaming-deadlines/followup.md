# Measurement driver follow-up

This continues the [initial partial investigation](README.md), preserving its
failed preflight and frozen N14 provenance limitation. The user requested the
real measurement driver, correctness validation and eventual quiet-window
measurements, while keeping PR #165 draft until those requirements are met.
The later preflight found root storage recovered to about 1 TB free. That does
not establish quiet eligibility: competing Cargo, Clippy and Solidity builds
were still observed. No service or competing task was interrupted.

## Driver and boundaries

The new `streaming::deadlines` module of the existing `node_e2e` target loads
and checks the three exact native requests. It restores per-block shared L1
endpoints for the production `RemoteProver`, and obtains the expected validation
identity from independently loaded local configuration. The server's advertised
identity never supplies that expectation. The real signer process runs under
the existing outbound-network-denied launcher, with public fixture configuration
and standard default resource limits. The production client independently
verifies the returned proof; the driver also checks it against the fixture's
input-derived digest and attester.

The existing test proxy supplies bounded observation: at most 4,096 events and
4 MiB of serialized events. Its upstream response decode limit is 64 KiB when
observation is enabled. Observation failures reject rather than truncate. The
proxy adds a loopback hop, serialization and clock-reading overhead to the
experiment. None of these results will be presented as direct production
network latency. Mutation/capture users remain unobserved by default.

The four cases are deliberately distinct:

| Case | Preparation | Recovery and expected Block transmissions |
| --- | --- | --- |
| Cold | None | N blocks on one session |
| Prepared | All N blocks | Resume, Rewind, terminal resubmission: N+1 blocks in total |
| Disconnected | Blocks 1..15, then the prevalidation transport ends | Resume at the matching cursor, send the remaining suffix: N blocks in total |
| Lost session | Blocks 1..15 | Restart only the owned fixture signer, observe NOT_FOUND, Begin/full backfill: N+15 blocks in total |

Full preparation replaces the prospective first-15-block "prepared" case from
the initial plan so that it differs from the explicit partial disconnect case.
The workload bytes themselves are unchanged. Lost-session recovery includes
all discarded preparation and L1 header charges. The driver asserts actual
header counts/bytes, Begin/Resume/Rewind counts and block totals, ACK roots,
independently expected proof digest, one signature and Proof/EOF completion.
Preparation must produce zero signatures.

Preparation timing covers real `RemoteProver::prevalidate_range`, including
its serialization, streaming and signer execution. This driver does not run the
Composer's watermark coalescer or capture new witnesses from a running chain.
Fixture file loading/context/identity construction, signer startup/readiness,
capability negotiation, prevalidation, owned restart/readiness, and complete
`prove`/response verification/EOF have separate monotonic measurements. The
complete-attempt interval begins at capability negotiation; fixture loading
and initial signer readiness are reported separately and must not be silently
included or excluded when comparing cases. Proxy request/response timestamps
also retain queue-inclusive exchange intervals; they do not isolate EVM CPU
execution. Only local test signer processes are restarted.

The captured L1 targets are static test evidence, not current submission
opportunities. No proof is submitted to L1. Logical cutoff, cancellation and
worker ownership assertions belong to the existing controlled package tests;
these workload durations are not an L1 inclusion guarantee or a new deadline
policy. The captures exercise V0 historical reconstruction under the current
version-9 reader, not production V1 workload capacity.

## Running it

Build and run correctness checks first, with owned target, log and datadir
paths. Use Rust 1.95.0, LLVM 22 and at most two build jobs (one under competing
work), locked dependencies and the standard shipping feature selection.
Clear source-test overrides as required by the repository instructions. The
ordinary correctness test executes all twelve fixture/scenario combinations:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  --exact streaming::deadlines::native_workload_preparation_and_recovery_are_checked \
  --test-threads=1 --nocapture
```

The explicit measurement utility is ignored during normal test runs. It
requires `EEZ_TEST_PREBUILT=1` and an exact verified local signer path, so it
cannot trigger a source build while sampling. Use the runner only after the
release binaries and correctness prerequisites are ready. Its arguments are
an existing release `node_e2e` executable, release signer executable, and a new
owned output directory. For example, after resolving the actual test hash:

```bash
python3 scripts/measure-streaming-deadlines.py \
  "$EEZ_MEASUREMENT_TEST_BINARY" \
  "$EEZ_MEASUREMENT_SIGNER_BINARY" \
  "$PWD/artifacts/issue148/quiet-window-01"
```

The runner requires committed, clean source and records source/protocol,
Cargo.lock and binary hashes before observing host quiet. It uses the initial
record's 120-second gate with full resource snapshots at 10-second intervals
and additional process probes every second, rejects hidden host process
visibility, and checks compiler/test/IO activity, CPU/load, disk, memory, swap
traffic and PSI. It also retains `/tmp` and `/dev/shm` capacity. Normal production
background services remain running. A process-name screen cannot establish
host isolation; CPU and pressure observations remain necessary.

The runner waits up to 30 minutes for admission (explicitly adjustable with
`--wait-seconds`). Any contamination during sampling terminates only its newly
created process group, retains the diagnostic evidence, and requires a new run.
Short-lived compiler/test work observed between full snapshots also invalidates
the interval. Even an unexpected build in the owned process group is rejected.
It never selects a cleanup target by command name. Post-observation continues
for another 120 seconds. A resource/observer failure cannot authorize a passing
performance report.

For each of twelve cases, the driver records three warmups and twenty measured
attempts, with a fifteen-minute group budget. An attempt-start record precedes
every attempt. Failures/timeouts are retained and stop the group; they cannot
be silently retried or omitted. The runner separates warmups, successes and
failures and only marks performance evidence eligible when all twelve groups
have twenty successful measured samples and the before/during/after gate holds.
Its distributions use the ordinary sample median, nearest-rank p90/p95, min/max
and nearest-rank interquartile range. Raw events and per-attempt records remain
available in the owned evidence directory. No SLA or general performance claim
is implied.

## Validation status

Implementation and validation are ongoing. Nine focused Python tests currently
pass, including actual cleanup of one owned process group while a separate
owned control group remains alive, resource-boundary rejection, host visibility,
compiler classification, short-lived build retention, published native-suite
selection and percentile/failure accounting. Rust/native builds
are running in the worktree's own target directory with one job. No new runtime
pass, independent replay result or eligible latency distribution is claimed yet.
The final run record will distinguish completed checks, first failures, exclusions,
actual measurements and any remaining quiet-window blocker.

## Retained setup failure and controller correction

The four affected debug package executables passed 406 tests (control RPC 6,
signer 341, client 38, testkit 21); five signer artifact-generation utilities
remain explicit exclusions. Their exact executable/log hashes are retained in
`artifacts/issue148/unit-results.json` locally. The signer run includes the 29
frozen native session transcripts. These are correctness results, not workload
timing. Integration contracts also built successfully with one Foundry thread;
inherited contract lint warnings remain in the log.

The first native correctness invocation selected one test but failed during its
source-runtime build, before starting a signer or reaching any workload assertion.
Direct binary execution supplied Cargo 1.95 without propagating Rustup's toolchain
selection: the `ff` dependency's own `rust-toolchain.toml` selected Rust 1.56,
which rejected `--check-cfg`. The first log/result are retained separately as
`native-correctness.log` / `native-correctness-result.json`. The corrected run
explicitly pins `RUSTUP_TOOLCHAIN`, `RUSTC`, `RUSTDOC` and Cargo to 1.95.0.
This setup failure does not reproduce a deadline defect. The documented
`cargo +1.95.0 test` procedure propagates the required toolchain normally.

Review also found that the initial controller could reap its harness before a
SIGTERM-ignoring descendant exited. A controlled regression fails against the
helper at `2ac63c71` and passes after cleanup also kills remaining members of
that newly created process group. A separate control group remains alive. This
is a correction to the new measurement tool, not an existing signer/runtime
defect or a flaky runtime pass. Linux's 15-byte process-name truncation is also
covered: underscore-named Rust test executables are screened even when their
hash suffix is truncated. No production process was selected or stopped.

The full default CI tooling suite selected 557 tests: 548 passed and nine were
skipped. The two scanner-dependent tests subsequently passed using verified
Gitleaks 8.30.1, leaving seven explicit opt-in Docker/Kurtosis/build probes
excluded. Dependency policy checks passed; `cargo audit` exited zero with twenty
allowed warnings. The final frozen-source inventory and actual timing status
will supersede this interim status without deleting these first observations.
