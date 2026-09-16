# Large staged Chiado workload

The requested workload reached canonical settlement for **870 source
transactions from 770 jobs**, using **47 posts**. The largest Sync block held
34 sources. Preparation estimation remained the most frequent admission
constraint; gas and blob capacity were generally available. This changes the
priority suggested by the smaller [packing experiment](packing-improvement.md):
measure a release build and preparation performance before treating the gas
projection as the primary bottleneck.

At the user's request, semantic verification stopped at 17:12:00 UTC:
**541 jobs passed, none had failed, two were interrupted and 227 had not
started**. This is not a complete 770-job semantic-verification pass. The
performance measurement and independent canonical placement checks cover all
870 sources. Compact results and evidence hashes are in
[large-staged-wave.json](large-staged-wave.json).

## Reproduction and unchanged runtime

The command ran from the clean protocol checkout at
`3c0cdf4955941a5c23639346932b55b33babebbd`, starting at 16:44:35 UTC on
2026-09-15. Verification was enabled; no scenario assertion or production code
was changed for this measurement.

```sh
PREPARE_PARALLEL=200 bash script/e2e/run/network-staged.sh \
  counter:100 counterL2:100 nestedCounter:50 nestedCounterL2:50 \
  multi-call-nested:50 multi-call-nestedL2:50 \
  multi-call-twice:50 multi-call-twiceL2:50 counter-multi-tx:50 \
  reentrant:20 revertCounter:50 revertCounterL2:50 \
  revertFromOtherChain:50 revertFromOtherChainL2:50
```

The existing Chiado profile supplied local ingress/RPC endpoints, plan-mode
preparation, 28 send workers, two verification workers and one-second receipt
polling. `PREPARE_PARALLEL=200` controls preparation concurrency, not the number
of send workers. Dedicated test funds supported `FUND_ETH=0.005` and
`FLOOR_ETH=0.005`; these overrides change worker funding, not transaction gas,
fees or values. Inspection of previous identical scenario plans found a maximum
per-job, per-chain signed gas reservation of 0.003766 native tokens. The active
network poster did not fund the test wallets.

The deployed composer SHA-256 was
`f6e672458cc14f3c8bd2d01513baae71cc0a8cc6dd087642780fdba8ed12cb3c`.
The composer, signer and beacon containers were retained. All 193 recorded
Rust/Cargo source identities still match the validated build. Settings remained
500 ms preparation, 500 ms proof allowance, 700 ms submission slack, automatic
transaction admission, zero speculative depth, 300-block empty consolidation,
pure-L2 `always`, and a 500-gwei postBatch execution-fee ceiling. The active
Chiado allowance was two blobs, each carrying 126,976 usable encoded bytes.

Build provenance matters here: this deployed validation executable came from
plain `cargo build`, using **development profile `opt-level=1`**, debug
assertions and overflow checks. The repository's normal Docker build uses the
release profile. The numbers below describe this exact deployed executable;
they do not establish production release-build throughput. No build/profile
change was made during the test.

## Measured placement and timing

The 14 scenario types created 770 jobs. The 50 `counter-multi-tx` jobs each sent
three sources, giving 870 sources total: **520 inbound and 350 outbound**.
Setup sent 2,850 deployment transactions; their receipt-monitoring phase took
94.096 seconds. The upstream runner checked 2,830 predicted code addresses
before firing triggers. All 870 ingress acceptances occurred within 1.352
seconds. No unrelated cross-chain ingress occurred during the measured wave.

| Measurement | Result |
| --- | --- |
| Successful posts carrying test sources | 47, each with one blob |
| Maximum sources in one Sync block | 34 |
| L1 settlement blocks | 23,071,257–23,071,307 |
| L2 Sync endpoints | 153,465–153,725 |
| Acceptance → canonical safe, median | 141.500 s |
| Acceptance → canonical safe, p95 / maximum | 251.229 / 270.975 s |
| Dispatch → canonical safe, median / maximum | 1.784 / 2.302 s |
| Bundle gas / L1 block limit, median / maximum | 41.9% / 62.8% |
| Encoded bytes / two-blob allowance, median / maximum | 21.5% / 44.6% |

Timing starts at composer ingress acceptance and ends at locally observed
canonical safe advancement. Safe settlement is distinct from consensus
finalization. It excludes deployment setup and subsequent scenario-verification
time. Most latency accumulated before dispatch as the queue drained. Inbound
and outbound medians differ, but the scenario order, unequal direction counts
and different execution costs prevent attributing that difference to a single
scheduling rule.

The first candidate overlapped remaining deployment history: 435 pure-L2
deployment transactions were still unsettled, and the combined estimated range
exceeded two blobs. A historical predecessor had to settle before the rich
candidate. The first source therefore reached safe after about 12.4 seconds.
This setup overlap and the much larger wave prevent a direct latency-regression
comparison with the earlier 30-source experiment.

Sources per successful post, in canonical order:

```text
16,21,20,24,24,21,21,23,11,21,21,22,18,18,18,15,15,14,10,12,10,17,10,16,
10,16,9,17,14,26,15,23,12,22,17,13,27,31,30,30,17,34,34,29,23,1,2
```

The 47 posts used 333,034,923 gas including their inbound companions and
2,588,791 encoded bytes. Some carried remaining deployment or empty history;
these totals are not a pure cross-chain payload or equal-payload fee comparison.
Utilization measures available capacity, not proof that an arbitrary larger
candidate meets the remaining execution, dependency and timing constraints.

## What limited packing

There were **36 preparation-time cuts** and **three gas-projection cuts**.
No L2 block-gas cut occurred. Prepared rich candidates took a median 368 ms
(maximum 408 ms); their one-blob preparation took a median 178 ms. Available
preparation time was 460–495 ms. Finishing reserves and estimated
next-candidate growth can stop admission before the wall-clock allowance is
exhausted. The unused time cannot simply be spent without preserving deadline
and final ordered-bundle checks.

The gas projection is also conservative. These comparisons concern the same
accepted 34-source candidates, not the rejected next transaction:

| Target L1 block | Projected poster gas | Measured poster gas | Outcome |
| --- | ---: | ---: | --- |
| 23,071,299 | 13,350,016 | 3,910,342 in native preflight | Target slot skipped; no receipt spend |
| 23,071,301 | 13,363,536 | 6,816,995 in receipt | Included |
| 23,071,302 | 13,322,920 | 4,805,193 in receipt | Included |

The planning allowance was 13,381,013 under the per-transaction gas maximum,
configured 20% margin and fixed planning reserve. Actual complete bundle gas
for the two included posts was 11,093,197 and 9,830,194. Companion consumption
must still be included when refining estimates; lowering poster projections
alone does not establish that the complete next candidate fits.

Four source-bearing attempts did not settle at their first target: two were
excluded and two targeted genuinely skipped Chiado slots. Canonical headers
show 16:50:30 → 16:50:40 and 16:51:05 → 16:51:15 timestamp jumps. Existing
backoff reduced subsequent selections from 26 to 13 and 34 to 17 after those
skipped slots. The final exclusion reduced three remaining sources to a
one-source selection, followed by two sources in the next successful post.
Setup exclusions and later idle-period missed slots are excluded from these
four wave attempts.

For the 50 three-source jobs, three settled in one Sync, 25 spanned two and
22 spanned three. Their source nonces and canonical receipts were consistent.
This motivates reviewing same-sender scheduling, while retaining dependency
execution and fairness to older work. It does not justify assuming every
three-source group always fits together.

## Correctness and evidence

Independent analysis checks unique source identities, successful canonical
receipts, transaction indexes, sender/nonce agreement, ownership generations,
L1/L2 settlement correlations, every settled L2 hash, safe advancement and an
empty cross-chain queue. Inbound sources execute after their matching post in
the same L1 block. The analysis handles arrays of settlements per L1 block and
orders posts by block and transaction index. No source was counted twice.

Terminal dispositions were 820 `Committed` and 50 `SourceCompleted`. Reverting
scenario outcomes are checked by the upstream verifier; a successful outer
source receipt does not imply that every nested operation succeeded.

Two setup attempts exceeded the full-range blob allowance and their bounded
prefix proofs missed the attempt cutoff. Subsequent progress settled the
deployment history and all requested sources. These retries are retained in
the logs; the report does not describe the run as error-free.

The live ownership RPC prunes terminal telemetry at finality. The independent
collector retained all 870 dispositions before that pruning, then canonical
receipt/hash checks validated them independently. Raw wrapper evidence and the
joined analysis input remain separate. The requested cancellation produced
upstream exit `-15`; stopping the surrounding collector produced exit `130`
after its final-state cleanup. These are recorded as user cancellation, not a
successful full script run or a network failure. An identical signed historical post
was redispatched after exclusion; the analyzer matches its included target
and successful canonical reconciliation, rather than taking its first attempt.

The upstream verifier's wall time is separate from settlement latency. Split
`counter-multi-tx` jobs spend approximately 30 seconds retrying a first-block
match before the existing broader scan succeeds. This runner behavior was
observed and left unchanged.

Full local logs, canonical RPC evidence, runtime identities, scenario manifest,
per-source timing CSV and executable analysis scripts are retained in
`artifacts/chiado-large-staged-20260915/`. The protocol run directory is
`eez-core-protocol/tmp/e2e-staged-net/20260915-164435/`. Wallet keys and raw
wallet-pool files are excluded from the audit. Separate non-test bridges
arrived after the measured wave; they are excluded from the 870-source
statistics. The final health check initially overlapped two such requests;
both subsequently settled and the queue emptied again.

The final snapshot at 17:17:44 UTC confirmed advancing L1/L2 heads, an empty
queue, unchanged containers/settings/executable, and the preserved pre-test
safe hash and state root. Lighthouse was synchronized with 25 connected peers.
The protocol checkout remained clean; the source-identity comparison, analysis
script syntax checks, audit secret scan and `git diff --check` passed. No Rust
build or source change was needed for this measurement.

## Revised optimization priorities

1. Repeat a controlled workload with the normal release build at identical
   source, timing and policy settings. Measure any improvement; do not assume
   a speedup factor.
2. Profile preparation, including blob work and the finishing estimate, because
   time estimation stopped this workload most often. Use measured candidate
   work, byte size and available time, without recognizing scenario names,
   test addresses or fixed source counts.
3. Refine protocol gas projections with complete ordered-bundle measurements,
   retaining all execution and gas-headroom validation.
4. Distinguish a canonically proven skipped slot from a genuine bundle exclusion
   before deciding whether to reduce the next selection.
5. Review same-sender grouping and direction scheduling with controlled workloads
   that preserve nonce dependencies and fairness. Evaluate repeated arrivals,
   bursts, cheap and expensive execution, multiple blobs, and empty backlogs;
   a single large test should not define the scheduling policy.

The new evidence confirms room to improve utilization and queue-drain time.
It does not demonstrate that this full wave can fit one Sync, guarantee
sub-five-second latency under load, or quantify release-build capacity.
