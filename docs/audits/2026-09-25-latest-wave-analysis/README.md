# Latest staged wave: admission, gas and missed slots

**Investigated 2026-09-25, read-only.** Run `20260925-220903` settled all 870
sources in 29 successful postings. Preparation and proving met their deadlines.
Gas admission and missed inclusion opportunities explain the split and elapsed
time. The earlier run used 33 successful postings, but fewer missed slots; both
source windows took about 186 seconds.

## Identity and scope

| Field | Value |
| --- | --- |
| Fork | `7c90266a5fe8cfdb46b249c61079badc2dce71a3` |
| Initialized, clean protocol | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Node image | `sha256:8f95f8b0c20ac5cfdebb85f89321081053f0604a61c1651eafbec63b858d1710` |
| Signer image | `sha256:d4db1e6de248aaafdaaf476b2ad0ec4a6b0ef1192af82e7bfd0475056efe328f` |
| Network | Chiado L1 10200, L2 6292, rollup 1 |
| Registry | `0x5826242cf480856c36ad4afa464954d3571d1b5b` |
| Latest source window | 22:15:05.098–22:18:10.717 UTC |
| Latest canonical postings | L1 23229349–23229379; 29 blocks contain postings |
| Workload | 770 jobs, 14 targets, 520 inbound and 350 outbound sources |

The [operational record](../2026-09-25-chiado-resync/staged-performance-tuned.md)
documents the authorized earlier configuration change, runner and setup relay.
The [runtime identity record](../2026-09-25-chiado-resync/README.md#executable-identity)
records CI provenance and its verification limits. This investigation changed
no runtime, configuration or service state. Existing runbook edits and recovery
audit files were preserved.

## Results and comparison

| Measurement | Previous `20260925-213320` | Latest `20260925-220903` |
| --- | ---: | ---: |
| Canonical successful source receipts | 870 | 870 |
| Successful postings | 33 | 29 |
| Prepared/dispatched attempts, including retries | 37 | 36 |
| Sources per successful posting, median / maximum | 27 / 50 | 33 / 51 |
| Canonically skipped 5-second slots | 2 | 6 |
| Produced blocks without a posting | 2 | 2 |
| Source send plus receipt monitoring | 186.006 s | 185.618 s |
| Postings plus inbound companions, receipt gas | 270,212,005 | 252,460,553 |

Both windows span 37 scheduled slot timestamps from their first target to their
last. The previous run used 33 slots successfully and lost four opportunities;
the latest used 29 successfully and lost eight. Four fewer successful postings
were offset by four more missed opportunities. This accounts for the similar
elapsed times without attributing the whole runner's much faster setup to
admission tuning.

The latest run used 12.1% fewer successful postings and 6.6% less total receipt
gas. These are observed differences from one run per configuration. Workload,
images, scale and concurrency match; chain state, ordering, slot production and
admission history differ. This comparison does not isolate a causal improvement.

## Preparation and proving

Active settings were `auto`, preparation 1,000 ms, proof 450 ms, submission slack
450 ms, and postBatch gas margin 500 basis points. The previous values were
500/750/700 ms and the default 2,000 basis points. No operator gas/blob cap was set;
the separate 600,000-gas planning reserve remained active.

All 36 latest attempts completed preparation, obtained an attestation, passed
ordered preflight and dispatched. Every attempt used one physical blob. There
were no preparation failures, prover deadline failures, or time/blob admission
cut events in the retained source window.

| Latest timing, 36 attempts | Median | Maximum |
| --- | ---: | ---: |
| Admission phase | 150 ms | 330 ms |
| Finishing phase | 186 ms | 276 ms |
| Blob preparation, included in finishing | 159 ms | 237 ms |
| Total preparation, monotonic log measurement | 336 ms | 559 ms |
| Proof request to response, wall-clock lifecycle measurement | 84 ms | 116 ms |

Available preparation windows were 981–997 ms. Even the least spare window had
438 ms remaining at proof handoff. Dispatch occurred 1,110–1,600 ms before the
target timestamp. Longer preparation allowance is not indicated by this run.
Blob preparation accounts for 5.896 of the aggregate 12.623 preparation seconds;
the logs do not separately profile each KZG operation.

The previous run also had no lifecycle preparation failures: its successful
preparation wall-clock range was 195–422 ms, median 318 ms. Wall-clock lifecycle
durations differ by roughly a millisecond from the latest monotonic timing
events because their measurement boundaries and rounding differ.

## Gas admission remains conservative

There were 25 gas cuts across all attempts, including retries:

- 16 hit `l1_tx_gas_budget`.
- Nine hit `l1_bundle_gas_budget`, which also reserves ordered inbound companions.

The resolved postBatch projection cap was 15,378,300 gas:
`floor(16,777,216 / 1.05) - 600,000`. At the old 20% margin the equivalent cap
was 13,381,013, so the applied 5% setting reached the running planner. Other
attempts can stop at an adaptive count selected after earlier cuts or submission
backoff. `selected` is a candidate count; it does not mean every candidate was
executed or admitted.

The [policy](../../configurable-settlement-policy.md#configuration)
and [gas estimator](../../../crates/eez-composer/src/composer/gas.rs) distinguish
these planning reserves from measured execution. Unknown or complex entry
shapes retain conservative reserves, and companions also need their next
declared gas limit to fit. Final ordered execution remains mandatory.

For example, L1 **23229374** admitted 51 inbound sources and cut on the bundle
estimate. The postBatch projection was **13,049,248**, while the canonical
postBatch receipt used **2,760,858**. PostBatch plus all 51 companions used
**8,453,802** against a block limit of **17,385,848** (48.6%). This demonstrates
substantial estimate headroom for that batch; it does not prove another whole
group would satisfy every declared-gas and execution check. Across all 29
successful postings, median bundle utilization was 48.6%, maximum 79.0%.

The first attempt admitted only one source because automatic admission starts
with a single group after node restart. Later gas cuts and source-order fairness
also constrain subsequent candidate counts. Raising the preparation timeout
alone would not remove these limits.

## Skipped slots, exclusion and adaptive backoff

Canonical headers establish absent slots at **22:15:40, 22:15:50, 22:15:55,
22:16:05, 22:17:05 and 22:17:30 UTC**. They account for 30 seconds without an
L1 block. Five speculative replacements were prepared.

Two produced blocks were empty:

- **23229356, 22:16:00:** no attempt targeted this timestamp. Both the original
  22:15:50 target and its 22:15:55 replacement were skipped. The scheduler
  [disarms after one replacement on an unchanged head](../../../crates/eez-driver/src/slot.rs),
  consistent with the [accepted ownership contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).
  Canonical reconciliation completed around 22:16:01 and the next attempt
  targeted 22:16:05.
- **23229372, 22:17:35:** the 45-source replacement was dispatched 1,189 ms before
  its target and passed preflight, but the produced block contained no
  transactions. Its observer reported `excluded`. The retained node evidence
  cannot establish the relay/builder/proposer reason for that exclusion.

The latest run also demonstrates a conservative admission interaction:

| Reconciliation UTC | Old submitted groups | Next count cap | Old attempt's canonical outcomes |
| --- | ---: | ---: | --- |
| 22:15:46 | 35 | 17 | One committed, 34 retryable after replacement |
| 22:16:11 | 31 | 15 | 30 committed, one retryable after replacement |
| 22:17:11 | 15 | 7 | 14 source-completed-with-missing-effects, one retryable |
| 22:17:36 | 45 | 22 | All retryable after actual exclusion |

The first three old targets were canonically skipped, but their replacements
had already consumed overlapping sources when reconciliation ran.
[The current rule](../../../crates/eez-composer/src/composer.rs) suppresses backoff
only when the skipped attempt has **all** sources still retryable. Mixed outcomes
therefore halve the old submitted count. This behavior is explicit in
`submission_backoff_required` and its existing unit tests; it is not a timeout
misconfiguration. Three other skipped attempts retained their estimates.

The 14 missing-effects labels belong to an old attempt's manifest. Its replacement
at 23229368 admitted and canonically completed all 15 of its own sources; all
29 included attempts' accepted counts match their canonical source counts.
Those labels alone do not establish missing effects in the final workload.
Any improvement here must distinguish proven replacement consumption from real
partial consumption before changing the capacity estimate.

## Evidence and executed checks

[attempts.csv](attempts.csv) records each target, admission count, preparation,
proof and dispatch timing, constraint, canonical gas, transaction identity and
reconciliation outcome. [summary.json](summary.json) includes both runs' canonical
posting summaries, skipped timestamps, queue snapshot, settings, and SHA-256
digests of retained raw evidence. Raw logs and RPC snapshots are local-only,
ignored, and have no guaranteed retention.

Executed from the repository root:

1. Inspected status, remotes, HEAD, protocol gitlink and recursive submodules;
   read settlement/ownership contracts and production admission/replacement paths.
2. Inspected existing container identities and safe environment fields; read
   retained node/lifecycle logs. Read the final tail with
   `docker logs --since 2026-09-25T22:18:10Z --until 2026-09-25T22:18:15Z eez-node-chiado`.
3. Parsed the runner CSVs and JSON lifecycle events. Joined attempts by lifecycle
   identity and final repriced post hash, and matched gas preflights by exact target.
4. Used local `eth_chainId`, `eth_getBlockByNumber`, `eth_getBlockReceipts` and
   `eez_getCrossChainQueueStatus` read-only RPCs. Matched every sent hash to a
   successful receipt, canonical block hash and transaction in that block body:
   **870/870 for each run**. Checked each included attempt's source counts and
   every skipped timestamp against consecutive canonical headers.
5. Confirmed all queue categories zero. Last completion was block 23229379,
   observed at 22:18:11.068 UTC; the final reconciliation marked 36 committed.

The first verification against runner-recorded L2 heights failed: 32 outbound
receipts in **each** run moved to later canonical Sync blocks during replacement.
Canonical block bodies and block receipts resolve all of them successfully.
A sampled baseline L1 transaction returned null from the transaction-hash receipt
lookup even though its canonical block body and block receipt contain it. The
cause of that hash-index discrepancy was not investigated; conclusions use the
verified block data. Neither discrepancy was silently counted as verification.

Both workloads used `--no-verify`. Scenario semantic assertions and independent
follower replay did **not** run. Receipt success, matching accepted counts and an
empty queue establish the reported operational completion, not full scenario
semantics or durable queue recovery. No Rust/Solidity runtime code changed;
runtime suites were not rerun. Documentation whitespace, links and artifact
consistency were checked for this record.

## Follow-up priorities

1. Quantify tighter gas estimates for the actual complex/deferred entry shapes,
   preserving final execution validation and ordered companion gas checks.
2. Evaluate replacement-aware admission learning so independently proven overlap
   does not look like capacity failure, while preserving partial-consumption and
   ownership rules. No behavior change was made in this investigation.
3. Obtain relay/builder delivery evidence for the produced-but-empty target at
   22:17:35. Local preparation timing alone cannot explain its exclusion.

Increasing the preparation allowance further is unsupported by these timings.
