# Cross-chain throughput, shared deadlines and admission visibility

| Field | Value |
| --- | --- |
| Design status | Strict FIFO admission and preparation refinement implemented at `b2ff3b8`, with 14 selected native cases and 286 Composer unit tests passing. Paired local workloads retain 536 sources in 13 posts per variant. The [September 28 Chiado trial](../audits/2026-09-28-chiado-strict-fifo/README.md) passed 870 canonical sources / 770 semantic checks but needed 29 posts / 186.521 seconds, versus 23 / 146.880 in the previous unpaired live run. Retained-prefix packing and two CI packing failures remain open. |
| Earlier rollout record | Implemented on `feat/cross-chain-admission-throughput`; local runtime `aa515c8` is deployed with policy `confirmed-failure-backoff-4` and estimator `companion-input-8`. All 870 sources settled in 25 source-bearing postBatches (26 total with ordinary history), versus 24 previously; all 770 semantic jobs and 50 selected native cases passed. Packing did not improve in this comparison. Canonical receipts and live Grafana fields were checked. Local checks, exclusions, the unrelated tooling flake and existing formatting failures are recorded in the audit; this is not CI promotion evidence. Versioned behavior is documented in the settlement/RPC specifications. |
| Source baseline | `main`, `7c90266a5fe8cfdb46b249c61079badc2dce71a3` |
| Protocol baseline | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Existing local work | Chiado deployment runbook edits and the September 25 recovery/wave audit directories; preserve them. |
| Related contracts | [Settlement policy](../configurable-settlement-policy.md), [original proposal](configurable-settlement-policy.md), [RPC failure handling](../../crates/eez-control-rpc/SPEC.md#6-failure-handling), [posting history](posting-lifecycle-history.md), [replacement design](skipped-slot-speculative-retry.md) |
| Evidence | [Initial wave investigation](../audits/2026-09-25-latest-wave-analysis/README.md); [deferred-storage comparison](../audits/2026-09-26-admission-deferred/README.md); [companion baseline](../audits/2026-09-26-admission-companions/README.md); [packing follow-up](../audits/2026-09-26-admission-packing/README.md); [September 27 refinement](../audits/2026-09-27-admission-refinement/README.md); [preparation](../audits/2026-09-27-admission-preparation/README.md); [failure tolerance and gas](../audits/2026-09-27-admission-backoff-gas/README.md) |

## 1. Objective and evidence

### September 28 live trial findings

Strict FIFO `b2ff3b8` remains deployed on cold copies of the existing Chiado state;
the [trial audit](../audits/2026-09-28-chiado-strict-fifo/README.md) records migration,
manual restart backfill, semantic assertions, canonical attribution and gas data.
All 35 proved candidates respected the FIFO invariant; 32 speculative bypasses
were discarded before proving. Five small posts drained 1–13 sources remaining
inside a retained retry prefix. Extending those admitted remainders with eligible
younger work is an unimplemented optimization that must preserve source ownership,
nonce dependencies, the target deadline and complete FIFO inclusion. No acceptance
criterion or strict FIFO requirement is relaxed by this finding.

Monitoring `2cb7d61` restores genesis identity validation against the upgraded
pruned Reth RPC using a bounded raw-header hash fallback. Live Grafana now exposes
complete estimates for all 870 settled sources and actual gas for all 29 posts.
No node/signer behavior changed during that monitoring fix. The historical live
performance comparison is not a fresh paired benchmark and does not establish
performance parity; the two CI packing assertions remain unresolved.

### September 27 correction: strict FIFO admission

**Accepted user direction:** restore FIFO because outbound-first admission can
exclude older inbound requests. This supersedes the relaxed FIFO packing policy
below. FIFO applies to the set admitted across batches: no younger eligible
source may be submitted while an older eligible source remains queued. Within
one batch, the signer's existing outbound-before-inbound execution order remains
mandatory. A fitting mixed snapshot should still use one batch.

Stop discovery beyond the first resource refusal. If canonical phase ordering
has already accepted younger sources, discard that speculative candidate and
recompose a queue prefix including the first omitted eligible source. Seed its
width from the number of groups just simulated; cutting directly at the first
omission needlessly produced tiny posts in the first FIFO iteration. Keep the
same parent, target, deadline and source ownership; every rebuild must shorten
the snapshot. Remember the retry prefix's last source for the next opportunity if the
deadline prevents rebuilding, so repeated outbound-first attempts cannot starve
the inbound head. Replacements inherit the boundary by sender/direction/nonce.
Canonical consumption, removal, or a classified source condition clears it.

The first FIFO iteration's 500 ms Ethereum fixture used nine measured posts and
15 repairs. Investigation found discarded blobs and elapsed work contaminating
future per-group/finishing estimates. Reset those candidate observations on a
rebuild, and train the next sample on the replacement alone. The original shared
clock still charges all elapsed work; this must never extend the deadline. Keep
both the first results and a deterministic accounting regression, then repeat
native FIFO/deadline cases and paired throughput checks before completion.

Fee, balance, nonce and escrow waits remain eligibility exceptions. Preserve
whole groups, nonce dependencies, exact final execution and five-failure backoff.
Do not promote a later source chain in the next snapshot merely because it was
not visited. Restore the original snapshot order when returning speculative work,
including concurrent replacements and recovered nonce predecessors.

Record discarded candidates and their replacement link as `fifo_prefix_rebuild`;
record a retained next-opportunity boundary as `fifo_prefix_limit`. Keep existing
gas/deadline reasons and the historical `fifo_bypassed` field readable. Such a
bypass can describe discarded simulation, never an authorized submission.

Acceptance: reproduce the current cheaper-source bypass, regress older-inbound /
younger-outbound capacity cuts, repeated deadline deferral, replacements, nonce
recovery and blocked-source liveness. Confirm fitting mixed waves retain their
packing and independent follower replay. No wire, protocol or storage migration
is introduced. These changes require new validation; earlier throughput records
do not establish their performance.

Maximize canonically completed cross-chain sources per produced L1 block and
per second, while preserving source ordering, whole groups, complete effects,
network limits and the target's submission deadline. Measure oldest request
age and p50/p95 completion latency alongside throughput so larger batches do
not hide starvation. Count unique canonical sources independently of retries;
show static-only completion separately from effectful delivery.

The initial investigation wave completed 870 sources using 29 successful postings and 36
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
The implementation uses covering proof samples when available, otherwise the
largest upward group/blob/witness ratio with 25% headroom, capped at the
greater of the configured bootstrap seed and the observed duration with
headroom, so scaling never prices larger work below an observed slow proof. It records the prediction basis; finalization keeps
covering-sample estimates. This permits learning across gradually growing witness
ranges without repeatedly imposing the cold seed. The absolute deadline still
bounds every attempt; extrapolation is a planning heuristic to validate with
observed prediction error, not an execution guarantee.
Reuse existing admission state and Reth task ownership; do not introduce a
second request scheduler or speculative branch.

First iteration keeps calibration in memory. Persisting measurements across
restarts is a later option requiring source/protocol/prover/configuration identity,
expiry and invalidation rules. Removing the forced one-source cap does not need
that extra storage compatibility work.

## 4. Gas packing and retry learning

### Explain and tighten the gas model

#### September 27 follow-up: cheaper capacity checks and tighter reserves

**Accepted scope:** the user authorized the gas-budget improvements together
with cheap rejection before expensive simulation and better measured multi-call
companion reserves. Source baseline `239b822`, deployed runtime `e5b2cde`,
protocol `9d13218`. The [budget review](../audits/2026-09-26-admission-packing/README.md#september-27-review-is-the-whole-bundle-gas-limit-too-restrictive)
found that 17 bundle-limited postings used 43.0% of their gas budgets on average;
their actual signed start envelopes needed 47.7%. The preceding wave also
performed 7,672 capacity-refused source simulations for three extra included
sources. These are separate sources of avoidable work and conservative packing.

- Before a source simulation, check the signed transaction-start requirement,
  known L2 transaction requirement and a minimum postBatch contribution. A
  rejection must follow from the current candidate's actual policy arithmetic,
  independent of unknown application behavior. Preserve ownership and nonce
  blocking; proceed with independent sources. Do not reuse a previous execution
  estimate across changed parents or accepted prefixes as a proven lower bound.
- Measure tighter deferred-storage and multi-call companion reserves against
  the pinned contracts, including cold storage, retained slots, repeated call
  identities, maximum queues, nested callbacks and outer reverts. Preserve the
  distinction between successful inner calls and committed queue progress.
  Unknown shapes retain conservative estimates. No blanket receipt-ratio discount.
- Investigate bounded refinement near an estimated cutoff. Whole-bundle native
  execution requires the final stitched roots and proof; any earlier estimate
  must state its scope and cannot bypass proof/signature validation. Keep final
  exact execution, block/transaction limits and the existing shared deadline.
- Report cheap refusals separately from simulated estimates in Grafana, including
  the bound and limiting budget, so absent full estimates are explained. Keep
  historical schemas and records readable and avoid per-hash Prometheus labels.

Acceptance: reproduce avoidable simulations and conservative gas fixtures at
the requested baseline, exercise boundary/nonfit/fitting cases, source retention
and nonce dependencies, and verify that cheap checks never introduce eviction
or attempt charges. Run focused Composer, contract, indexer and native admission/
queue checks, shipping builds, formatting and clippy. Deploy exact local images
and repeat the verified 870-source / 770-job wave, comparing canonical post count,
gas prediction error, full simulation count, preparation time and missed targets
against `e5b2cde`. Record failures and live noise. There is no authorization to
change the protocol pin, signer profile, canonical ownership or operator timing.

The cutoff-refinement investigation found that exact full-bundle execution is
currently after proving and after prepared-attempt ownership is installed. A
second admission pass would need a separate bounded prove/rebuild lifecycle,
with fallback to the already proved candidate under the same target deadline.
This iteration tightens measured reserves and retains final exact execution;
it does not add an extra proof cycle or simulate with proof validation bypassed.
That extension remains a separate optimization with lifecycle tests required.

**Completed September 27:** runtime `7fdb3fc` and the matching collector are
locally deployed. All 48 selected native cases are validated, including eight
after correcting stale version assertions in test-only commit `2d64c7d`.
All 770 live semantic jobs passed. The wave used 26 postBatches for 870 sources,
versus 28 previously; full gas-refusal simulations fell from 7,672 to 1,360,
with 6,601 cheap refusals. Companion estimates fell from 2.298× to 1.977× actual
gas overall, and the two multi-call families improved to 2.670× / 2.612×.
Median preparation was 456 ms versus 483 ms. Grafana covers all 31 candidates
and 9,007 source-attempt rows with explicit cheap-check bounds and nullable full
estimates. Live state, block budgets and refundable prefunding differ; this is
one observed comparison. See the [refinement audit](../audits/2026-09-27-admission-refinement/README.md)
for exact images, initial failures/corrections, canonical reconciliation,
cutoff details, gas tables and validation limits.

#### September 26 follow-up: packing with a FIFO preference

The September 27 strict FIFO correction above supersedes this section's
cross-batch bypass behavior. The earlier design and measurements remain below
as historical evidence.

**Accepted scope:** after the `e57c7a5` verified wave, the user explicitly
prioritized the most cross-chain sources in the fewest postBatches, with FIFO
preferred but not a mandatory cutoff. The source baseline is `b5caf1b`, the
previous deployed runtime was `e57c7a5`, and the protocol remains `9d13218`. The
[companion audit](../audits/2026-09-26-admission-companions/README.md) records
870 canonical sources / 770 semantic checks, 33 postings, two final FIFO cuts,
and 19 final whole-bundle gas cuts. The 170 signed-limit companion fallbacks
reserved 170 million gas versus 30.6 million observed receipt gas.

- Remove the independent `fifo_fairness_limit` count backoff. Preserve FIFO
  discovery/order where compatible with canonical outbound-before-inbound
  execution, source-chain nonce dependencies, and whole call groups. FIFO
  becomes a preference, not a promise of global cross-direction ordering.
  Retain bounded source discovery and its resume cursor.
- A group that does not fit the remaining gas/blob capacity should not end
  discovery of independent groups that may fit. Keep the group and its nonce
  successors queued, leave accepted state/budgets unchanged, and continue within
  the same finite snapshot and shared deadline. Genuine deadline/finishing
  limits still stop preparation. Per-source deferral reasons must remain accurate
  when a later independent source is accepted or hits a different constraint.
- Measure and extend bounded companion execution estimates to the observed
  multi-call, nested/reentrant and reverted-callback families. Require bounded
  action/ABI evidence and pinned-parent proxy registration; unsupported shapes
  retain a fallback. Charge separately measured L1 callback execution as well as
  source simulation and protocol overhead: intercepted calls return gas to the
  source simulator, so a fixed protocol reserve cannot cover arbitrary target
  work. Expose that execution component separately in telemetry. Preserve
  signed-limit start-of-transaction gas envelopes,
  final ordered native validation, and exact-target proof/submission. Do not
  substitute aggregate receipt discounts or infer cursor advancement from a
  successful entry when its enclosing frame may revert.
- Keep admission estimates and source decisions available in Grafana. Historical
  FIFO-cut records remain valid history; new producers stop emitting that count
  limit. Document estimator/policy versions and nullable new telemetry fields.

Acceptance: reproduce the count-cap and first-nonfitting-group behavior at the
baseline; regress independent-group packing, rejected-group retention, nonce
successors, blocked sources, whole nested groups and the shared deadline. Measure
new gas families and boundaries against the pinned contracts on the supported
forks. Run affected Composer/native admission, gas and queue cases, formatting,
clippy and shipping builds. Deploy exact local images and repeat the verified
770-job wave. Primary comparison is canonical postBatch count and sources per
posting, followed by gas prediction error, cutoff counts, latency, missed targets
and preparation overhead. Report live noise and any regression; elapsed time
alone does not satisfy packing improvement. No protocol, signer-profile, queue
persistence or speculative-replacement extension is part of this change.

**Completed September 27:** runtime `e5b2cde` is deployed in the existing Chiado
environment. The verified wave used 28 postBatches for 870 sources, versus 33 in
the preceding wave, with no FIFO count cutoff or calibration posting. All 520
inbound sources used bounded companion estimates; their total estimate fell
32.4%. All 770 semantic jobs are validated; 46 required a file-based transport
fix for calldata exceeding Linux's argument limit before the unchanged pinned
verifier could run. The original failed run is retained. Median preparation
increased from 378 to 483 ms, while send plus monitor fell from 199.540 to
166.540 seconds. Shared live conditions limit causal attribution. The
[packing audit](../audits/2026-09-26-admission-packing/README.md) records baseline
reproductions, local correctness checks, exact images, deployment, original
failures, canonical receipts, Grafana coverage and remaining gas/preparation
opportunities.

#### September 26 follow-up: companion costs and reverted storage

Authorized by the user's request to keep improving after the second repeat
(`20e81da`, deployed runtime `85f3691`, protocol `9d13218`). The
[repeat evidence](../audits/2026-09-26-admission-deferred/rerun2/README.md)
shows conservative postBatch reserves alongside underestimated companion work.
This iteration measures them separately before changing admission:

- Extend bounded deferred storage pricing to measured reverted/flagged metadata
  and ordinary revert payload sizes, including retained old slots. Price packed
  flags and nonzero reverted/static hashes correctly. Posting stores these
  fields; the later companion executes them. No signer/profile extension.
- Measure the missing companion protocol overhead with the pinned contracts;
  retain declared transaction limits, whole-group admission and final ordered
  native validation. An uncertain estimate must not evict an otherwise valid
  source solely because its predicted overhead exceeds its signed limit.
- Collect canonical inbound source receipts even when consumption logs revert.
  Receipt gas is operational evidence, not proof of complete consumption. Bind
  enrichment to canonical block/posting identity, retain null for absent receipts,
  and invalidate it on reorg. Upgrade additive schema/collector before dashboard
  consumers and Composer; old histories remain readable.

Acceptance includes cold caller-frame storage/consumption measurements, malformed
and boundary metadata, Composer gas-budget regressions, all four admission/policy
process suites, and receipt enrichment tests for missing logs, missing receipts,
wrong block/position and reorg cleanup. Run the shipping build and required
fmt/clippy checks, then deploy the exact local image and repeat the full verified
770-job wave. Compare actual companion/postBatch gas, cutoff distribution and
canonical completion with the recorded same-host baseline. Runtime warm state
and live slot variation remain explicit limits on performance attribution.

External builder non-inclusion and scheduler/retry-policy changes remain separate
follow-ups; this iteration does not infer their causes from unused block gas.

The initial companion model covers one deferred entry with bounded registered
callbacks and no nested rows. Larger groups, unregistered proxies, flagged
callbacks, or unmeasured execution outcomes reserve the signed source limit.
Root results may revert; a matching queue identity costs more to scan because
the contract checks root preconditions. The [settlement contract](../configurable-settlement-policy.md)
defines the bounds and the [monitoring contract](../../monitoring/indexer/README.md#admission-history)
defines nullable receipt coverage and mixed-version behavior. Broader companion
discounts require additional measurements. Report any packing regression caused
by correcting previously omitted gas.

#### September 26 follow-up: deferred storage and overwrite estimates

Authorized by the user after the `036b50a` wave: prioritize bounded inbound
callback/result storage and replacement of retained complex slots, with explicit
estimator fallback reasons in Grafana. Baseline checkout is `bfcb68d`, protocol
remains `9d13218`; deployed baseline and the 870-source comparison are recorded
in the [shape audit](../audits/2026-09-26-admission-shapes/README.md).

Price bounded storage writes and cleared tails using the exact canonical-parent
slot inventory. Read inactive mapping entries too. Bound arrays and byte lengths
before walking storage, charge unknown new words as fresh, and retain the general
reserve when either layout is unsupported. Preserve the existing immediate-call
model, whole groups, the 128-entry boundary, and final ordered native execution.
Root stitching must not invalidate the preliminary reserve: charge roots and
rolling hashes as nonzero even before their final values exist. No speculative
storage reuse, receipt-refund credits, protocol or signer-profile extensions.

Pinned CORE/EXECUTION_ENTRY specifications describe array queues and clearing;
the pinned Solidity uses mappings with reset active bounds and retained payloads.
The accepted fork settlement contract already documents this discrepancy's
operational behavior. Measure the pinned implementation; do not change its ABI
or reinterpret logical replacement as physical deletion.

Acceptance: cold caller-frame gas ladders before refunds for fresh, retained,
shrinking and expanding callbacks/results, including inactive slots; independent
storage-layout assertions; Rust boundary and group-repricing tests; mixed-history
PostgreSQL/dashboard checks; native admission/settlement and semantic replay.
Then build local shipping images, preserve runtime state during deployment, and
compare a semantically verified workload to the previous wave. Report preparation
overhead, required gas, fallback coverage and actual cutoff reasons as well as
receipt ratios. Keep upgraded additive monitoring views on runtime rollback;
historical missing fallback reasons stay unknown. No queue persistence is added.

Implementation `85f3691` is deployed locally. The [deferred-storage audit](../audits/2026-09-26-admission-deferred/README.md)
records the bounded storage measurements, native regressions, image identities
and live comparison. Gas overestimation improved, while the observed 870-source
window was longer; timing, fairness, skipped slots and a historical-data
predecessor remain relevant limits. Lower admission gas alone is not evidence
of higher throughput.

#### September 26 follow-up: selection visibility and gas shapes

The user authorized the first two follow-up priorities: explicit selection-cap
reasons in Grafana and more accurate gas estimates for nested/multi-call work.
Keep selection/fairness/retry behavior unchanged in this follow-up. Record the
effective count cap and its cause separately from the finite snapshot and from
the resource that stopped admission. Historical missing causes remain unknown;
do not infer queue exhaustion from a null reason.

The protocol engineer supplied separate measurements for posting/storage and
later call execution. Treat these as profiling leads, not replacement constants:
the measurement revision, proof system, storage state and transaction boundary
are unspecified. Reproduce relevant shapes against the pinned contracts with
cold access, fresh and retained queues, result-size boundaries and multiple
entries. Lower reserves only for bounded, tested shapes; preserve conservative
fallbacks, canonical-parent evidence, final ABI projection and native preflight.
No protocol, signer-profile or wire-format changes are intended.

Acceptance requires cause-preserving count-limit regressions, collector/SQL
mixed-history checks, pinned Solidity gas measurements, estimator boundary tests
and native admission/settlement regressions. Record measured gas and preparation
overhead; a lower estimate alone does not establish higher settled throughput.

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
can retain additive database tables and existing checkpoints. Collectors predating
the entire admission family ignore it; admission-aware collectors reject unknown
event kinds, so retain the upgraded collector on Composer rollback. Older
Composers resume their old timing rules. No destructive migration or database
reset is required.

For a future live rollout, follow the existing deployment runbooks, reconcile
pending work before replacing the node, preserve logs and source identities, and
record exact images/configuration plus first failures. Requests and calibration
remain in memory; monitoring persistence does not make queue recovery durable.
Deployment, wallet operations and live workloads are separate execution steps.

## 9. Additional opportunities after the first measurements

### September 27 failure tolerance and companion reserves (accepted for implementation)

The user requested delaying admission backoff until five errors and improving
gas estimation. The latest verified wave had six produced-target exclusions,
each immediately after a skipped target; immediate halving fragmented all six
following posts. This correlation does not establish the external omission cause.

- Automatic admission counts consecutive canonically confirmed, charged
  exclusions or incomplete settlements of nonempty source batches. Failures one
  through four preserve selection; the fifth and later failures halve the next
  submitted-group allowance. Complete canonical source settlement resets the
  streak. Preparation, relay acknowledgement, skipped slots and inconclusive
  observations neither increment nor reset it. Real pipeline deadline limits
  continue to apply immediately.
- Automatic submitted-attempt reconciliation permits six cumulative charged
  failures per source, allowing the fifth failure's reduced retry to execute.
  Keep the existing shared per-source episode counter and three-failure bound
  for prover/immediate relay rejection, and for numeric admission. Each eligible
  source is charged once; nonce-dependent eviction, late-original/replacement
  ownership and partial consumption keep their existing rules.
- Tighten only measured bounded companion execution reserves against the pinned
  protocol. Retain complete-prefix scanning, matching-identity root checks,
  fallback for unsupported shapes, signed start requirements, active network
  budgets and final ordered native execution. Do not derive discounts from
  average receipt ratios or assume inner success committed the queue cursor.
- Record the failure streak and threshold in retained candidate observations
  and Grafana. Version the changed scheduling and gas estimates. State is local
  and in memory; there is no wire, protocol, storage or follower migration.

Acceptance: reproduce immediate backoff at the baseline; test failures one
through five, canonical success reset, uncertainty and skipped targets, bounded
source retries and recovery of all sources. Measure companion fixture ladders
on Cancun/Prague/Osaka, including nested/reverted/max-queue cases; run affected
Composer and native admission/partial-consumption/reorg checks. Validate focused
tests before local Chiado rollout and compare the same verified workload with
the latest preparation wave. Complete broader applicable gates afterwards.

### September 27 preparation follow-up (accepted for implementation)

The user requested preparation profiling and a shorter devnet iteration loop:
focused correctness checks first, then the authorized local-image Chiado trial,
then broader applicable regressions before completion. The current baseline is
`2b9ca04` (runtime `7fdb3fc`, protocol `9d13218`), with the verified 870-source
wave recorded in the September 27 admission-refinement audit.

The 26 canonical candidates average 453 ms to proof request. Event intervals
attribute 199 ms to accepted-source lowering, prefix hashes and canonical append,
and 195 ms to KZG/witness preparation. Retained preparation logs identify 172 ms
of the latter as blob sidecar construction. These are wall-clock intervals;
prefix execution is not yet separately timed. The code rebuilds each growing
effect prefix from its parent, repeating previous transaction execution.

Implement a candidate-local checkpoint for canonical prefix hashing. Reuse only
an identical transaction prefix under one immutable parent and block environment.
Preserve pre-execution changes, receipts, cumulative gas and the complete REVM
state; finalize a separate copy through Reth's canonical block builder so
post-block changes never enter the next transaction's pre-state. Divergent or
shortened prefixes discard the checkpoint; any failed construction invalidates
it. Keep canonical append, the final independent block rebuild, witness replay,
signer checkpoint recomputation and signed-bundle validation intact. This cache
does not establish source acceptance or queue ownership.

Expose prefix-hashing and blob/witness timing separately in retained admission
details. Compare every checkpoint hash against the uncached builder across
supported forks, reverts, logs, state changes, prefix replacement and errors.
Measure execution counts and elapsed time on identical workloads before claiming
an improvement. Run relevant process coverage before the devnet trial; complete
the remaining admission/settlement and affected replay tests afterward. No wire,
protocol pin, configuration or historical decoding change is intended.

Further KZG optimization requires measurement; merely moving it to another
thread does not remove its work. Reusing speculative results across changed
prefixes, skipping verification, or treating receipt gas as a consensus budget
is outside this change.

Runtime `6c171fd` implements the prefix checkpoint and timing fields. Its verified
870-source wave reduced mean preparation from 453 to 346 ms; end-to-end settlement
was slower with more skipped/omitted targets and backoff. The
[September 27 preparation audit](../audits/2026-09-27-admission-preparation/README.md)
records the comparison, validation status and an accompanying monitoring query
fix. KZG/prover overlap remains a follow-up, not part of this implementation.

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

Native validation exposed a cold-estimate liveness boundary: a 500 ms preparation
allocation plus a 1,000 ms proof seed left slightly less than the 1,500 ms sum
of bootstrap finishing, proof and finalization reserves. No first observation
could be collected. The implementation permits a first-group timing probe only
when the regular estimate rejects it, retaining finishing/finalization reserves,
a 25 ms minimum proof opportunity, all resource gates and the original deadline.
The candidate summary identifies this fallback. It does not impose a one-group
limit on cold batches that fit the ordinary estimate.

### September 27 measured nested inputs and relay diagnostics (accepted for implementation)

The user authorized the next gas refinement and omission investigation after
the 24-source-post verified wave. Its companion reserve averaged 2.41–2.56 times
execution for the two multi-call families. This round will price bounded nested
input lengths from validated simulation evidence, retaining a measured minimum
allowance for repeated nested-table copying. Whole-prefix scan and rollback
allowances, signed limits, parent registration and ordered native validation
remain required. Unknown or mismatched evidence retains the declared limit.

Relay delivery observations will retain a bounded bundle identifier when the
relay provides one, target/parent, transaction count and acknowledgement slack.
A successful relay reply does not establish builder inclusion or source
consumption. Missing identifiers remain explicitly unavailable; no external
cause will be inferred from an acknowledgement. These additive diagnostics must
not change ownership, retry classification, target pins or deadlines. Existing
history readers must accept old events without the new optional fields.

Acceptance: compare measured input/row/callback/revert/queue boundaries with
the prior formula on the pinned contracts; reject unsupported evidence; test
relay null/hash/error/deadline paths. Run focused local checks before a local
Chiado image rollout, then compare the same verified workload and complete
applicable broader checks. Record actual deployed identity, per-source estimate
error, gas cutoffs and acknowledged omissions. No wire/storage migration or
protocol change is intended; old binaries can ignore additive observations.

Evidence: [nested-input and delivery follow-up](../audits/2026-09-27-admission-nested-delivery/README.md).

### Fixed admission overhead follow-up (accepted for implementation)

The user accepted this follow-up and requested a 5% default. Explicit operator
overrides retain precedence; absent values change from 2000 to 500 basis points.
Empty/malformed values remain errors in the runtime parser. Older binaries keep
their old default unless explicitly configured; pin 500 for mixed versions.
No wire, persistent state, protocol or signer profile change is intended.

The baseline deployed gas margin is 5% (`EEZ_L1_POSTBATCH_GAS_MARGIN_BPS=500`), although
its code default remains 20%. Its separate fixed admission reserve is 600,000
gas. In the latest wave, full sizing added at most 122,808 gas to the incremental
projection. A smaller fixed allowance would cover that cohort, but one/two-blob
observations do not establish coverage for larger history or calldata floors.

Replace the fixed allowance with candidate-dependent ABI framing, leading
anchor/root commitments, bounded proof placeholder and metadata sizing. Keep
the configured final gas margin and final ordered execution at the signed
limits. Account for root stitching changing nonzero calldata bytes, dynamic
array offsets, blob indices, active calldata-floor rules, and revocation of
shape discounts. Use existing sizing paths without adding repeated KZG or
full-prefix encoding to every admission attempt. Retain a conservative fallback
where exact incremental evidence is unavailable.

Before rollout, compare the incremental bound with final full sizing over
empty/history-heavy, maximum-entry/blob, nested, reverted, unsupported-shape,
discount-boundary and fork-floor cases. Verify that lower planning overhead
does not introduce full-projection failures or loss of complete groups. A live
comparison must show whether extra admission capacity reduces source posts;
released gas alone is not a throughput result. The broader companion scan
reserves remain a larger measured source of unused block capacity.

Implemented as `candidate-framing-10`, deployed from `e556e12`, with the 500-basis-point
default and preserved explicit overrides. See the [framing/default validation](../audits/2026-09-27-admission-framing/README.md)
for the verified live comparison, retained intermediate failures, native fixture
correction, gas/cutoff tables and validation limits.
