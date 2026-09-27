# Strict FIFO admission correction

Status: strict FIFO admission and its preparation refinement are validated at
`b2ff3b8`: 286 Composer unit tests, 14 selected native correctness cases and fresh
paired release comparisons pass. Packing is preserved in the measured workloads.
The initial iteration and its corrected performance defect remain documented
below. Existing live Chiado services and datadirs are unchanged.

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
- Initial candidate shipping source: `52f8e1560bbebd85ebbce6f80df9785ffc4df6fe`.
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

## Initial correctness checks (`52f8e15`)

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

## Initial performance (`52f8e15`)

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
The new shipping/native/performance evidence below supplements the initial
results; it does not replace the first failures or measurements.

### Refined release (`b2ff3b8`)

The [new image/executable identity](refined/identity.json) records committed
source `b2ff3b894faa509fe310b6b6d38781d335f8f5fc`, unchanged protocol and the
same release recipe/profile/features as both earlier variants. Byte-identical
archive files retained their previous mtimes to reuse Cargo caches; committed
file contents and recipe hashes were checked. The native harness remains the
frozen `d0d184f` harness (its only change from `52f8e15` is the documented test
expectation). Builds finished before serial process measurements began.

Fresh tight-window pairs use the same 128-source fixture, frozen harness and
500 ms preparation setting. Both profiles pass all canonical semantic and
independent follower replay assertions. Their one-source setup posts are excluded:

| Profile | Baseline source batches | Refined FIFO source batches |
| --- | --- | --- |
| Ethereum/Osaka | 39 + 63 + 26 = **3 posts** | 39 + 39 + 50 = **3 posts** |
| Gnosis/Prague | 72 + 56 = **2 posts** | 20 + 108 = **2 posts** |

The initial nine-post Ethereum penalty is removed. Recomposition still has a
cost: the refined Ethereum run discarded three candidates (one transaction-gas,
two pipeline-time triggers); Gnosis discarded five (one transaction-gas, one
bundle-gas, three pipeline-time). None reached proof generation. Each rebuild
must include the oldest omitted eligible source and retain the original deadline;
the successful source batches have no FIFO bypass. Counts here are one fresh
paired observation per profile, not a sustained throughput or latency guarantee.

[Tight-window commands/results](refined/tight-status.json), [post sizes and
cutoff details](refined/tight-admission.json), [log hashes](refined/tight-log-hashes.json),
[refinement unit/Clippy log hashes](refined/check-log-hashes.json).

The refined shipping binaries pass **14 selected native correctness cases**:
the two tight-window cases above plus [12 targeted cases](refined/correctness-status.json).
The latter repeat both older-inbound regressions, the cheaper-source cutoff,
whole-group gas splits on both profiles, fee/balance liveness, escrow recovery
on both profiles, partial consumption, blob cutoff and late-original inclusion.
No native case needed a retry. [Admission observations](refined/correctness-admission.json)
show zero discarded FIFO candidates reaching proof; [log hashes](refined/correctness-log-hashes.json)
identify the retained evidence. Included proposal source counts in that JSON are
admission counts, not a substitute for the process tests' canonical-consumption
and semantic assertions, particularly in the partial-consumption case.

Composer unit tests: **286 passed**, one manual profile ignored. Full workspace /
all-target Clippy with warnings denied and Rust formatting passed. The first
refinement Clippy run found a signed-to-unsigned cast in the new test fixture;
using an explicitly unsigned range corrected it, and the full rerun passed.
The stale-blob accounting regression was first run against the old reset path
and failed as expected; its failure and corrected unit run are separately hashed.
Monitoring/indexer code is unchanged from the initial 24-pass validation above;
the dashboard JSON also parses successfully. The PostgreSQL, full-workspace,
Kurtosis and live-deployment exclusions above still apply.

### Refined fitting-wave performance

A new eight-run paired comparison uses the same four fitting-wave cases,
30/40-source mixes, 1500/500/700 ms settings, alternating variant order, frozen
harness and release build selection described above. Each process runs two
waves. Builds were complete; measurements were serial on the shared host.
Explicit one-source setup posts in the 40-source fixtures are excluded.

| Measurement | Baseline | Refined strict FIFO |
| --- | ---: | ---: |
| Measured sources | 280 | 280 |
| Source postBatches | 8 | 8 |
| Mean candidate-start through delivery | 295.14 ms | 291.68 ms |
| Median candidate-start through delivery | 295.66 ms | 279.88 ms |
| Mean preparation | 157.63 ms | 147.38 ms |
| Median preparation | 161.50 ms | 142.00 ms |
| Median prefix hashing | 13.82 ms | 13.32 ms |
| Median proving | 48.29 ms | 50.87 ms |
| Median finalization | 69.48 ms | 67.65 ms |
| Median delivery | 18.02 ms | 18.66 ms |

All 16 measured waves passed canonical effects, source/nonce completion,
replacement assertions where applicable, one-post packing and independent replay.
All ended with `snapshot_exhausted`; no measured capacity/FIFO/deadline cuts or
posting failures occurred after first source selection. The 30-source cases
still require no calibration post. Packing is unchanged. Mean pipeline time is
1.17% lower, while median proving and delivery increased; paired pipeline ratios
range from 0.848 to 1.210. This small noisy sample establishes neither a speedup
nor the absence of small timing/tail-latency regressions. Together with the
128-source pairs, each variant processed 536 measured sources in 13 posts.

All 560 fitting-wave sources have estimates; all 328 inbound sources have native
companion preflight rows. Admission totals and framing reserves are identical
in all eight pairs. Full projections fit their bounds, and every signed postBatch
limit equals `ceil(native_required * 1.05)`. All 14 retained numeric receipt-cost
records match native preflight exactly. Two final warm waves lack numeric cost
records at teardown (identified by blank CSV fields); their canonical receipt,
effects and replay assertions passed. Native gas differs by at most 120 gas in
matched pairs. No gas-estimator or margin changes were made in this correction.

Setup-only failures before source selection remain separate: baseline four
prover-unavailable / three deadline; candidate three / two. No comparison case
needed a retry. [Commands/results](refined/comparison-status.json), [summary and
paired timings](refined/comparison-summary.json), [per-wave gas/timing CSV](refined/comparison-waves.csv),
[log hashes](refined/comparison-log-hashes.json).

Full logs, datadirs, image build contexts and frozen executables are local-only
under `artifacts/admission-strict-fifo-refined/`; the committed records contain
identities and sanitized results. No live Chiado workload, database migration,
image promotion or merge was performed. Final-head CI remains a separate gate.
