# Cross-chain throughput, shared deadlines and admission visibility

| Field | Value |
| --- | --- |
| Design status | Implementation authorized by the user on 2026-09-26; in progress on `feat/cross-chain-admission-throughput`. Current runtime contract is documented in the settlement/RPC specifications. Validation and deployment status are tracked in the September 26 audit. |
| Source baseline | `main`, `7c90266a5fe8cfdb46b249c61079badc2dce71a3` |
| Protocol baseline | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Existing local work | Chiado deployment runbook edits and the September 25 recovery/wave audit directories; preserve them. |
| Related contracts | [Settlement policy](../configurable-settlement-policy.md), [original proposal](configurable-settlement-policy.md), [RPC failure handling](../../crates/eez-control-rpc/SPEC.md#6-failure-handling), [posting history](posting-lifecycle-history.md), [replacement design](skipped-slot-speculative-retry.md) |
| Evidence | [Latest 870-source wave](../audits/2026-09-25-latest-wave-analysis/README.md) and its per-attempt CSV |

## 1. Objective and evidence

Maximize canonically completed cross-chain sources per produced L1 block and
per second, while preserving source ordering, whole groups, complete effects,
network limits and the target's submission deadline. Measure oldest request
age and p50/p95 completion latency alongside throughput so larger batches do
not hide starvation. Count unique canonical sources independently of retries;
show static-only completion separately from effectful delivery.

The latest wave completed 870 sources using 29 successful postings and 36
attempts. Preparation took 336 ms median / 559 ms maximum; proof round trips
took 84 / 116 ms. Six absent slots and two produced blocks without postings
offset the four-post reduction relative to the earlier run. These measurements
support improving packing and retry behavior, but do not demonstrate a current
preparation bottleneck.

Only 21 successful postings logged a final admission estimate because the
current total is emitted on gas cuts. Across those 21, estimated postBatch gas
was 1.93 times receipt gas. This is a selected subset, not the full wave's error
distribution. Refunds, declared-gas headroom, shared storage and batch shape
must be considered before turning that difference into lower admission limits.

The requested changes are:

- Remove the mandatory one-source cold-start batch in `auto` mode.
- Treat preparation time as a planning allowance; permit preparation and proof
  to share the available slot time. Crossing the preparation allowance alone
  must not trigger an empty Sync, discard a fitting candidate or reduce its
  next selection.
- Record estimates for every attempt and every examined source group, including
  successful admission, so Grafana can compare them with simulation and receipts.
- Improve conservative gas packing and replacement-related admission learning.

## 2. One deadline for the complete pipeline

For an exact target timestamp `T` and configured relay slack `S`, define the
exclusive deadline `D = T - S`. Preparation, proving, proof validation, fee
refresh, signing, ordered final preflight and relay delivery must fit before D.
Record actual local dispatch and relay acknowledgement separately: the current
`dispatched` event precedes the network request and cannot prove remote receipt.

Keep the existing trigger initially: with preparation/proof/slack settings
1000/450/450 ms, work starts near `T - 1900 ms` and has about 1450 ms to reach D.
The 1000 and 450 ms values supply initial planning allocations for exact-target
cross-chain work. They no longer create independent hard stage cutoffs there.
Explicit numeric user-count mode still limits source count; it uses the same
target deadline. Preserve `NextBlock` and ordinary-only behavior initially.

### Admission and proving rules

1. Estimate remaining group work, preparation finishing, proof round trip,
   finalization and delivery, with a measured variation allowance. Stop adding
   whole groups when the complete pipeline is predicted to miss D. Use separate
   finishing/KZG and proof measurements; fixed work should not scale with every
   additional source.
2. Crossing the preparation allocation is an observation. Keep the accepted
   candidate and continue to proving when usable time remains. Do not require
   the entire configured 450 ms proof allowance to remain before starting.
3. Give the proving operation the remaining absolute time up to D minus the
   reserved post-proof work. A fast proof can rescue a preparation overrun, and
   a fast preparation can leave more time for a slow proof. The configured proof
   planning value is not a second independent timeout for this path.
4. Propagate one absolute deadline through retries, repair calls and finalization;
   retries never reset it. Preserve typed retry classification and bounded
   backoff. If no viable retry fits, stop. Never run overlapping proofs to race
   the deadline. An expired RPC may leave a signer blocking worker holding its
   permit until it exits; preserve the [request lifecycle](../../crates/eez-proof-signer/docs/request-lifecycle.md).
5. Recheck the exact target, canonical parent and deadline before delivery. A
   completed proof cannot authorize an unvalidated or late bundle. Delivery
   timeout after sending is uncertain submission and retains ownership/history;
   it is not a preparation failure or evidence that sources can be released.
6. If the whole window expires before a valid bundle can be delivered, retain
   sources for the next opportunity and use the existing empty local Sync
   fallback where required. No unproved L1 posting is sent. Timing-based count
   reduction is tied to this full-window failure, not to crossing preparation's
   nominal allocation. Infrastructure uncertainty does not train a smaller
   workload as if it proved insufficient compute capacity.

For example, preparation 1200 ms + proof 100 ms + finalization 100 ms + delivery
20 ms = 1420 ms fits a 1450 ms window and should be submitted unchanged. This is
a deterministic test case, not a claim about measured relay latency.

Keep wall-clock target checks and monotonic elapsed measurements consistent.
Derive a monotonic deadline once per target, and never extend the permitted
window after a clock correction. Synchronous work remains cooperative: an
overrunning work unit can finish late, but no late dispatch is authorized.

### Production paths that must change together

| Path | Required change |
| --- | --- |
| `eez-driver/src/timing.rs`, `sequencer.rs`, `slot.rs` | Shared deadline, trigger semantics and late-entry checks; no stale fixed-full-proof gate for eligible cross-chain work. Preserve block grid and replacement bounds. |
| `eez-composer/src/composer/admission.rs` | Pipeline estimates, cold-start selection, observation of preparation overruns and cause-specific backoff. |
| `eez-composer/src/composer.rs`, `prover_retry.rs` | Remove the early proof-start rejection for this path; propagate remaining time and retain owned groups on real expiry. |
| `eez-composer/src/composer/posting.rs`, `eez-l1/src/submitter.rs` | Account for both preflight passes, signing and transport; preserve final target/fee/effect validation and uncertain-send handling. |
| `eez-prover-client`, signer integration | Trace all RPC/stream/cache-repair limits; verify cancellation and busy-worker behavior. Do not relax signer validation or permit ownership. |

This explicitly changes the current full-proof-reservation and per-attempt timeout
contract in RPC specification section 6.3. Update that specification and the
settlement/timing configuration docs with the implementation; existing wording
does not already permit the proposed behavior everywhere.

## 3. Remove the mandatory warm-up posting

Replace `sample == None => selected = 1` with a finite eligible snapshot and
incremental complete-group admission. Use conservative initial finishing and
proof estimates; refine estimates while processing the first candidate and
after each completed attempt. The first admitted group remains in that batch
while more fitting groups are added.

Calibrate initial reserves with repeated cold-process benchmarks for supported
profiles before merging. Include cold KZG initialization, large witness ranges,
blob transitions and a busy host. Select a documented upper-tail reserve with
headroom; the latest warm wave's 159 ms median blob time is insufficient as a
cold-start default. Unrecognized conditions use conservative bootstrap estimates
and the shared absolute deadline. Mark every sample's bootstrap/calibrated state.

Retain bounded rolling observations by relevant workload dimensions: physical
blob count/bytes, proof witness size and number/shape of admitted groups. Limit
the number of classes and samples; never create per-address estimators. Do not
scale proof cost solely by group count or assume linear KZG cost across blobs.
Reuse existing admission state and Reth task ownership; do not introduce a
second request scheduler or speculative branch.

First iteration keeps calibration in memory. Persisting measurements across
restarts is a later option requiring source/protocol/prover/configuration identity,
expiry and invalidation rules. Removing the forced one-source cap does not need
that extra storage compatibility work.

## 4. Gas packing and retry learning

### Explain and tighten the gas model

Reuse the existing `TxL1Gas`, `PostBatchGasBudget` and selected
`trace_admission_budget` calculations. Expose their terms, then profile the
largest discrepancies by direction, immediate/deferred/static/nested shape,
return-data size, parent queue contents and existing/new proxies.

Distinguish five values: incremental admission projection, final full-calldata
projection, preflight execution/required gas, signed gas limit, and canonical
receipt gas. Receipt refunds and the EIP-7623 floor mean these values are not
interchangeable. Do not apply the observed 1.93 factor as a blanket discount.

Prioritize measured reserve improvements for the shapes causing 3–5x gaps,
batch storage reuse, proof/framing overhead and the 128-entry discount boundary.
Record any batch-wide repricing when crossing a discount boundary; a group's
delta can include revocation of discounts on previous entries. It is not always
equal to that group's standalone cost.

After validated estimator improvements, evaluate a bounded exact-simulation
extension when a planning cut leaves headroom: checkpoint the valid prefix,
try an additional whole group, and validate its ordered execution with enough
time reserved for proving. Cap extra work by the shared deadline and a finite
probe budget; failed extension retains the earlier valid prefix. Do not rerun
full-batch simulation for every source without measuring the cost. This is a
separate measured optimization, not a dependency for removing the warm-up.
Carry the validated sizing result through final full-calldata sizing so the
earlier conservative projection does not simply reject the extension again;
network, declared-gas, complete-effect and signed-bundle checks still bind.

### Avoid learning network misses as batch-size failures

Use explicit original/replacement identities and canonical group evidence.
When a skipped old attempt overlaps a successfully settled replacement, update
remaining source ownership and record replacement completion without halving
capacity merely because the old manifest now has mixed outcomes.

Preserve conservative behavior for actual partial consumption, incompatible
effects, nonce dependencies and inconclusive state. An empty produced block
alone does not prove a bundle was too large. Separate full-window compute
failures, deterministic capacity failures, relay uncertainty, skipped slots and
source deferrals in both backoff decisions and Grafana.

Start by narrowing false failure attribution. For confirmed full-window compute
failures, retain bounded next-attempt backoff; a subsequent measured improvement
can calculate a fitting group count from fixed plus variable work instead of
always halving. Maintain sender/direction nonce order, group atomicity and the
existing prevention of outbound work repeatedly bypassing older inbound work.

## 5. Complete admission history in Grafana

“Per transaction” includes each postBatch and each source transaction examined
for that batch. A source with several nested calls remains one complete group;
call/entry counts explain its cost. Repeated attempts retain separate records.

### Data to record

| Level | Required fields |
| --- | --- |
| Candidate/attempt | Session/rollup identity, explicit candidate/rebuild/replacement links, exact target and canonical parent, source/protocol/estimator/config identity, bootstrap/calibrated state. |
| Admission totals, every path | Queued/eligible/examined/accepted/deferred counts; final prefix projection and its component sums; configured margin, fixed reserve, network/operator caps; companion estimates and ordered declared headroom; blob bytes/count; terminal stop reason. |
| Per examined source | Source hash, chain/direction, group ordinal, decision and reason; entry pin/discount/calldata/target terms; cumulative projection before/after and batch adjustment; source simulation gas, companion estimate and declared gas; simulation/admission time. |
| Pipeline | Actual and predicted admission/finishing/KZG/proof/finalization/delivery times; absolute deadline and remaining time; nominal preparation overrun, full-window failure, retry and backoff cause. |
| Final execution | Full-calldata estimate, preflight used/required gas, declared gas, final repriced post hash, companion gas, canonical block/hash and receipt gas, final source disposition and measurement coverage. |

Always emit totals when selected work fits, at queue exhaustion and on failure;
the next complete table must not depend on a gas-cut event. Record unexamined
suffix counts without inventing per-source estimates. Report attempted work on
rejected groups separately from cost committed to the accepted prefix.

Source receipts give exact source-chain transaction gas, and the postBatch
receipt gives exact aggregate L1 posting gas. There is no independent receipt
for each source's share inside postBatch. Use labeled simulation attribution
where available and leave shared overhead explicit. Do not call an allocation
of refunds/shared work “actual receipt gas per source.” Selected detailed EVM
traces can explain difficult outliers without enabling full tracing for all traffic.

### Storage and dashboards

Extend the existing nonblocking tracing -> retained JSON -> indexer/PostgreSQL
-> Grafana path. Reuse typed gas data without enabling the selected trace
writer's filesystem operations on every transaction. Allocate a candidate ID
before composition and link it explicitly to the later posting attempt and
repriced hash. Neither timestamps nor Sync height alone establish a join.

Use a versioned `eez.composer.admission.v1` event family on the existing retained
logging target, with bounded per-source records plus a final summary. Extend
the existing collector with typed validation, per-candidate sequencing and
atomic event/checkpoint import into additive admission tables. Preserve the
current posting event schema and sequence meanings. Existing collectors ignore
the new family; upgraded collectors link it using additive posting detail fields.
Keep each event within the current 64 KiB line bound; never serialize an entire
large wave into one array. Include expected record counts so losses are visible.

Extend the Prometheus allowlist for bounded aggregate counters/histograms:
accepted sources, stop causes, deadline outcomes, phase timings, estimator error
and telemetry drops. Labels are bounded enums and rollup/profile identity;
transaction hashes, candidate IDs and addresses belong in PostgreSQL. Emit event
counts where appropriate rather than inferring throughput from sampled gauges.

| Grafana view | Purpose |
| --- | --- |
| Throughput and opportunity use | Canonical unique sources/sec, sources/post, completion latency/oldest age, skipped slots, empty produced blocks and included targets. |
| Admission vs execution table | Every candidate/post, estimates, preflight, declaration, receipts, ratio and stop reason; filter by wave/time/rollup and export CSV. |
| Source drill-down | Every examined source's cost terms, accepted/rejected decision, cumulative headroom, measured simulation and source receipt. |
| Target-relative timeline | Preparation, proof, finalization and delivery against D; distinguish preparation overruns rescued by proof from real deadline misses. |
| Packing constraints | L1 transaction/bundle gas, L2 gas, blob capacity, dependency waits, timing forecasts, actual deadline failures and capacity backoff. |
| Estimator quality | Error by shape/direction/batch size, upper-tail error, bootstrap confidence and the number/percentage of rows with comparable measurements. |

Reorgs invalidate canonical comparisons but retain attempt history. Excluded,
unproven and missing-receipt attempts show unavailable actuals, never zero.
Static-only source completion and old-manifest missing-effects labels retain
their meaning. Telemetry failure must not block settlement: bound buffering,
surface dropped/incomplete history, and measure enabled-vs-disabled overhead.
Detailed per-source records are unsampled when enabled; retention/rotation limits
and any loss remain explicit. This remains operational history, not durable queue
ownership or a transactional no-loss journal.

## 6. Implementation order

| Phase | Deliverable | Exit criterion |
| --- | --- | --- |
| 1. Visibility | Unconditional totals, per-source records, identity links, collector/schema and Grafana panels. Update the monitoring contract with event fields and denominators. | Every recorded test candidate has explainable totals; included/excluded/rebuilt cases join correctly; no missing estimates solely because all selected sources fit. |
| 2. Shared deadline | Update the timing/RPC contract and all production gates; allow preparation/proof borrowing, preserve post-proof time and ownership. | A candidate exceeding preparation's allocation but meeting D posts unchanged; a true expiry never dispatches late and retains sources. |
| 3. First-batch throughput | Remove forced one-group selection and calibrate conservative bootstrap reserves. | A known fitting multi-source workload posts in one batch from a fresh process on both profiles; no mandatory warm-up posting. |
| 4. Packing and learning | Narrow replacement backoff; measure and tighten dominant gas reserves; assess bounded simulation extension separately. | Proven replacement overlap does not impose capacity backoff; real partial-consumption tests still pass; gas gains preserve final native validation. |
| 5. Comparative validation | Run baseline and changes incrementally under comparable workloads and publish measured throughput, latency, misses and overhead. | Correctness gates pass; cold/warm throughput improves without unexplained deadline/latency regressions. |

Phases can be separate reviewable changes. Phase 1 supplies evidence before
estimator changes; phase 2 precedes warm-up removal so cold estimates share the
correct window.

## 7. Acceptance and validation

| ID | Required check |
| --- | --- |
| A1 | Fresh-process, multi-source workload whose complete native bundle fits: more than one source and the complete workload in its first batch; repeat warm with the same semantics. |
| A2 | Controlled 1200/100/100/20 ms stage durations in a 1450 ms window: same accepted sources, proof invoked, no empty fallback or preparation-only backoff. Test just-before/equal/after the exclusive deadline. |
| A3 | Fast preparation / slow proof fits total window even if proof exceeds its planning allocation. Genuine total expiry retains all sources, charges no unsubmitted settlement failure and never dispatches late. |
| A4 | Retry waits, cache repair, delayed stream, finalization overrun, clock movement and late scheduler trigger all honor the same target deadline. Timed-out signer workers retain permits; no overlapping worker leak. |
| A5 | Gas/blob/L2 bounds, multi-blob transitions, 128-entry discount changes, cold proxies, nested/static/reverted groups, nonce dependencies and partial consumption preserve existing invariants. |
| A6 | Skipped old target with replacement consumption preserves capacity; actual partial application and late-original inclusion reconcile exactly once with correct source ownership. |
| A7 | Per-source accepted deltas plus shared/batch adjustments reproduce each summary. Every successful no-cut batch has an estimate; rejected and rebuilt candidates cannot be joined to another attempt's receipt. |
| A8 | Old logs, mixed producers, repricing, duplicates, conflicts, restart/rotation, missing records, malformed/oversized lines and L1/L2 reorgs produce correct histories and explicit gaps. |
| A9 | Execute actual dashboard SQL against a disposable database and render Grafana with fixtures. Check all panels, filters, CSV export, nulls and collector outage behavior. |
| A10 | Compare release builds against the exact baseline using the 870-source/14-target mix plus focused cold, warm, slow-proof, complex-call and blob-boundary cases. Include semantic assertions and independent follower replay. |

Use focused Composer/driver/prover-client/signer tests, then serial native
`dynamic_admission`, `settlement_policy`, `settlement_policy_gas`,
`settlement_policy_cost`, `partial_consumption`, `queue_liveness`, `queue_reorg`,
`resumption`, `cross_chain` and relevant chained-group coverage. Use locked Rust
1.94.0, Foundry 1.7.1, initialized protocol and the shipping feature selection.
Run required workspace fmt/clippy and affected configuration propagation checks.
Run the indexer's separate locked workspace tests and its opt-in recovery test
only on an owned disposable test database.

Benchmark phases separately and together using repeated baseline/candidate runs,
identical isolated starting state, build profile, worker concurrency and resource
allocation. Run cold-process and warm-process cohorts; randomize run order and
report distributions, sample counts and host load. Repeat enough to resolve
noise before attributing improvement. Measure telemetry CPU, output/storage
volume and tail latency with it enabled/disabled. Report canonical completion
throughput and observed slot availability separately; do not count proof or relay
acceptance as completed cross-chain work. The earlier `--no-verify` run supplies
performance context only, not these semantic acceptance checks.

## 8. Compatibility, rollout and rollback

No protocol ABI, commitment, native blob encoding or signer acceptance-profile
extension is planned. Canonical follower replay remains compatible. Preserve the
current protocol gitlink and dependencies. Scheduling changes affect Composer
behavior, and their config semantics must be documented explicitly.

Retain current setting names and startup validation; describe preparation and
proof as scheduling estimates in the changed exact-target path, with submission
slack authoritative. Trace defaults/absent/empty/invalid/conflicting inputs through
node, driver and all supported Compose profiles. Older Composers continue their
old stage cutoffs, so the same values have different timing behavior across
versions; identify the scheduler policy version in telemetry. Signer limits
remain authoritative and are not silently expanded.

Upgrade additive collector/schema/dashboard support first, then the Composer
producer and behavior changes after validation. Historical events keep null
estimates; do not reconstruct missing admission values from receipts. Rollback
can retain additive database tables and existing checkpoints; old collectors
ignore the separate admission family, and old Composers resume old timing rules.
No destructive migration or database reset is required.

For a future live rollout, follow the existing deployment runbooks, reconcile
pending work before replacing the node, preserve logs and source identities, and
record exact images/configuration plus first failures. Requests and calibration
remain in memory; monitoring persistence does not make queue recovery durable.
Deployment, wallet operations and live workloads are separate execution steps.

## 9. Additional opportunities after the first measurements

- Profile repeated prefix replay, witness reconstruction, duplicate encoding and
  the final two simulation passes. Reuse only immutable results tied to exact
  parent/target/batch identity; invalidate on changed input, fees where relevant,
  or reorg. Preserve final signed-bundle validation.
- Keep ordinary witness preparation and signer connections warm through existing
  cache/preparation APIs and bounded workers; measure useful cache hit rate.
- Measure relay acknowledgement and delivery lead, then investigate produced-but-
  empty targets with relay/builder evidence. Local gas headroom does not establish
  the external exclusion reason.
- Evaluate earlier adaptive triggers only after measuring the trade-off between
  preparation headroom, incoming work visibility and speculative execution.
- Increasing the number of replacements on an unchanged L1 head is a separate
  design: current one-replacement bounds and late-original recovery stay intact
  in this plan. Removing that bound requires evidence and resource/ownership tests.

Implementation and validation are recorded in the [September 26 audit](../audits/2026-09-26-admission-throughput/README.md).
The initial gas improvement discounts observed retained fixed storage words;
a bounded exact-simulation extension remains a follow-up optimization, contingent
on measurements. Skipped-slot capacity learning uses conclusive canonical header
evidence for the absent target itself; it does not need to reinterpret source
dispositions as successful replacement effects. Actual produced replacements
retain their own partial-consumption checks.
