# Reuse native fee-quote validation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-21; local correctness checks and two-round timing comparison passed |
| Scope | L2 quote execution reuse; nested validation, correct fee accounting and latency |
| Baseline | `eez-association/eez-rollupX` main `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074` |
| Candidate | `09934699357fc8da2c5bf80c9554edff1d1df3bf` on `codex/quote-replay-reuse` |
| Protocol | Unchanged `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774` |
| Environment | Shared Linux host, Intel Core Ultra 9 185H, 22 logical CPUs, approximately 61 GiB RAM; Rust 1.95.0, Foundry 1.7.1, Solidity 0.8.34; native development-profile runtimes |
| Design | [Requirements O1–O4](../../proposals/compact-fee-quotes.md#reuse-priced-native-validation-2026-10-21) |

## Change and validation boundaries

The [settlement reuse follow-up](settlement-reuse.md) records a subsequent
optimization against this candidate. Its results are separate from the
measurements below.

L2 pricing now validates the native source and callback effects and source return
data that it prices.
The compact endpoint no longer repeats composition and native replay after
pricing. L1 quote execution, gas discovery, settlement estimation, policies,
worker bounds, final signed admission/settlement and protocol state are unchanged.
Effect mismatches are required to pass at the converged price, preserving the
ability for a provisional fee to select a different path.

The observed nested L2 run count is 16 instead of 19 when gas is omitted and
10 instead of 13 with explicit gas, per transaction format. These are workload
observations, not a fixed API contract. L1 traces retain their original counts.

The repeated-quote process test exercises both formats, explicit/omitted gas and
traced/untraced debug requests. It retains five serial latency samples per
combination, checks priced execution fields, sufficient tip and source balance/
nonce preservation. Receipt round trips and independent follower replay remain
covered by the complete fee/Safe process suites. Wall-clock timing is not a test
assertion, and these shared-host measurements do not establish mainnet throughput.

## Evidence and resource ownership

Local-only build/test logs, copied runtime hashes, trace exports and timing samples
are retained under `artifacts/quote-optimization/` in the isolated worktree
`artifacts/quote-optimization-20261021/source`. These ignored files are not
available in another checkout. A sanitized [timing summary](latency-summary.json)
is committed alongside this record. No deployment or service restart is part of
this task.

The shared `/tmp` user quota prevents sandbox startup. Commands use the authorized
host execution path and disk-backed `TMPDIR` under this worktree. The single-job
runtime build monitors host memory and terminates only its own process group if
available memory falls below 3 GiB. No unrelated services or temporary data are
removed. The existing Cargo target cache is reused; process runtimes are copied
before building test-feature dependencies.

## Executed checks

- `node sdk/l2-fees.test.mjs`: 7 passed.
- Trace-viewer model tests: 10 passed.
- `forge test --root contracts`: 89 passed, none failed/skipped.
- Rust/Solidity formatting and `git diff --check`: passed.
- `cargo +1.95.0 build --locked --offline -j 1 -p eez-node -p eez-follower
  -p eez-proof-signer --bins`: passed in 28m33s, normal development profile.
- Workspace Clippy with all targets and warnings denied: passed. A final focused
  E2E Clippy run also covers the test frame-selector correction below.
- Composer/L1/node unit tests: 689 passed, none failed; one existing manual
  backlog profiling test ignored (`profile_blocked_backlog_discovery`).
- Final runtime rebuild: passed in 3m08s; the same normal development profile.
  Unit and harness builds override only the node library to `opt-level=0,
  debug=0` to limit build memory; frozen runtime executables are unaffected.
- Final native fee/Safe regressions: 26 passed, none failed or ignored. The
  boundary case passed in 31.32s; the other 25 passed in 1229.33s. These checks
  include receipt gas, underpayment, lower accepted margins, both directions,
  pending nonces, poster funding, nested creation/callbacks, diagnostics and
  independent follower replay. Newly added checks reject mismatched call effects
  and source output even when the native source transaction succeeds.
- Repeated nested quotes: 160 successful samples across two rounds per runtime,
  using the same final harness. Source gas and native overhead match throughout.
  The first pair also has identical L1 gas and fee targets. All samples reconcile
  the configured fee formula, including the small context-dependent variation
  in L1 gas/price in the repeated candidate fixture, detailed below.
- Trace-viewer browser tests: 2 passed, including imports of the actual final
  success/failure exports and retained partial traces.
- No live deployment, Kurtosis run, release-profile benchmark or production
  throughput claim. CI's release/Kurtosis matrix is not represented by these
  native development-profile results.

The preserved baseline runtimes were produced from `57ab7fbc305fae4f0c9d26a3a8cbf0606dabe083`.
A source comparison confirms that its runtime crates and Cargo manifests/lock
match main `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074`. Both runtime sets use the
normal development profile; the reduced node-library optimization used to build
the new test harness does not affect the frozen runtime processes being timed.

| Runtime | Baseline SHA-256 | Candidate SHA-256 |
| --- | --- | --- |
| Composer | `7d6383dbe624b2db0a529f52991222abddd1b8ac07d38c8399aa686a0c950279` | `1f13aed6d7ae4adc96bd26a7657572d52c1b1b3f53b872e1aa8c2faf31e39a72` |
| Follower | `183daea16a94788133903767b8ade01baf3d44c005bdb2dc6807349f4d1a1df0` | `675dbd32ff98a771be024ad3802c08b6e2b9a4f7317acadb7ab918ed5e6dbeb0` |
| Signer | `11c083d9304ad084cf087ba75a50be501038e5e606bcc14b2e624cb6cb75897e` | `69d885e0e149d8859158fa78f0fb2c5a04af68b7ef2eab26c7d3ce490f16a169` |

The final shared process-test harness is
`6e61fc4bcb5611b9be2df062f5a7c1df05fa84efd707a963b3d5ba534eddbfff`.
Both runtime sets use that same harness for the repeated-quote comparison.

The preserved 25 fee/Safe regressions initially passed against an intermediate
candidate before review restored the source return-data comparison. That earlier
pass does not validate the final runtime listed above. Final checks use a fresh
harness built from this patch, including the edited tracing/boundary cases and
new repeated-quote case. All process runs explicitly select frozen binaries and
retain independent log/datadir trees.

## Corrections and retained failures

The first new boundary-test run failed before the negative cases: its frame
selector counted only remote roots addressed directly to the application.
Cross-chain roots instead address the execution proxy, with the application in
a child frame. The saved trace shows the expected one then two remote execution
roots as the fee changes. The assertion now counts those roots, and the trace is
saved before the assertion. No runtime change or weakened expected count was
needed. The first failure, extracted trace and datadir remain in
`final-boundaries.log`, `first-failure-trace.json` and `final-datadirs/` under the
local artifact root. The corrected harness runs use `final-v2-*` paths; this is
a test correction, not a retry-only pass.

## Measured work and latency

The comparison ran candidate, baseline, candidate, baseline, serially on the
same shared host without a concurrent compiler or second process suite. Each
fixture made 40 requests: both formats, estimated/supplied gas, traced/untraced,
five samples per combination. These are `eez_debugQuoteFee` HTTP round trips;
tracing-off requests exercise the same calculation used by `eez_quoteFee`.

| Format | Gas | Tracing | Baseline median (ms) | Candidate median (ms) |
| --- | --- | --- | ---: | ---: |
| Legacy | estimated | off | 16.050 | 10.601 |
| Legacy | estimated | on | 16.470 | 14.998 |
| Legacy | supplied | off | 9.719 | 8.743 |
| Legacy | supplied | on | 14.343 | 11.849 |
| EIP-1559 | estimated | off | 13.757 | 10.246 |
| EIP-1559 | estimated | on | 16.359 | 14.761 |
| EIP-1559 | supplied | off | 10.517 | 8.489 |
| EIP-1559 | supplied | on | 14.168 | 11.520 |

Each cell has ten samples; the JSON retains ranges and separate round medians.
Baseline timings varied substantially between rounds, particularly with gas
estimation. Both rounds' per-combination medians favored the candidate, but the
second round's untraced gains were a more modest 7–18%. Do not extrapolate these
development-profile, shared-host measurements to mainnet latency or throughput.
No combination's median regressed in either round; individual sample ranges
overlap.

All traced baseline requests performed 19 runs with omitted gas and 13 with
supplied gas. Every corresponding candidate request performed 16 and 10 runs,
respectively, including full nested callbacks. Source gas was **183,149** and
native L2 overhead was **499,652** for all 160 samples. The independent fixtures
used different block hashes: L1 receipt gas was 276,380 in the first pair and
276,356 in the repeated candidate fixture, whose sampled L1 execution price also
changed by 35 wei. These are separately estimated contexts, not identical fee
inputs. The benchmark's non-default policy uses 10% risk, 5% profit and an L1/L2
conversion ratio of 2/3. Recomputing every sample verified execution gas × price,
blob gas, configured fixed costs, risk/profit rounding, conversion and final
per-source-gas tip rounding. Both runtime sets use the same fee formula.

## Reproduction

Build node/follower/signer binaries with the runtime command above and preserve
copies before compiling the process tests. Build the harness with:

```bash
cargo +1.95.0 test --locked -j 1 \
  --config profile.dev.package.eez-node.opt-level=0 \
  --config profile.dev.package.eez-node.debug=0 \
  -p eez-node-e2e --test node_e2e --no-run
```

Set `EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` to the verified runtime copies, and
`CARGO_MANIFEST_DIR="$PWD/crates/eez-testkit"` to select this checkout's built
contract fixtures. Give each runtime set separate `EEZ_TEST_LOG_DIR` and
`EEZ_TEST_DATADIR_DIR` locations and use a writable `TMPDIR`. Execute the selected
harness with `fee_quotes:: --test-threads=1`; this includes the Safe fee tests.
The final validation ran the boundary case first, then all other cases using
`--skip fee_quotes::compact_ordinary_calls_boundaries_and_fee_sensitive_rejection`.

For comparison, select
`fee_quotes::replay_validation::repeated_nested_quotes_preserve_priced_execution_and_source_state`
with `--exact --test-threads=1` against each runtime set. Its
`quote-samples.json` records five samples for each format/gas/tracing combination.
Use the normal runtime build profile for both; the test harness profile is not
the runtime execution profile. Run serially without a concurrent compiler or
another process suite, and retain raw samples when comparing latency.
