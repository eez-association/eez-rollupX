# Bounded final-runtime latency comparison

Status: the paired experiment, semantic checks and independent cold replay
completed successfully. **Performance equivalence remains unproven:** observed
canonical-settlement p95 was **31.312 seconds on the candidate versus 27.641
seconds on the baseline**, a 3.67-second / 13.3% difference. This one shared-host
pair does not establish its cause or a statistical regression.

[Compact results and evidence digests](latency.json) retain both sides, exact
identities, actual arrivals, initial states and the interrupted first baseline.
No production code was changed by this validation.

## Identities and experiment

- Baseline: `a50c326d99f7cd1c21406eedbf370a344015a6b5`.
- Candidate: `b1881cf737cdc64ee30a9767807f53d21a3daa86`, CI `40195871216`;
  producer checkout `57d212d80b89079f4ed9123ed379e94790a59bf7`. Their trees both
  equal `2f6d561c097cec4b1e4b874dacd0ba5d643e7dd2`.
- Both pin protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f` and use release
  artifacts. The existing runtime verifier checked source, protocol, lockfile,
  archive and image identity. Seven retained executables were hashed; five
  installed node/signer/helper executables matched the verified archive on each
  live network. Replay used each revision's verified archived test and follower.
- Fresh owned enclaves: `eez-latency-base-r2-20261025` and
  `eez-latency-final-20261025`, run serially. Argument values differ only in the
  three node/signer/deployment image digests. Each uses its own revision's
  deployment/genesis/startup wiring. Accepted history, native transaction,
  fee-recipient and signer-RPC differences remain enabled.
- L1 slots are 12 seconds; L2 timestamps advance by 2 seconds. Both enable
  cross-chain fees, use a 24-block proof-window cap, a 60-second pure-L2
  settlement interval and a 150-block unsettled limit.
- A **300-second calendar at five ordinary offer slots per second** runs beside
  four existing mixed cross-chain waves: 1,500 scheduled slots and 32 sources.
  The sender is sequential. An expired slot is recorded, not sent or retried.
  Four waves retain nonce/settlement barriers, exact values and wrapper returns.
- Ordinary transmission starts at L1 slot phase +2 seconds after preparation
  RPCs. Both use sender SHA-256
  `b1334bf5d8609d13905649fcde495953e3d4f4ba3d2624da24694d6641252660`.
- There was no concurrent owned build, process test, load, or artifact extraction
  through ordinary receipt drain. Later wave controls and evidence collection
  were outside the latency window; their latency is not compared. Foreign host
  activity was not controlled. Resource guards required 12 GiB available before
  load and would stop owned producers below 5 GiB; neither guard fired.

The retained adapters use the [previous workload sources](../2026-10-24-extended-validation/kurtosis/workloads/README.md).
They shorten the offer calendar and wave count, align the start phase, preserve
all semantic assertions, and fix the timer closure described below. The
collector additionally requires the full duration, successful supervisor and
child exits, and the frozen sender digest before accepting evidence.

## Results

| Observation | Baseline | Candidate |
| --- | ---: | ---: |
| Scheduled offer slots | 1,500 | 1,500 |
| Actual ordinary sends, accepted and canonically settled | 1,115 | 1,111 |
| Expired slots, never sent | 385 | 389 |
| Retries / rejected ordinary sends / backpressure slots | 0 / 0 / 0 | 0 / 0 / 0 |
| Accepted ordinary transactions per scheduled second | 3.717 | 3.703 |
| Local inclusion p50 / p95, seconds | 2.000 / 7.409 | 2.001 / 7.403 |
| Canonical settlement p50 / p95, seconds | 12.917 / 27.641 | 13.478 / 31.312 |
| Canonical settlement mean / max, seconds | 13.931 / 42.418 | 14.734 / 42.312 |
| Ordinary RPC response p50 / p95, milliseconds | 111 / 498 | 115 / 498 |
| Canonical Composer proof round-trip p50 / p95, milliseconds | 35 / 47 | 22 / 38 |
| Head minus safe p95 / max, blocks | 14 / 18 | 14 / 18 |
| Same-height replacements observed during ordinary load | 0 | 0 |
| Cross-chain sources / independent wrapper-return checks | 32 / 8 | 32 / 8 |
| Complete canonical blob claims verified with KZG | 46 | 47 |
| Cold replay terminal L2 block | 570 | 582 |
| Cold replay tests passed / failed / ignored | 1 / 0 / 0 | 1 / 0 / 0 |

Local inclusion joins each final receipt's exact block hash to its commit event.
Settlement joins advancement events to authenticated canonical claims by post
hash, L1 transaction and L1 height. Every accepted ordinary receipt has both
observations; there are no missing log matches. Proof round trips cover 22
canonical windows per side. Candidate signer logs omit per-window duration and
request context for these 22 windows, so signer-only durations are unavailable;
they are not inferred from nearby events or per-block timings.

All receipts, nonces, ordinary gas prices, sender and recipient balances passed.
The collector checked every non-native receipt's base fee and tip over the
measured block range, including cross-chain activity. Baseline base fees burn;
candidate base fees accumulate in FeeVault. Beneficiary tip deltas matched
exactly on both. Every cross-chain source reached terminal `Committed`, bound
to its canonical L1 block and authenticated batch; all value, state and wrapper
returns passed. No block-hash commitment divergence was reported.

The baseline archive's older receipt parser requires its 32 native `0x76`
receipts to remain separately checked. The candidate archive includes all 32
native `0x45` cross-chain receipts in its explicit map. Both retain the original
complete batches and independently replay their full execution commitments.

## Timing, initial states and attribution limits

Times below are UTC.

| Boundary | Baseline | Candidate |
| --- | --- | --- |
| Network startup | 13:22:09–13:24:33 | 14:01:10–14:03:34 |
| Bootstrap gate passed | 13:31:46.570 | 14:10:43.573 |
| Gate latest / safe | 266 / 264 | 258 / 258 |
| Before-sender L2 head / queued sources | 346 / 0 | 340 / 1 |
| Ordinary start | 13:34:32.001 | 14:13:33.002 |
| Deviation from intended slot phase | 1 ms | 2 ms |
| RPC/observer closure interval | 301.004418 s | 301.003836 s |
| Ordinary canonical receipt drain completed | 13:39:46.472 | 14:18:44.445 |
| Both workload processes completed | 13:41:55.424 | 14:20:47.405 |

Both passed the same read-only gate: nonzero safe height, at most 12 blocks of
head/safe distance, recent L1 head and an empty queue. Dedicated account funding
preceded the gate and was excluded from measured traffic. The later first wave
can change queue state before the ordinary sender starts. Warm-up and initial
state are therefore **not identical**. Supervisor launch was near L1 block 50
on both; observed phases were +3.14 and +8.64 seconds respectively.

The calendar is 300 seconds. The final ordinary RPC completed about 5 ms beyond
that boundary; shutdown also awaited the one-second observer loop. Together
these explain the approximately 301-second metadata interval. Transaction
latencies use individual send and canonical-event timestamps, not that duration.

Observed first/last cross-chain ingress offsets from ordinary start were:

| Wave | Baseline, seconds | Candidate, seconds | Sources inside ordinary interval, each side |
| --- | ---: | ---: | ---: |
| 1 | −5.271 to 82.828 | −5.515 to 83.596 | 3 |
| 2 | 113.260 to 203.572 | 109.048 to 191.184 | 8 |
| 3 | 228.977 to 311.106 | 221.632 to 311.805 | 7 |
| 4 | 341.544 to 431.536 | 337.213 to 419.376 | 0 |

These are exact node ingress observations, not inferred client send times.
Nonce/settlement barriers make actual arrivals runtime-dependent.

The plan analysis covers **start through metadata end +60 seconds**, about
361 seconds. It observes 30 plans on each side, with five baseline and four
candidate `prepare=false` decisions. Every `prepare=true` decision has a matching
invocation. Within the ordinary interval itself, both have three false decisions:
baseline at approximately +100/+112/+220 seconds; candidate +100/+208/+220.
All recorded false decisions have `queue_hint=false` and empty `reasons`.

Receipts whose local-to-safe interval overlaps an applicable false decision
number 107/1,115 (9.60%) on the baseline and 144/1,111 (12.96%) on the candidate.
Their settlement p95 values are 40.420 and 40.908 seconds. Without such overlap,
p95 is 17.836 and 17.992 seconds. This describes an association with recorded
opportunities; it does not prove counterfactual inclusion or explain why arrivals
and decisions differed. Conditioning does not establish performance equivalence.

The [earlier 900-second comparison](../2026-10-24-extended-validation/stabilization.md#settlement-latency-decomposition)
observed 25.415/31.599-second p95 on different workloads/binaries. Those results
remain valid only for their original runs. Neither the later scheduling changes
nor this new pair retroactively establishes their cause. No further load was
added to obtain a more favorable percentile.

## Executed checks and retained evidence

The local evidence root is `artifacts/latency-validation-20261025/`.
Each side retains startup, actual deployed bytes, genesis, supervisor and child
exits, resource samples, all accepted receipts, canonical claims/sidecars, fee
accounting, exact ingress observations, node/signer logs and cold-follower data.
The versioned JSON contains compact results and evidence SHA-256 digests;
**bulk evidence and local adapters are local-only**, unavailable to a new
checkout. Older network volumes and first failures were not deleted.

Executed for each side, from its appropriate checkout/configuration:

- `bash testing/kurtosis/start.sh <args.yaml>` with the explicit owned enclave
  and all image builds disabled; deployment/genesis checks passed.
- `bash testing/kurtosis/scripts/verify-eezl2-deployment.sh` — passed against
  the actual installed proxy, implementation, owner and system sender.
- `flock -n eez-core-protocol/tmp/.eez-network-e2e.lock python3 <tools>/run-mixed.py`
  — runs the frozen ordinary sender and existing `cross-chain-wave.sh`; both
  final processes passed with nonzero intended selections.
- `python3 <tools>/collect-mixed.py` — canonical full claims, KZG, receipt,
  value/return, correlation and complete measured fee accounting passed.
- Exact archived `node_e2e` executable with
  `kurtosis_policy_replay:: --ignored --test-threads=1` — one selected test
  passed on each side, in 7.43 and 7.16 seconds; no rebuild or binary fallback.
- Existing `summarize-load.py` plus the local latency decomposition — all
  accepted transactions accounted for. The analyzer first reproduced both
  previously frozen campaigns' recorded counts and percentiles.

This bounded pair does not repeat the previous high-offer-rate/pipelined load,
fault/restart campaign, or complete CI matrix. It does not establish capacity,
an SLA, durable queue recovery, or equivalent tail latency. The higher observed
candidate settlement tail remains the performance limitation to carry forward.

## First failures, correction and resource ownership

The first baseline, `eez-latency-base-20261025`, remains stopped with all its
containers and volumes. Its read-only deployment download first hit the host
`/tmp` quota; a task-local temporary mount resolved capture without changing the
runtime. Its workload scheduled 1,500 slots and accepted 1,160 transactions;
all canonical receipt, nonce, gas-price and exact balance checks passed.

The final duration assertion nevertheless failed at 299.999338936 seconds,
0.661064 ms early. The supervisor preserved that failure and stopped its wave
child after 24 of 32 sources. This is an incomplete harness run, not a successful
paired baseline or an observed runtime failure. The sender now loops until the
actual monotonic deadline; the duration assertion and every semantic assertion
remain intact. Both final sides use the same corrected bytes. A fresh baseline
avoided giving only one side the failed run's 1,160 transactions and three waves
as additional warm-up. Original scripts, diff, logs and successful partial
accounting remain alongside the failed supervisor record.

Candidate preparation also retained two local errors before measured traffic:
an incorrect metadata-label lookup, and a missing preparation directory. The
latter failed while saving funding intent, before `eth_sendRawTransaction`;
creating the directory did not retry or duplicate a broadcast. Its one funded
account and the complete final experiment passed.

The successful baseline was stopped at 13:46:11 UTC with every original
container and volume retained. After its measurement, collector and replay
completed, the candidate was handed over for the separately recorded fee-enabled
MissingProxy controls. Its measured configuration and evidence were frozen
before that handoff; later controls are excluded from this comparison.

After those controls, the candidate was stopped at **14:33:29.546 UTC**, again
preserving every original container and volume. The interrupted baseline, final
baseline and final candidate are all stopped; no latency-owned producer or
replay process remains active.
