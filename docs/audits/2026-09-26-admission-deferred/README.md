# Deferred posting storage estimates

| Field | Value |
| --- | --- |
| Status | Implemented, locally validated, deployed and measured; all 770 live semantic checks passed; observed throughput was lower |
| Authorization | User approved inbound storage/overwrite estimates and fallback telemetry; existing authorization includes local build, deployment and measurement |
| Branch / baseline | `feat/cross-chain-admission-throughput`, `bfcb68dce74b2196bff1df4a648ee0dc127f469d`; runtime baseline `036b50a3c7d0dee45e6308388a6233ded7a7d2ca` |
| Protocol | Clean `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Implementation | `85f3691cb9c7ccbf8c0b8700576733283e5af7fc`; subsequent changes are documentation/evidence only |
| Later rerun | [Requested repeat](rerun/README.md) failed full completion: 853/870 sources canonical, 753 job semantic passes and 17 missing-receipt failures after retry exhaustion. The first wave below remains its original successful result. |
| Tools | Rust 1.94.0, locked dependencies, Foundry 1.7.1, Solidity 0.8.34 |
| Contract | [Proposal](../../proposals/cross-chain-admission-throughput.md), [settlement policy](../../configurable-settlement-policy.md), [monitoring](../../../monitoring/indexer/README.md#admission-history) |
| Prior evidence | [Baseline live wave](../2026-09-26-admission-shapes/README.md) |

## Implementation

The `deferred-storage-3` model bounds current and retained entry layouts before
walking them. It prices storage writes, nonzero fresh words and deleted tails;
refunds never reduce admission. Canonical-parent evidence includes inactive
mapping entries. No speculative writes earn reuse credit. Roots, rolling hashes
and nested position hashes are charged as nonzero before stitching. The previous
reserve remains available for unsupported or more expensive layouts. Immediate
pricing, whole-group ownership and final native validation remain unchanged.

Grafana exposes per-entry pricing-reason counts and a separate whole-prefix
discount revocation flag. New reasons are additive; old history remains null.
Keep upgraded monitoring views/indexer when rolling back Composer. No protocol,
signer profile, configuration, genesis or queue persistence changes are made.

Pinned CORE/EXECUTION_ENTRY prose still describes arrays/deletion; Solidity uses
retained mappings and active bounds. This is a documented source/specification
disagreement. Measurements and storage-layout assertions use the pinned Solidity
and existing fork settlement contract.

## Validation history

Full logs and datadirs are local-only under ignored `artifacts/admission-deferred/`;
they are not available from another checkout. Small final summaries will be
retained here with the implementation/image identities.

- Baseline pinned gas fixture: 34 passed (`gas-baseline.log`).
- Final Composer suite: 251 passed, one existing manual profile ignored (`composer-final.log`).
- New/old admission history and all dashboard queries: two PostgreSQL tests
  passed, zero ignored (`indexer-db-first.log`). The task-owned disposable
  container is `eez-admission-deferred-db`, port 19433; production is separate.
- Final workspace and indexer Clippy passed with `-D warnings` (`clippy-final.log`,
  `indexer-clippy.log`). Workspace/indexer Rust formatting and the changed Solidity
  fixture pass. The repository-wide Solidity formatting check still fails for
  unchanged baseline files `ECDSAProofSystem.sol` and `DeployRollup.s.sol`; they
  are excluded from the formatting pass, not silently reformatted.
- Final pinned gas fixture: 39 passed on each of Cancun, Prague and Osaka
  (`gas-final4-*.log`); all integration contracts: 58 passed (`contracts-final.log`).
  The five new ladders provide 95 cold caller-frame measurements before refunds,
  including 1/10/127-entry batches, zero payloads, short/long bytes boundaries,
  retained inactive slots, shrinking and expanding layouts. All fit the storage
  formula; the tightest measurement uses 92.103% of its reserve.
  [Measurements](gas-ladders.json) retain the small numeric evidence.
- Explicit old-view to additive-new-view PostgreSQL migration retained seven test
  history rows ([migration](indexer-migration.json)). Production monitoring was
  upgraded first and preserved 8,753 admission events and two file checkpoints
  ([rollout](monitoring-rollout.json)).
- All four local shipping images were built from the exact implementation commit
  and protocol pin; labels, executable hashes and the existing genesis matched
  ([image identities](images.json)). These are local builds, not CI-promoted artifacts.
- Initial storage ladders passed three shapes; two maximum-size ladders exhausted
  Foundry's test transaction budget in fixture preparation. Setup/assertion work
  is now outside gas metering; the actual posting call remains metered and cold.
  All five ladders then passed. A later zero-payload extension exposed Solidity
  stack-allocation failures. Grouping fixture state in a memory struct resolved
  them; the resulting Cancun fixture passed all 39 tests. First failures remain
  in `gas-storage-first.log`, `gas-final*-*.log`.
- Initial Rust test compilation required explicit ABI struct fields; its first
  numeric fixture expectation omitted one fresh word and was corrected from
  343,000 to 358,000. These are test corrections, not passing initial results.

All 31 selected native process cases passed with zero failures/ignored tests:
21 dynamic-admission, two settlement-cost, four settlement-gas, two targeted
settlement-policy and two targeted cross-chain cases. They include semantic
assertions and independent follower replay. [Exact selections and log hashes](process-validation.json)
identify this run; [static/embedded checks](check-evidence.json) preserve its other results.

The node/signer image-only rollout completed at 15:05 UTC with a clean node stop,
empty queue, unchanged mounts and environment ([rollout](runtime-rollout.json)).
Its first health check failed because the existing zero-execution-peer setup
left a 17-block gap. The bounded recovery verified canonical raw bodies and
imported all 17 through `engine_newPayloadV4`, each returning `VALID`. Lighthouse
retained fork choice; no datadir reset or contract/genesis changes occurred.
Both heads became current and nonsyncing ([recovery](catch-up.json)).

## Measured wave

The task-only profile retained 770 jobs, 870 sources, 200 preparation workers,
28 send workers and four semantic verifiers. The first `.005` worker-prefund
readiness check found a 0.328336 xDAI shortfall. A second check using `.0045`
prefunding/floor passed with existing funds. No additional poster-to-workload
transfer occurred; gas limits, source mix and concurrency were unchanged. The
operator profile was preserved. This setup difference is recorded in the
[comparison](comparison.json).

All 2,850 ordinary setup transactions completed before the measured source
window. A task-owned loopback helper used local reads and forwarded identical
ordinary L1 raw payloads at five upstream requests/second, matching the preceding
wave's workaround for zero execution peers. Cross-chain ingress was unchanged.
Private profiles and bulk logs stay local-only. The initial readiness failure
is retained and is not reported as a passing first attempt.

All **870 sources** (520 inbound, 350 outbound) map to exactly one canonical
postBatch across L1 blocks **23,240,328–23,240,359**. Attribution corrects 112
initial L2 source heights after reconciliation. All have accepted estimates, complete event sequences and populated
reason counts summing to their entry counts. The later attribution check found
all sources unchanged and independently matched all 28 postBatch receipts
through the configured external RPC ([stability](canonical-stability.json)).
See the [estimate-versus-receipt table](candidate-admission-postings.csv),
[canonical summary](candidate-summary.json) and [coverage](admission-coverage.json).
All **770 job-level semantic checks passed**, zero failed, and the staged runner
exited successfully ([semantic results](semantic-verification.json)).

| Metric | Baseline `036b50a` | Candidate `85f3691` |
| --- | ---: | ---: |
| Sources | 870 | 870 |
| Send + settlement-monitor window | 147.557 s | 191.278 s |
| Canonical sources / second | 5.896 | 4.548 |
| Postings carrying sources | 25 | 28 |
| Admission gas, source-carrying postings | 328,380,722 | 275,770,290 |
| Receipt gas, source-carrying postings | 177,949,152 | 177,916,082 |
| Weighted admission / receipt gas | 1.845× | 1.550× |
| Admission / required gas, postings containing inbound work | 2.784× | 1.844× |
| Admission / required gas, outbound-only postings | 1.199× | 1.196× |
| Skipped slots between first/last source posting | 3 | 5 |
| Produced blocks between first/last source posting | 26 | 32 |

The new storage model priced **543 of 670 deferred entries** below their previous
reserve. The remaining deferred entries used `deferred_old_slot` (43),
`general_reserve_lower` (20), `deferred_reverted` (40) or `deferred_call_flags` (24).
Immediate pricing reasons were 50 registered and 300 unavailable/unsupported.
No settled source revoked the batch discount. Counts describe entry pricing,
not unique sources or exhaustive eligibility diagnostics.

Observed mean entry reserves per source fell from 370,000 to 195,800 for
`nestedCounter`, and from 967,200 to 477,220 for `multi-call-nested`.
[Per-family distributions](family-reserves.json) preserve the varying slot states;
these means are not claims about identical storage-state execution gas. The
pinned unit/fixture comparison separately isolates the storage formula.

### Throughput limits and negative result

**This wave did not demonstrate higher throughput.** Its measured source window
was 29.6% longer despite lower gas overestimation. This is one sequential wave
per revision, with different retained storage, source interleaving, ordinary
history backlog, slots and host/network conditions. No isolated causal
throughput claim is established. Receipt execution gas was nearly unchanged,
as expected for an admission-only change.

| Canonical admission stop reason | Baseline | Candidate |
| --- | ---: | ---: |
| L1 transaction gas | 13 | 5 |
| Whole-bundle gas | 3 | 2 |
| Pipeline time | 5 | 11 |
| FIFO fairness | 2 | 7 |
| Submission backoff | 1 | 1 |
| Eligible sources exhausted | 0 | 1 |
| Snapshot exhausted | 1 | 1 |

Gas limits stopped 7/28 canonical candidates, versus 16/25 before. Timing and
fairness now stopped 18/28. Median preparation rose from 440 to 547.5 ms;
maximum preparation was 791 ms and maximum proof time 205.72 ms. Median time to
the first source observation rose only from 4.36 to 5.82 ms; this includes model
opening but is not a dedicated estimator benchmark. Median admission-summary
time rose from 232.75 to 255.62 ms and subsequent preparation from 203.91 to
222.12 ms ([timing breakdown](timing-comparison.json)). Different source groups
and finishing work prevent attributing these changes to the estimator alone.

The first 36-source candidate required 265,916 encoded bytes, exceeding the
253,952-byte usable capacity of two blobs. The existing historical-predecessor
path posted 215,437 bytes of accumulated ordinary history first: 115 blocks and
493 ordinary transactions, with zero cross-chain sources, at L1 block 23,240,327.
Its receipt used 109,117 gas. This extra posting is excluded from both cohorts'
source-carrying posting totals, while its delay is inside this wave's source
window. It was a capacity prerequisite, not a nominal preparation timeout or
mandatory cold one-source batch ([evidence](initial-predecessor.json)).

All 37 admission attempts have complete event sequences; 28 became canonical
source postings. The initial candidate became the historical-only predecessor,
one examined no eligible source, and seven submitted attempts have no indexed
commitment. One replacement's delivery timed out: 65.09 ms elapsed with 64 ms
remaining, and it did not become canonical. These are retained failures, not
successful acknowledgements. No local evidence identifies an external builder's
reason for every uncommitted attempt.

Every canonical candidate's accepted count matches its canonical sources,
admission gas exceeds preflight required gas, and every bundle transaction's
required gas fits its signed limit. All canonical acknowledgements preceded the
deadline; the minimum headroom was 41.29 ms ([invariants](admission-invariants.json)).
No canonical preparation overran the nominal 1,000 ms allowance, so this wave
does not independently exercise soft-budget borrowing.

### Follow-up opportunities

- Investigate finishing/KZG/witness work and the pipeline predictor against
  comparable source shapes; timing now stops more candidates than gas.
- Examine FIFO fairness caps without weakening sender/direction dependencies or
  starving inbound work. An estimator improvement cannot bypass an explicit cap.
- Account for unsettled historical bytes earlier when considering source groups,
  while retaining any genuinely necessary canonical predecessor. The initial
  admission summary currently records its gas stop but does not itself expose
  the later capacity substitution; lifecycle linkage is a separate visibility gap.
- Compare repeated, controlled baseline/candidate waves before claiming higher
  throughput. Extra slot skips and ordinary-history settlement affected this run.

## Dashboard, final state and limits

The live Grafana browser check rendered all seven panels, executed seven queries,
and found zero browser/SQL errors. Both new fields were present and populated,
with 1,107 source-estimate rows in the selected window
([browser evidence](grafana-verification.json)). The canonical cohort join checks
all 870 sources independently of dashboard row limits or filters.

Final [deployment checks](deployment-final.json) confirm the expected image IDs,
healthy signer, current nonsyncing heads, original genesis, empty queue and
configuration unchanged except for the node/signer image references. Runtime and
monitoring remain deployed. No shipping code changed after the image build; this
subsequent commit records documentation and evidence only.

The task-owned disposable database was [removed](test-db-cleanup.json). The
forwarding helper was stopped after verifying its PID and resolved script path,
and its loopback port is closed. It forwarded 1,395 identical ordinary raw
transactions with zero upstream rate-limit responses and recorded 2,463
`BrokenPipeError` client disconnects. Those errors are not counted as successful
requests ([final counters](setup-forwarding.json)); no individual error payloads
are committed. The task-only workload profile points to the stopped helper and
is not a reusable operator configuration. Datadirs, retained logs, indexer
checkpoints and unrelated operator edits are preserved.

Full CI release/Kurtosis and the unchanged core-protocol suite are not rerun. Native tests include independent follower
replay; the live staged verifier itself does not run an independent follower.
Queue ownership remains in memory; monitoring persistence adds no durable queue
recovery guarantee.
