# Nested-input reserves and relay delivery evidence

| Field | Value |
| --- | --- |
| Date and status | 2026-09-27; runtime `aa515c8` deployed; verified live wave and 50 selected native cases complete; exclusions and tooling flake below |
| Baseline source | `c937ea05d8a024d36227f5c9155338d03367cceb` (`feat/cross-chain-admission-throughput`); runtime `9c11c1aead3c6f959bee4787a65f2a116d6e41f7` |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d`; clean |
| Scope | Evidence-based nested-input gas reserves and additive relay delivery diagnostics; unchanged settlement ownership, target binding and final native validation |
| Related records | [Proposal](../../proposals/cross-chain-admission-throughput.md); [previous verified wave](../2026-09-27-admission-backoff-gas/README.md) |

## Investigation

The prior formula charges eight input words per nested call, even when validated
action evidence supplies a shorter input. Nested resolution also copies the
expected-call table repeatedly, so removing that charge entirely requires
measurement. Complete-prefix queue scans remain necessary when enclosing
reverts restore the cursor. Relay acknowledgement alone cannot explain omitted
bundles; the current delivery event lacks the relay identifier and remaining
acknowledgement window.

Local evidence is retained under `artifacts/admission-nested-delivery/`; it is
ignored and unavailable to another checkout. Small result summaries and gas
tables are committed below. The following local checks preceded deployment.

## Local implementation evidence

The 30 input-boundary measurements match baseline execution exactly. Twenty-seven
reserves decrease, by up to 36,800 gas for four nested calls. The minimum new
reserve/execution ratio is 1.0671; fixture execution includes application work
that production separately adds. This is a measured bounded admission allowance,
not a universal EVM upper bound. See [measurement table](gas-input-comparison.csv).
All 61 gas fixture tests passed on Cancun and Prague; all 80 integration tests
passed on Osaka. Composer passes 269 tests with one existing manual diagnostic
ignored; L1 passes 143 tests, including delivery timeout and optional relay hash
handling. Process and deployment verification remain in progress.

First failures retained locally: the initial combined input-boundary fixture
exceeded Foundry's per-test gas ceiling after eight measurements; splitting it
into ten independent cases passed. The first Composer run had one incorrect
new expected discount because its nested action inherited a 256-byte input;
explicitly using a short input fixed that test. Neither was a runtime failure.
Initial Clippy flagged four Option-return simplifications; these were corrected.

Additional short-data fixtures cover empty/32-/64-byte root results and 4-/36-byte
callback inputs, including maximum matching queue and reverted spans. These four
measurements also fit the reserve. Final fixture selection is 63 tests on Cancun
and Prague and 82 integration tests on Osaka. The candidate runtime remains
`b6fe9f5ca3f21f45bd83d8e43445d6ffe9873f34`; the additional change affects test
fixtures only. Workspace Clippy passes with warnings denied.

The full tooling run selected 264 tests: 259 passed, four opt-in infrastructure
tests skipped, and `test_all_workers_follow_discovered_inventory` failed because
its four completed mock workers reached a measured concurrency peak of three.
The unchanged test requires a peak of four. First-run output is retained in
`tooling-first.log`; a diagnostic retry will be recorded separately, not promoted
to a clean first pass. No protocol network runner code changed in this round.

The first native process run reached canonical settlement but failed the new
parent-hash observation assertion. `Exact` transport targets leave their
optional derived-parent guard empty. Delivery observations now use the
already-validated canonical fee sample when it is the target's immediate
parent. This correction changes diagnostics only; admission and observation
classifications are unchanged. First process logs/datadirs are retained under
`focused-native/` and the original log is preserved before the corrected run.

## Deployed candidate and limits of the gas change

Runtime `aa515c8720f7943df2d9d0ec147e2252ace1a21f` supersedes the undeployed
`b6fe9f5` build. All three focused native cases passed after the parent-field
correction: the Gnosis 45-source repeated-wave/replay case, actual-companion
gas whole-group reduction, and partial consumption from a middle group.
Composer again passes 269 tests (one existing ignored); workspace Clippy passes.
The unrelated mock-worker concurrency test passes on a single diagnostic retry
with build load removed. Its first result remains **flaky-result = fail**.

The old cohort contains only 20 admitted inbound nested rows. This discount
can remove at most 184,000 gas (0.137%) from its 134,564,786 total companion
estimate. It does not materially address the two multi-call families' repeated
root/queue-scan reserves. Their names do not imply nested expected-table rows.
A larger throughput improvement must be demonstrated separately.

Fresh source proxies were also investigated: the pinned empty-target fixture
measures up to 323,928 gas per new entry, before application work. The proxy
constructor additionally permits a 100,000-gas recovery call for counterfactual
funding. No broad new-proxy reserve reduction was justified or implemented.
The complete-prefix scan allowance remains; inner call success alone cannot
prove that an enclosing source frame committed the queue cursor.

The authorized rollout on September 27 replaced node/signer images, preserving
all mounts, environment values other than image references, original genesis
and operator configuration. The matched deploy image was built but no contracts
were redeployed. The signer was made healthy before stopping the idle Composer.
An empty-queue check preceded both the rollout and measurement.

The first post-restart health check found the known peerless L1 gap. Twenty-two
raw canonical payloads were independently checked against their raw block hashes,
parent chain and local Beacon head, then every Engine import returned `VALID`.
Lighthouse retained fork-choice ownership. The immediate health check was still
catching up; the subsequent check passed with fresh nonsyncing L1/L2, an empty
queue and the original genesis. First logs are retained; no datadir was reset.

The same wallet-locked staged runner completed the candidate workload with
`--verify`, the unchanged fee/funding profile, 200 preparation workers, 28 send
workers, four semantic verification workers and the same ordinary-L1 forwarding
limit of five requests per second. Broader process tests started after
cross-chain sending/settlement measurement ended.

## Live comparison

Run `20260927-142207` completed all **770 semantic jobs**, with zero failures or
skips. All **870 sources** settled: 520 inbound and 350 outbound. Repeating
canonical attribution produced identical source/post CSVs; external RPC receipts
independently matched every source-bearing post. The queue was empty afterward.
The full live wave did not receive independent follower replay; selected native
tests provide replay coverage for their own workloads.

| Metric | Previous `9c11c1a` | Candidate `aa515c8` |
| --- | ---: | ---: |
| Canonical sources | 870 | 870 |
| Source-bearing postBatches | 24 | 25 |
| Posts including ordinary history | 25 | 26 |
| Source settlement window, seconds | 178.474 | 172.541 |
| Sources/second | 4.875 | 5.042 |
| Actual source-post gas | 177,292,623 | 177,509,959 |
| Companion estimate / receipt gas | 1.828 | 1.843 |
| Mean preparation, ms | 392.685 | 380.311 |
| Maximum preparation, ms | 652.513 | 702.612 |
| Produced targets omitting a source bundle | 4 | 2 |

**Packing did not improve.** The slightly shorter settlement window includes
fewer omissions and does not isolate a causal throughput benefit. Workloads,
funding/fee profile, shipping build recipe, concurrency and node settings were
held comparable, but parent state, source ordering, active block gas limits and
external inclusion vary between live runs. Companion reserve accuracy worsened
slightly in aggregate. The targeted reentrant family improved from 1.730 to
1.665 times receipt gas; the two multi-call families still measure 2.417 and
2.466. Every one of the 520 companion estimates exceeded its receipt gas
(minimum ratio 1.083); this is evidence for this cohort, not a universal bound.

Only 20 nested input rows were eligible, across three canonical posts. Restoring
the maximum old input allowance adds at most 184,000 gas in total. That upper
bound crosses the estimated block budget for one post, at block 23255433;
the other 24 accepted sets still fit. This counterfactual does not rerun the
original selection and cannot establish a different batch count.

The extra historical post at block 23255411 consumed 109,117 gas and two blobs.
It settled ordinary history needed before the source wave; it was not a warmup
or calibration post. Inventory distinguishes native blob posts from companion
transactions that also advance protocol commitments.
The [34 target slots](candidate/target-slots.csv) reconcile to 25 source posts,
one history post, four produced slots with no eligible source, two skipped slots
and two acknowledged omissions. One skipped slot also had no eligible source.

### Admission cutoffs

| Canonical stop reason | Posts | Sources |
| --- | ---: | ---: |
| `l1_bundle_gas_budget` | 14 | 498 |
| `l1_tx_gas_budget` | 9 | 345 |
| `eligible_sources_exhausted` | 1 | 9 |
| `snapshot_exhausted` | 1 | 18 |

No canonical candidate had a reduced selection limit or a nominal preparation
overrun. Five posts admitted seven independent sources after capacity deferrals.
Across source-related attempts, cheap checks avoided simulation for 6,774 of
8,950 gas refusals (75.69%, versus 87.33% previously); 3,134 full simulations ran.
The changed ordering and headroom matter when comparing this percentage.

For the 14 bundle-limited posts, the estimated ordered start envelope averaged
98.90% of the active parent budget. Actual consumption averaged 52.14%, and
the envelope preserving every original signed transaction limit averaged 57.04%.
This shows considerable estimator headroom, not permission to remove signed
start checks. Active parent budgets ranged from 17,453,689 to 17,762,955 gas.

Five other candidates examined no eligible source; their `source_fee_count`
values were 364, 276, 92, 92 and 50. The nine-source canonical post also had 416
fee-blocked sources. Other filtered counts include ordering/dependency effects;
they must not all be labelled fee failures. Submitted source bytes and their
authorized fee caps were preserved.

## Percentage margin and fixed admission reserve

Direct inspection of the deployed node and all 25 signed limits confirms:

| Allowance | Live value | Purpose |
| --- | ---: | --- |
| `EEZ_L1_POSTBATCH_GAS_MARGIN_BPS` | 500 (5%) | Headroom on pre-refund native execution gas, at least the active calldata floor; also reserved during admission |
| Code default for that setting | 2000 (20%) | Applies only when the environment does not override it |
| `POSTBATCH_DRAIN_MARGIN` | 600,000 gas | Admission allowance for sizing work absent from the incremental projection |

The observed postBatch planning cap is exactly
`floor(16,777,216 / 1.05) - 600,000 = 15,378,300`. Every final signed gas limit
equals the preflight requirement multiplied by 1.05, rounded up. The 600,000
reserve is not an additional fee charged on the receipt. Neither allowance is
a protocol requirement.

Across canonical source posts, the full-sizing estimate exceeded the incremental
admission estimate by 25,264–122,808 gas, averaging 81,817. All 28 completed
source-bundle projections also stayed below that maximum; they contained one
or two blobs. The canonical posts reached at most 59 entries. Every measured
sizing gap equals the final calldata gas minus the sum charged incrementally
([framing comparison](candidate/framing-gap.csv)). The fixed reserve is 4.886
times the largest observed gap.
A hypothetical 200,000 reserve would cover these observations and release
400,000 gas of planning capacity, but these measurements do not cover arbitrary
larger histories, calldata floors or unsupported shapes.

Recommendation: retain the live 5% signing margin, and replace fixed admission
overhead with a calculation using each candidate's framing, proof placeholder
and metadata, with fallback coverage for unsupported cases. Preserve final
ordered execution at the exact signed gas limits; gas forwarding and refunds
make receipt gas alone insufficient. This follow-up is **not implemented or
deployed** in this record. The roughly 1.20 admission/receipt ratio on tx-limited
posts is not evidence of a configured 20% margin. See [margin evidence](candidate/gas-margins.json).

## Relay observations and Grafana

All 28 source-bundle deliveries were acknowledged. The relay returned no bundle
identifier; the new field correctly remains null. Twenty-five were included,
one target slot was skipped, and two produced target blocks omitted the bundle.
For the two omissions, acknowledgement arrived with 677.5 and 875.7 ms remaining
before the shared deadline. Both expected parents matched, and both canonical
blocks contained zero transactions and used zero gas; external RPC checks agree.
Included deliveries had a 635.1 ms median acknowledgement window and a minimum
of 107.6 ms. These observations do not identify the relay/builder/proposer cause
and do not support attributing these two omissions to late acknowledgement or
full blocks. No relay or builder internal telemetry was available.

The live Grafana query returned all 34 candidates and all 28 delivery records
with target, parent, transaction count and acknowledgement window. Queries for
108 historical candidates still succeed with absent new fields. Three canonical
source drilldowns checked receipt joins. The optional fields require no schema
migration; the unchanged indexer reads the retained event JSON. Existing history
and checkpoints were preserved.

## Evidence and reproduction

- [PostBatch admission, preflight and receipt table](candidate/admission-vs-receipt.csv)
- [All 870 canonical source estimates](candidate/source-estimates.csv)
- [Companion family comparison](candidate/family-gas-summary.json)
- [Ordered bundle budgets](candidate/bundle-budget-per-post.csv)
- [Delivery evidence](candidate/delivery-evidence.csv) and [external checks](candidate/external-delivery-checks.json)
- [Preparation breakdown](candidate/preparation-summary.json)
- [Semantic verification](candidate/semantic-verification.json) and [canonical stability](candidate/canonical-stability.json)
- [Images and executable identities](images.json)
- [Final deployment health](final-health.json) and [local checks](local-checks.json)
- [Broader native selection](broad-native-plan.json), [results](broad-native-results.json) and [executables](broad-native-executables.json)

The exact-revision local shipping images were built from `git archive` of
`aa515c8` and the recursive pinned protocol checkout, with locked production
features. Source archive/build plans and logs remain local under
`artifacts/admission-nested-delivery/images-corrected/`. Signer, follower and
genesis helpers are byte-identical to the previous runtime; Composer changed.
Image hashes here identify local artifacts, not CI promotion evidence.

Repeated measurement uses the existing wallet-locked staged runner with
`bash scripts/run-chiado-staged-performance.sh --verify` and the preserved
private profile under `data/admission-nested-delivery-20260927/`. Reproduction
requires the live prepared environment and wallet coordination; do not treat
these evidence records as instructions to redeploy contracts or reset data.
The analysis commands were the existing `measure-wave.py`, `measure-admission.py`,
`analyze-cutoffs.py`, `profile-preparation.py` and local packing/budget checks,
followed by [analyze-delivery.py](analyze-delivery.py). The latter closes a
repeated investigation gap by joining retained delivery events to exact target
headers. Bulk logs, raw snapshots, private profiles and datadirs remain local.

The task-owned ordinary transaction forwarder was stopped after semantic
verification; its PID is gone and port 19445 is closed. It forwarded 1,397
unchanged transactions with zero upstream rate limits. Its 2,396 errors were
closed-client `BrokenPipeError`s (the comparable baseline had 2,441); no other
error classes occurred. Node, signer, Beacon, indexer and retained volumes remain
running. Pending request ownership remains in memory; this rollout does not
establish durable queue recovery.

### Validation status

- Composer: 269 passed, one pre-existing manual diagnostic ignored; L1: 143 passed.
- Pinned gas fixtures: 63 passed on Cancun and 63 on Prague; integration contracts: 82 passed on Osaka.
- Workspace Clippy with warnings denied and Rust formatting passed for the deployed revision.
- Three focused native cases and all 47 additional cases passed serially, with exact nonempty test selection verified. The deployed Rust/Solidity source had no patch during the broader run; executable hashes are retained above. These results validate this runtime, not a future margin/reserve change.
- Live: 770 semantic jobs passed, zero failed/skipped; all 870 source attributions and 25 source-post receipts reconciled.
- Tooling: 259 passed, four opt-in infrastructure skips, one concurrency flake; diagnostic retry passed, retained as **flaky-result = fail**.
- Changed Solidity fixture formatting passes. Whole integration formatting still fails in unchanged `ECDSAProofSystem.sol` and `DeployRollup.s.sol`.
- CI artifact promotion, full Kurtosis matrix and full-wave independent follower replay were not run. Twenty unrelated ordinary-only trigger/history/restart native cases are excluded from this selection; their previous results do not validate this binary.
