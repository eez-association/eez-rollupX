# Companion gas and canonical receipt coverage

**Status: locally validated, deployed and measured.** The subsequent packing
iteration is authorized by the user's instruction to prioritize fewer postings
and treat FIFO as a preference rather than an independent admission cutoff.

Baseline checkout `20e81da05c33fd6e829477652fa4e52ea91a50a1` on
`feat/cross-chain-admission-throughput`; deployed runtime
`85f3691cb9c7ccbf8c0b8700576733283e5af7fc`; clean protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`.
Scope/acceptance are in the [authorized proposal](../../proposals/cross-chain-admission-throughput.md).
Existing operator runbook edits and unrelated audit directories are preserved.

The [baseline verified wave](../2026-09-26-admission-deferred/rerun2/README.md)
completed 870 sources and 770 semantic checks. It records the 50 source-receipt
gaps, conservative reverted storage estimates and missing companion overhead.

Baseline regression reproduced with:

```bash
cargo +1.94.0 test --locked -p eez-composer --lib \
  reverted_flagged_storage_is_priced_with_bounded_payloads
```

One selected test failed because a bounded reverted entry received
`DeferredReverted` instead of a storage estimate. The unmodified estimator was
tested with only the regression added. The first failure is retained locally at
`artifacts/admission-companions/baseline-reverted-storage.log`; ignored artifacts
are unavailable in another checkout. No runtime changes had been deployed at
this baseline check.

## Implementation and local checks

Estimator `companion-storage-4` extends storage inventory/result bounds to 256
bytes, prices packed call flags and reverted/static nested hashes, and separates
companion simulation from a bounded protocol reserve or signed-limit fallback.
Final native gas/effect validation is unchanged. The monitoring collector binds
source receipts to the canonical posting block and order, including receipts
without registry logs; it does not add these sources to settlement cost totals.

- Composer: **256 passed**, one existing manual backlog timing profile ignored.
- Integration contracts: **62 passed**. Gas pins: **43 passed on each of
  Cancun, Prague and Osaka**, including reverted/flagged retained storage,
  256-byte inputs/results, cold callbacks, and 126 skipped queue entries.
- Indexer: **25 passed**, including all opt-in PostgreSQL/recovery cases against
  the task-owned `eez-admission-companions-test-db`, port 19439. No operator
  database was used for destructive tests.
- Workspace clippy and standalone indexer clippy passed with warnings denied.
  Rust formatting passed. The edited Solidity file passes its configured
  formatter; the full formatter retains unrelated pre-existing differences in
  `contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`.
- Native validation: **58 unique cases passed** across `dynamic_admission` (21),
  `settlement_policy` (31), `settlement_policy_gas` (4), and
  `settlement_policy_cost` (2), with no ignored cases. Admission evidence combines
  unchanged cases with the corrected-helper rerun as described below. The other
  three suites passed in full, serially, from source. Local image builds are
  complete; runtime deployment and the new live wave follow this gate.

First failures are retained locally. An initial Rust test used `Default` for an
ABI type without that implementation; a later compile caught an argument wired
to the adjacent trace call while tightening companion evidence. Both were
corrected before the passing Composer run. The first indexer test invocation
could not bind mock RPC sockets under the sandbox; the network-enabled run used
the same locked dependencies and passed. These are not flaky assertion retries.

## Additive monitoring upgrade

The first atomic live schema application rejected a view column change from
`numeric(78,0)` to unconstrained `numeric`. PostgreSQL rolled back the transaction.
The expression now explicitly preserves the historical type; the database test
first creates the old typed projection and then applies the new schema. After
all 25 indexer tests passed again, the schema upgrade committed at
**2026-09-26 18:45:23 UTC**. Admission event/file-checkpoint counts remained
**24,080 / 3** before and after. This makes new mounted dashboard columns
queryable. The old collector/runtime remained running until their respective
image upgrades described below.

Local evidence is under `artifacts/admission-companions/`: `composer-final.log`,
`gas-pins-{cancun,prague,osaka}.log`, `contracts-final.log`, `indexer-final.log`,
`clippy-final.log`, `indexer-clippy.log`, and `schema-upgrade.json`. Raw runtime
configuration and first schema diagnostics remain private under `data/`.

## Image identity and monitoring verification

Runtime implementation: **`e57c7a545671d9fab082a2cb524859d75f55ef12`**.
Test-only follow-up: **`9f466b1`**; its only difference is
`crates/eez-node-e2e/tests/dynamic_admission.rs`. Shipping source and protocol
are identical. Local images use an exact Git archive and the same release
recipe as the `85f3691` baseline, including Rust 1.94.0, `--locked`, and the
shipping package/bin/example selection. This is a local build, not a CI promotion.

| Image | Local image ID |
| --- | --- |
| Node | `sha256:efa887627148264c7c3a4378cc3fe3b13526cafd776870307c525d0c1d0a59b4` |
| Signer | `sha256:32eace4d05970066a22fa449f1fc07b168921d829d373c22a48e5ef089957be3` |
| Deploy | `sha256:49818529dc2e28c4f1109c4b02a1f463e1a8a3a333c667147c4132555c90e3cd` |
| Indexer | `sha256:6ae79bf2bdc5b67e2134f8b61b3e4659140dcc0e94ab0db1f7d64af9d9802411` |

Labels, binary hashes and genesis helper outputs were checked; the existing
genesis remains `0x30a494dfb7923c81cb51590655a79591d3e3aac2d7a42cf3ba44767ee8bf3381`.
Commands, recipe/lockfile hashes and source archives are recorded locally in
`images/build-plan.json`, `images/build-status.json`, and `images/verification.json`.

The indexer upgrade completed at **18:54:00 UTC**, preserving mounts, environment,
command, and the 24,080 events / 3 file checkpoints. The prior wave now has
**520/520 inbound source receipt values**, including **50/50 `revertCounter`**
sources. Every value matched the prior independently collected canonical receipt.
The live Grafana `eez-admission` source query returned **6,292 rows** with the new
columns and no query error. See local `baseline-receipt-backfill.json` and
`grafana-live.json`. The disposable PostgreSQL test container was removed after
validation; the operator database and retained logs remain intact.

The first dynamic admission suite finished **15 passed / 6 failed**. All six
failures were the same obsolete `deferred-storage-3` assertion at line 672,
after both fitting waves settled. The test-only follow-up checks the new version
and verifies that simulation plus protocol reserve reaches the actual admission
budget, below the unchanged signed limit. All eight cases using that helper
(including the two formerly passing single-wave cases) passed on rerun: six
fitting cases and two short-result cases. Together with the 13 unaffected
passing cases, this validates all 21 unique admission cases. The original six
failures remain recorded; this was not a fresh full-suite pass. This corrects test
expectations; it does not relax gas, canonical settlement, or follower replay
assertions. The three other required settlement suites passed in the serial run.
The accounting gate at `artifacts/admission-companions/native-gate.py` checks
every case, retains the first failures and writes `native-results.json`. The
settlement suites include independent follower replay, restart, backlog,
whole-group gas reduction and retained source ownership checks. CI's full
release/Kurtosis matrix and a live independent follower are not covered by this
local validation.

The next verified workload passes readiness with task-only worker prefunding
and floor of **0.0039 xDAI**, down from 0.004 in the baseline; source gas limits,
workload and concurrency are unchanged. The first check found a 0.0695945 xDAI
upfront funding shortfall. The smaller prefund uses existing funds and requires
no additional poster-to-source transfer. Original operator profiles are preserved.

## Runtime rollout

The [image-only rollout](final-runtime-rollout.json) completed at **20:33:24 UTC**
after the 58-case native gate. The queue was empty, the old node exited cleanly,
and mounts/environment were identical. The first two health observations found
stale, syncing heads: this installation's existing zero-peer execution setup
missed parents while the node was stopped. This was not a passing startup check.

The previously audited [bounded recovery helper](../2026-09-26-admission-throughput/recover-engine-rpc-gap.py)
was reused with only its output directory changed. It verified canonical raw
bodies, headers, transaction hashes, parent continuity, empty requests and
agreement with the local Beacon head, then submitted **28 payloads** through
`engine_newPayloadV4`. Every result was `VALID` with the exact expected hash.
Lighthouse retained fork-choice ownership. No data reset, state override,
peer-setting change or contract deployment occurred. [Recovery evidence](catch-up.json)
retains the range and full-artifact hashes; raw bodies remain local only.

At **20:36:43 UTC**, [deployment verification](deployment-verification.json)
confirmed the expected image IDs, unchanged genesis/configuration, current
canonical L1, both layers nonsyncing, and an empty queue. The staged read-only
check then passed. The verified wave started after this recovery, using the same
task-owned ordinary-L1 forwarding workaround as the baseline. Persistent
execution-peer reliability remains an operational limitation.

## Canonical wave and gas measurements

Run `20260926-203702` retained 770 jobs, 870 sources (520 inbound / 350 outbound),
200 preparation workers, 28 send workers and four semantic verifiers. All 870
sources have canonical posting/range attribution and accepted admission
estimates. Repeating attribution produced identical source/posting tables;
all 33 postBatch receipts also matched the external L1 RPC. One source's initial
reported L2 height changed before the canonical measurement. The runner completed
at **20:59:53 UTC**, exit **0**, with **770/770 semantic job checks passing on
attempt 1** and every phase passing. The live verifier does not run an independent
follower; independent replay was exercised in the native process suites.

| Measurement | Previous verified wave (`85f3691`) | Candidate (`e57c7a5`) |
| --- | ---: | ---: |
| Canonical sources | 870 | 870 |
| Send + receipt monitoring | 213.527 s | 199.540 s |
| Sources / second in that window | 4.074 | 4.360 |
| Postings with cohort sources | 27 | 33 |
| Admission postBatch gas | 275,063,662 | 258,721,542 |
| PostBatch receipt gas | 178,005,930 | 175,340,686 |
| Admission / postBatch receipt | 1.545× | 1.476× |
| Absent target slots | 6 | 4 |
| Excluded from produced targets | 9 | 2 |
| Failed proof candidates | 1 | 0 |

This comparison uses sequential live waves and preserved storage, not isolated
identical starting state. The candidate process restarted; the baseline was a
warm repeat. Slot availability, cache state, source ordering and host load differ.
The observed 7% source-window throughput increase is not evidence of a repeatable
causal speedup. Packing regressed from 27 to 33 postings as conservative companion
reserves became the dominant constraint. Setup/semantic verification durations
are outside the reported send/monitor window.

All 33 postBatch preflight gas-used values exactly matched receipts. Required gas
totaled **178,873,943**, below signed limits totaling **187,817,655**. The
[per-post table](postbatch-estimates.csv) preserves admission, full projection,
preflight, signed limit and receipt, plus timing and cutoff reason. Admission
estimates are budgeting predictions; receipt gas is observed execution.

### Cutoff reasons

| Final admission stop | All candidates | Canonical postings | Canonical sources |
| --- | ---: | ---: | ---: |
| `l1_bundle_gas_budget` | 23 | 19 | 436 |
| `l1_tx_gas_budget` | 7 | 7 | 261 |
| `fifo_fairness_limit` | 2 | 2 | 72 |
| `pipeline_time` | 2 | 2 | 71 |
| `submission_backoff` | 2 | 2 | 21 |
| `snapshot_exhausted` | 3 | 1 | 9 |

The [cutoff table](cutoff-summary.csv) includes timing and gas utilization.
`fifo_fairness_limit` is a temporary count cap after outbound-first preparation
bypasses an older eligible request. The next selection is capped at
`max(1, min(accepted, floor(selected / 2)))`. A completed attempt clears the old
cap, though another FIFO bypass can install a new one. It protects older work
from starvation and can leave resource headroom. Nine canonical candidates had
that selection cap; only two reached it as their final stop. Seven met a gas
or timing constraint first. Selection-limit and final-stop counts are distinct.

All 39 candidate histories are complete. Their 1,008 source-attempt rows contain
976 accepted rows, 30 gas deferrals and two timing cuts before an estimate. All
870 eventual canonical sources have estimates; a missing estimate in an earlier
attempt does not imply missing telemetry for the successful attempt. No batch
discount was revoked and no cold timing probe was used. Both produced-but-
excluded candidates were acknowledged by the relay: one target contained a
119,137-gas transaction and the other was empty. Local evidence does not establish
why the builder/validator excluded either bundle.

### Companion estimate versus receipt

| Basis | Sources | Estimate gas | Receipt gas | Estimate / receipt |
| --- | ---: | ---: | ---: | ---: |
| Bounded protocol model | 350 | 79,141,900 | 42,736,868 | 1.852× |
| Signed-limit fallback | 170 | 170,000,000 | 30,616,555 | 5.553× |
| Total | 520 | 249,141,900 | 73,353,423 | 3.397× |

Raw companion simulation was **22,830,958 gas**, still far below observed L1
execution. No bounded or fallback estimate was below its canonical receipt in
this wave. This is observed coverage, not a general proof for arbitrary calls.
The [family table](companion-estimates.csv) identifies the remaining excess:

- `multi-call-twice`: 50 sources, fallback / receipt **7.583×**.
- `reentrant`: 20 sources, **6.040×**.
- `revertFromOtherChain`: 50 sources, **5.720×**.
- `multi-call-nested`: 50 sources, **4.177×**.

These unmeasured execution shapes deliberately retain the signed-limit fallback.
The next gas optimization should measure and admit these specific families into
a bounded protocol model, covering multiple roots, nested callbacks, rollback
and queue scans. The signed limit still matters for the maximum gas envelope at
each execution position, even when estimated consumption is lower. A blanket
receipt-ratio discount would not establish that bound.

Deferred storage estimates now cover **653 entries**, versus 547 in the previous
wave. The previous reverted, call-flag and old-slot fallbacks disappeared in this
cohort. The 300 immediate-evidence-unavailable entries remain a separate target
for measurement; this iteration does not extend that admission profile.

Grafana's actual panel-3 datasource query returned **1,008 wave attempt rows**,
including all **870 canonical sources** and **520/520 inbound estimate/receipt
pairs** with `available` coverage. The 50 reverted-root sources remain covered.
Per-source export is available through Inspect → Data on the admission dashboard;
local full tables are `candidate/source-admission.csv` and
`candidate/detail/source-estimates.csv`. They are ignored artifacts, unavailable
in another checkout. Receipt coverage does not allocate aggregate postBatch gas
to individual sources or by itself prove protocol consumption.

## Final state and evidence

The deployed node, signer and collector retain the recorded `e57c7a5` images.
Final health verification found fresh canonical heads, both layers nonsyncing,
unchanged genesis/configuration and an empty cross-chain queue. The task-owned
ordinary-L1 helper stopped and its port 19445 was confirmed closed. It forwarded
1,397 original setup payloads, encountered no upstream rate limits, and recorded
2,294 client disconnects (`BrokenPipeError`); the runner's setup phases and full
verification still passed. The baseline helper had the same disconnect pattern.

The complete runner took **1,370.952 seconds**, including **812.453 seconds** of
semantic verification. These are separate from the 199.540-second source window.
The exact job/phase counts and input hashes are retained in
[semantic verification](semantic-verification.json), with compact
[measurement evidence](measurement.json). All 39 candidate companion totals
reconcile to their accepted source estimates, and all 30 gas rejection envelopes
reconstruct their recorded constraint. Every candidate identifies estimator
`companion-storage-4`.

The backward-compatible [cutoff analyzer](../2026-09-26-admission-deferred/rerun2/analyze-cutoffs.py)
now uses `companion_estimate_gas` when present and the historical simulation field
for older producers. It also exports companion estimates in family summaries.
Python compilation and the actual candidate analysis passed; `git diff --check`
passed. Complete raw evidence remains local-only under
`artifacts/admission-companions/`; operator inputs remain private under `data/`.
Full CI/Kurtosis, the unchanged core-protocol suite, and live independent follower
replay were not run. The unrelated Solidity formatting differences noted above
remain. No dependency, protocol pin, contract, signer-profile, or durable queue
recovery claim is introduced by this iteration.
