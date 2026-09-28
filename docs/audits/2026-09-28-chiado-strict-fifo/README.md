# Chiado strict FIFO trial

Status: candidate remains deployed on separate cold copies of the existing
Chiado state. All 870 sources settled canonically and all 770 semantic job checks
passed. The wave needed 29 postBatches / 186.521 seconds; this does not establish
performance parity with the previous 23-post / 146.880-second wave. Two CI packing
assertions remain unresolved. Monitoring's pruned-genesis compatibility defect
was reproduced, fixed, tested and deployed separately.

## Scope and identity

The operator requested trying the strict FIFO change on the existing Chiado
rollup. Checkout `642fc2d0dfbf135bdf24a7decd8fc28d4d9e17b2`; candidate shipping
source `b2ff3b894faa509fe310b6b6d38781d335f8f5fc`; unchanged initialized protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`. The [local FIFO validation](../2026-09-27-admission-strict-fifo/README.md)
records executable identities, 286 Composer units, 14 native cases and paired
packing comparisons. Existing operator runbook edits and unrelated audits are
preserved.

The pre-trial deployment ran `e556e12` node/signer images on Chiado 10200 / L2
6292. Timing remains preparation/proof/slack 1000/450/450 ms; admission is automatic;
the explicit gas margin is 500 bps. No contracts, genesis, keys or public endpoints
were changed. Procedures: [recovery/upgrades](../../runbooks/chiado-deployment.md#recovery-and-upgrades)
and [existing rollup](../../runbooks/chiado-existing-rollup.md).

## Trial boundary

Main's Reth upgrade has not been validated against these existing databases.
Stop the sole node writer with an empty queue, retain the original L1/L2 datadirs,
and start the candidate on complete cold copies. Retain the prior configuration
and image identities. Never open a candidate-modified database with the older
binary as a rollback shortcut. Check genesis, retained canonical history,
catch-up, new canonical settlement and restart before a verified workload.
The successful earlier fresh-network suites do not establish this migration.

Run the existing staged Chiado helper with its wallet lock and `--verify`;
measure unique canonical sources, batches, directions, preparation and gas/cutoff
records. Semantic workload checks and independent follower replay are separate
claims. Pending queues are in memory; an empty queue is required at handover.
Only task-owned copies and temporary workload resources may be cleaned up.

## CI finding before operation

[CI run 36353826692](https://github.com/eduadiez/rollupX/actions/runs/36353826692)
failed two cross-chain shard packing assertions. The gas/FIFO fixture observed
four posts instead of two; the tight Ethereum fixture never observed a candidate
larger than 12 sources. Their receipt/effect/nonce and follower checks preceded
the failed assertions. Lint, runtime production, all embedded shards, two other
cross-chain shards and Kurtosis passed. The failures remain unresolved and are
not classified as flakes or waived. This is an operator-authorized local devnet
trial, not release promotion or a claim of full CI success.

Raw CI logs and later operational evidence are local-only beneath
`artifacts/chiado-strict-fifo-20260928/`. Private configuration copies and cold
trial state live beneath `data/chiado-strict-fifo-20260928/` with restricted
permissions. Sanitized results and evidence hashes will be added after execution.

## Executed handover and restart

The initial host copy failed only on the protected L1 discovery-secret file.
The old node was automatically restored; no candidate database had been opened.
The incomplete copy and first record are retained separately. The old runtime's
restart stalled with zero execution peers; 31 raw canonical payloads were checked
against external headers and the local Beacon checkpoint and imported using
`engine_newPayloadV4`. Every response was `VALID` with the expected hash;
Lighthouse retained fork-choice ownership. Both heads became current again.

The second handover stopped the sole node writer cleanly (exit 0), checked that
no running container mounted its databases writable, and copied complete L1/L2
state through a network-disabled task-owned container. The sources were mounted
read-only. This preserved protected files and ownership without printing secrets.
Only the node/signer image settings and the two execution data paths changed in
`data/.env`; rendered Compose equality checked all remaining settings. Original
datadirs, private configuration, genesis/profile, bindings, JWT, log volumes and
other services were retained. The active copies are
`data/chiado-strict-fifo-20260928/attempt2/l1` and `attempt2/l2`.

The candidate opened the copies, reconciled retained L2 history and caught up.
New Reth reports pruning error 4444 for full-block/header RPC requests below L1
block 23,000,000, including genesis. The raw genesis header remains available:
its independently computed Keccak hash and decoded state root match the prior
runtime. Deployment-block and later L1 samples and genesis/safe/finalized L2
samples also match by canonical hash/state root. A candidate recovery preparation
became unnecessary when normal sync caught up; no candidate payloads were imported
in that first startup. The readiness monitor was replaced to use the raw-header
check. A separate initial freshness probe rejected a slightly future L2 timestamp;
the corrected check uses absolute age, as the staged runner already does.

At L1 block 23266252, candidate postBatch
`0x08743990a2c15ccf5e66e6c37ce212d6efff79fdce655a94068519486a2e03e9`
succeeded from the configured poster. The registry commitment resolved to a
canonical local L2 range, the block matched the independent execution RPC, and
Lighthouse returned the selected full 131,072-byte blob. Gas was 108,302 used /
113,718 signed. This was startup settlement evidence, not the measured workload.

A subsequent graceful candidate restart used the same copied databases and image.
Peer backfill stalled again; 66 canonical payloads were independently reconstructed
and all returned exact `VALID` hashes on import. Canonical history/posting/blob
checks passed after 175.4 seconds of readiness polling. This establishes the
observed restart with bounded manual recovery, not unattended restart reliability.
No independent KZG recomputation or fresh follower replay has run in this live
trial yet.

The staged `--check --verify` gate then passed: empty queue, current/non-syncing
chains, aligned worker/funding nonces and sufficient existing funds. Workload
`20260928-072409` uses 770 jobs / 870 sources, preparation concurrency 200,
send concurrency 28 and the prior 0.0033 xDAI worker target/floor. Its temporary
ordinary-L1 bridge uses local reads and forwards identical signed raw transactions
at five upstream request starts/second; cross-chain ingress is unchanged.

## Verified wave

The staged runner completed once with exit 0. All 770 distinct `:verify` job rows
passed on attempt 1, including nested/reentrant/revert workloads. All 2,850 setup
deployments mined. The bulk send took 1.401 seconds and receipt monitoring took
185.120 seconds. Total setup, workload and semantic verification took 1,501.333
seconds; verification time is excluded from the throughput window.

Canonical attribution joined 520 inbound and 350 outbound sources to 29 successful
postBatches in L1 blocks **23266418–23266448**. Forty-five initially observed L2
receipt heights changed during reconciliation; the report uses canonical ranges,
not those speculative heights. Repeated attribution was identical for all 870
sources, and all 29 post receipts matched the independent L1 endpoint by hash,
block, status and gas. The final queue was empty, chains were current, and a new
post-wave commitment and its full blob were independently located. No live
follower replay or independent KZG recomputation was performed.

| Measurement | This trial |
| --- | ---: |
| Canonical sources / semantic jobs passed | 870 / 770 |
| Source-bearing postBatches | 29 |
| Sources per postBatch | 30.0 |
| Send + monitor | 186.521 s |
| Sources per second | 4.664 |
| Bulk-send-to-canonical latency p50 / p95 / max | 120.530 / 175.530 / 185.530 s |
| Preparation median / maximum, canonical candidates | 269 / 474 ms |
| Proving median / maximum, canonical candidates | 90.558 / 165.130 ms |

The first two batches each contained **37 inbound / 0 outbound** sources; the
third contained 26 inbound / 11 outbound. All 35 proved/dispatched candidates
reported `fifo_bypassed=false`. Thirty-two speculative mixed candidates reported
a bypass, were discarded, and emitted no proof request or dispatch. Concurrent
sender completion order does not independently establish total ingress order:
these live invariants and direction counts complement the controlled older-source
native regressions in the preceding audit.

There were 35 relay acknowledgments: 29 included in their target, five targeting
skipped slots and one targeting a produced block without the post. A relay
acknowledgment alone did not complete those six attempts. Six skipped L1 slots
occurred between first and last included posts. The ordinary L1 adapter recorded
2,329 client-disconnect `BrokenPipeError`s, zero upstream rate limits and no failed
deployment/source stage. Its transport errors are retained separately; they are
not a count of failed transactions. It was stopped after verification.

## Cutoffs and performance

| Final cutoff | Canonical postBatches | Sources |
| --- | ---: | ---: |
| `snapshot_exhausted` | 22 | 637 |
| `l1_bundle_gas_budget` | 4 | 125 |
| `l1_tx_gas_budget` | 2 | 74 |
| `pipeline_time` | 1 | 34 |

`snapshot_exhausted` often means the reduced FIFO snapshot ended while the wider
queue still held work. Seven canonical candidates inherited `fifo_prefix_limit`.
Five of those posted only **3, 2, 13, 3 and 1 inbound sources**, at blocks 23266425,
23266429, 23266431, 23266435 and 23266446. The pool retains the retry boundary
until its terminal source leaves; if a preceding attempt consumes only part of
that prefix, its small remainder can occupy a separate posting. Strict FIFO does
not itself require leaving room unused after that remainder has been admitted.

The 32 discarded FIFO rebuilds were triggered by 21 transaction-gas cuts, three
bundle-gas cuts and eight pipeline-time cuts. Their start-to-summary intervals
totaled 3.564 seconds. Do not sum their `finished` intervals: those include nested
replacement work. Across 67 candidate attempts there were 2,334 simulations and
30 cheap gas refusals; the latter avoided simulation for 58.8% of gas refusals.
No cold calibration post or canonical nominal preparation timeout occurred.

The [previous live run](../2026-09-27-admission-framing/README.md#final-estimator-deployment-and-measurement)
used 23 posts / 146.880 seconds for the same 870-source workload. This trial used
six more posts and 39.641 more seconds (+27.0%). These are different-day runs with
different parent state, block gas limits and slot production; there is no fresh
live paired baseline. The earlier matched local comparison remains 536 sources /
13 posts per variant. Neither that local result nor successful semantic checks
erases the observed live packing loss or the two CI packing failures.

Next improvement candidates, preserving strict FIFO:

1. Allow an already admitted retained-prefix remainder to be followed by younger
   eligible work in the same candidate, while preserving ownership and refusing
   any candidate that omits an older eligible source. Regress partial consumption,
   replacement and deadline retries before another measured rollout.
2. Avoid repeatedly building the outbound-heavy candidate that will require a
   FIFO rebuild. Preserve the pinned parent/target and exact final execution.
3. Refine companion reserves using the verified multi-call family measurements
   below; keep native signed-limit validation. Blob preparation is still the
   largest canonical preparation interval: mean 174.6 ms out of 273.5 ms (63.8%).
   Prefix hashing averages 52.2 ms and overlaps other source intervals.

These follow-ups were identified by the trial and are not implemented here.

## Gas estimates and Grafana

[Per-post table](evidence/postbatch-report.csv) includes all 29 blocks, direction
counts, complete admission bounds, native preflight, signed limits, receipts,
timings and cutoffs. [Companion table](evidence/companion-breakdown.csv) groups all
520 inbound estimates against their own canonical L1 transaction receipts.

| PostBatch gas, sum over 29 posts | Gas |
| --- | ---: |
| Admission bound including framing | 237,333,762 |
| Full projection | 235,315,954 |
| Native preflight used / actual receipt used | 173,212,787 |
| Native preflight required at execution | 175,540,787 |
| Signed limits | 184,317,838 |

Every native preflight used-gas value matched its receipt exactly. Every full
projection fit the complete admission bound; every signed limit equals the
rounded-up required gas plus **5%**. The admission bound is 1.370× actual gas in
aggregate. Candidate framing reserves ranged 29,824–206,720 gas; the fixed 600,000
reserve is absent. The 520 companion estimates totaled 123,569,998 versus
73,607,502 actual gas (**1.679×**). Nested multi-call and twice-call families remain
at 2.213× and 2.071×. Companion signed transaction limits are distinct from actual
used gas and must still fit the ordered block-start envelope.

The four bundle-limited batches estimated 98.3% of their parent budgets; actual
used gas was 59.8%, and the verified signed-start envelope averaged 64.4%. All
signed-start envelopes fit. This identifies conservative estimation headroom,
not permission to raise a network gas limit or ignore final validation.

All 870 canonical sources have accepted admission estimates and all 520 inbound
companions have canonical receipt coverage. Missing full estimates on cheap
refusals are intentional. Grafana's actual provisioned queries returned 67
candidates / 2,368 source-attempt rows, the FIFO policy and all 32 rebuild details,
and 29 receipt rows totaling exactly 173,212,787 gas. Full estimates and cheap
minimum-check fields have different applicability; nullable fields are not zeros.

## Monitoring compatibility fix

The old indexer stopped canonical collection after handover because every cycle
validated genesis using `eth_getBlockByNumber(0)`, which now returns pruning code
4444. Log collection continued, so admission estimates were visible while actual
receipts were missing. The first failed measurement is retained. Source
`2cb7d611e3a4aa879ef027de20d0a7cf85408240` adds a fallback only for that code:
fetch bounded raw genesis bytes, compute Keccak locally and require the configured
genesis hash. HTTP identity, head sampling and WS setup use the same validation.
Unrelated RPC failures, malformed/missing data and wrong identities still fail.

The regression suite first reproduced three failures on the old implementation;
the candidate passed all four new cases. All 28 default standalone tests,
formatting and all-target Clippy with warnings denied passed on Rust 1.95.0 and
locked dependencies. Two disposable-PostgreSQL tests were excluded; no schema
changed. Node workspace suites were not repeated for this independent workspace
fix; preceding node results and unresolved CI failures remain separate evidence.

The local indexer image was built from its committed archive with the existing
Docker recipe except Rust 1.95.0 in the builder. Its image/executable hashes and
recipe/archive digests are in [trial identity](evidence/trial.json). The image-only
rollout uses the additional local override
`data/chiado-strict-fifo-20260928/monitoring.compose.yml` and preserved rendered configuration, command, mounts and checkpoints:
260,175 admission events / 28 log-file records before and immediately after.
The indexer backfilled canonical receipts, head sampling and Grafana without
resetting history. Companion receipt enrichment lagged that recovery briefly;
its first incomplete-coverage assertion is retained, followed by full coverage.

## Evidence and retained state

The node/signer remain at `b2ff3b8`; the indexer is at `2cb7d61`. Original L1/L2
directories remain separate from candidate-written copies. Restoring those old
copies now requires catch-up/reconciliation of later canonical settlements; an
old binary must never open the candidate-modified databases. Operator edits,
Lighthouse, explorers, monitoring database and retained posting logs were preserved.

Executed validation commands included:

```bash
bash scripts/run-chiado-staged-performance.sh --check --verify
bash scripts/run-chiado-staged-performance.sh --verify
cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml
cargo +1.95.0 clippy --locked --manifest-path monitoring/indexer/Cargo.toml --all-targets -- -D warnings
cargo +1.95.0 fmt --manifest-path monitoring/indexer/Cargo.toml --check
git diff --check
```

The workload commands used the private task profile and existing wallet lock.
Canonical analysis adapts the [retained measurement helper](../2026-09-26-admission-throughput/measure-wave.py)
to start its timestamp search at registry deployment block 23226793, avoiding
pruned pre-deployment history. The initial pruning failure is retained. All
receipt, block, uniqueness and correlation assertions remain enabled.

Small summaries and tables are committed under [evidence](evidence/).
[Artifact manifest](evidence/local-artifact-manifest.json) records paths, sizes
and hashes for local-only logs, operational records and raw attribution/events.
Those ignored artifacts and private state are unavailable to another checkout;
their hashes are references, not independently retrievable evidence. This trial
does not validate durable queue recovery, unattended restart, full historical
replay or the unresolved CI packing assertions. No merge or release-tag promotion
was performed.
