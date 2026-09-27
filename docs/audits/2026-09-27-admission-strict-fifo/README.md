# Strict FIFO admission correction

Status: initial FIFO implementation validated locally; a preparation-accounting
refinement is being validated. Both reported bypasses reproduce on the baseline
and pass at `52f8e15`. That version's evidence below does not validate newer code.

## Contract and baseline

- User correction: older inbounds must not wait behind younger outbounds;
  restore FIFO admission. [Accepted scope](../../proposals/cross-chain-admission-throughput.md#september-27-correction-strict-fifo-admission).
- Branch / PR: `feat/cross-chain-admission-throughput`,
  [#42](https://github.com/eduadiez/rollupX/pull/42), base `main` at
  `fd77a2ebc147850333171045dbd263ec74156268`.
- Source baseline: `f6e49735f8d4289c896c205080b664fdcd5a0315`.
- Protocol: clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`;
  no protocol, dependency or wire changes.
- Runtime baseline: recorded shipping binaries from `62b8e06`, whose production
  source equals `f6e4973` (intervening commits change tests/evidence only).
  Composer SHA-256 `b8bf1f034211cfccc03fdf10750bd51d938584f4b4cb4883908832f5c390bc76`.
  Full [previous identity](../2026-09-27-admission-main-alignment/README.md) is dated evidence,
  not validation of this correction.
- Operator Chiado runbook edits and unrelated audit directories were preserved.
- Candidate shipping source: `52f8e1560bbebd85ebbce6f80df9785ffc4df6fe`.
  Test assertion correction: `d0d184f` (no runtime change).
  [Executable, image, harness and build identities](identity.json).

## Reproduction and correction

Canonical composition processes outbound groups before inbound deliveries. The
previous selector treated capacity refusals as local source-chain blocks, allowing
later cheap sources through and allowing younger outbounds to consume the budget
before older inbounds. The diagnostic `fifo_bypassed` did not prevent submission.

`gnosis_gas_cut_preserves_fifo_even_for_cheaper_independent_work` fails against the
recorded baseline at its explicit `fifo_bypassed == false` assertion: the observed
value is true. All four sources completed and the follower converged before the
assertion. First native failure logs/datadir and executable hashes are retained
locally in `artifacts/admission-strict-fifo/baseline-gas-native/`.
The initial sandbox invocation could not bind local ports and did not reach the
node; its separate `baseline-gas/` record is an environment failure.

The mixed-direction regression also fails on the baseline at the required L2
settlement-order assertion, after receipts and independent replay succeed.
Both sources were in the same initial snapshot. The younger outbound was
accepted with projected gas 2,791,236 against a 2,857,142 planning cap; a cheap
capacity check then refused the older inbound. The inbound settled in a later
batch. [Sanitized baseline observations and log hashes](baseline-reproduction.json).

The correction stops younger selection at capacity refusals. Mixed candidates
that omit an older eligible source are discarded and recomposed from a smaller
FIFO snapshot before proof generation, with the same deadline and target. The
pool remembers that source boundary across unfinished attempts and restores
snapshot order without losing queued replacements or nonce predecessors.
Classified source conditions remain eligibility exceptions. Fitting mixed batches
retain the canonical outbound-first profile and require no repair.

The first unit run caught a restoration bug: reverse front insertion could move
an independent younger source before a newly recovered predecessor. Restoration
now seeds the original snapshot order and inserts concurrent arrivals using the
pool's nonce rule. The first failure is retained in `composer-unit.log`; the
corrected run has a separate log.

On the candidate release, both mixed-direction regressions pass: the older
inbound settles first, the discarded outbound-first proposal never reaches
proving, and the replacement is linked by candidate identity. The cheaper-source
test's first candidate run passed ordering, effects, replay and post count, then
failed an obsolete assertion expecting `[0, 0]` attempt counts on both posts.
Strict FIFO correctly splits those four sources 1+3, still two postBatches. The
updated assertion verifies `[0]` then `[0, 0, 0]` and passes on the same runtime.
This is a corrected fixture expectation, not a retry-only flaky pass. Both test
executables and the first failure are retained.

## Correctness checks

- `cargo +1.95.0 test --locked -p eez-composer --lib`: **284 passed**, one manual
  backlog profile ignored. First restoration failure retained separately.
- Workspace/all-target Clippy with `-D warnings`: passed, including the corrected
  test assertion. Rust formatting and `git diff --check`: passed.
- **14 selected native cases passed** against the extracted candidate shipping
  binaries. [Case inventory, commands, hashes and first failure](native-correctness.json):
  five gas/FIFO/whole-group cases; two fee/balance eligibility cases; escrow
  recovery on both profiles; partial-consumption replay; tight preparation
  windows on both profiles; blob cutoff; late-original settlement recovery.
- Standalone indexer: **24 passed**, two disposable-PostgreSQL cases excluded.
  The initial sandbox attempt could not bind a mock RPC listener; the native
  rerun passed. New FIFO policy/details and historical admission parsing passed.
  Live PostgreSQL execution of the edited dashboard query was not tested.
- Shipping images use the same recorded recipe, locked dependencies, release
  profile, Rust 1.95 and production features as the baseline. Only committed
  source was archived. Extracted binary hashes were verified before each run.
- `forge build --root contracts`: passed with unchanged cached artifacts;
  existing missing-default-`forge-std` path/typecast warnings remain. Contracts
  and the protocol pin were unchanged; full Solidity/core suites were not rerun.
- [Check log hashes](check-log-hashes.json). Full workspace unit, Kurtosis and
  final-head CI matrices are not represented by these focused local checks.

## Performance

Fresh paired release comparison: eight process runs, alternating baseline /
candidate order across Ethereum/Osaka and Gnosis/Prague. Each case runs two waves
of 30 or 40 sources using one frozen harness, identical release recipe and
1500/500/700 ms preparation/proof/slack settings. Local compilation had finished;
existing shared-host services continued running. The 40-source cases each use
one explicit setup source, excluded from measured totals. The 30-source cases
have no calibration source.

| Measurement | Baseline | Strict FIFO |
| --- | ---: | ---: |
| Measured sources | 280 | 280 |
| Source postBatches | 8 | 8 |
| Mean candidate-start through delivery | 280.17 ms | 286.45 ms |
| Median candidate-start through delivery | 282.13 ms | 294.59 ms |
| Median preparation | 142.00 ms | 151.00 ms |
| Mean preparation | 144.88 ms | 149.38 ms |
| Median prefix hashing | 13.09 ms | 14.28 ms |
| Median proving | 47.18 ms | 47.25 ms |
| Median finalization | 63.75 ms | 69.92 ms |
| Median delivery | 19.07 ms | 19.50 ms |

All 16 measured waves exhausted their snapshots and passed canonical effects,
nonces, source completion, one-post packing and independent follower replay.
There were no measured capacity/FIFO/deadline cuts or posting failures after
source selection. Packing/throughput at this workload is preserved. Mean pipeline
time increased 2.24%; preparation and finalization also increased. Paired pipeline
ratios span 0.933–1.108, with median 1.043. This small shared-host sample cannot
establish a speedup or rule out a small timing regression. It does not measure
saturated live Chiado throughput or tail latency under sustained overload.

All 560 measured sources have admission estimates; all 328 inbound sources have
native companion preflight records. Admission totals and framing bounds match
in all eight pairs. All postBatch limits equal `ceil(native_required * 1.05)` and
all full projections fit their admission bounds. All 15 retained numeric
postBatch receipt-cost records match native preflight exactly. One final warm
Ethereum 40-source candidate has no retained numeric cost record at teardown;
its canonical receipt/effect/replay assertions passed, and the CSV leaves receipt
gas absent rather than substituting a simulation. Matched native gas differs by
at most 168 gas; estimates and default 5% headroom were not changed here.

Setup-only prover failures before source selection were separate: baseline had
four unavailable / three deadline failures; candidate had three / two. No retries
of failed performance cases were needed. The first candidate FIFO gas fixture
failure above is separately retained and is not part of this comparison.

[Summary and paired ratios](comparison-summary.json), [per-wave gas/timing
CSV](comparison-waves.csv), [retained log hashes](comparison-log-hashes.json).

Strict FIFO can leave spare capacity when an older source does not fit; repairing
a mixed candidate repeats preparation from the parent. Fitting snapshots retain
the existing single-pass path. The two constrained FIFO fixtures each still used
two source postBatches; that does not prove optimal packing for every queue mix.

## Operational limits

Local-only artifacts under `artifacts/admission-strict-fifo/`
are ignored by Git and unavailable in another checkout. The existing live Chiado
node/signer and writable datadirs are unchanged. The main-branch Reth migration's
existing-network compatibility limitations remain in effect.

## Tight-window finding and refinement

Post-run inspection of the 128-source, 500 ms preparation fixture found a real
cost in the first FIFO iteration: Ethereum used nine source postBatches and 15
repairs (plus its explicit one-source setup post). Gnosis used two source posts
and one repair. The earlier `62b8e06` correctness records used three and two
source posts respectively; those earlier runs are not a new paired comparison.
[Initial per-case admission counts](correctness-admission.json) retain the finding.
No discarded non-FIFO candidate reached proving in any of the 14 cases.

Two implementation choices explain avoidable work. First, a candidate that had
simulated 39 fitting groups was reduced to the first omitted source's position,
creating a three-source batch. Repair now seeds its FIFO width from the number
of completed groups while including the omitted source; all effects are rebuilt
and every resulting proposal must still pass the FIFO check. Second, discarded
blob counts and elapsed simulation contaminated the next candidate's predictions.
A deterministic regression fails with eight stale blobs retained after rebuild.
The correction resets candidate observations and predicts future work from the
replacement alone, while the unchanged absolute clock charges all discarded
work. The next sample excludes discarded preparation; conservative finishing
reserves and final execution/deadline validation remain in force.

The refined unit suite passes 286 tests with the same one manual exclusion.
New shipping/native/performance evidence is pending and will be appended without
replacing the initial results.
