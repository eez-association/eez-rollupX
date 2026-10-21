# Automatic gas preparation and regression investigation

Status: investigation and implementation authorized on 2026-10-21; matched
performance comparison and all scoped local checks passed.
PR #100 remains a draft. No merge or live deployment is part of this follow-up.

| Input | Identity |
| --- | --- |
| Before immutable preparation | `ec9a22a04bd590ae183c1de80df508dd542443a8` |
| Reported regression revision | `0fc519193b5cd7b5f4e8ef9f42a12bb085053d3c` |
| PR base | `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074` |
| Protocol | Unchanged `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774` |
| Design | [Automatic gas preparation](../../proposals/compact-fee-quotes.md#automatic-gas-preparation-authorized-follow-up-2026-10-21) |

## Investigation

The [earlier measurements](immutable-preparation.md) showed regressions,
especially in automatic nested quotes. Those results remain valid observations
of that experiment; they did not establish which stage became slower.

Temporary, identical instrumentation timed gas discovery and fee simulation in
both revisions. Four serial nested fixtures ran in baseline/current/current/
baseline order, on logical CPUs `1,3,6,8`, using the same frozen harness and
matched development builds. All 320 quotes succeeded. Instrumentation was
removed before the candidate build and is not part of the RPC contract.

This experiment did not reproduce the regression. Automatic, untraced pooled
medians were 12.158/12.230 ms before and 11.389/11.485 ms after immutable
preparation (legacy/EIP-1559). Gas-discovery medians were 2.237/2.273 ms versus
2.108/2.138 ms; fee simulation was 4.204/4.361 ms versus 3.821/3.924 ms.
One baseline round slowed substantially in both stages, even though the gas
discovery implementation was unchanged between revisions. Host variation and
build layout remain possible confounders, not established causes of the earlier
slowdown. The services sharing this host were not stopped or isolated.

The old harness always measured automatic gas before supplied gas, then legacy
before EIP-1559. That ordering confounds variant comparisons with startup and
time-dependent load. The revised common harness interleaves gas modes, formats
and tracing order, with 25 samples per combination instead of ten. Both old
runtimes and the candidate use that same harness; old/new harness results are
not directly pooled.

## Change and invariants

Inspection found unnecessary repeated native L2 prefix construction during gas
discovery. The usual two-probe outbound path opened the prefix ten times: twice
for initial environment/account preparation and four times per probe. It now
opens it once and forks that prepared state. Accessing its immutable environment
also avoids creating a database fork only to read the environment.

Each composition, native setup/user/callback replay and fallback still has its
own disposable execution state. Native setup and its user execute sequentially
on one fork using the existing transaction execution implementation. Source and
system nonces, cumulative gas, pre-execution changes and owned predecessors come
from the same prefix. No probe's writes, consumed nonce or gas reach another
probe. L1-origin estimation also avoids redundant environment preparation, but
still performs its required prefix-hash construction and native delivery checks.

High/selected gas probes, gas-sensitive fallback, nested action/output checks,
fee convergence, exact L1 replay identity, funding and final signed validation
are retained. The pricing loop and policy arithmetic are unchanged. There is no
API, configuration, dependency, protocol, storage or rollback migration.

## Validation and evidence

Local-only evidence is under `artifacts/quote-optimization/auto-gas-regression/`
in the isolated worktree `artifacts/quote-optimization-20261021/source`. It
retains phase samples, source backups, build logs, process logs/datadirs,
runtime hashes and comparison scripts. The first instrumented build failed
because its temporary timer lacked an import; that log is retained. Corrected
diagnostic builds and the uninstrumented candidate compiled successfully.

All builds use Rust 1.95.0, locked dependencies, one Cargo job and the same
development node overrides as the preceding experiment (`opt-level=0`,
`debug=0` for `eez-node`; repository defaults elsewhere). The candidate runtime
was frozen before test-only features were compiled. The build guard stops only
its own process group if available memory falls below 3 GiB. No existing
services, worktrees, wallets or datadirs were changed.

Candidate Composer SHA-256:
`796c692f4d4165ed6389815d05b62194c21f5a51ae881b3b1eab61b28b317fa9`.
Follower and signer match the preceding experiment. This is a local development
binary, not a published release or deployed image.

An initial candidate passed the focused gas-sensitive process test. Review then
retained the inbound deadline check before prefix preparation; the final runtime
was rebuilt and frozen for the complete comparison/suite. Initial artifacts are
retained separately. Three Clippy findings in the new unit test were corrected
without suppressions, and that test and workspace Clippy passed afterwards.

Completed: 694 unit tests (402 Composer, 194 L1, 98 node), including the new
fork isolation/native execution comparison; seven SDK tests. The existing
manual backlog profiler remains ignored. Final workspace Clippy with all targets
and warnings denied, formatting, whitespace and edited documentation path checks
passed. The process suite uses the final frozen candidate, not Cargo's mutable
target directory.

All 25 non-benchmark fee/Safe process cases passed in 1,218.4s, with no failed,
ignored or retried tests. They cover both formats/directions, selected-limit
fallback with changed return data, fee-sensitive effects and output, 64 KiB
calldata, refreshed-price shortfalls, explicit/omitted pending nonces, affordable
and unfunded posters, partial traces, 1%/5% admission below a 20% quote, actual
Safe creation and independent follower replay. Together with the three repeated
quote fixtures, that is all 28 fee/Safe cases on the candidate. The additional
standard wallet-estimation cases exercise both directions on Ethereum/Gnosis
and the post-batch wrapper.
All three passed in 170.4s, without failures, ignored cases or retries. Total:
31 unique native fee/Safe/wallet cases passed against the final candidate.
Frozen runtime and source identities remained unchanged throughout validation.

## Final timing comparison

The common harness SHA-256 is
`27daf2484579b2cff4232848ccd4ad789b8dafe27e0ab6bce2dac6e45e723eb8`.
Each fixture makes 200 successful quotes across legacy/EIP-1559, supplied/automatic
gas and tracing off/on. Runtime order is current/fixed/before/before/fixed/current;
workload order also reverses in the second round. No owned compiler or second
suite runs during measurement. All 18 fixtures passed on their first attempt:
3,600 quotes, 50 samples per runtime/combination. The
[complete summary](automatic-gas-latency-summary.json) retains every combination,
round median, range, gas observation and raw-file/executable digest.

The runtime build used:

```bash
cargo +1.95.0 build --locked --offline -j1 \
  --config profile.dev.package.eez-node.opt-level=0 \
  --config profile.dev.package.eez-node.debug=0 \
  -p eez-node -p eez-follower -p eez-proof-signer --bins
```

For each runtime/round/workload, the driver selected the frozen binaries and
fresh evidence directories. The nested case invocation was:

```bash
EEZ_TEST_PREBUILT=1 \
EEZ_TEST_RUNTIME_BIN_DIR="$RUNTIME" \
EEZ_TEST_PROOF_SIGNER_BIN="$RUNTIME/eez-proof-signer" \
CARGO_MANIFEST_DIR="$PWD/crates/eez-testkit" \
EEZ_TEST_LOG_DIR="$EVIDENCE/logs" \
EEZ_TEST_DATADIR_DIR="$EVIDENCE/datadirs" \
taskset -c 1,3,6,8 "$HARNESS" \
  fee_quotes::replay_validation::repeated_nested_quotes_preserve_priced_execution_and_source_state \
  --exact --test-threads=1
```

`RUNTIME`, `HARNESS` and `EVIDENCE` identify that run's frozen runtime directory,
common harness and unused evidence directory. The Safe cases use
`safe_fee_quotes::repeated_nested_safe_quotes_preserve_priced_execution_and_source_state`
and `safe_fee_quotes::repeated_nested_gnosis_safe_quotes_preserve_priced_execution_and_source_state`.
The driver supplies the disk-backed `TMPDIR` described above. These are native
process tests; the [Kurtosis guide](../../../testing/kurtosis/README.md) owns the
separate topology/DA/PBS procedure.

Automatic gas, tracing off, pooled median milliseconds:

| Workload | Format | Before cache (`ec9a22a0`) | Current PR (`0fc51919`) | Fixed | Gain vs current |
| --- | --- | ---: | ---: | ---: | ---: |
| Nested callbacks | Legacy | 11.473 | 11.545 | 10.653 | 7.7% |
| Nested callbacks | EIP-1559 | 11.514 | 11.562 | 10.985 | 5.0% |
| Nested Safe, Ethereum | Legacy | 11.640 | 11.520 | 10.703 | 7.1% |
| Nested Safe, Ethereum | EIP-1559 | 11.760 | 11.829 | 10.809 | 8.6% |
| Nested Safe, Gnosis | Legacy | 11.730 | 11.459 | 10.551 | 7.9% |
| Nested Safe, Gnosis | EIP-1559 | 11.938 | 11.452 | 10.705 | 6.5% |

Automatic quotes improve against both earlier revisions in **each** round for
both tracing modes: all 48 per-round comparisons. Pooled untraced gains are
5.0–8.6% against the current PR and 4.6–10.3% against the pre-cache revision;
traced gains are 5.3–8.0% against the current PR. All 24 pooled combinations
improve, but supplied-gas results are mixed per round: six comparisons against
the current PR regress by 0.3–1.6%. Supplied-gas execution does not use the changed
L2 discovery path; these data do not establish a generic speedup there.

Selected gas limits and source receipt gas agree across all revisions: 183,149
source gas for callbacks and 130,042 for nested Safe creation. These fixtures
still record 14 runs with automatic gas and eight with supplied gas; validation
executions were not removed. L1 measured gas varies slightly across fresh chain
contexts, as it did previously. Every sample independently reconciles execution
and physical blob costs, service allowances, risk/profit margins, conversion,
rounded effective tip and estimated charged fee. Including the diagnostic
experiment, **3,920 quotes** passed those arithmetic checks.

This establishes a repeatable local improvement for the tested automatic-gas
workloads, not a production latency or throughput guarantee. It does not prove a
single cause for every earlier regression. The robust changes are removing
duplicate prefix work and correcting benchmark ordering; the older observations
remain documented. Release-profile/PBS/Kurtosis and concurrent-load validation
are separate. The Safe fixture uses unmodified Safe 1.4.1 factory/MultiSend
creation with four nested children; this is not a Falcon workload qualification.

## Merge gate

At `0fc51919`, CI run `39627515775` fails the dependency-policy step in job
`119316546645` on RUSTSEC-2026-0381, -0382 and -0383 in the existing certificate
dependencies. No dependency update or advisory suppression is included here.
The follow-up needs its own CI; prior revision results do not validate it.
