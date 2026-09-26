# Packing with FIFO preference and measured companion execution

| Field | Value |
| --- | --- |
| Date and status | 2026-09-26 UTC; baseline regressions and implementation in progress |
| Scope | More canonical cross-chain sources per postBatch; FIFO preference without a standalone count cutoff; continue after nonfitting independent groups; bounded companion gas |
| Source | `eduadiez/rollupX`, `b5caf1b01a90ea35525b839da5b7ce3fac9f8d10`, branch `feat/cross-chain-admission-throughput`; initial patches add only regression tests and this plan/record |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`; unchanged |
| Deployed baseline | `e57c7a545671d9fab082a2cb524859d75f55ef12`, local shipping images recorded in the prior audit |
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
follower replay. Candidate implementation and all post-change checks remain
pending. No new runtime rollout or workload has been executed in this iteration.

Unrelated operator runbook edits and other audit directories remain untouched.

## Candidate implementation and checks so far

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

Completed focused checks (candidate has not been deployed):

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
dependencies. Final Composer unit and native results remain pending.

Local evidence: `focused-results.json`, contract/gas/indexer logs and
`baseline-executables.json` under `artifacts/admission-packing/`. Runtime tests
clear prebuilt overrides and retain `native/logs` and `native/datadirs`. No new
runtime rollout, wave or performance claim has been made in this iteration.

The first candidate native gas run selected five tests: four passed and the new
packing receipt assertion failed. Its retained admission history proves the
candidate admitted A and C together (`accepted=2`, one B gas deferral, projected
3,212,966 under a 3,566,666 planning cap), but the cold proof of 61 witnesses hit
`prover_deadline` after 1,669 ms. Deadline backoff then split canonical settlement.
This is not a passing packing regression. The fixture now starts with explicit
2,000 ms preparation / 2,000 ms proof planning allowances to isolate its gas
assertions; deadline-focused tests retain their own settings. The original
baseline used the default shorter fixture window, so that initial comparison is
not an identical timing-environment comparison. Final source reruns are required,
including the added FIFO/capacity telemetry assertions and infrastructure-learning
guard. First-run logs and datadirs remain retained.

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
Runtime deployment and the verified workload are still pending.

Workspace all-target Clippy with `-D warnings` passed after the final source edits.
The final native selection is recorded in [native-plan.json](native-plan.json):
48 cases across seven targets, serial execution. Twenty unchanged ordinary-only
settlement policy cases are explicitly excluded from this focused iteration;
the previous revision's full suite is not counted as coverage of this candidate.
Final runtime checks and image identities will be appended after execution.
