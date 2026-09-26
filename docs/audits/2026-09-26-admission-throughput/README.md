# Admission throughput implementation and validation

| Field | Value |
| --- | --- |
| Date/status | 2026-09-26; local validation and live comparison passed; local runtime `b352124` deployed |
| Authorization | User requested a new branch, implementation, local tests, local images, deployment and measurement with correctness checks |
| Branch/base | `feat/cross-chain-admission-throughput`, fork baseline `7c90266a5fe8cfdb46b249c61079badc2dce71a3` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`; no pin/ABI changes |
| Toolchain | Rust 1.94.0 with locked dependencies; Foundry 1.7.1 |
| Design | [Accepted implementation direction](../../proposals/cross-chain-admission-throughput.md), [runtime policy](../../configurable-settlement-policy.md), [RPC cutoff](../../../crates/eez-control-rpc/SPEC.md#63-retry-procedure-and-settlement-cutoff) |
| Earlier evidence | [September 25 waves](../2026-09-25-latest-wave-analysis/README.md) |

## Implementation

- Exact-target cross-chain preparation and proving share the exclusive target
  timestamp minus submission slack. Finalization/delivery are reserved. Typed
  retries and cache repair stay inside the original window; uncertain delivery
  retains ownership. Ordinary-only and `NextBlock` timing behavior is preserved.
- Cold automatic admission considers the finite queue without a mandatory
  one-source calibration post. Preparation overruns alone do not discard an
  accepted candidate or halve capacity. Bounded workload-aware timing history
  uses covering samples or bounded upward proof scaling across group/blob/witness
  dimensions, with prediction basis recorded for analysis.
- Gas admission discounts fixed words observed nonzero in retained deferred
  storage at the exact parent. Unknown shapes and dynamic slots keep existing
  reserves. Full-ABI sizing, signed gas checks, native complete-effect validation,
  grouped calls and original target binding remain authoritative.
- A canonically proven skipped target does not measure capacity, including when
  replacement consumption makes the old source dispositions mixed. Produced
  targets still apply normal partial-consumption checks.
- Candidate/source gas and timing events use the retained tracing sink and the
  existing collector checkpoint. Additive SQL views and **EEZ Cross-chain
  admission** show estimates versus final preflight and canonical receipt gas,
  source decisions, rebuilds, delivery and missing-event coverage.
- The staged workload wrapper adds `--verify`, preserving its shared-wallet lock
  and default throughput-only mode.

## Validation history

All large artifacts below are **local-only**, under ignored
`artifacts/admission-throughput/`; they are not available in another checkout.
A sanitized final summary and executable/image identities will accompany the
completed validation record.

| Check | Kind | Result |
| --- | --- | --- |
| Baseline `automatic_selection_learns_once_then_uses_finite_snapshot_and_actual_time` in a tracked-source archive | Unit | 1 selected/passed; establishes the original one-source cold selection (`baseline/cold-admission.log`) |
| Composer/driver/L1 unit suites after shared deadline and delivery wiring | Unit/local RPC | 237 Composer passed, 1 existing ignored; 57 driver passed; 141 L1 passed (`runtime-unit-tests.log`) |
| Retained-word gas helper and workload calibration | Unit | Later Composer runs: 238 and 239 passed, 1 existing ignored; further changes require final rerun |
| Original gas pin fixture | Embedded EVM | 22 passed (`gas-pins-baseline.log`) |
| Retained-word transitions: old/new 0/31/32/64-byte return data, 1/10/127 entries, changed hashes/roots, pinned storage layout | Embedded EVM | 26 passed (`gas-pins-candidate-2.log`); native repeated-wave validation pending |
| Indexer existing tests | Unit/mocked HTTP | Passed after allowing disposable localhost servers; database recovery test intentionally opt-in |
| Admission schema, replay, duplicate/conflict atomicity, gaps and every actual dashboard SQL statement | Unit/disposable PostgreSQL | 2 passed, zero skipped (`admission-db-tests-2.log`) |
| Staged wrapper profile and semantic-mode forwarding through wallet lock | Mocked shell | 4 passed (`staged-runner-tests.log`); does not establish live correctness |
| Cold/warm Gnosis waves with follower | Native process | 1 selected/passed: 30 sources in one first posting without calibration, then 30 in one second posting; canonical receipts, values/nonces and independent replay asserted (`native-cold-gnosis.log`) |
| Workspace Clippy, all targets | Static | Passed with `-D warnings` (`workspace-clippy.log`) |
| Cancun/Prague/Osaka gas pin fixtures | Embedded EVM | 26 passed per fork, zero skipped (`gas-pins-{cancun,prague,osaka}.log`) |
| Existing indexer canonical restart/reorg/checkpoint recovery | PostgreSQL/mocked RPC | 1 selected/passed (`indexer-recovery.log`) |
| Chiado configuration propagation | Mocked Compose/config | 13 passed; Chiado and Kurtosis policy checker passed |
| Final Composer/driver/L1/RPC/client unit run at `3cb712c` | Unit/local RPC | Composer 239 passed/1 manual profile ignored; driver 58, L1 144, control RPC 6 and prover client 30 passed. `--lib` did not select the signer binary; its separate run passed 330 tests, with two diagnostic utilities intentionally ignored (`signer-tests.log`). |
| Complete integration contracts | Embedded EVM | 45 passed, zero failed/skipped (`contracts-full.log`) |
| Cold timing probe correction | Unit | Composer 240 passed, 1 existing manual timing profile ignored (`cold-probe-composer-tests.log`) |
| Final Composer proof prediction and floor at `7b36ffe` | Unit | 242 passed, 1 existing manual timing profile ignored (`final-composer-tests.log`) |
| Final workspace/indexer Clippy and formatting | Static | All targets passed with `-D warnings`; both workspaces format clean (`workspace-clippy-final.log`, `indexer-clippy-final.log`) |
| Proof prediction SQL, populated and historical-null rows | PostgreSQL | 2 passed, zero skipped (`admission-db-predictions-final.log`) |
| Skipped slot and late original at `088c9d5` | Native process | Both original regressions passed with unchanged timing/assertions; final floor refinement is included in the broader final run |
| Grafana browser validation | Browser/PostgreSQL | All seven panels render; candidate/source filters, SQL metacharacter literal, canonical CSV export, missing/open history and collector outage checked. Updated prediction panel renders without SQL errors. |

### First failures retained

Sandbox socket restrictions prevented 13 L1 RPC tests and one indexer HTTP test
from binding localhost. Their logs were retained; the authorized local-server
runs subsequently passed. These were environment failures, not retry-only
application passes. An initial new Solidity test combined all retained-slot
transitions into one test and exhausted the test transaction gas budget. It was
split into four tests by prior return-data shape, preserving every transition;
all pass. Initial compile errors while extending internal test fixtures and the
submitter helper are retained in iteration logs.

The first full native admission run at `3cb712c` exposed two recovery fixture
failures and a real cold-estimate starvation defect. The old relay-hold fixtures
assumed acknowledgement could remain pending past the delivery deadline; their
ordinary transaction was first sequenced after reconciliation, so there was no
orphan to recover. Retained canonical blocks/receipts and follower logs establish
that ordering. The corrected fixture independently interrupts canonical header
observations and asserts that its predecessor descends from the held Sync.

The skipped-slot test's initial source exposed the starvation defect: the
500/1,000/1,500 ms preparation/proof/slack configuration repeatedly selected zero
groups because the cold reserves consumed the entire planning window. The
bounded first-group timing probe described in the proposal corrects that case.
The obsolete full run was stopped after those failures; its logs/datadirs remain
under `native/`, with ownership/termination recorded in `native-initial-stop.json`.
It is not reported as a passing suite. The corrected `f37fb4f` run passed
19 of 20 cases; both recovery fixtures passed. Its remaining skipped-slot failure
showed that increasing witness counts repeatedly restored the cold proof seed,
preventing the expected next-slot replacement dispatch. The subsequent correction
scales recent proof observations upward across all three dimensions, capped at
the greater of the configured seed and observed proof duration with headroom.
The focused skipped-slot regression passed at `088c9d5`; the final refinement
ensures extrapolation cannot fall below a measured slow proof. The original
failure remains in `native-dynamic-admission-corrected.log`.

Actual Grafana rendering caught a nested `$__timeFilter` expansion failure in
the timeline panel that the SQL-only check did not model. Moving its timestamp
expression into a subquery lets Grafana filter a simple column. The original
browser failure is retained as `grafana-initial-macro-failure.json`; all seven
panels subsequently queried successfully. Filter/export/outage checks use only
the owned disposable database and Grafana on localhost port 19301.

## Operations and remaining validation

The original operator configuration, keys, datadirs and running Chiado services
have been preserved. A separate owned PostgreSQL container,
`eez-admission-test-db-20260926`, uses localhost port 19433 and database
`eez_indexer_test`; production monitoring uses a different container/port. No
production schema reset or contract deployment is part of this change.

At 07:56:49 UTC, the additive indexer upgrade was applied using an image-only
Compose override, after comparing the rendered environment and log mounts with
the active service. Image `eez-indexer:3cb712c-local` is
`sha256:9e17c58af36478fdf5ed7a7769166807bf3dd4eac443e9b8248fb2cd5c5f93d8`.
History/checkpoints were retained, the restricted Grafana reader has access to
the new views, and the collector reports no error. Prometheus configuration was
checked with `promtool` and reloaded. At that point the node/signer remained at baseline pending
corrected native validation and image rebuilds; the executed rollout is recorded below.

Baseline live run `20260926-075533` uses the same 14-target/870-source workload
and current configuration, with the new wrapper's semantic verifier enabled.
It needed 0.47 L1 and 0.1 L2 test-token top-ups from the established funding
accounts. Setup again stalled on local L1 propagation. All 352 original pending
L1 setup payload hashes and network identity were checked before relaying to
the existing external endpoint. The first concurrent relay accepted 297 and
received 55 HTTP errors; the remaining 55 were rechecked and accepted sequentially.
No signatures, nonces, fees or values changed. Source send plus monitoring took
201.354 seconds, with 870 successful source receipts; all 770 per-job semantic verifications subsequently passed. Raw artifacts and each relay result remain local-only.

The final local unit/static/process checks and image rollout are recorded below.
The candidate wave, semantic verification, canonical comparison and populated
Grafana checks subsequently passed; final results are below. Earlier receipt-only waves
are performance context and do not replace those correctness checks.

## Live baseline (original runtime)

The original `7c90266a5fe8cfdb46b249c61079badc2dce71a3` runtime processed the
locked, semantically verified staged run `20260926-075533` on existing rollup 1,
L1 Chiado 10200 and L2 6292. It completed at 08:18:04 UTC: all 770 per-job
semantic checks passed, all 870 sources (520 inbound / 350 outbound) had
canonical successful receipts. Independent canonical block/receipt and settlement
range inspection corrected 146 source heights recorded before reconciliation.

| Baseline measure | Result |
| --- | --- |
| Send plus source monitoring | 201.354 s |
| Source throughput | 4.321/s |
| Postings containing workload sources | 30 |
| Canonical L1 block span | 23235692–23235725 |
| Skipped slots inside that span | 4 |
| Total postBatch receipt gas | 177,245,257 |
| Per-job semantic verification | 770 passed, zero failed |
| Bulk send to canonical settlement p50 / p95 | 115.362 / 195.362 s |
| Per-source send uncertainty | At most the 1.226 s bulk send duration |

Preparation/setup and semantic verification are outside the measured source
window. Setup needed relay of original signed setup transactions after local
propagation stalled; chain/genesis/common-head identity, each raw hash, active
wallet lock and sender nonce order were checked. An initial parallel relay had
55 HTTP failures; the sequential remainder succeeded. No source transaction was
re-signed or repriced. Source-window logs, canonical CSVs, private block/receipt
snapshots, funding and relay evidence are retained under `baseline/`. This is one
live baseline cohort; candidate comparisons must account for skipped slots,
network timing and runtime build profiles.

## Local image build identity

Early `3cb712c-local` and `f37fb4f-local` images used separate package builds.
Comparing Cargo unit graphs against `Dockerfile.runtime` found four dependency
feature differences. Those node/signer images were not deployed. The subsequent
shipping build uses the exact five packages, three binaries and four examples
selected by `Dockerfile.runtime`, Rust 1.94.0, locked dependencies and the release
profile. It uses local cargo-chef/mold caching and omits CI test-archive generation.
Each context contains only an archived commit and its recursive pinned protocol
submodules; operator files are excluded. Recipes, commands and image IDs are
retained in `images-<revision>-shipping/`. The final source/image identities and later executed rollout are recorded below.

### Repeating the canonical measurement

The read-only [measurement helper](measure-wave.py) rechecks source block bodies
and receipts, block hashes, and exact canonical L2 settlement ranges. It exists
because the upstream runner's initial mined heights can change during
reconciliation. It is intentionally scoped to these 870-source, L2 6292 cohorts
and rejects ambiguous multiple postings in one L1 block. It does not replace
per-job semantic assertions or independent follower replay. Retain private
output directories; full blocks/receipts are not committed.

```bash
python3 docs/audits/2026-09-26-admission-throughput/measure-wave.py \
  eez-core-protocol/tmp/e2e-staged-net/20260926-075533 \
  artifacts/admission-throughput/baseline
```

Run the same command with the candidate run/output directories after its source
monitor completes. The summary reports send plus monitoring time separately
from canonical settlement timestamps. Per-source send uncertainty is bounded
by the bulk-send duration; neither timestamp measure is prover-only latency.

### Telemetry scope refinement before deployment

Review of the candidate emitter at `7b36ffe` found that it logged the whole
pending snapshot and every untouched deferred source on each attempt. That
created repeated work proportional to the backlog rather than examined groups.
The refinement records sources when they reach individual checks and includes
examined, unexamined and selection-filtered counts in the summary. It also fixes
a doubled queue count: `candidates()` leaves sources in the queue until claimed.
Source checks-to-estimate elapsed time is now attributable to an explicit start
marker; earlier records without that marker remain null. Scheduling, gas gates
and ownership transitions are unchanged by this telemetry refinement. Its
database checks passed (2 tests, zero skipped), all seven Grafana panels
rendered, and workspace/indexer Clippy passed. The native telemetry capture
and final image rebuild are pending.


### Final local build and pre-rollout checks

The final runtime revision is `b352124d80f7ba9309fbb529450b7ba5270e16f7`.
[Local image identities](local-images.json) record all four image IDs, source and
protocol labels, runtime binary hashes and the existing genesis comparison.
These are locally built development images, not CI-promoted artifacts. The
shipping package/feature selection matches `Dockerfile.runtime`; the deploy
image was built for matching tools but no contracts are redeployed.

The final 40-case admission/gas/queue/reconciliation process run passed with
zero failures or ignored cases. Its runtime/test binaries were built at
`7b36ffe`, before the telemetry-only refinement; the subsequent focused policy,
semantic and bounded-telemetry checks use `b352124`. The full 20-case dynamic
admission target passed, including the original skipped-slot and late-original
regressions. Logs and failure-preserving datadirs remain local-only.

At 09:43:57 UTC the collector was replaced with the verified `b352124` image.
Rendered configuration comparison allowed only its image reference to change;
container environment and mounts match the preceding instance. The collector
retained 2,273 posting events and reported no error after restart. Node/signer
rollout and candidate measurements follow the remaining process checks.

The [compact canonical baseline](baseline-summary.json) and
[30 canonical baseline postings](baseline-postings.csv) are retained in this
checkout. Large source receipts and block bodies remain local-only. The
baseline was an already-running process; the candidate will be restarted.
One live cohort per revision cannot isolate host load, cache state or network
variance, so the comparison must report observed results rather than a
repeatable speedup guarantee.


### Executed runtime rollout and execution catch-up

[Runtime rollout](runtime-rollout.json) records the image IDs, container IDs,
clean node exit, unchanged mounts/environment values, and the 0.1 test-xDAI
workload top-up. The first local preflight parser incorrectly assumed every
Docker environment entry contained `=`; explicit unset entries were normalized
before continuing. The post-recreation comparison then detected array ordering,
not changed values. Both checks were corrected and independently compared by
key/value before measurement. Funding was sent once, with the node's poster
stopped and its latest/pending nonces equal. The signer is healthy.

The restarted embedded L1 stalled with zero execution peers despite both
configured peer TCP endpoints being reachable. Metrics recorded repeated
remote capacity rejections, connection failures and short-lived sessions.
The other eight bootnodes in pinned `reth_gnosis` were unreachable in the
bounded TCP check. Beacon remained connected, but optimistic while execution
could not fill its missing parents. No peer configuration or firewall changed.

The initial read-only Beacon recovery preparation failed: retained local
blinded blocks could not reconstruct missing execution payloads, and the
configured checkpoint source did not serve those historical blocks. That
attempt submitted no payloads. The existing external execution RPC supplied
canonical raw bodies instead. The [executed helper](recover-engine-rpc-gap.py)
is retained because a short peerless execution gap is a concrete recovery gap
in the available procedures and the conversion/validation must be reproducible.
It is narrowly scoped to this installation and rejects nonempty execution
requests rather than guessing their encoding.

Before import, the helper checked chain/genesis identity, the starting canonical
hash, every raw RLP round trip and header/transaction hash, sequential parents,
blob versioned hashes, empty requests commitments, and agreement of the terminal
hash with the local Beacon's head. It then submitted 195 payloads through the
standard [Engine API V4](https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_newpayloadv4),
requiring `VALID` and the exact `latestValidHash` for each. Lighthouse retained
fork-choice ownership; no fork-choice call, data reset, contract deployment,
state override or peer-setting change occurred. [Compact recovery evidence](catch-up-recovery.json)
records the exact range and full-evidence digests. Raw bodies and full responses
remain local-only under `artifacts/admission-throughput/engine-rpc-gap/`.

The subsequent staged readiness check passed: both layers synchronized, worker
nonces aligned, sufficient funding and an empty cross-chain queue. This
establishes recovery for this bounded gap, not reliable execution peering on
future restarts. The candidate workload started as run `20260926-103224` with
semantic verification enabled and unchanged concurrency/profile settings.


### Candidate setup propagation failure retained

Candidate attempt `20260926-103224` stopped in funding after 122 seconds: the
original L1 source-to-faucet transfer remained local with zero gossip peers and
the runner timed out. No measured cross-chain sources were submitted. Its
failed status/logs remain under local-only
`artifacts/admission-throughput/candidate-setup-failed-103224/`; this is not a
passing performance cohort. The exact original transfer was checked against
its sender, recipient, value, chain ID, raw hash and common canonical head,
then relayed through the existing external RPC and confirmed locally.

Fresh attempt `20260926-103808` preserves the same workload profile, 200 prepare
workers, 28 send workers, four verifiers and source ingress routes. Its eight
original L1 worker-funding transactions also needed relay. Each calldata and
value was compared against its 100-worker slice of `funding-L1.csv`, with
original signatures/hashes/nonces retained. The existing runner held the
wallet lock throughout; no new funding transaction was signed by the helper.
Contract setup then proceeded. These operations precede the measured source
window and must remain visible in the comparison's infrastructure caveats.


### Final local check totals

[Process validation](process-validation.json) preserves exact commands, selected
counts and runtime revisions: 40 admission/gas/reconciliation cases at `7b36ffe`,
six policy and six semantic cases at `b352124`, all passed with zero ignored.
The additional instrumented blob-split repeat at `b352124` passed semantic and
independent follower assertions and checked six candidate summaries, four
unexamined suffixes, and 11 source timing observations. The final Composer run
at `b352124` passed 242 tests; one existing manual profiling test was ignored.
Workspace and indexer Clippy/format, SQL tests and actual Grafana rendering had
already passed after the final code edits. Full CI/Kurtosis release promotion
is not part of this local development deployment and has not been run.


## Final result

The runtime remains on `b352124d80f7ba9309fbb529450b7ba5270e16f7`, with the
unchanged pinned protocol and operator timing/admission settings. Subsequent
commit content is evidence, documentation, clarification of the skipped-target
comment, and dashboard column ordering; it does not change the measured
executable behavior. The mounted dashboard puts admission and receipt gas next
to each other and was rechecked in the live browser after provisioning reloaded.

| Measure | Baseline `7c90266` | Candidate `b352124` |
| --- | ---: | ---: |
| Canonical sources | 870 | 870 |
| Per-job semantic assertions | 770 passed | 770 passed |
| Source send + monitor | 201.354 s | 167.797 s |
| Sources/second | 4.321 | 5.185 |
| Canonical postings containing sources | 30 | 27 |
| Produced blocks from first to last posting | 34 | 30 |
| Skipped slots in that span | 4 | 4 |
| Bulk send to settlement p50 | 115.362 s | 97.011 s |
| Bulk send to settlement p95 | 195.362 s | 162.011 s |
| Total postBatch receipt gas | 177,245,257 | 179,639,982 |

This cohort observed **20.0% more sources/second**, a **16.7% shorter source
window** and **10% fewer postings**. PostBatch receipt gas increased **1.35%**;
there is no claim of a gas-cost saving. The observed trade-off includes different
batch composition and retained-storage transitions; this experiment does not
isolate the cause of the gas increase. One live cohort per revision, different
process/cache state, background host activity and public-network timing limit
causal claims. L1 block gas limits also drifted: the baseline posting blocks
ranged from 17,016,600 to 17,436,843, versus
17,521,957 to 17,919,685 in the candidate. Admission derived its
bounds from the active rules in each case; this live comparison does not
isolate the effect of that capacity difference. Setup relays occurred outside
both measured source windows;
the candidate needed more relay work and one failed setup attempt. All signed
source payloads, profile settings and source ingress routes were preserved.

The candidate's first cross-chain posting after restart contained **29 sources**,
and its largest contained **62**. It did not require a one-source calibration
posting. All 870 canonical source rows and 27 posting rows remained byte-identical
after the semantic verifier completed. Queue counts were zero. Independent
follower replay ran in the native suites; a separate live Chiado follower was
not run for this cohort.

### Admission table and observability

- [27-row admission estimate versus actual postBatch table](candidate-admission-postings.csv)
- [Canonical posting gas/blob/source counts](candidate-postings.csv)
- [Comparison summary](comparison.json), [live semantic checks](live-verification.json)
- [Estimate coverage](admission-coverage.json), [live accounting/cutoff checks](admission-invariants.json)
- [Canonical stability after verification](canonical-stability.json)

All **27** canonical postings have admission, full-calldata, preflight, signed
limit and receipt gas. All **870** settled source hashes join to their accepted
estimate in the correct canonical posting. Initial shared gas plus accepted
source deltas reproduces each of the 27 admission totals; every receipt stays
inside its signed gas limit. The collector retained 4,490 observations covering
34 complete candidate histories, with 1,164 examined-source rows. The same
4,490 count appears in Prometheus, along with 34 observations each for
preparation, proof, finalization and delivery. All 34 relay acknowledgements
arrived before the exclusive cutoff; the smallest observed acknowledgement
headroom was 35.829 ms.

Preparation on canonical postings was 392 ms median / 762 ms maximum; proof
was 90.328 / 172.011 ms. No canonical posting exceeded the configured nominal
1,000 ms preparation allowance in this live mix. Borrowing across that allowance
is established by controlled tests, not by an overrun observed in this wave.
Thirteen canonical batches stopped at the postBatch gas estimate, two at whole
bundle gas, four at the pipeline estimate and eight at their selection/queue
boundary. Existing conservative backoff for incomplete outcomes in **produced**
targets remains; the change prevents skipped targets/replacement consumption
from being treated as capacity failures.

Weighted admission gas was 344,692,698 versus 179,639,982 receipt gas (1.919x).
The largest per-post gaps were approximately 3.7–4.2x in nested/multi-call mixes.
This supports the proposal's separate next step: profile those reserves and
assess a bounded exact-simulation extension. It does not authorize a blanket
ratio discount or removal of final native/signed limits.

All seven populated Grafana panels rendered with seven successful datasource
queries and zero browser/SQL errors. The first live browser check used `/d/...`
instead of this installation's `/grafana/d/...` path and reached the login page;
that failure is retained. Prometheus similarly uses `/prometheus`. The local
fixtures had no prefix. Live browser JSON/screenshots and the complete per-source
CSV remain local-only in `artifacts/admission-throughput/candidate/`.
The dashboard UID is `eez-admission`.

The source table records estimates and timing for both directions. Its exact
source-chain receipt-gas join covers inbound L1 transactions; outbound L2 receipt
gas is not collected by the indexer and remains null. It never presents a share
of aggregate postBatch receipt gas as an exact per-source cost. The canonical
measurement's local source CSV includes independently fetched receipts for both
layers. Old producers/ordinary postings and incomplete history remain explicitly
labelled rather than receiving invented estimates.

### Operational limits and retained state

Execution gossip peers were still zero at the post-recovery health check, while
L1/L2 were fresh and synchronized and Beacon was non-optimistic with 34 peers.
The validated payload backfill and setup relays establish this run's recovery;
reliable execution peering for future restarts/ordinary ingress remains an
infrastructure limitation. No datadir/genesis/key/contract reset, peer-setting
change or shared-resource cleanup was performed. The final runtime image choices
are persisted in the existing private `.env`, with its original bytes backed up.
Local images are development artifacts; CI promotion and the full CI/Kurtosis
matrix were not run. Native tests establish independent replay, and live job
assertions establish workload semantics; receipts/startup alone are not used as
correctness evidence.


After validation, only the two containers labelled `eez.task=admission-throughput`
for disposable PostgreSQL (port 19433) and Grafana (port 19301) were removed,
including the test database's anonymous volume. Production monitoring, retained
posting logs, databases and runtime services were preserved. The cleanup record
is local-only at `artifacts/admission-throughput/disposable-cleanup.json`.


[Final deployment check](deployment-final.json) independently matched all 27
posting receipts against the existing external execution RPC after verification,
including success, block hash and gas used. It also verified the running
node/signer/collector image IDs and zero queued/in-flight/blocked/unknown sources.
A byte comparison of private `.env` against its backup confirmed that only the
two runtime image references changed. [Final monitoring check](monitoring-final.json)
records the successful seven-panel render and adjacent estimate/receipt columns.
