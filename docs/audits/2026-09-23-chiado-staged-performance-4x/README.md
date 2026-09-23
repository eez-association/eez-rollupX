# Fourfold staged performance workload on the existing Chiado network

| Field | Value |
| --- | --- |
| Date | 2026-09-23, 08:29:34–09:07:22 UTC for the retained run and resend |
| Status | Complete after paced recovery; 3,080/3,080 jobs and 3,480/3,480 source receipts |
| Fork/runtime source | `1832f970884ca9460646a4cde87cca70eaffb3e1` |
| Protocol gitlink | `3c0cdf4955941a5c23639346932b55b33babebbd`; pinned checkout clean |
| Node / signer images | `sha256:1de4b3f90648f49b2cfe749c5a6a84c51ffffa6a8178dd26ef4294f58fa4ac7b` / `sha256:1fe82bae33fd602f436c714e9e33c1776a05b5ca8bd855845907008b71a24778` |
| Network | Existing Chiado L1 10200, rollup 1, L2 6290; no service restart or contract redeployment |
| Workload | Same 14 targets as the earlier onefold run, each count multiplied by four; `--no-verify` |
| Authorization | User requested a fourfold performance test and funded the dedicated L1 source |

## Execution and recovery

The fork helper was extended with `EEZ_STAGED_SCALE=4`, retaining the 14-scenario
mix and its complete-run wallet lock. The pool was expanded from 770 to 3,080
unique workers under that lock. The existing private profile and source account
were retained. The user funded the dedicated L1 source; the L2 source was topped
up from the existing public fixture wallet. Worker allocation was 0.005 native
token per layer; it is not a gas-cost measurement. The preflight then passed its
chain, ingress, nonce, code, empty-queue, and funding checks.

The first launch sent too many wallet-funding transactions concurrently. Both
fronts returned `txpool is full`, so this launch stopped before setup or source
transactions. A retry used `MULTISEND_BATCH=220` instead of 100 and completed
all 11,400 setup receipts and 11,320 predicted-address code checks. Its 28-worker
source burst submitted 3,480 transactions in 7.585 seconds. Only 1,456 source
receipts arrived during the next 435.649 seconds; a sample of 100 of the 1,504
pending L1 hashes was unknown to the ingress front, despite initial send
acceptance. The runner was stopped at this point, preserving the run directory.
The unpaced burst therefore **did not complete** the workload.

The helper's new `--resend` mode retained the complete-run wallet lock and reused
the saved signed transactions. It selected 1,624 jobs with no mined source
transaction and sent them with `SEND_WORKERS=4` and `SEND_PAUSE=1.5`. This phase
lasted 774.877 seconds; the following receipt monitor completed in 10.026
seconds. The resend did not regenerate wallets, repeat setup, or restart services.
The run directory records all 3,080 jobs as passed after the resend.

## Results

| Measure | Observation |
| --- | ---: |
| Setup receipts | 11,400/11,400, all status `0x1`; 5,560 L1 and 5,840 L2 |
| Predicted addresses with code | 11,320/11,320 |
| Source receipts | 3,480/3,480, all status `0x1`, unique hashes |
| Jobs represented by source receipts | 3,080/3,080; 2,880 jobs with one source and 200 with three |
| Source chain split | 2,080 L1 and 1,400 L2 |
| Source blocks | L1 23,189,493–23,189,658; L2 409,710–410,345 |
| Final pending / send failures | 0 / 0 |
| Planning / setup mining | 246.893 / 608.673 seconds |
| Pacing / final receipt monitor | 774.877 / 10.026 seconds |

After completion, `eez_getCrossChainQueueStatus` reported zero queued, in-flight,
blocked and unknown requests. L2 safe height was 411,085, beyond the last source
L2 block 410,345. That block resolved through `eez_getSettlementByL2Block` with
`canonicalL2=true`, `l2Finalized=true`, and an L1 transaction in block
23,189,608. This establishes canonical settlement for the final L2 source block,
not independent verification of every scenario's expected state.
Docker inspection found the same node and signer image IDs still running since
2026-09-22 21:54 UTC and 21:53 UTC respectively, with no test-time restart.

The onefold run completed 870 source receipts in 411.426 seconds, including a
220.724-second source monitor. The fourfold run needed an interrupted burst and
a paced resend; its 774.877-second send interval cannot be compared with the
onefold 1.506-second burst as an isolated throughput regression. The failure
does show that the current ingress path did not retain a 3,480-transaction
burst. The paced recovery completed, but these results do not establish the
maximum sustainable send rate.

## Posting outcomes and skipped-slot gap

The local monitoring index covers this interval with canonical L1 headers.
For target times from 08:29:34 to 09:07:23 UTC, its `posting_diagnostics` view
contains 213 distinct targets: 162 included at target, 19 network slots
skipped, 21 blocks produced without inclusion, seven preparation failures,
three deferrals before dispatch, and one inclusion at another target. During
the source-transaction phase alone (08:45:07–09:07:23), the counts are 145
included, 15 skipped, eight produced without inclusion, and one preparation
failure across 169 targets. These are target-outcome fractions, not fractions
of every Chiado block. The onefold run's 35/40 included figure used conclusive
dispatched attempts as its denominator and is not directly comparable to
145/169.

The 19 skipped outcomes are backed by consecutive canonical parent/child
headers bracketing absent target timestamps. In 16 cases the next canonical L1
head arrived five seconds after the skipped target, while the next Composer
posting target was ten seconds after it. In two cases the next head was five
seconds later but the next posting target was fifteen seconds later; in one
case the next head and target were ten and fifteen seconds later, respectively.
The regular extra posting gap therefore remains. The running
`eez-driver/src/slot.rs` scheduler arms a target only after seeing its preceding
L1 head: after a missing head it cannot arm the immediate following slot. The
two longer gaps despite a head five seconds later require more event-level
evidence before attributing their additional delay.

Further local SQL grouping by distinct target timestamp on 2026-09-23 found
that, during the source-transaction phase (08:45:07–09:07:23 UTC), all eight
"Block produced without inclusion" targets were followed by another posting
target five seconds later, while all 15 "Network slot skipped" targets were
followed ten seconds later. Across the full setup-plus-source interval,
produced-without-inclusion had eight five-second and 13 ten-second gaps. Eleven
of those 13 had a canonical block in the immediately following slot, so they
cannot all be attributed to another skipped slot. Those 13 occurred during
setup/historical posting, often retrying the same L2 height with no recorded
trigger reasons. That path selects the earliest target with its measured
preparation budget and may deliberately choose a later slot; the event-level
cause of each case has not been verified. The source-phase comparison supports
the narrower claim
that canonical exclusion currently recovers in the next slot for that workload.

The local Lighthouse Beacon API returned a block for a confirmed skipped
slot's predecessor, HTTP 404 for the missing slot, and a block for its
successor. Its HTTP 404 means only "block not found" at the time of the query;
before a successor is known it cannot distinguish a missed proposal from a
delayed observation. At the 1,000 ms proof budget the following slot's proof
starts 2.8 seconds after the suspect slot, before the following block normally
exists. Beacon observations can guide speculation but do not supply canonical
evidence soon enough to reuse the conclusive exclusion path without keeping
the first candidate alive.
The deployed source includes commit `8794a2b`'s canonical skipped-slot
admission exception and millisecond trigger precision. That exception prevents
an absent network slot from unnecessarily shrinking the next batch; it does
not create a fallback trigger for the immediately following slot. The dashboard
changes likewise only report the gap. No next-slot scheduling fix was deployed.

The first launch's setup failure was separate from these posting outcomes:
`orchestrator-lib.sh` attempted a wave of 31 asynchronous MultiSend funding
transactions per chain with `MULTISEND_BATCH=100` and consecutive faucet
nonces. The configured chain RPCs returned `txpool is full` at L1 workers
1600–1699 and L2 workers 2100–2199,
before any setup deployments. Increasing the chunk size to 220 reduced that
wave to 14 transactions per chain; the retry funded all workers and completed
setup. This points to ingress txpool capacity under the funding burst, not a
revert or insufficient balance. The exact pool limit/configuration was not
measured.

Grafana's `Preparation failed` label describes a different event. Seven such
posting attempts occurred in the full interval: five `prover_deadline` during
setup and two `preparation_deadline` (one during setup, one during source
monitoring). They are local missed proof/preparation windows, not canonical
network-slot skips or failed source receipts. Node logs show the two preparation
gates were 61 ms and 152 ms past the proof-start cutoff. For the five proof
deadlines, Composer used its configured 500 ms per-attempt budget and exhausted
the target's retry window. Across setup it recorded 40 individual
`DeadlineExceeded` retries, five exhausted retry episodes, and 42 successful
attestations. The source-transaction phase recorded 168 successful attestations
and no proof retries. The signer logged no WARN/ERROR event in the run window.
The evidence localizes the missed opportunities to Composer's tight proof
window under setup load; it does not identify whether signer execution,
transport, or host contention consumed each 500 ms attempt. The configured
proof and preparation budgets should be tested against measured request
latencies before changing them.

This workload disabled semantic assertions. Receipts, code checks, queue
drainage, and canonical advancement demonstrate execution and liveness, but do
not establish scenario-specific postconditions or independent follower replay.
No comparable fourfold fork-baseline run was made.

## Evidence and checks

The run directory, wallet pool, profile, and logs are local-only and ignored.
The logs may contain private network endpoints; do not publish them. Local
evidence is retained at `artifacts/chiado-staged-4x-20260923/` and
`artifacts/fix-mixed-outbound-source/eez-core-protocol/tmp/e2e-staged-net/20260923-102934/`.

| Evidence | SHA-256 |
| --- | --- |
| `run.log` (funding failure) | `95eb67b6dde943d69704cf66f9329d9f354308c1c4f69962b8a88e21263ffdf2` |
| `retry.log` (setup and dropped burst) | `74901e52dc383a8811974b4c8c31e942e431b7ece8a3e92673ec1513207be60a` |
| `resend.log` | `62edd8fe2cab7b39bd0a18f4bb97642363bd92324914c162eefc2445a80a5eda` |
| `timings.csv` | `84241e596336ba1cec8c4474dfee85ca5037eeeb3933283983cb85d453df68bd` |
| `mined.csv` | `88158193ab7657b18bdadc5e1116661d82820a20c76ef2932df699e8b6e4c913` |
| `deploy-mined.csv` | `2e335bd19c56bc4c264d5d614cd366a3fbe72e693be72d754cc1b61673183161` |

`bash -n scripts/run-chiado-staged-performance.sh` and `git diff --check`
passed. The shell syntax check is static; the successful live run exercised
scale, full-run locking, and resend. No semantic verifier or independent replay
was run for these 3,080 instances.
