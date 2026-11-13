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
verifies the returned proof by recomputing its digest. The driver additionally
checks the expected digest and attester stored in the capture's `response.json`;
it does not itself recompute that frozen expectation. The captured request must
separately match its recorded Keccak before decoding.

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
traffic and memory PSI (CPU and IO PSI are recorded, not gated). It also retains `/tmp` and `/dev/shm` capacity. Normal production
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
attempts, with a fifteen-minute group budget and a per-attempt cap of the
smaller of three minutes or the remaining group budget. These utility safety
bounds are separate from the production client's ten-minute exchange timeout
and signer's ten-minute ordered-operation limit. A utility-budget failure
cannot be classified as a signer deadline violation. An attempt-start record precedes
every attempt. Failures/timeouts are retained and abort the entire utility; they cannot
be silently retried or omitted. The runner separates warmups, successes and
failures and only marks performance evidence eligible when all twelve groups
have twenty successful measured samples and the before/during/after gate holds.
Its distributions use the ordinary sample median, nearest-rank p90/p95, min/max
and nearest-rank interquartile range. Raw events and per-attempt records remain
available in the owned evidence directory. No SLA or general performance claim
is implied.

Each attempt starts a fresh owned signer process. “Cold” means no retained
prevalidation/session state; OS page caches are not flushed. The fixed order is
bidirectional, lookup, changing-static; within each capture it is cold, prepared,
disconnected, lost-session. Warmups are the first three attempts of each group,
not discarded failures. Samples run sequentially; there is no load generator or
concurrent client workload. Readiness uses the existing 100 ms polling interval,
so startup figures include that granularity. The complete-attempt timer ends
after the production client's proof verification and EOF. The check that preparation
has not signed reads the current signer log inside this timer, before any
lost-session restart. The check runs in all four scenarios. It is included in
the complete-attempt figure but none of the individual stage figures, so those
stages do not sum to the complete figure. Later independent fixture assertions,
trace extraction, the final
signer-log read and process teardown are outside that timer. These choices
constrain comparisons and tail estimates.

## Validation status

Release/archive preparation and native correctness passed. The later
[completed hosted campaign](timing-results.md) supplies actual eligible workload
timings. The initial blocked admission and earlier validation below remain
historical evidence, with first failures preserved in the
[local validation record](followup-validation.json). Final evidence review and
exact-final-head CI still gate the authorized merge; #148 remains open.

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
The corrected invocation passed all twelve fixture/scenario combinations;
its executable and runtime identities are recorded below. Its 2,270-second
command duration includes the 37-minute shipping-entrypoint source build and
is not workload timing. This setup failure does not reproduce a deadline defect.
The documented
`cargo +1.95.0 test` procedure propagates the required toolchain normally.

Review also found that the initial controller could reap its harness before a
SIGTERM-ignoring descendant exited. A controlled regression fails against the
helper at `2ac63c71` and passes after cleanup also kills remaining members of
that newly created process group. A separate control group remains alive. This
is a correction to the new measurement tool, not an existing signer/runtime
defect or a flaky runtime pass. Linux's 15-byte process-name truncation is also
covered: underscore-named Rust test executables are screened even when their
hash suffix is truncated. No production process was selected or stopped.

The first full default CI tooling suite selected 557 tests: 548 passed and
nine were skipped. The two scanner-dependent tests subsequently passed using
verified Gitleaks 8.30.1. After the additional cleanup regression, the final
suite selected 558 tests: 551 passed and seven explicit opt-in
Docker/Kurtosis/build probes were excluded. Dependency policy checks passed; `cargo audit` exited zero with twenty
allowed warnings. The final frozen-source inventory and actual timing status
will supersede this interim status without deleting these first observations.

## Earlier native correctness identity (historical)

The ordinary correctness test selected one non-ignored test containing all
twelve combinations and passed. Its source was `2ac63c71870066dc4225da2a2bbaca73c8d7964d`;
later changes through `117ad079` affect only the explicit measurement utility's
group/attempt budget and the Python observer. The production runtime source is
unchanged from the requested `601ebfa9` baseline. No supported runtime defect
has been established. This test exercises static captured workloads through
fresh real processes; it is not a fresh on-chain capture or follower replay.

| Executable | SHA-256 |
| --- | --- |
| Debug `node_e2e-9c9a287727624945` | `dd16d5b55415a5fadcd880640b0dd7b20a623f2005b5d02be53b4ef0067463fc` |
| Debug `eez-composer` | `d24e60e13835bef2547065e0759d231773a987556daffde8087fcac8c08bd662` |
| Debug `eez-follower` | `979c2b780093e36aaa4df039daa400d5a227c8f061fdcd73d7ae3a7a10d21f60` |
| Debug `eez-proof-signer` | `57b051596b92f82c2787e3d1b629053213538b6e1f7deb61ee503a0aa3fb6723` |

The first failure and corrected result remain separate local records. The
prebuilt overrides were unset; all three runtime entrypoints were built by
the existing source harness with default features, locked dependencies, the
workspace dev profile, LLVM 22, incremental compilation disabled and one job.
Rust 1.95.0 (`59807616e1fa2540724bfbac14d7976d7e4a3860`) and Cargo 1.95.0 were
forced for every nested build. The protocol remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`.

Review reproduced another observer failure against `117ad079`: interruption
while writing an attempt could leave a partial JSON line and crash report
generation. The runner now preserves the raw file and emits an explicit
ineligible diagnostic summary. A mocked end-to-end observer regression fails
on the previous helper and passes with this correction. It does not run a
workload or supply timing evidence.

The first real host-observer preflight at `311ac5d8` also failed while parsing
`ps` output: kernel workers such as `UVM deferred release queue` have spaces
in their process names. Splitting only the PID/parent columns fixes this. The
previous parser fails the new regression; the corrected parser passes and
records the host's competing compiler activity and load, rejecting admission.
The failure and process-name shape are retained locally. This is an observer
correction, not a runtime deadline reproduction. An optional supplemental
cgroup-v2 probe found those paths unavailable; that is recorded as unavailable
rather than as a resource bound. No workload was timed in either probe.

The observer now cleans its newly created process group once, immediately
before post-observation. Previously, a contaminated run cleaned it twice,
including another signal after the 120-second observation interval. That delayed
reuse of a numerical process-group ID is unnecessary. The regression covers
both normal exit and contamination and fails against `61f73a10`; all eleven
focused tests pass after the change. No other task or production process was
signalled by this test.

A final observer regression also fixes the transition to post-observation:
the first post interval now starts at the final during snapshot, rather than
reusing the preceding snapshot and accidentally averaging two intervals. Both
normal-exit and contamination paths fail this assertion against `ed74c9eb` and
pass after the correction. This preserves the documented ten-second resource
interval instead of smoothing the first post interval over twenty seconds.

The process screen also covers every published standalone native vector and
blob-verifier test executable, including Linux-truncated names. The inventory
regression fails against `88b1b505`, where those tests could evade the name
screen, and passes after the correction. Process probes remain a practical
screen on this shared host, with CPU, memory, swap and disk checks retained.


## Merge-review corrections

Claude Fable 5.2 reviewed frozen functional head `ca76bb87` with only Read,
Grep and Glob, with no model fallback or delegation. It confirmed the Rust
workload driver and proxy by source inspection, and identified two corrections:
the quiet period must begin at the first clean snapshot after a rejection,
and the timing-method text must include the preparation-only signer-log read.
The coordinator corrected both before any claimed timing distribution. A
controlled good/bad/good host sequence and a failed first snapshot both launch
too early at the old head; the corrected runner waits the full 120 seconds
from the next clean observation. The earlier runtime/release build remains
frozen at `ca76bb87`; these observer, documentation and scanner changes do not
alter Rust, Cargo, workloads or protocol source. The final sections below record release identities and the actual failed
quiet-window admission; workload measurements remain pending.

The failed lint job scanned merge `8941f886` (parents `601ebfa9` and
`ca76bb87`). Its six findings are the same three public workload verification
commitments in original commit `d9b974c1` and the CI merge diff. The scanner
correction reuses only the exact-value AND path exceptions already reviewed in
PR #163, plus its rejection controls and PR #164's neighboring-path tests. No
commitment value beyond PR #163's three reviewed values, broad path exclusion
or suppression bypass is introduced. The
coordinator retains the redacted failed report, both test results and current
all-ref scan evidence in its task scratch files; the failed CI run remains
available. Original frozen review-input provenance is still not independently
checked. Fresh exact-head CI is required after this update.

The bounded Fable verification passes the quiet-window correction, its two
regression subcases and the timing-method correction. It did not execute tests.
The observer suite passes twelve tests; removing the reset after a bad snapshot
makes the new sequence test fail. Both exact-scanner regression tests pass.
Optional review caveats remain: SIGTERM/SIGHUP can bypass Python cleanup, and
release profile/source binding is operator-asserted rather than established by
the directory name. Retain each binary's actual build source and hashes beside
the later observer source identity; folder naming alone is not provenance.


## Frozen release validation and actual measurement outcome

This is a continuation of frozen N14, not a fresh audit. The association baseline
is `601ebfa9c7e1a779f1afbd077bd31a3207e7a61b`, with protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. Original review-input provenance
remains independently unverified. No new production deadline defect has been
established; #43/#45's old preparation-ingestion defect remains fixed.

### Executable and observer identities

The release binaries were built at `ca76bb8744667f3045f5d839a920c011825999f9`.
The corrected observer is from `04bbb115901c88b1ccdbb6f15ab4cfd89662c5d9`.
Measurement checkout identity is `d99e58929d11d0d1a51fb7e690d7f48e66cd142e`.
These identify different things; the binaries were not rebuilt at either later
commit. Source equivalence was checked before the safe fast-forward. The
correction changes exactly five observer/scanner/documentation files. Its merge
with reviewed main preserves those five files byte-for-byte and adds accepted
main tests, harness changes and a runtime comment. The whole crates tree is not
identical to ca76bb87. Relevant signer/client/RPC/testkit/node-e2e/prover/protocol/
L1 source trees, Cargo inputs, shipping Dockerfile and native archive tooling are
identical. The separate ingress change is comment-only.

| Release executable | SHA-256 |
| --- | --- |
| `node_e2e-0e56ad39fba51c37` | `85eff313c0006db93923b4dc01c4445e0302babf9c03e5c705bffa3bb2b36753` |
| `eez-proof-signer` | `ebaf852b59ee626fff8b580941ddca7efd53e053e04c50c658acb472907bca1f` |
| `eez-composer` | `982c83bdc1af31f012a93c9e932d996c28ccb3b67dbf938684467e554252c910` |
| `eez-follower` | `e2c58f6276c142bc310ca1206d565c1613c1b183cab11f1014d2389a001fbd51` |

The sealed local manifest SHA-256 is
`43ab119273f6bd696338ca9445e265c83acafd82b376f8533e9957e9135d78b4`.
The archive records ca76bb87, protocol 84a5b520, Rust 1.95.0, release profile,
default shipping features and `RUSTFLAGS=-C link-arg=-fuse-ld=mold`.
Release settings are opt-level 3, thin LTO, 16 codegen units, stripped symbols
and unwind panic handling. One Cargo job and one Mold linker thread were used.
Mold 2.40.4 and dependencies came from verified distribution packages extracted
only to owned temporary paths. No system package, Cargo cache or dependency
version was changed. The manifest is a local validation record, not a published
CI artifact. The Python runner's profile string is operator-asserted; the sealed
manifest, source comparison and manually rechecked executable hashes supply
the provenance binding for this run.

### Completed correctness and build checks

All preparation finished before starting the host gate. The standard shipping
release build passed. The existing guarded native archive invocation compiled
only workspace crates, archived ten binaries and rechecked frozen runtime hashes
unchanged. All 70 producer tests passed with zero skips. The ordinary release
correctness selection passed one test containing all twelve capture/scenario
combinations, with 212 unrelated tests filtered. Build and correctness command
durations are not workload latency evidence.

The final affected debug executables passed 406 tests: RPC 6, signer 341,
client 38 and testkit 21. Five explicit signer artifact-generation utilities
were ignored. The signer selection includes 29 frozen native-session transcript
passes; those fresh executions are correctness checks, not fresh workload
timing. Composer's 22 logical cutoff/retry checks passed with 433 tests filtered;
virtual-clock/model assertions remain separate from wall-clock measurements.

Five serial source-based process checks passed on ca76bb87: minimal bidirectional
settlement, outbound static round-trip with independent follower replay, tampered
post-batch calldata rejection, tampered witness rejection, and the twelve-case
workload preparation/recovery correctness check. Prebuilt source overrides were
unset. The source harness built the production entrypoints with Rust 1.95.0;
their bytes remained equal to the prior debug correctness run.

Workspace format, all-target Clippy with warnings denied, signer all-target
checking and Rustdoc with warnings denied passed. The complete default tooling
run selected 560 tests: 553 passed and seven opt-in Docker/Kurtosis/build probes
were excluded. Subsequently changed observer tests pass all twelve cases, and
both scanner regressions pass. Integration contracts built; inherited lint
warnings remain in the log. Dependency policy passed; Cargo audit exited zero
with twenty allowed warnings. CI owns the complete release/Kurtosis matrix. These are
mixed-head checks: the [per-check source fields](followup-validation.json) identify
Composer at `88b1b505`, full tooling at `61f73a10`, and Clippy/check/Rustdoc at
`d5f313c0` plus the exact correction later committed as `ca76bb87`. Final affected
unit/process/release executions are at `ca76bb87`; corrected observer checks are
at `d99e5892`. Earlier dependency checks have no separately recorded source head;
the subsequently successful `d99e5892` Lint lane supplies exact-head CI coverage.
Unchanged valid checks are reused; this prose update triggers fresh CI.


### First failures and review corrections

First observations remain retained: the original sandbox mount quota failure
before command execution; resource-starved preflight; nested source build selecting
Rust 1.56 without propagated Rustup configuration; 32 sandbox socket-creation
failures in the complete tooling run; the new closure's strict Clippy failure;
the orchestration selector treating the signer's bin target as a lib; and the
owned linker's initially missing libraries. Corrected runs are separately
identified. These setup/tooling failures are not signer deadline reproductions.

Observer regression evidence retains the previous implementations' failures for
descendant cleanup, partial JSON, process names with spaces, truncated Rust/native
test names, single cleanup and adjacent post intervals. Fable 5.2's required
first-clean baseline correction and timing-method correction were applied before
sampling. Both initial-failed and good/bad/good boundary subcases fail at old
source; deleting the reset fails the new control. All twelve current observer
tests pass. These are controlled observer assertions, not workload measurements.

The six history-scanner findings in failed run 43289481559/job 131434460726 were
independently classified as three public workload verification commitments twice,
in d9b974c1 and exact CI merge 8941f886. The correction reuses already-reviewed
exact-value AND path exceptions and retains neighboring-path rejection controls.
No broad exclusion, detection weakening or valid fixture edit was made. Parent
full-history scan: zero findings across 1,159 commits/743.07 MB; final patch and
integration-commit scans: zero. Original failure and redacted classifications
remain retained. Fresh CI is required after the final evidence commit.

### Method limits to preserve in any final report

Inputs are captured fixture witnesses reserialized through the real production
client, not newly captured Composer preparation or live chain posting. Captures
cover small V0 historical workloads under the current version-9 reader, not
production V1 capacity. Expected identity is independent of the server reply
but uses the same shared identity library with local configuration; it is not
an independent frozen identity algorithm oracle. Finalize equivalence is by
expected digest, not bit-for-bit equality with the old capture.

The disconnected case is normal prevalidation EOF after fifteen ACKs, not
midflight loss or lost ACK. The lost-session case restarts the owned signer
behind a stable proxy, not a client-channel reconnect or capability re-agreement.
Block/header charges are observed client transmissions, not independently
asserted signer accepted-charge/quota counters; client bytes are recorded.
Existing separate tests cover permits, worker ownership, cancellation and logical
deadline boundaries. The workload driver does not replace those assertions.

Each repetition starts a fresh signer. OS page caches are not flushed. Fixed
ordering, loopback/proxy observation overhead, shared normal production services
and 100 ms readiness polling constrain interpretation. The pre-proof
preparation-only signer-log check runs inside complete_attempt_ms in all four
cases, and outside the individual stage intervals. Later independent fixture
assertions, trace extraction, final log read and teardown are outside the complete
timer. Stages therefore need not sum to the complete interval.

SIGTERM/SIGHUP can bypass Python cleanup; a process-observer subprocess exception
can omit the summary after child cleanup. These optional limitations do not
authorize eligible evidence. No host isolation, SLA, general tail guarantee,
saturation capacity, production deployment or durable queue recovery is claimed.

### Initial timing status (retained historical observation)

The corrected observer completed its configured admission wait on 2026-11-12,
22:03:12–22:33:14 UTC (1,802.19 observed seconds). It retained 178 full
before snapshots plus one-second competing-process probes. CPU idle was below
70% in 89 intervals; swap traffic occurred in 50 intervals (overlap possible).
The longest clean sequence was 30.55 seconds, below the required 120 seconds.
No compiler/test/IO process, storage, available-memory, memory-PSI or load rejection
was reported. Normal production services remained running on this shared host.

CPU idle ranged from 67.70% to 72.39% (median 69.996%, just under 70%); load1 ranged from
7.06 to 9.67 on 22 available CPUs. Available memory remained 33.53–33.92 GiB,
with about 7.99 GiB swap in use and 1,336 swap-in pages during observation.
Minimum free space was about 1,075.38 GiB on root/evidence, 15.51 GiB on
`/tmp` and 25.99 GiB on `/dev/shm`. These are resource diagnostics, not
workload timing distributions. Exact counters and hashes are retained in the
[validation record](followup-validation.json).

The observer exited 1 with `quiet window unavailable`, `complete=false` and
`eligible_performance_evidence=false`. It launched no workload: **zero warmups,
zero attempts, zero eligible samples**. Consequently no during/after workload
phases or latency distributions exist. There was no contaminated workload to
count or discard. The empty driver log, manifest, provenance supplement,
resource snapshots and summary remain locally retained with committed hashes.

At this initial phase, actual preparation/streaming/recovery measurements were
pending. Later retries and the unchanged-criteria hosted path produced the
[completed result](timing-results.md). Correctness durations and transcript
passes were never substituted, criteria were not relaxed, and normal services
were not stopped. No optimization/runtime fix or baseline/candidate speedup is
claimed. The final result retains the earlier admission failures and limits;
#148 remains open. The authorized merge still requires final evidence review,
all thirteen fresh exact-head normal CI checks and final-main compatibility.
Prerequisite #164 subsequently merged as c30a7fff.

### Bounded evidence review

Claude Fable 5.2 (`claude-fable-5-2`, no fallback or delegation) approved
publication of this partial evidence after eleven read-only file reads. It ran
no commands, tests, scans or hashing and did not independently verify Git facts,
raw host counters or retained test logs. It found no blocking factual/method
error. Six non-blocking wording comments were corrected: memory-only PSI gating,
mixed validation heads, the below-threshold rounded CPU median, whole-utility
abort behavior, the fixture-supplied expected digest, and reuse of already-reviewed
scanner values. The initial sandbox DNS failure reached no model turn; the
approved-mechanism retry completed. Both first failure and review are retained
locally with hashes in the validation record.

That review approved only the partial record at its recorded phase; no
ready/merge action was taken then. The later completed timing record and its
final bounded Fable review supersede the timing gap without converting the old
review into approval of data it never read. Final exact-head CI/merge gates
remain independently required.
