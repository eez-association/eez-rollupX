# Second repeat: admission cutoffs and gas estimates

**Status: complete. All 870 sources canonically settled and all 770 jobs passed
semantic verification; runner exit 0.**

Requested fresh repeat after the [incomplete prior run](../rerun/README.md).
This record preserves that failure; the new wave uses different source hashes.

## Identity and procedure

| Item | Value |
| --- | --- |
| Branch / checkout | `feat/cross-chain-admission-throughput`, `6f5ace2c09290075e27328e2cc31e50b7305f28f` |
| Deployed runtime | `85f3691cb9c7ccbf8c0b8700576733283e5af7fc`, unchanged |
| Protocol | Clean `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Protocol run | `eez-core-protocol/tmp/e2e-staged-net/20260926-170439` |
| Workload | 770 jobs, 870 source transactions; 200 preparation, 28 send and 4 verifier workers |
| Command | `bash scripts/run-chiado-staged-performance.sh --verify`, with the task-private `DEVNET_ENV` |
| Active admission | `auto`; nominal preparation 1,000 ms, proof seed 450 ms, submission slack 450 ms, signed-gas margin 500 bps |
| Changes | No implementation/configuration rollout, image rebuild, runtime restart, genesis change or datadir reset |

[Runtime identity](runtime-before.json), [process identities](runtime-processes.json),
[active settings](runtime-admission-settings.json), and [run parameters](rerun-plan.json)
pin the environment. Timing calibration was retained in the running processes.
The staged runner's shared wallet lock was respected.

Readiness initially lacked 0.305556056771081423 xDAI at the prior 0.00445
worker-prefund setting. The task-only worker prefund/floor was reduced to 0.004;
readiness then passed with existing funds. No additional poster-to-source transfer
was made. Source gas limits, workload and concurrency were unchanged. All 2,850
setup transactions mined and all 870 sources were sent, with no recorded send
failures. Original operator profiles and runtime environment are preserved.

The same task-owned loopback forwarding helper handles ordinary L1 setup
transactions at five upstream requests/second. Cross-chain fronts are unchanged.
This is a sequential live repeat, not an isolated performance experiment:
storage, historical backlog, source ordering, chain slots and host load differ.

## Completion and performance

- **870/870 sources canonical:** 520 inbound, 350 outbound, in **27 postings**.
- L1 source-posting range: **23,241,627–23,241,662**; six skipped slots.
- Send plus receipt-monitor window: **213.527 seconds**, or **4.074 sources/s**.
- Canonical settlement timestamps place median source completion at **91.983 s**,
  p95 **206.983 s**, maximum **211.983 s** after bulk-send start. Per-source send
  uncertainty is at most the 1.309-second bulk-send duration.
- 77 source heights changed from the monitor's optimistic receipts.
- Repeated attribution preserved all 870 sources and 27 postings; every L1 posting
  receipt matched the external canonical RPC ([stability](canonical-stability.json)).
- [Semantic verification](semantic-verification.json) passed all 770 jobs without
  receipt-wait intervention or agent source resubmission. The full setup/run/verify
  procedure ran from 17:04:39 to 17:30:29 UTC; verification is outside the measured
  source-settlement window.

The first complete candidate wave took 191.278 seconds at 4.548 sources/s.
This repeat took 11.63% longer, with 27 rather than 28 source postings. The binary
is identical; this difference is not evidence of a code performance regression.
The intervening 853/870 run remains an incomplete result and is not treated as a
complete-workload throughput benchmark.

## Cutoff reasons

[All cutoff counts and every posting](tables.md), [candidate details](candidate-cutoffs.csv),
and [cutoff aggregates](cutoff-summary.csv) distinguish retries from canonical work.
All 43 candidates examined at least one fresh wave source. The send-to-last-
settlement window contains no additional candidate without wave source events.
All event sequences are complete. Counts of accepted work across retries are
not unique-source counts.

| Cutoff | All candidates | Canonical postings | Canonical sources |
| --- | ---: | ---: | ---: |
| `pipeline_time` | 15 | 7 | 264 |
| `l1_tx_gas_budget` | 8 | 7 | 262 |
| `submission_backoff` | 9 | 5 | 76 |
| `fifo_fairness_limit` | 5 | 4 | 143 |
| `l1_bundle_gas_budget` | 4 | 2 | 73 |
| `pipeline_deadline_backoff` | 1 | 1 | 18 |
| `snapshot_exhausted` | 1 | 1 | 34 |
| **Total** | **43** | **27** | **870** |

Nine canonical postings were gas-limited, seven timing-limited, ten stopped at
fairness/backoff count caps, and one drained its finite snapshot.

### What the labels establish

- `pipeline_time` rejects more work when predicted remaining preparation,
  proving and submission no longer fit the shared target window. It is an
  admission prediction, not an actual proof/finalization timeout. These seven
  canonical posts used an average **66.5% of their postBatch planning cap**.
- `l1_tx_gas_budget` stopped seven canonical posts at an average **99.4% of the
  planning cap**. The cap was **15,378,300 gas**: the active 16,777,216 transaction
  ceiling, divided by the 1.05 signed-gas factor and rounded down, minus the
  600,000 drain reserve. Adding the next whole group exceeded it by **27,895 to
  414,580 gas**. Physical block space alone cannot override the transaction cap.
- `l1_bundle_gas_budget` checks the postBatch estimate plus drain reserve and the
  ordered inbound-companion allowance against the parent block gas limit. The
  allowance retains the maximum of preceding simulated consumption plus the next
  transaction's declared limit. It does not simply sum all declared limits.
- `fifo_fairness_limit` is a count cap after prior selection bypassed older
  eligible FIFO work. These four posts stopped at 31–40 sources, averaging
  **56.7% planning-cap utilization**. It protects ordering/fairness and is not a
  measured gas ceiling.
- `submission_backoff` shrinks selection after conclusive submission failure.
  These five canonical posts carried only 10–29 sources, averaging **23.0%
  planning-cap utilization**. It is not proof that a smaller batch was required
  by gas or preparation time.
- `pipeline_deadline_backoff` followed the actual first proof timeout and capped
  the next candidate at 18 rather than 36 sources.
- `snapshot_exhausted` accepted all 34 remaining sources. It is normal completion
  of that snapshot, not a capacity rejection.

The implementation paths are [automatic selection](../../../../crates/eez-composer/src/composer/admission.rs)
and [gas admission](../../../../crates/eez-composer/src/composer.rs).
`selection_limit_reason` can name an active cap which was never reached;
`admission_stop_reason` identifies the actual stop. Three canonical posts had an
active fairness cap but stopped first for another reason. A later proof failure
is reported separately from its earlier admission cutoff.

### Preparation overrun and proof timeout

Candidate `1-1790435613106:1:78` admitted 36 sources from a 56-source snapshot:
its [recorded phase boundaries](proof-timeout.json) are retained separately.

| Boundary | Elapsed / observation |
| --- | --- |
| Admission summary | 368.41 ms; `pipeline_time` cut, 36 accepted |
| Full encoded projection | 435.06 ms; 2 blobs, 37 entries |
| Proof request | **1,244 ms preparation**, 130 witnesses |
| Remaining shared time | **197.92 ms** |
| Reserved post-proof work | **150 ms** |
| Proof prediction | **450 ms**, `scaled_sample` |
| Actual proof failure | `prover_deadline`, after **49.35 ms** |

Preparation passed its nominal 1,000 ms allowance and continued into proving.
The remaining proof allowance was about 47.92 ms; this was an actual shared-
deadline failure, not a hard preparation timeout or a warmup posting. The next
18-source candidate prepared in 513 ms, proved in 152.86 ms and settled.

The gap from encoded projection to proof request was about 809 ms. The large
witness/blob workload is recorded, but these admission events do not isolate
every finishing subphase; do not attribute the entire gap to witness generation.
Across canonical posts, preparation ranged **208–836 ms**, median **402 ms**;
proof median **101.37 ms**, maximum **170.14 ms**. No canonical post exceeded the
nominal preparation allowance. No candidate used a cold timing probe.

Timing headroom was not uniformly generous. The tightest successful relay
acknowledgement left only **13.77 ms**. Another candidate had **238 ms** at
dispatch and timed out waiting for acknowledgement after 238.20 ms; its target
slot was absent. Loosening all timing guards would ignore these observations.

## Gas estimates versus execution

| Gas measurement, canonical postBatch cohort | Total |
| --- | ---: |
| Admission estimate | **275,063,662** |
| Full encoded projection | 277,128,562 |
| Final native preflight gas used | **178,005,930** |
| Final native preflight gas required | **182,218,454** |
| Signed transaction limits | 191,329,393 |
| Canonical receipt gas used | **178,005,930** |

Admission/receipt is **1.545×** (54.5% over receipt use), versus 1.550× in the first
complete candidate wave. Admission/preflight-required is **1.510×**. Native
preflight gas used matched the receipt for **every posting**, with zero absolute
difference. Required gas fit every observed signed postBatch limit, including
submitted candidates that did not settle. [Checked gas invariants](gas-invariants.json)
also verify all 520 inbound companion requirements against their declared limits.

The full projection prices the complete encoding, including the proof allowance
and calldata floor. It is still an estimate. Native required gas is a different
measure from receipt consumption and determines the signed limit with margin.
These totals exclude inbound companion transaction gas.

[Directional aggregates](directional-ratios.json) show where conservatism remains:

| Cohort | Postings | Admission / receipt | Admission / preflight required |
| --- | ---: | ---: | ---: |
| Outbound only | 8 | **1.206×** | **1.206×** |
| Contains inbound work | 19 | **1.987×** | **1.884×** |

### Two block-budget cuts with substantial actual headroom

| L1 block | Sources | PostBatch estimate | PostBatch receipt | Actual companions | Actual whole bundle | Block limit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 23,241,660 | 37 | 13,180,248 | 5,034,337 | 5,274,545 | **10,308,882** | 17,267,407 |
| 23,241,661 | 36 | 13,660,600 | 5,281,684 | 5,673,973 | **10,955,657** | 17,284,268 |

The next complete groups projected whole-bundle requirements of 17,299,025 and
17,337,598 gas, exceeding their **parent** block caps by 48,463 and 70,191 gas.
The inclusion block limit can differ from that parent limit.
[Exact rejected-group calculations](gas-rejections.csv) retain both values.
These are conservative cuts; this measurement does not execute an enlarged
counterfactual batch or establish exactly how many more groups would fit.

The batches were dominated by `revertCounter`, `revertFromOtherChain` and
`reentrant` fallback shapes ([composition](block-budget-examples.json)). Their
postBatch estimates were 2.618× and 2.586× actual receipt use.

However, the companion simulation terms were only **2,145,869 / 1,700,862 gas**,
versus **5,274,545 / 5,673,973 actual companion gas**. Across all 520 inbound
sources, that simulation term was 22,761,258 versus **73,314,510** measured by
both final native preflight and canonical source receipts. Overall conservative
batch estimates do not prove every component is conservative. Tightening the
postBatch reserve must also account for companion execution cost; retain final
native bundle validation.

### Per-source estimates and missing data

[Per-source estimates](source-estimates.csv) contain all 870 final accepted
sources, their transaction hashes, workload family, estimate components, pricing
reason, candidate/postBatch linkage and directly checked source execution gas.
[Family aggregates](source-families.csv) summarize those components. A source's
own receipt gas is **not** its share of postBatch gas; outbound source execution
is on L2. No exact allocation of aggregate postBatch receipt gas is claimed.

The 1,020 priced entries comprise:

| Pricing reason | Entries |
| --- | ---: |
| `deferred_storage` | 547 |
| `immediate_evidence_unavailable` | 300 |
| `immediate_registered` | 50 |
| `deferred_old_slot` | 49 |
| `deferred_reverted` | 38 |
| `deferred_call_flags` | 18 |
| `general_reserve_lower` | 17 |
| `deferred_legacy` | 1 |

There was no batch-size discount revocation. The 300 immediate fallback entries
still reserve 370,000 gas each; the 50 registered immediate entries reserve
105,000. `immediate_evidence_unavailable` combines unsupported execution and
missing registration; this label alone cannot identify which check failed.

All sources eventually have estimates. Across 1,501 source-attempt records,
1,474 were accepted, 12 failed gas admission, 12 passed gas but were not appended,
and three stopped at `pipeline_time` before an estimate. A blank estimate for
those three individual attempts is expected, not missing final-source coverage.

The dashboard's indexed source-receipt column is separately null for all 50
inbound `revertCounter` sources. The indexer stores receipts for transactions
with selected protocol events ([collection](../../../../monitoring/indexer/src/lib.rs));
reverting consumption can unwind those logs. Their directly checked canonical
receipts and native companion preflights exist and agree. All 50 successful
receipts contain zero registry logs ([receipt-gap evidence](source-receipt-gap.json)). The exported
`canonical_source_execution_gas` supplies that independently checked value;
the original nullable indexer column is preserved to expose the coverage gap.

## Non-inclusion and next improvements

[Attempt outcomes](attempt-outcomes.json) account for 43 candidates: 27 canonical,
one not submitted after proof timeout, six submitted to absent target slots,
and nine excluded from produced target blocks. **All nine produced target blocks
were empty**, all nine bundles were relay-acknowledged, and their signed postBatch
gas limits fit available block gas. The external reason for exclusion remains
unknown. An acknowledgement and a successful local preflight do not establish
builder/proposer inclusion or explain a rejection.

The evidence supports these next investigations, not a blanket gas/time discount:

1. **External inclusion and retry backoff:** distinguish builder availability or
   selection failures from genuine batch-capacity failures before shrinking
   selection. Current backoff still reduces batch size for non-inclusion even
   with ample physical block space. Preserve bounded recovery, source ownership
   and nonce dependencies; policy changes require the recovery contract and
   regressions to change together.
2. **Finishing-time prediction:** use historical/witness/blob work when reserving
   the tail before proof, and expose its subphase durations. The first candidate
   reserved a 450 ms proof but handed off with only about 48 ms available.
3. **Gas components together:** refine bounded reverted/flagged/unsupported entry
   shapes and model companion execution overhead explicitly. Split the combined
   immediate-evidence reason before assuming registration alone fixes it.
4. **Submission timing:** compare measured relay latency with its reserve. Native
   finalization and relay delivery are distinct; this run had a near-deadline
   successful acknowledgement and a delivery timeout.
5. **Grafana completeness:** collect canonical source receipt gas even when
   consumption logs were reverted, retain pre-simulation cutoff reasons, and
   expose source terminal disposition. Existing per-source estimates are complete
   for this wave; the receipt-coverage issue is separate.

These findings do not implement policy, estimator or dashboard changes.

## Evidence and reproduction

Use the existing [canonical measurement helper](../../2026-09-26-admission-throughput/measure-wave.py)
and retained indexer, then [analyze-cutoffs.py](analyze-cutoffs.py) with the measurement
output directory. The helper is retained because repeated candidate/source joins,
retry de-duplication and rejected-group budget reconstruction are error-prone.
Its output matched the first completed wave's prior canonical counts, cutoff
counts and gas totals before it was applied to this repeat. It performs read-only
RPC/SQL analysis through the existing measurement workflow and writes reports;
it does not send transactions or alter the indexer.

Raw snapshots, retained admission events, private profiles and workload logs stay
in ignored `artifacts/admission-deferred-rerun2/` and the protocol run directory;
they are local-only and unavailable in another checkout. Sanitized tables and
summaries are retained here. They do not provide independent live follower replay.
No implementation changed, so Rust/native/contract/CI/Kurtosis suites were not
repeated. JSON/CSV parsing, report cross-checks, local links/anchors, Python
compilation and `git diff --check` passed. The Python helper was exercised against
the first full wave and this full wave; that is read-only live-data evidence,
not a new runtime test suite. [Diagnostic hashes](diagnostic-evidence.json)
identify local retained inputs and raw evidence.

[Final health](deployment-final.json) confirms current nonsyncing heads, healthy
signer and an empty queue with the same images, processes and genesis. In this
run, completion is independently established by canonical attribution and the
semantic checks, not inferred from the empty queue. The [temporary helper](setup-forwarding.json)
was stopped and its port closed; it forwarded 1,397 ordinary raw transactions,
with zero upstream rate-limit responses and 2,366 `BrokenPipeError` client
disconnects. Disconnects are not successful calls. Its private profile refers
to the stopped helper and is not a reusable operator procedure. Runtime/operator
input hashes are unchanged. Unrelated operator edits are preserved.
