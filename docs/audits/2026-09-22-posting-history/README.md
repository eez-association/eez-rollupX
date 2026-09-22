# Posting lifecycle history — implementation and live validation

Date: 2026-09-22. Implemented and deployed to the existing Chiado test network.
See the [requirements/coverage](../../proposals/posting-lifecycle-history.md) and
[operator procedure](../../../monitoring/indexer/README.md#posting-lifecycle-history-optional).

## Source and deployment identity

- Base: `b6fdaf7594e90c57de32b27bfd8807f49a95f60b` plus the reviewed working-tree changes.
- Protocol: `3c0cdf4955941a5c23639346932b55b33babebbd` (unchanged).
- Runtime build-context SHA-256: `6ba71c9bf82460cc942ab9b5b501457d25f0a382c22c51fc81bcaf9899f2ee48`.
- Cargo.lock SHA-256: `00217f48dc4d6c974d11b20692ab28e27e32e7dd60262138a9c8c3c3d668fbc6` (unchanged).
- Composer image: `sha256:323c4803e524f66cf49d62c48aaec9672e79c3d65137fc67e734482ce2957995`.
- Matching signer image: `sha256:614a7279c065192e5b26dac01c7ce3d370c77d1de1c5d07e9b635a48d59e7029`.
- Standalone indexer image: `sha256:663ba8aaa29ac4046a7575f9debd5b83f4ac32295f1b7cfc3e1079c7ee825660`.

These are local release builds, not CI-published artifacts. The build context,
logs, pre-rollout configuration, database dump, canary results and verification
JSON are retained locally under
`.runtime/chiado-observability-20260918/validation/posting-history-20260922T112833Z`.
The canary directory is recorded there in `canary-path`. These private artifacts
are not part of the repository. Runtime build-context hashing uses sorted relative
paths and file bytes for Cargo.toml, Cargo.lock and crates; Dockerfile is excluded.

## Observed behavior

The queue was empty before restart. Composer and signer stopped cleanly. Startup
preserved the previous safe L2 block/hash and genesis; L1/L2 heads were fresh and
not syncing. The four-hour pure-L2 interval and 500-bps gas margin were retained.
No datadir reset, contract deployment or unrelated service restart was performed.

A small canary submitted one inbound and one outbound transaction through the
existing queue endpoints under the existing wallet lock. Both succeeded, with
canonical source receipts, matching destination events and final contract values.
They produced two postings with four recorded stages each: prepared, repriced,
dispatched and included. Both captured `cross_chain` as the reason. Repricing
changed the signed transaction hash while preserving each attempt/posting ID.

| L2 Sync | L1 inclusion | Prepared → dispatch | Dispatch → inclusion observation |
| --- | --- | --- | --- |
| 326030 | 23176896 | 104.4 ms | 2.002 s |
| 326035 | 23176897 | 78.5 ms | 4.192 s |

All eight PostgreSQL events match the retained structured logs. Identifiers,
stages, hashes, reasons and counts were compared exactly; floating duration
comparisons allowed 1e-9 ms for JSON numeric normalization. Both included receipt
hashes/heights and prepared L2 hashes were checked through the corresponding RPC.
Grafana's PostgreSQL queries returned both canonical postings, their intact
sequences and two captured cross-chain trigger reasons. Browser inspection
confirmed the live status, recorded trigger chart, populated table and units.

Restarting only the indexer preserved all eight events with no duplicates and
restored fresh, error-free collection. The source mount was verified read-only.

## Checks

- Rust 1.94.0 `cargo test --locked -p eez-composer --lib`: 219 passed; the existing
  manual component timing profile remains ignored.
- `cargo clippy --locked -p eez-composer --all-targets -- -D warnings`: passed.
- Standalone indexer `cargo test --locked`: 20 passed; the database test is opt-in.
- Disposable PostgreSQL recovery test with `--ignored`: passed. Covers atomic
  import/checkpoints, duplicate/conflicting replay, out-of-order events, completeness,
  canonical joins/reorgs, actual dashboard SQL, configured worker ingestion and
  cancellation/lock release, alongside the existing canonical scanner regressions.
- Indexer `cargo clippy --locked --all-targets -- -D warnings`: passed.
- Cargo format checks, `git diff --check`, Compose overlay validation, dashboard
  JSON IDs/layout and live datasource queries: passed.
- Matching release node/signer/follower and genesis helpers built successfully;
  indexer release container built and deployed.

Socket-based tests require execution outside the restricted sandbox; their
initial sandbox invocation could not bind localhost. They passed with socket
access. The full workspace/process-test matrix and performance benchmark were
not run for this observational increment. The live canary checks semantic effects,
but does not replace independent follower replay or a performance comparison.

## Boundaries

Collection is disabled by default. No Composer database client, telemetry network
call, new worker, dependency update, Reth patch or CI job was introduced. File
reads occur in the standalone indexer; empty files do not cause cursor rewrites.
Events do not own requests or change retry/settlement decisions.

Reth's existing nonblocking logger can lose buffered events on a crash/overflow;
retention can remove unread files. Sequence checks expose partial recorded attempts,
but cannot prove that no entire attempt or later final event was lost. This is
retained operational history, not a transactional outbox. The canonical index
remains authoritative for commitments, and a receipt is not proof that every
cross-chain effect completed. Historical log collection starts with this rollout;
older off-chain attempts cannot be reconstructed from sampled metrics.

## Expected inclusion timing extension

Added `posting_timing`, an independent SQL view over retained events and canonical
blocks, plus a five-second-refresh point graph and expected/actual table fields.
No binary, configuration, worker or source-event changes were needed. The separate
view preserves the old `posting_attempts` signature, including earlier indexer
startup schema application. Applied only the new view and provisioned the dashboard;
no services were restarted and no additional transactions were sent.

Live RPC checks confirmed both existing postings hit their target height and exact
header timestamp: L1 23176896 and 23176897. Dispatch offsets were -1.351 s and
-3.367 s; receipt observations followed the block timestamps by 0.651 s and 0.825 s.
The graph uses the target timestamp as zero and keeps each attempt's points separate.
Missing canonical data remains null, and an observed hash that differs from canonical
inclusion cannot supply receipt-observation timing. Host clock skew remains a limit
of event-versus-header wall-clock comparisons; block-height comparison is independent.

Validation: 20 standalone indexer tests passed, plus the explicitly enabled disposable
PostgreSQL recovery test. The latter now checks on-target, earlier/later targets,
missing dispatch/timestamp, mismatched receipt hash, reorg invalidation, and actual
Grafana table/graph SQL. Indexer Clippy, formatting and diff checks passed. An initial
test edit placed a DB assertion in the socket-only test; it failed compilation and
was corrected before the passing runs. Browser inspection confirmed the graph's
points, zero baseline, colors and legend. No performance benchmark or workspace/node
suite was repeated for this SQL/dashboard-only change.

Dashboard backup, applied SQL, query results, RPC checks and browser capture are
local-only in `.runtime/chiado-observability-20260918/validation/posting-target-timing-20260922T122525Z`.

The timing graph also includes a purple **Prepared** point, using the explicit
prepared event timestamp rather than the first available event. This marks the
submission after proving, before final repricing. The existing two attempts show
-1.456 s and -3.446 s relative to their targets. This addition changed only the
panel SQL/configuration, its PostgreSQL regression and documentation. No schema
change, restart or new collection was required. The actual graph query and missing
prepared-event case passed the disposable PostgreSQL test; focused Clippy, formatting
and diff checks passed. Live Grafana returned all four series. Results and the
pre-change dashboard are saved in the timing evidence directory above.

## Requested staged workload (13:25–13:38 UTC)

Ran the unchanged `scripts/run-chiado-staged-performance.sh` against the current
Chiado rollup (L1 10200, L2 6291), using the initialized protocol checkout at the
same pinned commit. The root/worktree helper copies matched. Used 16 preparation
workers and 28 send workers; the script's semantic verifier remains disabled.
The existing profile referenced the previous registry and an unfunded current-L2
source. A separate private profile corrected the registry and retained the L2
manager predeploy. Funded that test source with 5 L2 ETH from an existing public,
genesis-prefunded fixture account under the shared wallet lock. Existing profiles,
node configuration and chain state were preserved; no services were restarted.

Results: 770/770 jobs prepared, all 2,850 setup transactions mined, and 870 source
transactions submitted (520 inbound, 350 outbound). Preparation took 63.7 seconds,
setup mining 126.3 seconds and source submission 1.398 seconds. Monitoring timed
out after 600 seconds: 138 source receipts observed (33 L1, 105 L2, all status 1),
732 unmined, exit code 1. Total runner duration was 819.28 seconds. Source receipts
are not proof of completed cross-chain effects; L2 observations may be speculative.
No semantic verification or independent follower replay was performed.

At the 13:39:15 UTC post-run snapshot, history contained 47 attempts: 7 included
in their expected blocks, 12 excluded, 15 slot-skipped, 12 preflight target-change
deferrals and one dispatched attempt. Five excluded attempts shared a transaction
hash eventually included by a later attempt; they are not extra successful posts.
Successful attempts' receipt observation delay was 1.051 s median, 1.4811 s p95,
1.494 s maximum. The queue retained 735 queued and one in-flight request; its
ownership counts need not equal the runner's unmined-receipt count. Composer
continued processing after runner timeout. No automatic resend or queue reset.

Prometheus scrape health was 100% across the run and post-run snapshot. The collector
had no error; all 47 observed event sequences were intact. For 163 collected events,
wall-clock ingestion delay was 1.000 s median and 1.928 s p95. These observations
validate collection under this workload, not zero telemetry overhead: no comparable
telemetry-disabled baseline was run. Logs also showed proof-attempt cutoff expiry;
local final bundle simulation succeeded and builder replies arrived for excluded
attempts, which does not establish the builder's reason for omission. Investigation
of exclusions, skipped slots and preparation/proof timing is the useful follow-up.

Local-only evidence: `.runtime/chiado-observability-20260918/validation/staged-posting-timing-20260922T132352Z`
contains the private profile, funding receipt, runner log, timing/result CSVs,
`result.json`, `observations.json`, sanitized warning summary and browser graph.
Upstream run artifacts remain in `eez-core-protocol/tmp/e2e-staged-net/20260922-132506`.
Do not blindly rerun/resume while the original requests remain owned by Composer.

## Builder/relay investigation of the staged exclusions

Used the operator-supplied [Chiado log viewer](https://logs.chiado.gcd.ovh/) only
as investigation evidence, not as a runtime dependency or a mainnet requirement.
Downloaded 13:28:35–13:38:50 UTC using the viewer's read-only historical API, explicit
log levels, bounded intervals and recursive splitting of full 500-row responses.
Retained 2,252 builder groups, 1,733 relay records and 136 execution-client groups.
Correlated target heights/timestamps, local events, canonical headers and relay
payload hashes. The earlier post-run snapshot also includes continued attempts;
its counts must not be mixed with this bounded set of 32 dispatched attempts.

| Canonical outcome for dispatched targets | Count | Evidence |
| --- | ---: | --- |
| Included at the target | 7 | Transaction appears in the canonical target block; its hash matches the relay's delivered payload |
| Target slot skipped | 15 | Consecutive canonical parent/child headers bracket the missing timestamp |
| Target block produced without the transaction | 10 | Exact-timestamp canonical block does not contain the submitted hash |

All 32 target slots had relay-validated eligible bids before their timestamps.
For the 25 unsuccessful targets, the retained relay logs contain no header HTTP
request, delivered bid or delivered execution payload. All 7 successful targets
have matching header/payload delivery. Candidate parents match canonical parents
for produced slots. There is no `no MEV-Boost relay data` discard for these targets.
The ten omitted blocks report Nethermind (7) or Erigon (3) in their extra data,
and two fee-recipient addresses; this does not identify validator ownership or
prove their configuration. We cannot distinguish intentional local production,
relay connectivity/configuration, or another proposer-side decision from these logs.

Example: target L1 23177858 at 13:29:20 had an eligible bid at 13:29:16.716,
3.284 seconds early. The canonical block omitted our transaction and this relay
received no header request. Conversely, target 23177867 at 13:30:25 was delivered
by this relay and its delivered hash exactly matches the canonical inclusion.
For skipped target 23177863 at 13:30:00, a bid was eligible 3.504 seconds early,
but canonical headers confirm no block at that timestamp.

Local final simulation passed for all 32 submissions. Twenty-eight candidate
block gas totals exactly match local bundle gas plus the builder payment; four
also contain other transactions. Correlation by slot, parent, timing, blob usage
and gas supports the diagnosis, but undelivered payload bodies were not retrieved:
do not claim cryptographic proof of their full transaction contents. The directly
proven failure boundary is no delivery from this relay for unsuccessful targets.
There is no basis here to increase the gas margin or fees to fix these omissions.

Local work also lost opportunities before dispatch. Twelve preflight failures
reported a changed target/parent/window. Their nearby policy selections line up
with missing expected parent timestamps and waits of about 6.5–7.8 seconds. This
association is inferred: the prepared event currently lacks its target, and
nearest-log pairing is not generally reliable across concurrent attempts. A
concrete example is the 13:35:50 target: its expected 13:35:45 parent slot was
absent, and the expected parent height instead has timestamp 13:35:50. The existing
`prepare_policy_submission` guard correctly refuses that incompatible target.

Four separate attempts exceeded the **proof-start** cutoff before calling the
signer. Measured preparation/allowance was 600/491, 564/489, 577/489 and 267/216 ms;
blob preparation consumed 414, 375, 377 and 233 ms respectively. These are local
preparation-budget overruns, not evidence of slow signer responses. The current
post-proof `prepared` event cannot capture those failed opportunities.

Recommended small follow-ups, not implemented by this read-only investigation:

1. Split displayed outcomes into included, network slot skipped, produced block
   without inclusion, and pre-dispatch deferral; preserve an unknown external reason.
2. Capture expected target height/time on the prepared event, before dispatch,
   and add a bounded event for preparation failing before proof starts.
3. Investigate the measured blob-preparation reserve against the proof-start
   cutoff; retain the safe deadline and target checks.
4. Use our events, receipts and canonical L1 headers for mainnet monitoring.
   Relay participation/connectivity is an external operational investigation;
   do not integrate this Chiado log site into normal collection.

Evidence is in the staged run's local-only `investigation/` subdirectory:
`correlation.json`, `canonical-blocks.json`, `local-causes.json`,
`gas-correlation.json`, retained local/remote logs, source request provenance,
correlation script and SHA-256 manifest. No code, runtime configuration, funding,
submission, retry, queue or service state was changed during this investigation.

## Follow-up implementation: diagnosed outcomes and prompt enrichment

Worktree `feat/submission-outcomes-metrics`, base `b6fdaf7594e90c57de32b27bfd8807f49a95f60b`,
protocol unchanged at `3c0cdf4955941a5c23639346932b55b33babebbd`. This is a dirty
local development build, not a published CI release. Exact source copies, source
hashes, build logs, image IDs, pre-update configuration and PostgreSQL dump are in:

```text
.runtime/chiado-observability-20260918/validation/posting-diagnostics-20260922T151311Z
```

These artifacts are private/local-only. The independent final indexer source is
`indexer-final-source`; `indexer-final-image.json` supersedes its initial image.
The snapshots retain untracked implementation files as well as the tracked diff.

The pre-update database already had all 200 posting rows, but 36 lacked fee and
outbound totals because collection required finalized L2 ranges. Canonical L1
indexing was current. Enrichment now accepts verified canonical ranges, rechecks
provisional cached hashes on each pass and on promotion, and retains explicit
L2 status. Up to four different transaction rows run concurrently, with existing
per-row scan budgets and failure backoff. Finalized cached history is untouched.
No new service, database, Composer metric, dependency or CI lane was added.

Live progress after indexer deployment: fee pending 36 → 9 → 1 → 0;
outbound pending 36 → 4 → 0. All 200 postings then had complete fee/count data,
including 36 still-canonical ranges. The last range comprised 5565 blocks and
finished through resumable 128-block scans. This verifies backlog recovery under
the current shared-host load; it is not a controlled performance comparison or
a guarantee that large ranges complete in one dashboard refresh.

The new posting view recomputes outcomes from canonical headers, distinguishing
proven skipped timestamps from blocks produced without observed inclusion. Target
parent/window gates expose typed reasons while preserving the boolean API and
existing decisions. The producer records exact targets before dispatch and
unsigned proof-gate/prover failures. No proposer intent is inferred from omission,
and public Chiado infrastructure logs are not used by the runtime.

Validation executed:

- Composer library: 220 passed, one existing manual timing profile ignored.
- L1 library: 139 passed. Fake-clock tests check expired windows, unavailable
  parents and mismatched parent timestamps without changing waits.
- Indexer suite: 21 passed; the PostgreSQL test was separately selected against
  a disposable PostgreSQL 18.6 container and passed. New regressions cover legacy
  pending-cache recovery, canonical totals, finality promotion without receipt
  rescans, reorg invalidation, target consistency and unsigned failure payloads.
- PostgreSQL fixtures verify produced slots, adjacent-header proof of a skip,
  missing-history uncertainty and preparation failures with no fabricated hash.
- Workspace/all-target Clippy and independent indexer/all-target Clippy passed.
  Rust formatting and `git diff --check` passed.
- All 13 live PostgreSQL dashboard queries passed, including mixed-data-source
  panels. Initial verification used the panel's mixed datasource rather than its
  target datasource and returned HTTP 404; correcting that verification request
  passed without changing the dashboard query.
- Public Grafana browser rendering showed the outcome categories and queue state;
  panel IDs/layout were checked for duplicates/overlap.

The existing broader native E2E matrix and controlled metrics-overhead benchmark
were not rerun for this observational increment. Local simulation/prover failure
branches are fixture-tested; deliberately breaking live proving/parent availability
is not required to validate telemetry and was not done.


Final deployment and canary:

- Indexer `sha256:f1f4341152db1509e89845fa713157c7eebf26e56f839426292dbad96e575213`.
- Composer `sha256:1671f293d8e550581a160fcd78d51c047cb6f35cbcef5bf266fed2ba10810528`.
- Signer `sha256:c56ab63b03450ec9277b9d4ccb7939ee41876264294621b1079d912b859f2b9c`.
- Updated indexer first. Rechecked an empty queue, stopped node/signer cleanly,
  then verified fresh heads, the original genesis and retained safe hash. Gas
  margin remains 500 bps; pure-L2 interval remains 14400000 ms.
- One existing historical row temporarily returned to pending during the RPC
  restart; its normal retry restored complete totals. All 200 historical posting
  rows were again complete. No database/queue reset was performed.

The two-source canary at `posting-diagnostics-canary-20260922T152713Z`
**failed its 300-second inclusion deadline** (runner exit 1). Both source receipts
remained absent. The inbound source experienced three produced-but-excluded targets;
the outbound source experienced two skipped timestamps plus three
produced-but-excluded targets. The canonical reconciliation records charge the
six produced-block exclusions and do not charge the two skipped slots. Queue
counts then became zero. This matches the unchanged `MAX_BUNDLE_ATTEMPTS=3`
resolution rule, which stops retrying each source after three charged episodes;
an empty queue is not evidence of successful completion.

The source hashes, signed inputs, failed runner record and reconciliation evidence
are retained. No automatic resend, fee change or retry-limit change was made.
The live test does **not** establish successful inclusion or destination effects
for the newly deployed node. Successful-path event parsing and canonical joins
remain covered by the unit/PostgreSQL fixtures and the earlier deployment's canary;
that earlier canary does not validate successful execution of this new binary.

The exclusion telemetry itself was verified: all 32 events across eight attempts
match retained JSON logs; sequence numbers are contiguous; target height/time are
present from preparation and unchanged through dispatch/outcome. All eight outcomes
match the canonical header categories; ingestion health had no error. No live
proof/parent failure was induced. Those branches remain fixture coverage only.

Remaining follow-up: investigate proposer-side omission and review whether the
existing source retry limit is appropriate under sustained relay/proposer
non-inclusion. Neither changing that ownership policy nor depending on the public
Chiado log viewer belongs to this monitoring change. The precise external reason
for omitting a transaction cannot be established from canonical headers alone.


## Detailed failed-canary investigation

The [per-attempt builder/relay/proposer report](excluded-canary.md) separates the
six produced-block exclusions, identifies the proposers, compares a successful
control, reviews retry accounting and provides a concrete operator handoff.
The [CSV](excluded-canary.csv) includes exact target slots, transaction hashes and
validator pubkeys. No runtime or policy changes were made during that investigation.

## Wallet bridge UI repair

The [bridge configuration repair](bridge-ui-fix.md) records the separate protocol bridge deployments and public browser verification.

## Final PR update verification

Before updating PR #34, rechecked the accumulated source and final dashboards:

- `cargo +1.94.0 fmt --all --check` and independent indexer formatting passed.
- `cargo +1.94.0 test --locked -p eez-composer -p eez-l1 --lib`: 220 Composer and 139 L1 tests passed; the existing manual timing profile remains ignored. Initial sandbox runs failed on denied mock-server socket binds; rerunning with loopback permission passed.
- Independent indexer `cargo +1.94.0 test --locked`: 21 passed; the PostgreSQL test was explicitly run separately against a disposable PostgreSQL 18.6 database and passed.
- The PostgreSQL test now executes the combined chart's outcome and annotation SQL and verifies two attempts at one target retain distinct details.
- Workspace/all-target Clippy and indexer/all-target Clippy passed with `-D warnings`. Compose rendering with the optional posting overlay, dashboard JSON parsing and `git diff --check` passed.
- Public Grafana browser checks verified the combined timing/outcome panel, real per-attempt hover details, and removal of only `EEZ Settlement`; `EEZ Settlement history` remains. The four attempts from the bridge investigation appear at their expected timestamps.
- Updated the deployment runbook to distinguish wallet bridge contracts from workload helpers. No private runtime files, keys, raw signed bundles or database exports are included.

The controlled performance benchmark remains deferred at the user's request; the full native E2E/follower matrix was not rerun for this final observational update. The latest live binary's unsuccessful inclusion canary remains a failure, with its telemetry and external investigation documented above. Neither earlier successful waves nor passing fixtures erase that limitation. No retry-policy or relay configuration change was made to obtain a passing result.
