# Admission minimum checks and measured reserve refinement

| Field | Value |
| --- | --- |
| Date and status | September 27, 2026 UTC; implemented, locally validated, deployed and measured; 770/770 semantic jobs passed |
| Scope | Avoid repeated capacity-refused source simulation; tighten measured posting/companion reserves; expose cheap bounds and full simulations separately |
| Source baseline | `239b8224e04b049bb9303fc1bd756033a6b9f2a4`, branch `feat/cross-chain-admission-throughput`; deployed runtime `e5b2cde65c9166ceae87bd4ee723820ed63ea5f6` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Environment | Rust 1.94.0, Foundry 1.7.1; locked dependencies; local native process tests and existing Chiado deployment |
| Related records | [Accepted follow-up](../../proposals/cross-chain-admission-throughput.md#september-27-follow-up-cheaper-capacity-checks-and-tighter-reserves), [baseline and budget review](../2026-09-26-admission-packing/README.md), [admission contract](../../configurable-settlement-policy.md), [monitoring contract](../../../monitoring/indexer/README.md#admission-history) |

## Baseline and implementation

The preceding verified wave settled 870 sources / 770 semantic jobs using 28
postBatches. Seventeen bundle-limited postings consumed 43.0% of the actual
network block budget on average; their signed transaction-start envelopes needed
47.7%. The cap itself is the canonical parent block limit. Gas estimation, rather
than a percentage reduction of that cap, was the conservative component.

The baseline recorded 7,672 capacity-refused full source simulations. A replay
of the old estimator's minimum arithmetic identified 7,100 of these before
simulation. This is a retrospective arithmetic check, not a measured candidate
speedup; version 6 uses a lower inbound minimum and may reject fewer cheaply.

Policy `packing-minimum-check-3` checks current accepted budgets before source
execution. It preserves source ownership, sender/direction nonce dependencies,
whole groups and independent-source packing. Empty candidates run ordinary
classification. The bound concerns the admission model, not actual EVM gas.
Cheap misses do not train execution timing samples. No source execution result
is reused across a changed parent or accepted prefix.

Estimator `companion-storage-6` distinguishes zero, retained and fresh storage
words; fresh nonzero writes retain 23,000 gas. Measured protocol framing charges
are reduced, while matching-identity queue scans increase from 12,000 to 12,500.
Every root still reserves scans of the entire accepted queue prefix: successful
inner calls alone cannot prove that an outer source committed cursor progress.
Unsupported shapes retain their previous conservative fallback.

The new `source_capacity_check` event reports the minimum contribution, signed
requirements and active budgets. Grafana distinguishes these from full estimates
and shows full simulation and cheap rejection counts. Upgrade the strict
admission-aware collector and additive schema before the new Composer; keep
them on Composer rollback. Existing JSON history and checkpoints are preserved.

Exact whole-bundle execution currently follows proof production and prepared
attempt ownership. An extra near-cutoff execution pass would require another
bounded prove/rebuild lifecycle and a proved fallback under the same target.
This iteration retains final exact native execution and does not implement that
extra proof cycle or bypass proof validation.

## Executed local checks

Commands run from the repository root. Raw logs are local-only under
`artifacts/admission-refinement/`, unavailable in another checkout. Final compact
results and executable identities will be added after validation.

| Check | Result |
| --- | --- |
| Unchanged baseline gas fixtures, Prague | 50 passed |
| Revised gas fixtures, Cancun / Prague / Osaka | 50 passed on each fork |
| All integration contract tests | 69 passed, zero skipped |
| Composer library tests | 264 passed; one existing manual test ignored |
| Indexer locked workspace, including owned PostgreSQL recovery test | 26 passed, zero ignored |
| Native admission/queue process selection | All 48 intended cases validated: 13 unchanged first-run passes plus 35 corrected/remaining passes; no ignored selected tests |
| Workspace and indexer clippy | Both passed; workspace rerun after the test-only correction also passed |

First failures are retained. The initial Composer run exposed a stale test
expectation for a zero-to-nonzero packed flag: it now changes the base charge as
well as adding the fresh allowance. The assertion was corrected to the full
20,000 delta; 264 tests then passed. A sandboxed indexer run could not bind its
mock HTTP server; the authorized run with loopback and the owned disposable
PostgreSQL database passed all 26 tests. Initial workspace clippy rejected an
inverted `if` layout; the equivalent branches were reordered. Native compilation
was deliberately interrupted before tests to validate the corrected source.

`forge fmt --check --root contracts` reports existing formatting differences in
unchanged `ECDSAProofSystem.sol` and `DeployRollup.s.sol`; these are outside this
change. The edited gas fixture itself has no formatting difference.

## Operational work and remaining acceptance

Four local shipping images are built from archived runtime revision `7fdb3fc`;
[image and executable identities](images.json) include verified unchanged genesis
hash/root. The same local release recipe, locked dependencies and production
binary selection as the measured baseline were used. These are local development
images, not CI-promoted release artifacts.

The additive views and collector image were upgraded first. All 55,231 retained
admission events and seven file checkpoints remained present; mounts, command
and environment were preserved. Grafana queries against the preceding wave
returned 33 candidates / 8,771 source rows with the new nullable fields. The
Composer remained on the baseline until the native process gate passed.

The first native run encountered stale version-5 assertions shared by eight
single-Sync cases. The first cold wave admitted and independently replayed all
30 sources in one batch before the version assertion. Test-only commit `2d64c7d`
updates both explicit version checks and the reserve floor to version 6; runtime
source is unchanged. The first dynamic suite finished with 13 passes and eight
failures, all at line 663 comparing emitted `companion-storage-6` with expected
`companion-execution-5`. The [diagnosis](native-first-diagnosis.json) and
[initial executable identities](native-first-executables.json) are retained.
The eight corrected cases plus 27 remaining cases all passed serially.
[Final results](native-results.json) reconcile the exact 48-case inventory and
retain both first-run and corrected evidence; [final executable identities](native-executables.json)
identify the source-built binaries. This is a deterministic test correction, not
a retry-only pass attributed to flakiness. The [selection](native-plan.json)
lists exclusions: 20 unchanged ordinary-only settlement cases were not run.
Semantic assertions and independent follower replay ran in the selected native
suites, including gas cuts, complete groups, nonce dependencies, partial
consumption, restart and reorg recovery.

[Local checks](local-checks.json) retain compact results and hashes of local-only
logs. [Gas comparisons](gas-fixture-comparison.csv) cover 27 logged companion
cases: actual gas is identical at baseline/candidate, 26 reserves decrease and
one increases. Every measured case fits; the smallest reserve/execution ratio is
1.093893. All 50 fixtures also pass on each of the [three forks](gas-test-results.json).
This bounded measurement does not establish a universal EVM gas guarantee.

The staged read-only readiness check initially found a 0.114244 xDAI shortfall
against its upfront reserve. The prior wave's maximum sum of signed gas-plus-value
requirements was 0.003000000042 xDAI per worker on L1. Task-only refundable worker
prefunding was lowered from 0.0035 to 0.0033 xDAI; the profile is otherwise byte
identical. Readiness then passed with an empty queue, aligned worker nonces and
sufficient existing source funding. No poster-wallet top-up was made. See the
[prefund check](prefund-bound-check.json) and [profile adjustment](prefund-adjustment.json).
This difference must be included in the live comparison.

At 08:31:32 UTC the existing node and signer completed their image-only rollout
to the verified `7fdb3fc` images. The queue was empty, the previous node stopped
with exit code zero, and mounts/environment were preserved. Only the two runtime
image references changed in the operator dotenv. See [rollout](final-runtime-rollout.json).

The first freshness check found the same peerless embedded-L1 restart gap as the
baseline rollout. The bounded recovery helper verified 17 raw canonical payloads
against external execution headers/transaction hashes and local Beacon agreement.
All 17 Engine imports returned `VALID` with the exact payload hash; Lighthouse
retained fork-choice ownership. No datadir, genesis or contracts were replaced.
[Recovery evidence](engine-gap-recovery.json). The subsequent [deployment check](deployment-verification.json)
found fresh, nonsyncing L1/L2 heads, canonical agreement, healthy signer, unchanged
genesis and an empty queue. The verified staged wave ran with the shared worker
lock and the same 5 requests/second ordinary-L1 forwarding method. It exited zero
at 09:01:40 UTC. The final [deployment check](deployment-verification-final.json)
again found healthy/current services, canonical agreement, unchanged genesis and
an empty queue. The task-owned forwarding helper was stopped after the shared
wallet lock was released. Its [final counters](ordinary-forwarder-final.json)
show no upstream rate limits; 2,485 client disconnect/broken-pipe responses were
recorded during concurrent setup reads, comparable with 2,489 in the baseline.
All setup sends and source/semantic checks passed; these responses are not
reported as transaction failures.

The two requested implementations and their local rollout/measurement are
complete. The separate additional proof/refinement cycle remains a future
optimization, as scoped above. Preserve operator
configuration, contract/genesis identity, datadirs and retained monitoring logs.
Live performance comparisons share mutable chain state and cannot establish a
causal speedup from one run. The extra proof/refinement cycle remains separate.


## Canonical wave and gas comparison

Protocol run `20260927-083400` settled **870/870** sources (520 inbound, 350
outbound) in **26 canonical postBatches**, L1 blocks **23251717–23251744**.
The repeated source/block attribution was unchanged, and all 26 receipts matched
the independent external canonical L1 endpoint. The staged `--verify` runner exited zero, and all **770/770 semantic jobs** have
passing timing records and the complete verifier marker. No job was skipped or
manually resent, and no post-run re-verification workaround was needed. The
existing file-based transport handled large calldata using the unchanged pinned
verifier. [Semantic results](wave/semantic-verification.json), [run record](wave/run-status.json).
Canonical assignments still matched after verification completed.

| Metric | Baseline `e5b2cde` | Candidate `7fdb3fc` |
| --- | ---: | ---: |
| Canonical postBatches | 28 | 26 |
| Sources per postBatch | 31.07 | 33.46 |
| Send plus receipt monitor | 166.540 s | 152.930 s |
| Canonical sources per measured second | 5.224 | 5.689 |
| Median / maximum preparation | 483 / 744 ms | 456 / 673 ms |
| Full source-estimate events | 8,768 | 2,406 |
| Gas refusals requiring full simulation | 7,672 | 1,360 |
| Cheap capacity refusals | 0 | 6,601 |
| PostBatch estimate / actual receipt gas | 1.477× | 1.348× |
| Companion estimate / actual receipt gas | 2.298× | 1.977× |
| Multi-call nested estimate / actual | 3.118× | 2.670× |
| Multi-call twice estimate / actual | 3.027× | 2.612× |
| Skipped slots between first/last source posting | 2 | 3 |
| Produced blocks without a wave posting | 2 | 2 |

This is one historical baseline and one live candidate wave, with identical job
counts, release build profile, sending concurrency and ordinary forwarding rate.
It is not a randomized repeated A/B benchmark. Refundable prefunding, mutable
history/storage, slot availability and active block budgets differ. Candidate
parent budgets were 17.066–17.454 million gas, versus 17.522–17.955 million in the
baseline. Treat the observed 7.1% fewer posts and 8.2% shorter measured window
as this run's result, not a guaranteed speedup. No full-wave independent live
follower replay was run; the selected native suites include independent replay.
[Comparison](wave/comparison.json), [canonical results](wave/canonical-summary.json),
[external/stability checks](wave/canonical-stability.json).

### Cheap decisions and estimate coverage

There were 31 candidate attempts and 9,007 examined source-attempt rows. All event
sequences were complete. Exactly 6,601 rows were cheap refusals (2,271 transaction
budget, 4,330 bundle budget), with a recorded minimum bound and no fabricated full
estimate. Exactly 2,406 source simulations ran: 1,044 accepted attempts, 1,360 gas
refusals and two pipeline-time deferrals after passing the gas check. Accepted
attempts include retries; only 870 distinct sources canonically settled.

Cheap checks handled 82.9% of all 7,961 gas refusals; full gas-refusal simulations
fell 82.3% from the baseline. The larger total refusal count reflects repeated
candidate examinations, not lost or charged transactions. Every cheap outcome
and candidate counter reconciles with retained events. Eight canonical candidates
accepted ten additional independent sources after an earlier capacity refusal,
versus three such sources previously. [Minimum-check totals](wave/minimum-checks-summary.json),
[per-candidate counters](wave/minimum-check-candidates.csv), [packing totals](wave/packing-summary.json).

All 870 canonical sources have full accepted estimates, all 520 inbound sources
have canonical receipts and bounded companion components, and all 26 postings
have complete admission/preflight/receipt joins. Their postBatch preflight gas
used equals receipt gas exactly; required gas fits the signed limits. No proof or
preflight failure event occurred in the measured candidate cohort, and all 31
delivery observations were acknowledged.
No FIFO count cutoff, cold timing probe or nominal preparation overrun occurred.
The five unconfirmed candidate attempts are retained separately from canonical
postings; a successful relay acknowledgement alone does not establish inclusion.

Live Grafana queries returned all **31 candidates and 9,007 source-attempt rows**,
including minimum costs, limiting budgets, simulation counters and cheap-check
timing. Missing full estimates on cheap rows are intentional and explicitly
labelled **Capacity deferred before simulation**. Historical fields remain null.
[Grafana query evidence](wave/grafana-verification.json).

### What the cutoff reasons show

| Final stop reason | Canonical batches | Canonical sources |
| --- | ---: | ---: |
| `l1_bundle_gas_budget` | 16 | 536 |
| `l1_tx_gas_budget` | 5 | 177 |
| `pipeline_time` | 2 | 82 |
| `submission_backoff` | 2 | 31 |
| `snapshot_exhausted` | 1 | 44 |

Bundle-limited candidates nearly fill the estimate-based start envelope (98.8%
on average), while actual bundle gas uses 54.7% of the parent budget and the
actual signed transaction-start envelope needs 59.6%. Previously those actual
fractions were 43.0% and 47.7%. The network cap remains appropriate; the remaining
headroom is evidence to refine estimates, not permission to exceed that cap.
PostBatch-transaction-limited candidates use 99.8% of their postBatch planning cap.
The two pipeline cuts retain finishing/proof/delivery headroom; their measured
median preparation is 630 ms. Submission backoff still produces two small
batches (13 and 18 sources) after exclusion; this change preserves that policy.
Snapshot exhaustion is completion of the finite selection, not a resource limit.
[Cutoff table](wave/cutoff-summary.csv), [per-post budget envelopes](wave/bundle-budget-per-post.csv).

Multi-call estimates are improved but still roughly 2.6× actual gas. Full-prefix
queue scan reservations remain conservative, since inner-call success does not
prove committed cursor progress. Further pricing or a bounded prove/rebuild
cycle needs its own evidence and lifecycle checks. The median time reduction is
modest despite fewer simulations: accepted work, encoding and witness/proof-request
preparation remain necessary and candidate sizes grew.

## Reproduction and evidence availability

The small summaries, gas comparisons, native inventories, image identities and
26-row admission-versus-receipt table are checked in. Full source CSVs, event
JSON, build/test logs, private profiles and retained native datadirs are local-only
under `artifacts/admission-refinement/`, the protocol run directory
`eez-core-protocol/tmp/e2e-staged-net/20260927-083400/`, and
`data/admission-refinement-20260927/`;
they are not supplied by another checkout. The three audit analysis scripts are
retained because repeated cohort reconciliation and nullable cheap/full-event
accounting are error-prone. Per-source CSVs are available locally at
`artifacts/admission-refinement/candidate/detail/source-estimates.csv` (870 accepted
sources), `source-admission.csv` in the candidate directory (all 9,007 attempts),
and `detail/minimum-checks.csv` (6,601 cheap refusals); Grafana supports filtering
and CSV export of the same retained history. They consume the canonical measurement and admission
exports from the linked earlier audits; they do not broadcast transactions.

Runtime builds and tests used Rust 1.94.0 with `--locked`; source process tests
cleared prebuilt-binary overrides and ran serially. Native checks selected the
seven targets in `native-plan.json`; the exact commands and source/executable
identity are recorded in the native result files. The standalone indexer used
its own locked workspace and an owned disposable PostgreSQL 18.6 database; that
test container and its owned volume were removed after all 26 tests passed.

The full CI release-artifact/Kurtosis matrix, unchanged ordinary-only settlement
cases, core-protocol test suite and full-wave live follower replay were not run.
The protocol pin, core implementation, ABI, signer profile, wire schema, runtime
storage and dependency locks were unchanged. Local builds are not CI promotion.
The first test/assertion and operational failures remain documented above.


Reproduce analysis from the repository root after using the read-only canonical
and admission exporters linked in the prior audit:

```bash
python3 docs/audits/2026-09-27-admission-refinement/analyze-packing.py artifacts/admission-refinement/candidate
python3 docs/audits/2026-09-27-admission-refinement/analyze-minimum-checks.py artifacts/admission-refinement/candidate
python3 docs/audits/2026-09-27-admission-refinement/analyze-bundle-budgets.py artifacts/admission-refinement/candidate
```

These commands require the local-only input exports; they do not reconstruct
missing historical estimates. The [26-row admission/receipt table](wave/admission-vs-receipt.csv)
is checked in for direct review. The full 9,007-row export remains local and in
Grafana rather than being committed as a bulk log.
