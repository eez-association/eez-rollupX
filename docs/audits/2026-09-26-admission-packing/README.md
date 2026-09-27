# Packing with FIFO preference and measured companion execution

| Field | Value |
| --- | --- |
| Date and status | 2026-09-26–27 UTC; deployed and measured; all canonical sources and semantic jobs validated |
| Scope | More canonical cross-chain sources per postBatch; FIFO preference without a standalone count cutoff; continue after nonfitting independent groups; bounded companion gas |
| Source | `eduadiez/rollupX`, baseline `b5caf1b01a90ea35525b839da5b7ce3fac9f8d10`, candidate `e5b2cde65c9166ceae87bd4ee723820ed63ea5f6`, branch `feat/cross-chain-admission-throughput` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`; unchanged |
| Deployed baseline | `e57c7a545671d9fab082a2cb524859d75f55ef12`, local shipping images recorded in the prior audit |
| Validation follow-ups | Test fixture `1e6845b`; large-calldata verifier transport `0856619`. Neither changes the deployed runtime source `e5b2cde`. |
| Environment | Rust 1.94.0, locked dependencies, Foundry 1.7.1; native Gnosis/Prague and Ethereum/Osaka, existing Chiado 10200 / L2 6292 |
| Related records | [Accepted packing scope](../../proposals/cross-chain-admission-throughput.md#september-26-follow-up-packing-with-a-fifo-preference), [verified baseline wave](../2026-09-26-admission-companions/README.md) |

## Baseline findings

The user explicitly made FIFO a preference and canonical sources per posting the
primary objective. The last verified wave settled 870 sources / 770 semantic
jobs using 33 postBatches, up from 27 in the previous iteration. Two canonical
batches stopped at the independent FIFO count cap. Nineteen stopped at the whole
L1 bundle gas envelope. Of 520 inbound sources, 170 used signed-limit consumption
fallbacks totaling 170 million gas versus 30.6 million receipt gas.

Inspection found that a gas/blob refusal terminates the remaining drain, even
when a later independent group could fit. Source simulations run on forks and
only accepted groups commit state. Per-source nonce dependencies must still hold
when extending discovery past a rejected group, and a later transient abort must
retain earlier resource deferrals too.

Inbound companion estimates also omit the separately recorded L1 target gas.
`compose_crosschain` measures source gas and `l1_target_gas(actions)` separately;
the inspector returns intercepted-call gas to the caller. Fixed protocol framing
reserves do not bound arbitrary callback application work. The new estimate must
charge that measured execution component explicitly and expose it in telemetry.

## Executed baseline checks

Commands run from the repository root. Local-only evidence under ignored
`artifacts/admission-packing/` is unavailable in another checkout.

```bash
cargo +1.94.0 test --locked -p eez-composer --lib \
  fifo_bypass_does_not_cap_the_next_fitting_snapshot
```

**Expected failure reproduced:** one selected test failed, zero passed. After a
completed 30-group candidate bypassed older work, the next 100-source selection
was limited to 30. Only the new regression was present; production code was
unchanged (`baseline-fifo.log`).

Integration contracts built successfully. The native regression reproduced the
expected packing failure (one selected test failed): A and the independent cheap
C landed in different blocks, after the test established the initial four-source
snapshot and B's `l1_tx_gas_budget` rejection. It ran with stale prebuilt executable
overrides cleared, `CARGO_BUILD_JOBS=2`, and retained logs/datadirs
(`baseline-independent-packing.log`, `baseline-native/`). It supplies a fitting expensive group, a second expensive
group that cannot fit the remaining budget, its cheap nonce successor, and an
independent cheap group. It requires one initial snapshot, the specific gas
rejection, canonical receipt ordering, application storage checks and independent
follower replay. This was the pre-implementation reproduction; candidate checks and rollout follow below.

Unrelated operator runbook edits and other audit directories remain untouched.

## Candidate implementation and focused checks

Policy `packing-fifo-preference-2` removes standalone FIFO count backoff. Gas,
L2 capacity and blob misses retain the claimed group, block its same-sender /
same-direction nonce suffix for this candidate, and continue discovering
independent work. A later transient abort requeues earlier capacity misses along
with accepted groups and the failing remainder. Per-source outcomes and summary
`capacity_deferrals` preserve each reason; `fifo_bypassed` is diagnostic.

Estimator `companion-execution-5` adds measured L1 callback application gas and
extends bounded evidence to up to four sequential roots, four callbacks and four
nested rows per root, including callback revert spans. Unsupported profiles keep
the signed-limit fallback. The shared deadline, real resource budgets, whole
groups and final native ordered validation remain mandatory. The additive SQL
views/Grafana expose callback gas, individual source outcomes and deferral counts.

Completed focused checks before rollout:

- Integration contracts: **69 passed**, zero failed/skipped.
- Gas pins: **50 passed each** on Cancun, Prague and Osaka; cold queue scans,
  sequential roots, nested callbacks and forced reverts included.
- Indexer: **25 passed**, zero ignored, using a task-owned disposable PostgreSQL
  database. Tests executed the additive migration and actual dashboard SQL,
  checked historical nulls, and validated new fields. The container was removed.
- Indexer all-target Clippy with warnings denied passed. Rust formatting and the
  edited Solidity fixture formatting passed; `git diff --check` passed.

Iteration failures are retained, not hidden: the old FIFO-backoff unit assertion
was updated for the accepted policy; the new ownership regression initially
asserted global queue order even across independent chains after requeue, and
was corrected to require exact ownership/identity preservation, nonce order and
unchanged attempt counts. One new contract test exhausted its setup gas budget
when many large fixtures shared one test transaction; splitting those measured
fixtures into separate tests passed. Indexer test compilation initially used an
unavailable JSON SQL conversion; explicit text decoding fixed it without changing
dependencies. Later Composer and native results follow below.

Local evidence: `focused-results.json`, contract/gas/indexer logs and
`baseline-executables.json` under `artifacts/admission-packing/`. Runtime tests
clear prebuilt overrides and retain `native/logs` and `native/datadirs`. These focused checks preceded
runtime rollout and do not establish a performance result.

The first candidate native gas run selected five tests: four passed and the new
packing receipt assertion failed. Its retained admission history proves the
candidate admitted A and C together (`accepted=2`, one B gas deferral, projected
3,212,966 under a 3,566,666 planning cap), but the cold proof of 61 witnesses hit
`prover_deadline` after 1,669 ms. Deadline backoff then split canonical settlement.
This is not a passing packing regression. The next fixture revision used explicit
2,000 ms preparation / 2,000 ms proof allowances. Its startup misconfiguration
and final correction are recorded below; deadline-focused tests retain their settings. The original
baseline used the default shorter fixture window, so that initial comparison is
not an identical timing-environment comparison. This required final source reruns,
including the added FIFO/capacity telemetry assertions and infrastructure-learning
guard, recorded below. First-run logs and datadirs remain retained.

Final Composer units: **261 passed**, zero failed, one existing ignored manual
case (`composer-final.log`). The stale-tail regression caught an overly broad
infrastructure-learning guard during iteration; the corrected guard still allows
a bounded probe when a stale pipeline estimate prevented work, while a provider
abort after a gas miss preserves the previous selection limit. Both regressions
now pass. Earlier failures remain in the local logs.

At 22:04:29 UTC the tested additive monitoring views were applied atomically to
the existing monitoring database, without changing runtime/collector images.
Grafana binds this checkout's dashboard directly, so making its new columns
available preceded the pending image rollout. Admission history remained at
28,078 events and all four collector checkpoints were retained. The real Grafana
API subsequently executed panels 1 and 3 successfully (123 candidate rows and
3,929 source rows in the six-hour window), exposing all four new fields.
`schema-rollout.json` and `grafana-schema-verification.json` retain the evidence.
Runtime deployment and the verified workload were still pending at that point.

Workspace all-target Clippy with `-D warnings` passed after the final source edits.
The final native selection is recorded in [native-plan.json](native-plan.json):
48 cases across seven targets, serial execution. Twenty unchanged ordinary-only
settlement policy cases are explicitly excluded from this focused iteration;
the previous revision's full suite is not counted as coverage of this candidate.
Runtime checks and exact image identities follow below.

## Exact candidate images and monitoring rollout

All four local shipping images were built from an archive of candidate
`e5b2cde65c9166ceae87bd4ee723820ed63ea5f6` and its exact recursive protocol
submodules. The release recipe, package/bin/example selection and lockfiles match
the deployed `e57c7a5` baseline. These are local development images, not promoted
CI artifacts. Build commands and logs are retained under
`artifacts/admission-packing/images/`.

The image checks verified source/protocol labels, image IDs, shipping binary
hashes, and the existing L2 genesis block hash/state root. Matching deploy tooling
was built; no contracts or genesis were redeployed.

At 22:24:06 UTC the new collector image completed its image-only rollout.
Its mounts, environment and command were preserved. Admission history stayed at
28,078 events and four checkpoints. The Grafana API again executed the two
admission tables successfully, with the new fields available. Composer and signer
remained on the baseline until native validation completed.

The staged runner's read-only readiness check initially reported a 0.2853999234
xDAI L1 shortfall at the previous 0.0039 xDAI worker prefunding level. A private
task profile using 0.0035 xDAI passed readiness without additional funding. This
changes refundable worker prefunding only; workload size, gas settings and
concurrency are unchanged. The actual wave followed the runtime checks below.

## Native matrix and fixture correction

The complete planned matrix selected 48 cases: **47 passed and one failed**.
All 21 dynamic admission, five partial consumption, three queue liveness, one
queue reorg, 11 selected settlement policy, two receipt cost and four existing
gas cases passed. The new packing fixture failed before node startup because its
2,000 ms preparation + 2,000 ms proof + 100 ms slack exceeded the disposable
chain's 4,000 ms Future region. This run does not validate the packing assertion.

Test-only commit `1e6845b` sets preparation to 1,500 ms, retaining the 2,000 ms
proof allowance and every assertion. Its 3,600 ms total fits the fixture's
configuration contract. The sole changed file relative to the built runtime
candidate is `crates/eez-node-e2e/tests/settlement_policy_gas.rs`; shipping source,
configuration, protocol and dependencies are unchanged. The focused rerun used
fresh processes with prebuilt overrides cleared: **one passed**, zero failed or
ignored. It checked the four-source snapshot, exact gas deferral and FIFO-bypass
telemetry, A/C inclusion in the first canonical batch, B/successor nonce order,
all four application storage effects, exactly two canonical postBatches, zero
charged attempts and independent follower replay. It does not assert that every
speculative candidate reaches dispatch or that the first target is included.

The original `native-final.log`, execution metadata, ten executable hashes and
the failing datadir are retained under `artifacts/admission-packing/`. The
corrected run has separate `native-packing-corrected.log` and log/datadir paths.
The evidence gate requires all 47 original passes plus the corrected case, keeps
the original failure explicit, and verifies the test-only source difference.

The [native evidence](native-results.json) accounts for all **48** selected cases
as 47 unchanged original passes plus the corrected fixture. The final workspace
all-target Clippy run passed with warnings denied after that correction;
formatting and whitespace checks also passed.

## Runtime rollout and readiness

At 23:29:50 UTC, the node and signer completed their image-only rollout to the
verified `e5b2cde` images. The queue was empty, the previous node stopped with
exit code zero, and mounts/environment were preserved. The only operator dotenv
changes were the two image references. The collector had already been upgraded.
See [rollout](final-runtime-rollout.json) and
[deployment checks](deployment-verification.json).

As in the baseline rollout, the peerless embedded L1 stopped at its restart
parent while Beacon continued receiving newer payloads. The first freshness check
also observed a stale upstream latest response. Read-only comparison established
the execution gap. The existing bounded recovery helper independently checked
34 canonical raw payloads (blocks 23245687–23245720), header and transaction
hashes, parent continuity and agreement with Beacon. All 34 Engine imports
returned `VALID` with the exact payload hash. Lighthouse retained fork-choice
ownership; no database, genesis or contracts were replaced. The subsequent
check found both chains current and not syncing, an empty queue and matching
canonical L1 hashes. [Recovery evidence](engine-gap-recovery.json).

The full staged `--verify` workload started with the shared worker lock and the
same loopback ordinary-L1 forwarding method as the baseline. The task profile
differs only in refundable worker prefunding, verified byte-for-byte after
normalizing those two values. Workload results and performance comparisons follow below.

## Canonical wave and semantic verification

Protocol run `20260926-233435` settled **870/870** sources (520 inbound, 350
outbound) through **28 canonical postBatches**, L1 blocks 23245806–23245835.
The canonical recheck preserved every attribution, and the external canonical
RPC independently matched all 28 posting receipts. Initial polling recorded 96
sources at earlier speculative L2 heights; canonical attribution uses current
receipts and settled-range hashes. No source was manually resent. Native tests
supplied independent follower replay; a separate live follower replay of the
complete 870-source wave was not run.

The original staged process exited **1**: **724 semantic jobs passed and 46
failed**. All failures belonged to one larger posting (31 `nestedCounterL2` and
15 `multi-call-nestedL2` jobs). Its full hex calldata was 131,338 bytes, exceeding
Linux's single-argument limit before Forge could execute. The pinned runner
captures that error, then its error-output filter exits without printing the
captured text. Reproducing `E2BIG` with the identical argument established the
transport failure. Receipt refresh was already correct; moved heights were not
the cause.

The staged runner now sources [a shell bridge](../../../scripts/chiado-forge.sh).
Its function receives large arguments before `execve`, writing the exact public
calldata to a private temporary file in Foundry's permitted artifact directory.
[The file adapter](../../../contracts/script/VerifyL1BatchFromFile.s.sol) invokes
the **unchanged pinned verifier**. Expected tables, fork height, registry and all
field/live-root checks are preserved; small inputs keep the existing path. This
concrete large-batch failure is why the helpers are retained. No protocol pin,
contract deployment or signer-profile change was needed.

The first adapter used `this.run`; Foundry rejected the ephemeral script-address
reference. The final adapter creates a separate verifier inside the local script
simulation. The actual full batch then passed, and changing only its selector
was rejected by the pinned verifier. Shell tests checked byte identity, cleanup
and propagation of verifier failures. The runner's mocked fixture initially
lacked the new helper; adding it and asserting function/context propagation
corrected that fixture. First failures remain in local logs.

After the original run ended, the shared wallet lock was reacquired and only
those 46 jobs ran their **complete** pinned verification stages with the bridge.
All **46 passed**, including remaining L2/application checks. Inputs, original
logs and transactions were preserved; nothing was sent or funded. All **770
unique semantic jobs are validated**: 724 original passes plus 46 after a
deterministic verifier fix. This is not a clean first-pass staged result or an
unmodified retry-only pass; the original failed run status remains unchanged.

Evidence: [semantic inventory](results/semantic-verification.json),
[correction and per-job log hashes](results/semantic-correction.json),
[positive/negative verifier checks](results/large-calldata-verifier-checks.json),
[canonical stability](results/canonical-stability.json).

## Comparison with the preceding verified wave

| Metric | Baseline `e57c7a5` | Candidate `e5b2cde` |
| --- | ---: | ---: |
| Canonical sources / semantic jobs validated | 870 / 770 | 870 / 770 |
| Canonical postBatches | 33 | **28 (15.2% fewer)** |
| Sources per postBatch | 26.36 | **31.07** |
| Send + monitor | 199.540 s | **166.540 s** |
| Sources per measured second | 4.360 | **5.224** |
| Actual postBatch gas | 175,340,686 | 176,867,771 (+0.9%) |
| Admission / actual postBatch gas | 1.4755× | 1.4765× |
| Companion estimate gas | 249,141,900 | **168,392,914 (−32.4%)** |
| Actual companion receipt gas | 73,353,423 | 73,277,802 |
| Companion estimate / actual ratio | 3.3965× | **2.2980×** |
| Signed-limit companion fallbacks | 170 / 520 | **0 / 520** |
| Median preparation per canonical candidate | 378 ms | 483 ms (+27.8%) |
| Total preparation of canonical candidates | 12,366 ms | 12,863 ms (+4.0%) |
| Source-attempt history rows | 1,008 | 8,771 |

The 520 companion estimates reconcile exactly to 23,039,974 source-simulation
gas + 10,910,940 callback-simulation gas + 134,442,000 protocol reserve. All use
the bounded model in this workload; unsupported shapes retain the signed limit.
Callback work is now explicitly charged, even where it increases an older
bounded estimate. All 870 canonical sources have accepted estimates. For all
28 postBatches, final native gas-used predictions equal receipt gas exactly and
required gas fits signed limits. PostBatch admission accuracy itself is essentially
unchanged. See [per-post estimates](results/admission-vs-receipt.csv),
[companion families](results/companion-breakdown.csv) and
[comparison data](results/comparison.json).

This is one candidate wave against the preceding baseline on the same live
network/host, with the same release recipe, protocol, workload and concurrency.
Refundable prefunding changed as documented. Cache/history state, slots, ordering,
other host work and external inclusion vary; these are observed results, not an
isolated causal benchmark. The older `85f3691` iteration had 27 posts under a
different estimator; this candidate does not establish a historical minimum.
Telemetry overhead was not isolated with an enabled/disabled comparison.

## Cutoffs and remaining opportunities

| Final admission stop reason | Baseline canonical batches | Candidate canonical batches | Candidate sources |
| --- | ---: | ---: | ---: |
| `l1_bundle_gas_budget` | 19 | 17 | 490 |
| `l1_tx_gas_budget` | 7 | 5 | 176 |
| `fifo_fairness_limit` | 2 | **0** | 0 |
| `pipeline_time` | 2 | 3 | 115 |
| `submission_backoff` | 2 | 2 | 48 |
| `snapshot_exhausted` | 1 | 1 | 41 |

Final reasons differ from individual misses while filling a candidate. Across
33 candidates, there were 4,059 postBatch-gas and 3,613 bundle-gas deferrals.
Per-source outcomes reconcile exactly with summary counts. Three canonical
candidates accepted one additional independent source after an earlier capacity
refusal. FIFO bypass occurred in ten candidates, with **no FIFO count cap**;
two candidates retained real submission backoff.

Of five noncanonical candidates, one stopped before proving/dispatch after
admitting 36 tentative groups; two targeted absent slots and two were not
included in produced blocks. Sources were retained. There were no reported proof
failures, no cold timing probes, and no calibration/empty postings in the measured
window ([all posting calls](results/no-empty-postings.json)). Canonical proof time had median 95.8 ms and maximum 134.5 ms; minimum
dispatch lead was 401 ms. Acknowledgement and unused block gas do not establish
the external reason for non-inclusion. [Target classification](results/noncanonical-targets.json).

The next useful optimization is a cheap rejection check before repeating full
simulation for a source provably too large for remaining capacity. The 7,672
refusals yielded only three extra canonical sources and increased per-candidate
preparation and event volume. A valid lower bound must be tied to the current
parent/prefix, preserve independent work and nonce dependencies, and retain final
native validation. It must not restore a FIFO cutoff or blindly stop at the first
refusal.

The other opportunity is tighter **measured protocol reserves** for multi-call
companions (about 3× receipt gas here), which still dominate the bundle budget.
Receipt ratios alone cannot justify scaling reserves: some cold/boundary fixtures
have much less margin, and each signed transaction's start-of-execution gas
envelope must fit. Gas, blob, nonce and shared-deadline constraints remain necessary.

## Monitoring and final checks

Actual Grafana datasource queries returned **33 candidates and all 8,771 source-
attempt rows**, including capacity deferrals, FIFO bypass, per-source outcomes
and callback gas. SQL joins independently verified accepted estimates for all
870 canonical sources. Historical rows retain nullable added fields.
[Grafana evidence](results/grafana-wave-verification.json).

Final applicable checks: Composer **261 passed, one existing manual case ignored**;
native **48 covered as described above**; indexer **25 passed**, including real
PostgreSQL migration/dashboard queries; integration contracts **69 passed** after
the verifier addition; gas pins **50 per Cancun/Prague/Osaka**; workspace/indexer
Clippy with warnings denied; Rust/Solidity formatting, shell syntax and whitespace
checks. Tooling selected **259 tests: 255 passed, four opt-in cases skipped**.
Skips were real Docker payload extraction, real Kurtosis resource probing,
tiny-workspace native archive relocation and real Docker runtime-fixture builds.
They are not coverage. The native plan's 20 unrelated ordinary-only policy
exclusions remain explicit. Full CI release/Kurtosis matrices were not run locally.

The final [runtime health check](results/final-runtime-health.json) at 00:17:40 UTC
confirmed the exact candidate images, a healthy signer, both chains current and
not syncing, external canonical L1 agreement, unchanged genesis and an empty
queue. The operator dotenv still differs only in the two authorized image
references. The temporary ordinary-workload forwarder was stopped after checking
its PID and exact script identity; deployed services and retained history remain
running. Its [final counters](results/ordinary-forwarder-final.json) record 1,397
ordinary transactions forwarded, zero upstream rate limits and 2,489 client-side
broken pipes over the process lifetime, including verification. The original
[staged run status](results/run-status.json) retains its
verification failure and exit code.

Small sanitized results are committed here. Detailed source CSVs and raw event/
chain snapshots remain ignored local artifacts, identified by path, size and
digest in [the evidence inventory](results/local-evidence.json); they are absent
from another checkout. Retained node logs and indexer history provide the
per-transaction Grafana records. The supplementary [packing analyzer](analyze-packing.py)
records the new component/deferral reconciliation absent from the older report.
