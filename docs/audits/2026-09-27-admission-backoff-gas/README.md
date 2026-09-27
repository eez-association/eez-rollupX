# Confirmed-failure backoff and companion reserve refinement

Status: implementation and local deployment complete. All 870 sources settled,
all 770 semantic jobs passed, canonical receipts were rechecked, and all 50
selected process cases passed. Exclusions and existing formatting failures are
recorded below; these are local results, not CI promotion evidence.

| Field | Value |
| --- | --- |
| Requested change | Wait for five errors before shrinking admission; improve measured gas estimation |
| Branch / baseline checkout | `feat/cross-chain-admission-throughput` / `69359e362476108fcfb6a030f07a8b289c800c0f` |
| Baseline runtime | `6c171fdb3221a6d6183e113e9d05e4c7f657099b` |
| Candidate runtime / additional test | `9c11c1aead3c6f959bee4787a65f2a116d6e41f7` / `8c2b424` (runtime unchanged) |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Tools | Rust 1.94.0, Foundry 1.7.1, locked dependencies |
| Requirements | [Accepted follow-up](../../proposals/cross-chain-admission-throughput.md#september-27-failure-tolerance-and-companion-reserves-accepted-for-implementation), [settlement policy](../../configurable-settlement-policy.md), [ownership contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) |
| Prior comparable wave | [Preparation iteration](../2026-09-27-admission-preparation/README.md): 870 sources, 770 semantic jobs, 28 posts, 206.023 seconds |

## Changes

Automatic admission preserves selection through four consecutive canonically
confirmed, charged incomplete settlements. The fifth and later failures halve
the next allowance; complete canonical settlement resets the streak. Skipped
slots, uncertain observations and preparation/dispatch do not train it. Real
pipeline deadline guards remain immediate. Numeric admission is unchanged.

Canonical reconciliation allows automatic sources six cumulative charged
failures, so the fifth can receive its smaller retry. Prover/immediate relay
rejection keeps three on the existing cumulative source counter. No new durable
queue, worker, wire field or protocol rule is introduced. Per-source dispositions,
nonce dependencies and old/replacement ownership remain authoritative.

| Companion reserve | Before | Candidate |
| --- | ---: | ---: |
| Root | 80,000 | 70,000 |
| Callback | 40,000 | 35,000 |
| Nested result row | 30,000 | 25,000 |
| Input/result word | 2,500 | 2,300 |
| Ordinary preceding identity scan | 3,000 | 2,600 |
| Matching identity scan | 12,500 | 12,500 |
| Revert span | 25,000 | 25,000 |

The existing bounded shapes, full-prefix scan allowance, application gas,
declared-limit fallback, signed-start constraints and final ordered execution
remain. Posting storage prices are unchanged. Policy is
`confirmed-failure-backoff-4`; estimator is `companion-reserve-7`. Candidate
`started` details and Grafana's preparation table expose the streak and threshold;
historical/numeric rows remain null. Existing collectors require no migration.

## Initial evidence

- The new five-failure regression failed on the unchanged baseline at failure
  one: selection was 40 instead of 100. The candidate Composer suite passed
  269 tests, with one existing manual diagnostic ignored.
- Baseline companion fixtures passed nine tests / 27 logged measurements.
  Candidate gas fixtures passed 51 tests on each of Cancun, Prague and Osaka;
  all 70 integration contract tests passed. Eighteen new measurement cases cover
  short/long result storage boundaries and 128-entry scans. All 27 matched
  reserves decrease; the smallest measured reserve/execution ratio is 1.0863.
  The fixture measurements include application work, which production estimates
  additionally charge. These bounded measurements are not universal gas bounds.
  See [gas results](gas-tests.json) and [comparison](gas-fixture-comparison.csv).
- The first process run demonstrated four full-size retries, halving after the
  fifth exclusion, and complete recovery of all four sources. Its new assertion
  incorrectly indexed an absent optional tracing field: `None` is omitted, not
  serialized as JSON null. The assertion has been corrected to accept either;
  the original failure and complete process diagnostics are retained. The
  corrected tests passed. Together with the unchanged first-run passes, all
  eight focused cases passed. The new sixth-failure case also verified complete
  nonce-suffix release and successful explicit resubmission of identical signed
  bytes, followed by independent replay. This was a deterministic assertion
  correction, not a retry-only pass attributed to a flake. See the
  [focused results](focused-native-results.json), [executable identities](focused-native-executables.json),
  [first failure](focused-first-diagnosis.json) and
  [counter observations](backoff-process-observations.json).
- Workspace Clippy passed with warnings denied, including the additional test.
  The tooling suite ran 264 cases: 260 passed and four explicitly opt-in
  infrastructure probes were skipped. [Local checks](initial-local-checks.json)
  record the exclusions. Formatting and whitespace checks passed for edited files.
  Whole-contract `forge fmt --check --root contracts` remains nonzero on the
  unchanged `ECDSAProofSystem.sol` and `DeployRollup.s.sol`; the changed gas
  fixture passes its focused formatting check. [Formatting evidence](format-checks.json).
- The live provisioned Grafana query returned all 41 preceding-wave candidates
  with the new fields null and existing preparation fields intact. No collector
  restart or migration was required. [Historical query check](grafana-historical-fields.json).

Raw evidence is local-only under `artifacts/admission-backoff-gas/` and is not
available from another checkout. No private operator inputs are included here.

## Local image rollout

The [build plan](build-plan.json) archives exact runtime source and pinned
recursive submodules, with the same release/shipping recipe and locked
dependencies as the baseline. These are local images, not published CI artifacts.
[Image verification](images.json) confirms the Composer binary changed while
signer, follower and genesis helpers are byte-identical to the baseline, and
both original genesis commitments still match.

The [rollout](runtime-rollout.json) replaced only node and signer images under
an empty-queue check and the shared wallet lock. Original genesis, datadirs,
mounts, service environment, Lighthouse and monitoring history were retained.
The first health check exposed a restart gap in the existing peerless embedded
L1. Initial read-only recovery preparation received an incomplete upstream block
response and imported nothing. The second preparation verified all 19 raw
canonical payloads against hashes, parent continuity, transaction hashes and
local Beacon head. All Engine imports returned the expected `VALID` hash;
Lighthouse retained fork choice. [Recovery](restart-gap-recovery.json) and
[healthy heads](health-after-rollout.json) record the result. Failed logs remain
local under `artifacts/admission-backoff-gas/`.

The same private worker-funding profile passed readiness without extra source
funding. Staged run `20260927-122320` uses `--verify`, 770 jobs / 870 sources,
200 preparation workers, 28 send workers, four semantic verifiers and the same
5-request/second ordinary L1 forwarding as the baseline. The runner held its
wallet lock. No extra build/test load ran during its bulk throughput window.

## Live measurement

The same workload completed with an empty canonical queue. The figures below
count unique sources and source-bearing postings, independently of retries.

| Measure | Baseline `6c171fd` | Candidate `9c11c1a` |
| --- | ---: | ---: |
| Canonical sources | 870 | 870 |
| Source-bearing postBatches | 28 | 24 |
| All canonical postBatches during measured interval | 28 | 25 |
| Mean sources per such postBatch | 31.07 | 36.25 |
| Send + monitor seconds | 206.023 | 178.474 |
| Sources / second | 4.2228 | 4.8747 |
| Backoff-limited canonical posts | 6 | 0 |
| postBatch admission / receipt gas | 1.3471 | 1.3461 |
| Companion estimate / receipt gas | 2.0007 | 1.8277 |
| Mean canonical preparation ms | 346.04 | 392.68 |
| p50 source settlement latency seconds | 110.456 | 72.647 |
| p95 source settlement latency seconds | 200.456 | 167.647 |

Observed throughput increased 15.44%, and elapsed time decreased 13.37%.
The larger batches take longer to prepare individually; their total canonical
preparation time decreased from about 9.69 to 9.42 seconds. Source order,
contract/queue state, block gas limits, fees and slot production differ between
these live waves. This is one wave per revision, not an isolated estimate of
the implementation's throughput effect. The controlled fixture comparison
establishes the reserve reduction separately.
Latency is canonical posting time minus bulk-send start; source-specific send
uncertainty is bounded by the 1.261 / 1.289 second send durations.
Summed receipt gas for source-bearing postings and companions increased 0.28%
(250.215 to 250.916 million); the additional history posting adds 0.109 million.
Fewer postings did not reduce aggregate execution gas in this live comparison.

There were **25 total canonical postings during this wave**, including one
ordinary-history posting with zero wave sources (109,117 execution gas, two
blobs). Actual predecessor data exceeded the two-blob capacity. The first
history submission was omitted and the second settled; this is capacity-driven
history settlement, not calibration or preparation-timeout warmup.
See [all window postings](candidate/window-postings.csv) and the separately
verified [external history receipt](candidate/history-receipt.json).

All 36 scheduled candidate slots are accounted for:

- 24 canonical cross-chain postings;
- four relay-acknowledged cross-chain submissions omitted from produced blocks;
- three skipped slots;
- two history-prefix selections, one successful and one omitted;
- three slots with no eligible sources because signed source fee caps were
  15 wei and target base fees were 16 wei. The remaining cohorts contained
  421, 187 and 120 inbound sources respectively, all with that cap.

The generic target classifier calls both history-prefix candidates
`produced_without_candidate`, because their cross-chain candidate was replaced
by an ordinary-history posting. Joining posting lifecycle and canonical
receipts yields the refined counts above. The three no-source candidates have
no wave-source events and are included explicitly. These are not additional
prover failures. [Attempt outcomes](candidate/attempt-outcomes.csv) and
[comparison / fee evidence](candidate/wave-analysis.json) preserve the distinction.

Four isolated confirmed incomplete source settlements logged streak 1 and
threshold 5, with no reduced allowance. Every following source-bearing retry
kept the same accepted count (36, 51, 16 and 28); successful settlement reset
the next streak to zero. The live wave did not reach five consecutive failures;
the controlled process tests establish the fifth/sixth boundary.

### Cutoffs and gas

| Canonical cutoff | Posts | Sources |
| --- | ---: | ---: |
| `l1_bundle_gas_budget` | 14 | 496 |
| `l1_tx_gas_budget` | 9 | 346 |
| `snapshot_exhausted` | 1 | 28 |

All 24 source-bearing postings and all 870 canonical sources have admission
estimates. Posting admission totaled 238,645,674 gas against 177,292,623 receipt
gas; final preflight used-gas predictions matched every receipt exactly.
Companion admission totaled 134,564,786 against 73,623,513 receipt gas across
all 520 inbound sources. The largest remaining family ratios are multi-call
twice (2.561) and nested multi-call (2.410). Posting-storage estimates were not
changed in this iteration.

Bundle-limited batches averaged 98.49% of estimated capacity, while actual
used gas consumed 52.72% and the ordered signed-start requirement consumed
57.53%. All signed-start requirements fit their active parent-derived budgets
(17.539–17.815 million gas). Receipt gas alone cannot establish that another
signed companion would fit. Final ordered simulation remains authoritative.
Cheap checks handled 9,260 of 10,603 gas refusals (87.33%), with 2,507 full
source simulations across the 33 candidates that examined wave sources.

Evidence: [per-post estimates / receipts](candidate/postbatch-estimate-vs-actual.csv),
[companion families](candidate/companion-estimate-vs-actual.csv),
[cutoffs](candidate/cutoffs.csv), [bundle budgets](candidate/bundle-budget-review.json),
[cheap checks](candidate/minimum-checks-summary.json),
[preparation](candidate/preparation-summary.json).

The live Grafana preparation table returns all 36 candidates with nonnull
failure streak and threshold, along with prefix-hash timings. Blob/witness
timings exist for the 31 candidates that proceeded through the cross-chain
proof path; the two history selections and three fee waits correctly lack
those cross-chain stages. All prior-wave rows retain null failure fields.
[Live query evidence](candidate/grafana-fields.json).
The provisioned source table was also checked for ordinary, reverted and
nested multi-call sources, including their unique canonical accepted row and
receipt gas. [Source query checks](candidate/grafana-source-checks.json).

All [770 semantic jobs](candidate/semantic-verification.json) passed without
skipped verification. Repeating canonical attribution after verification
produced byte-identical source/posting CSVs, and the independent configured L1
endpoint matched all 24 source-bearing posting receipts, hashes, success statuses
and gas values. [Canonical stability](candidate/canonical-stability.json).
The task-owned ordinary L1 forwarder was then stopped; shared Chiado services
remain running. [Cleanup](forwarder-cleanup.json).
The forwarder recorded 1,397 original ordinary broadcasts and no upstream
rate limits. It also recorded 2,441 client `BrokenPipeError` disconnects versus
2,379 in the preceding wave; no workload send or semantic job failed.
[Forwarder counters](ordinary-forwarder-final.json).
The [final health check](final-health.json) confirms the intended image digests,
fresh nonsyncing L1/L2 heads, external canonical agreement, the original genesis,
an empty queue and unchanged operator settings apart from the two runtime images.

### Remaining opportunities

Increase source fee headroom in a separately comparable workload; the node
must retain the signed fee cap. Investigate relay/builder omission behavior:
acknowledgment establishes receipt, not inclusion or its failure reason.
Further companion reserve reductions need worst-case scan/result fixtures,
particularly for multi-call shapes; current minimum measured headroom is only
1.0863 in the most restrictive fixture.

## Completed process gate and limits

The [42 broader native cases](broad-native-results.json) passed on their first
execution, with zero failures or ignored tests. Their seven suites covered
dynamic admission, partial consumption/restart, queue liveness, queue reorg,
settlement policy, receipt cost and gas packing. The runner started at
12:34:55 UTC, after the measured window ended at 12:32:40 and the empty canonical
queue check passed. It completed at 13:19:53. Together with the eight focused
cases, 50 intended process cases were validated. The earlier focused assertion
correction remains recorded above. [Exact executable identities](broad-native-executables.json)
and the [selected/excluded inventory](broad-native-plan.json) accompany the results.

Twenty unchanged ordinary-only policy cases were excluded explicitly. The one
existing manual Composer diagnostic and four opt-in tooling probes were not
executed. Full CI/Kurtosis and full-wave independent follower replay were not
run; the native process cases include independent replay. The protocol pin and
production signer/follower binaries are unchanged. Whole-contract formatting
still fails on two unchanged files; the edited fixture and Rust formatting pass.
No image was published or promoted to a CI deployment tag.

## Reproduction references

- The focused and broader plan JSON files contain the exact Cargo commands and
  intended inventory; use Rust 1.94.0 with locked dependencies and the source
  revisions recorded in their results. Source-based process runs clear prebuilt
  overrides and retain logs/datadirs under `artifacts/admission-backoff-gas/`.
- The [build plan](build-plan.json), [image identities](images.json) and
  [rollout record](runtime-rollout.json) bind the local deployment to runtime
  revision `9c11c1a`. Later changes in this iteration contain tests and records.
- Canonical attribution used
  `docs/audits/2026-09-26-admission-throughput/measure-wave.py` with protocol run
  `eez-core-protocol/tmp/e2e-staged-net/20260927-122320` and output directory
  `artifacts/admission-backoff-gas/candidate`. The same attribution was repeated
  after semantic verification; sources and postings were byte-identical.
- Cutoff analysis used the retained admission events and
  `docs/audits/2026-09-26-admission-deferred/rerun2/analyze-cutoffs.py`.
  Preparation used
  `docs/audits/2026-09-27-admission-preparation/profile-preparation.py`.
  Local packing/budget helpers under `artifacts/admission-backoff-gas/` were
  adapted only for policy `confirmed-failure-backoff-4` and estimator
  `companion-reserve-7`; the checked-in compact CSV/JSON evidence includes their
  reconciled totals. Raw logs, snapshots, per-source tables and private operator
  profiles remain local-only and are not included in the public record.
