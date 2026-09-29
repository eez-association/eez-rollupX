# Preparation profiling and prefix execution reuse

Status: completed. Candidate deployed; the verified live workload, focused gate
and broader applicable native regressions passed.

| Identity | Value |
| --- | --- |
| Branch | `feat/cross-chain-admission-throughput` |
| Baseline checkout | `2b9ca04e87f278a1e9bed0d25420f3fa20b86a08` |
| Baseline running source | `7fdb3fc4dae1156051a77bcbdf00301914085f5b` |
| Candidate runtime source | `6c171fdb3221a6d6183e113e9d05e4c7f657099b` |
| Candidate runner correction | `698dee3` (runtime sources unchanged) |
| Monitoring query correction | `e0b892ca17369954ba5230a884a2425eef2a20f7` (runtime sources unchanged) |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Baseline wave | Protocol run `20260927-083400`, 870 sources, 26 canonical posts |

The user authorized focused validation before the local-image Chiado trial,
followed by broader applicable tests before completion. Design and invariants:
[preparation follow-up](../../proposals/cross-chain-admission-throughput.md#september-27-preparation-follow-up-accepted-for-implementation).
The previous [runtime/wave audit](../2026-09-27-admission-refinement/README.md)
establishes the baseline image identities and semantic assertions.

## Where preparation time goes

Canonical candidates only. Mean total 453.29 ms, median 456.44 ms, maximum
672.87 ms. Proof and finalization are outside these preparation intervals.

| Interval | Mean per post |
| --- | ---: |
| Accepted-source lowering, prefix hashes and canonical append | 198.81 ms |
| KZG blob sidecar construction | 172.31 ms |
| Witness and remaining proof-request preparation | 22.50 ms |
| Final block build, encoding and full gas projection | 15.09 ms |
| Refused-source checks/simulation and subsequent rejection work | 14.35 ms |
| Accepted-source checks and simulation | 13.19 ms |
| Other admission setup/scheduling | 10.91 ms |
| Cheap rejection checks and outcome recording | 6.13 ms |

These wall-clock intervals include scheduling and logging. The first row cannot
be attributed entirely to prefix hashing from historical events. KZG comes from
integer-millisecond `admission.prepared` logs, joined to candidate identity and
cross-checked against accepted counts and preparation time. The new runtime
adds separate prefix, blob and witness timing to existing retained event kinds.

Reproduce using [profile-preparation.py](profile-preparation.py). The
[summary](baseline/preparation-summary.json) and
[per-post intervals](baseline/preparation-per-post.csv) are sanitized evidence.
Detailed admission history remains local at
`artifacts/admission-refinement/candidate/detail/events.private.json` and the
sanitized retained timing records at
`artifacts/admission-preparation/baseline/prepared.json`; these ignored local
artifacts are unavailable in another checkout.

Blob admission already estimates the complete group's encoded size without
KZG. Final sidecar generation is cryptographic work required for submission;
estimating blob count cannot replace it. All baseline posts used one blob and
none stopped on blob capacity. Moving or overlapping KZG is a separate
optimization requiring bounded worker ownership and shared-deadline accounting.

## Change and initial evidence

The existing `hash_group` rebuilds every requested growing prefix from its
parent. The candidate retains one checkpoint before post-execution changes,
executes only the newly added suffix, and finalizes a copy with Reth. Changed
or shortened prefixes discard reuse, and construction errors invalidate it.
Final canonical append, block rebuild, witnesses, signer checkpoint derivation
and ordered signed-bundle validation remain independent.

Initial focused local tests passed all three new regressions. A 64-prefix
comparison executed 2,080 transactions in the baseline helper versus 64 in the
candidate: 79.16 ms versus 6.40 ms in that run. This uses a mock provider and
does not measure trie work or establish live speedup. Complete pre-finalization
state and receipt/gas equality are checked separately from header equality
because the mock provider does not compute real state roots. Native and devnet
evidence are required before concluding the live improvement is correct.

Composer unit suite: 267 passed, one existing manual diagnostic ignored.
Focused Composer and [workspace Clippy](clippy-workspace.json) passed with warnings
denied. All selected process tests passed. See [local checks](local-checks.json)
and the completed validation below.

## Readiness correction

The first unchanged readiness check rejected the next trial for a supposed
0.201012662866298632 xDAI L1 funding shortfall. Inspection at the protocol pin
showed that `_plan_funding` already funds only deficient workers, and
`_topup_faucet` reserves gas only for nonempty funding chunks. The fork's staged
preflight still assumed 770 empty wallets.

The helper now reads balances under the existing wallet lock and applies the
pinned calculation per chain. The same profile passes: L1 needs 520 worker
top-ups and a 0.739265059018197372 xDAI faucet budget; L2 needs none. Existing
source/faucet funds cover both. Worker targets remain 0.0033 xDAI and no extra
poster transfer is required. Eight focused offline runner tests and Bash syntax
checks passed. Retained before/after checks are local in
`artifacts/admission-preparation/readiness-{before,corrected}.log`.

## Focused process gate

Five source-based native cases passed across `chained_interstate`,
`dynamic_admission`, `partial_consumption`, and `settlement_policy_gas`:
interdependent mixed-direction work, existing-proxy multi-call work, a Gnosis
mixed wave, middle-group resumption, and independent packing after a gas refusal.
These exercise real state/trie construction, signer validation and follower
replay. All selected cases ran once; no failure or ignored case was counted as a
pass. The test run started before the implementation commit, so its original
checkout field names `2b9ca04`; recorded hashes of all three changed Rust files
match committed candidate `6c171fdb3221a6d6183e113e9d05e4c7f657099b` exactly.

## Local image rollout

The local release build used the same shipping feature selection and build
recipe as the baseline. Its context contains the exact runtime commit and clean
pinned recursive submodules. [Build plan](build-plan.json) and
[image verification](images.json) record image and executable identities.
These are local development images, not CI-promoted artifacts.

The [rollout](final-runtime-rollout.json) replaced only the existing node and
signer images. Datadirs, original genesis, mounts and service environment were
preserved. The initial rollout retained the indexer image: the new timing fields
fit existing event details and require no schema migration. A subsequent,
separately validated indexer query correction is described below.

The first health check found a stale L1 head after the peerless restart. A
read-only preparation failed on an incomplete external RPC response; it imported
nothing. A subsequent complete fetch verified raw headers, transaction hashes,
parent continuity, empty requests and the local Beacon target. All 65 bounded
Engine payload imports returned `VALID` with the expected hash; Lighthouse
retained fork-choice ownership. See [recovery](restart-gap-recovery.json) and
[healthy canonical heads](health-after-rollout.json). Original failed checks and
raw payloads remain local under `artifacts/admission-preparation/`.

## Verified live comparison

Candidate protocol run `20260927-100219`: 770 jobs / 870 sources, same 0.0033
xDAI worker target, 200 preparation workers, 28 send workers, four semantic
verifiers and the same rate-limited ordinary L1 forwarding as the baseline.
The staged runner held its wallet lock and used `--verify`: **770 passed, zero
failed, zero not verifiable**. Contract setup is outside the timing window.
The broader native builds/tests started after that measured window ended.

| Measure | Baseline | Candidate |
| --- | ---: | ---: |
| Canonical sources | 870 | 870 |
| Canonical postBatches | 26 | 28 |
| Send + receipt monitor | 152.930 s | 206.023 s |
| Preparation mean | 453.29 ms | 346.04 ms |
| Preparation median | 456.44 ms | 358.07 ms |
| Preparation maximum | 672.87 ms | 683.65 ms |
| Accepted-source lowering/hash/append mean | 198.81 ms | 94.95 ms |
| Blob sidecar mean | 172.31 ms | 181.79 ms |
| Witness + remaining proof-request mean | 22.50 ms | 19.91 ms |
| Skipped attempted target slots | 3 | 7 |
| Produced target blocks without the acknowledged candidate | 2 | 6 |
| Backoff-limited canonical posts | 2 | 6 |

Mean preparation fell 23.7%, median 21.6%; the accepted-source interval's total
across the same 870 sources fell 48.6%. New timing attributes 85.89 ms/post to
prefix construction. This overlaps the source intervals, including work later
deferred by timing, and must not be added to their total.
Blob construction is now 52.5% of mean preparation. Maximum preparation did not
improve: the largest blob interval was 353.01 ms. All posts used one blob.
Proof mean was 104.35 ms and finalization mean 122.14 ms, outside preparation.

Overall throughput worsened in this run. The 41 attempts contain 28 canonical
posts, seven skipped target slots and six produced targets omitting the
acknowledged candidate. Baseline had 31 attempts / 26 posts / three skipped
targets / two produced omissions. The candidate reaccepted 458 sources in
noncanonical attempts versus 174 previously. The observations establish the
omissions, not their builder-side cause. No proof or preflight failure was
recorded; all 41 attempts were acknowledged. Comparing one wave per revision
does not isolate code from slot production, inclusion, ordering, chain state or
changing gas limits. This is evidence for reduced preparation work, not a claim
of improved end-to-end throughput.

Reproduce target classification with [classify-targets.py](classify-targets.py).
See [candidate target outcomes](candidate/targets.json),
[preparation intervals](candidate/preparation-summary.json),
[per-post gas and timing](candidate/admission-vs-receipt.csv) and
[canonical stability](candidate/canonical-stability.json). The repeat canonical
join retained all 870 source assignments and all 28 posts; every posting receipt
also matched the external L1 RPC. 140 initial speculative L2 receipt heights
moved during reconciliation; final attribution uses canonical correlations.

### Measurement-window correction

The first attribution check failed for 36 sources in two early outbound-only
posts. Its old L1 scan started five blocks before the first inbound receipt and
missed both posts. The corrected [measurement helper](../2026-09-26-admission-throughput/measure-wave.py)
scans from the bulk-send timestamp through the observed canonical head. It does
not drop unmatched sources. The candidate then attributes all 870 sources; a
repeat baseline scan produces byte-identical source and post CSVs. Original
failures are retained locally as `candidate/measure-canonical-{first,diagnostic}.log`.

## Cutoffs and gas

| Final admission reason | Canonical posts | Sources |
| --- | ---: | ---: |
| `l1_bundle_gas_budget` | 14 | 485 |
| `l1_tx_gas_budget` | 6 | 222 |
| `submission_backoff` | 6 | 105 |
| `pipeline_time` | 1 | 51 |
| `snapshot_exhausted` | 1 | 7 |

No FIFO cut, calibration post or nominal preparation overrun occurred. The live
nominal preparation setting remains 1,000 ms. Gas estimator and packing policy
remain `companion-storage-6` / `packing-minimum-check-3`. Cheap checks avoided
9,293 simulations; 4,016 complete simulations remained, including 2,688 full
gas refusals. More retries explain the increased total work. Six canonical
candidates accepted seven independent sources after a capacity refusal.

PostBatch admission estimated 238,293,854 gas versus 176,895,640 receipt gas
(1.347×). Final preflight used gas matched all receipts exactly; required gas
fit every signed limit. Companion estimates totaled 146,691,338 versus
73,319,028 receipt gas (2.001×). Multi-call nested/twice remain about
2.670× / 2.688×. These prices were not changed in this preparation iteration.

Parent budgets were 17.539–17.920 million gas. Bundle-limited candidates used
98.99% of their estimated budget; actual ordered gas consumption was 49.69%
and the actual signed-start envelope 54.55%. This supports more work on reserve
accuracy; it does not authorize replacing a sequential signed-start constraint
with a sum of receipt gas. [Budget evidence](candidate/bundle-budget-review.json)
and [companion breakdown](candidate/companion-breakdown.csv) retain the detail.

## Monitoring correction and checks

Grafana's preparation table returns all 41 attempts with non-null prefix time,
executed-prefix transaction count, blob time and witness time. Historical rows
remain nullable. No new event kind or schema migration was needed.

Receipt enrichment stalled as the old query repeatedly aggregated the full
admission dashboard views. PostgreSQL cancelled it at its 10-second worker
limit; an unbounded diagnostic also timed out at 60 seconds. This left 357
sources without optional enrichment and 50 reverted-call gas fields blank.
The new query starts at canonical posting links and reads only those candidates'
source events. It retains latest-estimate / earliest-selection direction
precedence, accepted-only selection, the existing 32-source/four-RPC bounds,
block/position rechecks and foreign-key reorg protection.

The new full-history query measured 264.284 ms. Old and new queries selected the
same 357 pending wave records in one read-only snapshot, with no missing or extra
rows. Full-history equivalence could not be executed because the old query timed
out; focused database fixtures cover direction precedence, rejected work,
repricing, missing/mismatched receipts, canonical rechecks and deletion on reorg.
See [query evidence](receipt-query.json) and [indexer checks](indexer-checks.json).

The [local indexer build](indexer-build.json) and [image-only rollout](monitoring-rollout.json)
retained all 124,183 admission events, 14 file checkpoints, mounts, environment
and command. All 520 inbound sources then reached `available` coverage and their
gas values match independently fetched canonical receipts. Actual Grafana
source-filter queries were verified for reverted, nested multi-call and counter
families; see [source checks](candidate/grafana-source-checks.json). The node and
signer were not restarted for this monitoring fix.

## Completed validation

| Check | Result |
| --- | --- |
| Composer unit tests | 267 passed; one pre-existing manual diagnostic ignored |
| Focused process gate before deployment | Five passed once |
| Broader process gate after measurement | 72 passed once across nine targets; zero failures/ignored cases |
| Standalone indexer | 24 default tests plus both opt-in PostgreSQL tests passed |
| Tooling | 259 passed; four opt-in infrastructure checks skipped |
| Formatting and Clippy | Workspace and standalone indexer checks passed, warnings denied |
| Live semantic workload | 770 jobs passed; 870 canonical sources; zero failures/not-verifiable jobs |
| Canonical stability | 870 source assignments and 28 posts unchanged; all 28 external receipts matched |
| Grafana | All 41 preparation rows complete; all 520 inbound receipt observations available |

[Native results](broad-native-results.json) and
[executable identities](broad-native-executables.json) retain exact tests,
commands, source revision and log digest. The [plan](broad-native-plan.json)
names the excluded ordinary-only settlement cases; five focused cases were
already validated and were not repeated. There is no separate `resumption`
target at this revision; middle-group and restart cases are in
`partial_consumption`. No selected native case required a retry. Runtime files
remain byte-identical to `6c171fd`; runner and indexer commits do not change them.

The tooling skips are the opt-in Docker payload, isolated Kurtosis resources,
native artifact relocation and Docker runtime fixture lanes. Full release/CI
and Kurtosis matrices were not run locally. Protocol/ABI/codecs were unchanged;
this iteration did not rerun the full core Foundry or protocol Anvil matrix.
The original static/type-check failures, readiness false rejection, restart-gap
fetch failure and initial measurement-window failure remain in local evidence;
passing final checks do not erase those records.

## Remaining opportunities and limits

1. Overlap final KZG sidecar construction with witness/prover work, using bounded
   worker ownership and one shared deadline. Blob count estimation already
   avoids KZG; it cannot replace cryptographic sidecar generation or validation.
2. Investigate produced-block bundle omissions and refine backoff using conclusive
   evidence. A skipped slot or relay acknowledgment does not establish oversized
   work or canonical consumption.
3. Continue measuring multi-call protocol reserves; the unchanged estimator still
   reserves roughly twice actual companion gas overall.

Full-wave independent follower replay was not run on Chiado. Native process
tests supply independent replay coverage; live semantic assertions are separate.
Pending queues remain in memory. This run does not establish durable recovery,
full CI/Kurtosis coverage, or performance under other workloads. Raw logs,
datadirs and per-source attempt history remain in ignored
`artifacts/admission-preparation/`; compact sanitized records here are committed.
The task-owned ordinary RPC forwarder and disposable PostgreSQL fixture were
stopped after use. [Final live health](health-final.json) verifies exact deployed
images, original genesis, fresh canonical heads and an empty queue.
