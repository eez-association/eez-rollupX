# Reuse identical L1 fee measurements within a quote

| Field | Value |
| --- | --- |
| Date and status | 2026-10-21; local correctness checks and two-round timing comparison passed; release CI remains separate |
| Scope | Follow-up to the [first replay optimization](README.md), preserving final priced source validation |
| Baseline | RollupX `09934699357fc8da2c5bf80c9554edff1d1df3bf`, draft PR #100 |
| Candidate | Follow-up commit containing this record on `codex/quote-replay-reuse` |
| Protocol | Unchanged `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774` |
| Environment | Same shared host and normal development-profile runtimes as the first comparison; Rust 1.95.0, Foundry 1.7.1 |
| Design | [Identical settlement reuse](../../proposals/compact-fee-quotes.md#identical-settlement-replay-reuse-authorized-follow-up-2026-10-21) |

## Boundary under test

An estimator is created inside each L2 envelope's quote calculation, borrowing
the immutable posting runtime and quote. Every fee iteration composes and
natively executes the source, rebuilds settlement calldata, checks funding and
deployment, and prepares exact posting/verifier inputs. Only equality of the
prepared transaction, verifier data, endpoint, rollup and callback inspector
allows reuse of an earlier successful gas measurement. Payment arithmetic runs
again. Calldata equality alone is insufficient: nested expected return values
and success flags are validated independently by the inspector.

The first measurement retains the generous and selected-limit L1 probes and
the existing high-limit fallback. Failed measurements are not cached. One
successful measurement is retained per envelope; it never crosses formats,
requests or snapshots. The cumulative signed-admission caller creates a fresh
estimator for each measurement. Final signed posting is unchanged.

The additive debug counter `l1SettlementReuses` explains omitted duplicate L1
runs. Existing actual L1 traces are retained; no execution is fabricated.
Compact wallet responses, fee policy, protocol, stored state and configuration
are unchanged. No service restart, live submission, deployment or merge was
performed.

## Evidence inventory

Large logs, datadirs, runtime copies and raw samples are **local only**, under
`artifacts/quote-optimization/settlement-reuse/` in the isolated worktree
`artifacts/quote-optimization-20261021/source`. These ignored files are unavailable
to another checkout. The first optimization's frozen `candidate-runtime/`
binaries are preserved and serve as this comparison's baseline; their hashes
are recorded in the [earlier evidence](README.md#executed-checks).

The baseline and candidate repeated nested-quote process tests each passed 40
requests using the same frozen harness. Traced run counts changed from 16 to 14
with automatic gas discovery and 10 to 8 with explicit gas. Candidate diagnostics
record one reused measurement per quote. Both sets used 183,149 source gas and
276,380 L1 settlement gas. An independent arithmetic check reconciled all 80
quotes, including physical blob gas, 10% risk, 5% profit, fixed service costs,
the fixture's 3/2 conversion and per-source-gas tip rounding. This fixture uses
nondefault policy values; production fee settings are unchanged.

| Executable | Candidate SHA-256 |
| --- | --- |
| Composer | `73b5287e94655c22628fd89a8beedcb65013797cd92332e13d809b4e80a9728d` |
| Follower | `675dbd32ff98a771be024ad3802c08b6e2b9a4f7317acadb7ab918ed5e6dbeb0` (same as baseline) |
| Signer | `69d885e0e149d8859158fa78f0fb2c5a04af68b7ef2eab26c7d3ce490f16a169` (same as baseline) |

The first comparison used the unchanged repeated-quote case in harness
`6e61fc4bcb5611b9be2df062f5a7c1df05fa84efd707a963b3d5ba534eddbfff` for both
runtimes. The runtime build passed in 15m55s using the normal development profile,
one Cargo job and the existing memory guard. Contract artifacts were verified
current with `forge build --root contracts`. SDK tests (7), viewer model tests
(10), formatting and whitespace checks passed.

Composer/L1/node unit checks passed: 399 + 194 + 98 = 691 tests, with the existing
manual backlog profiler ignored. The new mocked cache regressions require replay
for fourteen distinct transaction/proof/endpoint/callback mutations and verify
that failures are not retained. Workspace Clippy passed for all targets with
warnings denied. Unit and harness builds use the existing node-library-only
`opt-level=0, debug=0` override to limit memory; frozen runtimes retain the normal
development profile.

The updated process harness is
`d7c0a650c0400f727c1e3dcaa67c3da3428140c6d0d3e3881ca1bd5d0810df55`.
Both edited process cases passed on their first run: nested tracing in 41.79s,
fee-sensitive/boundary validation in 30.89s. A single request for both formats
records one reuse and two real settlement probes per format. The fee-dependent
case records zero reuse and four real settlement probes for each format, with
different initial/priced calldata. Gas-sensitive source-effect/output mismatch
rejections remain intact. Browser tests passed (2), importing these actual
exports. The remaining 24 native fee/Safe cases passed in 1184.48s, with none
failed or ignored: **26 process cases passed** in total. These include actual
receipt-cost comparisons, underpayment/price-shift rejection, 1% and 5% accepted
margins below the 20% quote, explicit/omitted pending nonces, both directions,
64 KiB calldata, poster/source funding, partial traces, real Gnosis Safe and
nested factory initialization, and independent follower replay. All new tests
passed on their first run; there were no retries masking failures.
After runtime/native-harness freezing, final review changed only the cache unit
fixture's origin and parent to describe a valid L1 root rather than a
self-referential parent. The two selected cache unit tests passed again, and
workspace Clippy was repeated. This is `cfg(test)` data; no production code
changed after the recorded runtime build.

## Repeated latency comparison

The committed [summary](settlement-latency-summary.json) retains round medians,
ranges, sample counts and raw sample digests. The order was baseline, candidate,
candidate, baseline. The second candidate fixture ran within the serial native
suite; no other owned test or compiler ran concurrently. Each round used the
same harness for both runtimes, and the benchmark source was unchanged. Each
runtime produced 80 successful quotes: ten samples per format/gas/trace
combination across two independent fixtures.

| Format | Gas | Tracing | Baseline median (ms) | Candidate median (ms) |
| --- | --- | --- | ---: | ---: |
| Legacy | estimated | off | 12.042 | 9.090 |
| Legacy | estimated | on | 15.950 | 12.405 |
| Legacy | supplied | off | 9.028 | 6.986 |
| Legacy | supplied | on | 12.051 | 9.140 |
| EIP-1559 | estimated | off | 9.919 | 8.668 |
| EIP-1559 | estimated | on | 13.865 | 11.436 |
| EIP-1559 | supplied | off | 8.125 | 6.808 |
| EIP-1559 | supplied | on | 11.346 | 8.846 |

Pooled medians improved by 12.6–24.5%. Every combination favored the candidate
in both rounds, but ranges overlap and baseline timing changed between rounds.
The second round's untraced median improvements were 8.3–23.0%; these numbers
do not establish a production speedup. All 160 quotes retained source gas
183,149 and L1 gas 276,380, and passed independent payment arithmetic checks.
Traced counts were consistently 16→14 with estimated gas and 10→8 with supplied
gas, per format, with exactly one reused measurement in every candidate quote.
Combined with the first optimization, this fixture has 14 instead of 19
recorded runs (8 instead of 13 with supplied gas).

## Reproduction commands

Run from the candidate checkout with the initialized protocol and LLVM 22. The
runtime build uses the shipping packages' default features; source dependencies
and `Cargo.lock` are unchanged:

```bash
cargo +1.95.0 build --locked --offline -j1 \
  -p eez-node -p eez-follower -p eez-proof-signer --bins
cargo +1.95.0 test --locked --offline -j1 \
  --config profile.dev.package.eez-node.opt-level=0 \
  --config profile.dev.package.eez-node.debug=0 \
  -p eez-composer -p eez-l1 -p eez-node --lib
cargo +1.95.0 test --locked --offline -j1 \
  --config profile.dev.package.eez-node.opt-level=0 \
  --config profile.dev.package.eez-node.debug=0 \
  -p eez-node-e2e --test node_e2e --no-run
cargo +1.95.0 clippy --locked --offline -j1 \
  --config profile.dev.package.eez-node.opt-level=0 \
  --config profile.dev.package.eez-node.debug=0 \
  --workspace --all-targets -- -D warnings
```

Runtime binaries are copied and hashed **before** building test-feature targets.
The process runs explicitly set `EEZ_TEST_PREBUILT=1`,
`EEZ_TEST_RUNTIME_BIN_DIR` to the frozen runtime directory,
`EEZ_TEST_PROOF_SIGNER_BIN` to its signer, and `CARGO_MANIFEST_DIR` to
`$PWD/crates/eez-testkit`. Each run uses separate retained `EEZ_TEST_LOG_DIR`
and `EEZ_TEST_DATADIR_DIR`, and the worktree's disk-backed `TMPDIR`.
The two edited cases above use their full test names with `--exact`; the
remaining selection is `fee_quotes::`, skipping those two names. All process
invocations use `--test-threads=1`. The repeated benchmark is
`fee_quotes::replay_validation::repeated_nested_quotes_preserve_priced_execution_and_source_state`,
selected with `--exact`. Reuse the **same** benchmark harness for each compared
runtime and stop concurrent owned compilation before taking timings.

## Limits

The timing fixture measures serial HTTP quotes against embedded local chains,
not production RPC latency or concurrent throughput. The timed workload reuses
one L1 measurement. Changed fee-dependent inputs have native correctness
coverage, but their latency and cache bookkeeping overhead are not characterized
by these timings. Shared-host ranges and repeated rounds must accompany medians.
There is no latency threshold in the regression tests.

No local Kurtosis/PBS run, release-profile benchmark, Falcon workload, live
deployment or full protocol stress run is part of this follow-up. The ignored
manual backlog profiler is not coverage. The release CI gate remains separate
from native correctness checks and the existing dependency finding below.

## Separate CI finding

PR #100's first lint job stopped in `cargo deny` on existing lockfile advisories
RUSTSEC-2026-0381, RUSTSEC-2026-0382 and RUSTSEC-2026-0383 (webpki roots and
certificate validation). The [failed job](https://github.com/eez-association/eez-rollupX/actions/runs/39591100270/job/119191964232)
identifies the versions. No dependency update or advisory suppression belongs
to this performance change. Local correctness/performance evidence does not
establish a passing release gate.
