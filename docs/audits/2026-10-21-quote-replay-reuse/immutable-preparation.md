# Immutable L1 preparation within a fee quote

Status: implementation authorized on 2026-10-21; local correctness checks and
both timing experiments completed. No general latency speedup is established.

| Input | Identity |
| --- | --- |
| Baseline | `ec9a22a04bd590ae183c1de80df508dd542443a8`, draft PR #100 |
| PR base | `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074` |
| Protocol | Unchanged `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774` |
| Design | [Immutable preparation reuse](../../proposals/compact-fee-quotes.md#immutable-preparation-reuse-authorized-follow-up-2026-10-21) |

## Change and boundaries

The outbound settlement estimator reuses registry/deployment validation,
verification-key lookup and poster-account reads within one format's pricing
loop at one pinned L1 state and target. Preparation is lazy, successful-only
and bound to the rollup and expected settled root. Different roots/rollups
repeat lookup/validation; a failed lookup cannot supply a cached success.
Different estimators, requests, formats and snapshots start fresh.

Every iteration still rebuilds posting/proof inputs and callback expectations,
reserves fees for its current blob count and computes affordable gas. Source
execution, native validation, exact L1 replay identity, receipt pricing and
signed admission/final posting remain unchanged. This removes untraced setup
work, so recorded execution-run counts need not fall further. There is no API,
configuration, storage, signer, protocol or live-deployment change.

## Validation and evidence

The runtime comparison rebuilds both revisions with identical development
settings and one common harness. Nested-callback quotes and real Safe 1.4.1 creation through
MultiSend (one outer Safe plus four initialized children) exercise both formats,
automatic/supplied gas and tracing on/off. The Safe workload runs on Ethereum
and Gnosis profiles. Timings do not replace the separate fee/receipt/replay
regressions or establish production throughput.

Local-only raw evidence is under
`artifacts/quote-optimization/immutable-preparation/` in the isolated worktree
`artifacts/quote-optimization-20261021/source`. It includes the verified baseline
hashes, source diff, toolchain/environment, build logs and retained process
logs/datadirs. The shared host's sandbox cannot start because of its `/tmp`
mount quota; authorized host execution uses a disk-backed `TMPDIR`. The
single-job build terminates only its own process group if available memory
falls below 3 GiB. Existing services and other worktrees are preserved.

The first normal development build stopped at that threshold while compiling
the node package. Its log and the stop reason are retained. Both comparison
runtimes were then built with `profile.dev.package.eez-node.opt-level=0` and
`profile.dev.package.eez-node.debug=0`; other packages retain the repository's
development settings. Baseline build: 200s; candidate: 105s. These are compile
durations, not quote timings. No dependency or repository profile was changed.

For the baseline, only this task's production file (`fees/settlement.rs`) was
temporarily restored from `ec9a22a0`, with a byte-for-byte candidate backup and
guarded restoration. A source diff confirmed the runtime crates/manifests matched
the baseline before building. The candidate was restored before its build;
each runtime was copied and hashed before compiling test-feature dependencies.

| Executable | Baseline SHA-256 | Candidate SHA-256 |
| --- | --- | --- |
| Composer | `bdb6eae4c011ebce756bd48a4d9cbeca30bf0f51a7edd29cef45ed934b5e11ab` | `7296f43a350c3bcf3571a78c578ecf50e9b31150485700a4c1c8d0206472dadc` |
| Follower | `675dbd32ff98a771be024ad3802c08b6e2b9a4f7317acadb7ab918ed5e6dbeb0` | Same |
| Signer | `69d885e0e149d8859158fa78f0fb2c5a04af68b7ef2eab26c7d3ce490f16a169` | Same |

The common harness SHA-256 is
`f300ccbf0a87340fd8bb12399947f9e12510c3152c8f84c11a92c2cae1b5c6d6`.
Its source includes both new Safe benchmarks and the shared matrix helper.
Every timed RPC requests one format; default dual-format correctness is covered
separately by the native compact/tracing cases.

Completed: both matched-profile runtime builds; 693 Composer/L1/node unit tests
(401 + 194 + 98, one existing manual profiler ignored); workspace Clippy with
all targets and warnings denied; SDK (7 tests); Rust formatting, whitespace and
documentation path checks. The shared harness selects 28 fee/Safe cases.
All 28 passed on the candidate: the three repeated-quote workloads above, plus
25 other native regressions in 1,215.3s, with zero failures or ignored cases.
Those include fee-sensitive changed payloads, final effect/output rejection,
underpayment, price changes, 1%/5% accepted margins, both directions/formats,
explicit/omitted pending nonces, 64 KiB calldata, funding and partial traces,
ordinary L2 behavior, real Safe creation and independent follower replay.
The signed Safe receipt/replay cases use EIP-1559; repeated unsigned Safe quotes
exercise both formats. Frozen runtime hashes remained unchanged after all runs.

## Initial unrestricted-CPU measurements

Baseline, candidate, candidate, baseline each ran the three benchmark fixtures
serially with no concurrent owned compiler or second test suite. All twelve
process runs passed. Each fixture recorded 80 quotes: ten per format/gas/tracing
combination. All 960 samples passed an independent calculation of execution and
physical blob costs, service allowances, margins, currency conversion, rounded
tips and estimated charged fees.

These timings do **not** establish a consistent speedup: pooled medians improved
in 13 of 24 combinations, but only three improved in both rounds. Eleven pooled
medians regressed; several nested combinations regressed in both rounds. Retain
these results rather than selecting only favorable cases. Source gas remained
183,149 for nested callbacks and 130,042 for nested Safe creation. L1 gas varied
by 12 across some independent fixtures; their L1 contexts differ and each fee
calculation was verified against its own measured gas/prices.

The host allows migration between performance, efficiency and lower-frequency
cores (reported maximum frequencies up to 5.1, 3.8 and 2.5 GHz). CPU placement is
a possible confounder, not an established explanation for every difference. A
second counterbalanced experiment restricted to the same performance cores ran
after the separate native settlement regressions finished. Other host services
remained running; this is not an isolated production benchmark.

## Controlled comparison and interpretation

The second experiment set affinity to logical CPUs `1,3,6,8`, four distinct
performance cores, for each test and its child processes. Both runtimes used
the same affinity and harness. Runtime order was candidate, baseline, baseline,
candidate; no owned compiler or second test suite ran alongside it. All twelve
process runs and another 960 fee-arithmetic checks passed. Total: 1,920 checked
quotes across both experiments. The
[summary](immutable-latency-summary.json) retains every combination, per-round
medians, ranges, sample counts, gas observations, raw-file digests and executable
identities. Do not pool timings across the two affinity settings.

Automatic gas, tracing off, pooled medians in the controlled experiment:

| Workload | Format | Baseline (ms) | Candidate (ms) |
| --- | --- | ---: | ---: |
| Nested callbacks | Legacy | 12.215 | 12.512 |
| Nested callbacks | EIP-1559 | 11.656 | 12.154 |
| Nested Safe, Ethereum | Legacy | 11.467 | 11.500 |
| Nested Safe, Ethereum | EIP-1559 | 11.388 | 11.497 |
| Nested Safe, Gnosis | Legacy | 12.482 | 12.400 |
| Nested Safe, Gnosis | EIP-1559 | 12.082 | 12.186 |

There are 20 samples per runtime per cell. Controlled pooled medians improved
in 11 of 24 combinations; only four improved in both rounds. Supplied-gas,
untraced nested callbacks improved by 1.0%/2.9% (legacy/EIP-1559), and the
equivalent Ethereum Safe cases by 4.8%/2.7%. These gains are not universal:
automatic-gas nested cases regressed in both rounds, and Gnosis timings varied
substantially (including a 16.4% pooled regression for traced automatic legacy
quotes). CPU affinity did not remove all variability or establish an overall win.

The established improvement is less duplicate work: a successful preparation
avoids the two getter EVM calls, deployment fingerprints and account read on
subsequent iterations at the same rollup/root/context. The native execution
trace still records 14 runs with automatic gas and eight with supplied gas in
these fixtures. Source gas remained 183,149/130,042; L1 gas across the controlled
fixtures was 276,368–276,380 for callbacks and 1,552,588–1,552,612 for Safe.
Each payment, including rounded effective tip, matches its own gas/prices and
policy. This cleanup must not be advertised as a production latency improvement
without a representative release-profile benchmark.

## Reproduction and limits

Use the build/test commands and frozen-runtime environment in the
[preceding record](settlement-reuse.md#reproduction-commands), adding
the same `profile.dev.package.eez-node.opt-level=0` and
`profile.dev.package.eez-node.debug=0` overrides to **both runtime builds**.
The shared harness's benchmark selection is:

```bash
"$HARNESS" fee_quotes::replay_validation::repeated_nested_quotes_preserve_priced_execution_and_source_state --exact --test-threads=1
"$HARNESS" safe_fee_quotes::repeated_nested_safe_quotes_preserve_priced_execution_and_source_state --exact --test-threads=1
"$HARNESS" safe_fee_quotes::repeated_nested_gnosis_safe_quotes_preserve_priced_execution_and_source_state --exact --test-threads=1
```

`HARNESS` selects the frozen test executable identified above. Set separate log
and datadir paths for each invocation. For the controlled experiment, prefix
each command with `taskset -c 1,3,6,8` on this host; those CPU IDs are not portable.
Both runtime sets and the production source were rehashed after validation.

No local Kurtosis/PBS run, release-profile performance measurement, concurrent
throughput test, Falcon workload or live deployment is included. The fee-sensitive
changed-payload path has correctness coverage but no dedicated latency benchmark.
The earlier PR revision's release/native/Kurtosis lanes passed; its Lint job
[failed dependency policy](https://github.com/eez-association/eez-rollupX/actions/runs/39598744986/job/119217748505)
on existing certificate dependencies. These local results do not establish a
passing CI gate for this follow-up; dependencies and their policy are unchanged.
