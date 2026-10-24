# Extended Kurtosis validation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-24 UTC; MissingProxy admission negatives, baseline and final campaigns complete; all three owned enclaves stopped without deletion |
| Scope | Negative admission, sustained mixed traffic, canonical settlement, fees, recovery and independent replay |
| Checkout at resumption | `b0195b0bc1ca90d71792fcfbc8613e8b00b00473`, branch `codex/l1-history-offline-proving` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Earlier runtime | CI `40104687331`, source `319ebc2dbfdace3d5e708a2c9881cc741529aec4`; earlier campaigns remain attributed to those bytes |
| Final runtime | `28049ff734ab931cbde01246c5d238cce6ef65bf`, CI `40123208325`; verified final images; fresh comparison load, fees, three controls and cold replay passed |
| Preserved enclave | `eez-native-v1-review-20261024`, UUID `78c4b5eee3a844b99f4864b33c0d0643`; node, builder and signer stopped cleanly |
| Fresh final comparison | `eez-final-ab-20261024`, UUID `ca2e0c2232ce4c17a42704d8002f92bb`; fresh genesis/contracts/data, identical baseline arguments except the three verified application images |
| Earlier evidence | [PR readiness network validation](../../2026-10-24-pr-readiness/kurtosis/README.md) |

## MissingProxy: the rejection occurs before queue ownership

Both retained upstream fixtures were sent once through their normal fee-enabled
front. Both reached the executor's exact missing static-callback proxy check.
The front rejected them with RPC `-32003`, `settlement tariff admission failed`.
There were no source receipts, no queue ownership and no changes to either
sender's nonce or balance. Subsequent observations repeated those checks after
13 more L1 blocks and 81 more L2 blocks, respectively; both callback proxies
still had no deployed code.

This establishes the fee-enabled admission contract. It does **not** pass the
pinned staged runner's different oracle, which requires an accepted source to
become terminal with `SimulationFailed`. `ingress.rs` runs signed fee simulation
before `gate_and_hold`, so these sources never enter that ownership lifecycle.
The existing wave runbook already distinguishes early fee-admission rejection
from later poison eviction on profiles with collection disabled. No fees,
validation rules, gas fields or signed trigger bytes were changed for this check.

The generic `quote_unavailable` RPC code alone is insufficient evidence. The
portable [negative result](missing-proxy.json) binds each exact executor error
to the sole request's time interval, callback proxy and chain, and retains the
transaction hash, response and unchanged-state checks. The public debug trace
also shows the expected call path. The compact quote adapter currently projects
`EstimateError::Unavailable` to a generic message; it does not expose the
specific missing-proxy cause in its RPC result.

The earlier staged attempt remains a failure: its quote phase aborted before
either trigger was sent. A new read-only capture initially failed because
`cast decode-transaction --json` returned a JSON-encoded string; the parser was
corrected before any new quote or submission. Neither failure is overwritten.

## Sustained campaign contract

The final operational window opened at 19:54:53 UTC, after the exact archived
pending-RPC regression and local checks passed. Its
[final runtime and archived tests](final-runtime-identity.json) have been verified
from CI `40123208325`: the producer checkout is synthetic merge `c35cf5bc`, whose
tree equals `28049ff7`. Source, protocol, lockfile, thirty test files, all seven
runtime binaries and the test/runtime archive linkage match. Seven binary copies
extracted from the three images match the archive; all nine genesis/profile
outputs match the previously verified network. The extraction containers were
never started and have been removed. This establishes artifact identity; complete CI lane outcomes are recorded separately.
All seven binary hashes differ from the earlier installed
`319ebc2d` package, so old live results remain separately identified.

The baseline
fixture setup began at 17:49:49 UTC on the independently verified `a50c326d`
runtime. Its measured interval began with wave 1 at 17:51:29.418 UTC and lasted
900.962 seconds. The exact [generators and limits](workloads/README.md) are
preserved for both revisions. The shared design is:

1. Record source, protocol, seven executable hashes, actual deployed role bytes,
   genesis, configuration, chain heads, resource availability and empty queues.
2. Offer 15,000 ordinary L2 transfers in three 300-second stages at 5, 15 and
   30 transactions per second from a
   dedicated account. Persist signed intents before submission and never retry
   an ambiguous send. Gas is 21,000, maximum fee 100 million wei and priority fee
   10 million wei: the maximum gas budget is 0.0315 test ETH. Fund this account
   with 0.05 test ETH, separately from setup and cross-chain actors.
3. In parallel, use the existing `cross-chain-wave.sh` in `mixed` mode for twelve
   waves, eight operations each, with a 20-second gap and its normal nonce and
   settlement barriers. This covers calls with and without returns, wrapped
   calls, deposits and withdrawals in both directions. The two fresh actors
   receive one test ETH each. Preserve all quote and submission failures.
4. Record L2 headers every second, queue and safe-head observations every five
   seconds, and owned container CPU/memory plus host memory every ten seconds.
   Require at least 12 GiB available before starting; below 5 GiB stop only this
   campaign's producers. Cap ordinary pending transactions at 128 and record any
   withheld offers as incomplete load, rather than reducing the expected count.
5. Allow up to ten minutes to drain. Check every accepted ordinary receipt,
   exact nonce progression and balance costs, cross-chain destination state and
   values, canonical L1 completion, blob commitments and signer attestations.
   Run an independent follower only after the load phase to avoid resource
   contention. Preserve its actual receipt/header comparison scope.
6. After the mixed workload, run a separate 180-second control offering 30 ordinary
   transactions per second, with at most 16 concurrent RPC sends and 128 pending
   transactions. It uses the same funded actor and persists each reserved nonce
   before its sole submission. The maximum additional gas reserve is 0.01134 test
   ETH; even full admission in both phases fits the original 0.05 ETH funding.
   Preserve submission ambiguities and abort rather than filling gaps with retries.
7. Run a separate recovery phase with a 72-second interruption of only the owned
   signer, bounded ordinary traffic and unconditional restoration. Measure L2
   continuity independently of settlement, then require canonical catch-up.
   Keep this phase outside any workload comparison interval.
8. On the final candidate, repeat the two retained L1 controls after the comparable
   load and signer recovery: pause the embedded-L1 Beacon observer, then separately
   pause the validator client to produce missed slots. Each uses 72 seconds of
   observation after its original settling interval, 24 ordinary transfers and
   unconditional restoration. Check the different L1 conditions, repeated history
   and PREVRANDAO, execution progress, exact account costs and canonical recovery.
   Record actual head ages and commit gaps without a new wall-clock SLA. The
   [prepared fault plan](l1-fault-plan.json) binds the services, budgets, assertions
   and final replay scope; their completed results are recorded below.

A new baseline enclave was created using verified artifacts for
`a50c326d99f7cd1c21406eedbf370a344015a6b5`, protocol `84a5b520…`. Its name is `eez-main-ab-20261024`, UUID
`ad654d451d2148d095678a22b0c9942b`. Both published payloads and all seven
image/archive binary copies were verified; no Rust or Docker build ran. The existing enclave consumes approximately 8 GiB, including 2.75 GiB
for its node and 3.09 GiB for its builder. Host memory available at preflight was
15.8 GiB and swap was full. Concurrent networks would leave little margin, so
the networks are serialized. The candidate node `a56d567d…546d1` and builder
`449d5944…f8a877` stopped cleanly with exit 0; eight complete, hash-checked
archives retain their data and configuration. The candidate safe/latest heads
were 12456/12462. Its canonical L1 and signer stayed active. This interval is
maintenance and is excluded from liveness measurements.
Different chain histories and intentional protocol changes prevent treating
equivalent offered workloads as a clean performance benchmark or expecting
identical block hashes.

## Final runtime maintenance and fresh comparison

The final binaries were copied into the old enclave's retained containers and
verified by copying them back. An initial supervisor assertion confused a stopped
container's empty Docker `IPAddress` with its retained static `IPAMConfig`;
this failed before the node started. A continuation checked the static address,
network identity and ownership, then started only that node. The signer had
already passed its actual Capabilities check. Genesis and the prior safe hash
remained unchanged.

After more than two hours of deliberate node and builder downtime, L2 caught up
to wall time and embedded L1 caught up to canonical L1. Settlement initially
stalled while the builder's execution client and its continuously running beacon
client resynchronized. The retained diagnostics distinguish accepted relay
submissions, excluded bundles and actual canonical progress; a fresh network does
not turn those exclusions into successes. The final observation before clean
shutdown was L2 latest 17004, safe 12504. Two further bounded parts had become
canonical, but full maintenance catch-up was not completed. This maintenance
interval is excluded from liveness and load measurements.

The old node, builder and signer were stopped cleanly, with exit 0 and no OOM.
Eight complete archives preserve node data, embedded L1, all bundle captures,
genesis/bindings and builder data/configuration. Their compressed bytes and full
readback were verified. The original services and volumes remain in place. The
[compact preservation record](final-maintenance.json) retains the identities,
archive digests and limits; bulk evidence remains local-only. Its beacon clients,
validators and canonical L1 stayed active during that earlier maintenance; all remaining services were stopped only at final closure.

To compare fresh networks without spending another long interval draining
maintenance-only backlog, `eez-final-ab-20261024` starts from the final verified
images and fresh state. Its YAML values equal the reference network's except the
three application image digests. Startup began at 20:10:25 UTC with 18.82 GiB
available and no builds. The earlier network remains preserved for independent
recovery diagnosis. The [independent builder analysis](maintenance-builder-analysis.json)
records 26 exclusions followed by reconciliations through 12480 and 12504. A
third post through 12528 was included but had not reconciled before shutdown.
Its Engine became online 432.346 seconds after EL restart and its beacon client
reported synchronization at 484.479 seconds. The exact cause of this reconnect
delay is not established; no production defect is demonstrated by this trace.

Fresh startup finished at 20:13:15 UTC. The [network identity](candidate-network.json)
records five actual role-byte comparisons, the successful live deployment check,
and the same stable-parent debug/latest/pending plus eth_call control used on
both earlier revisions. All RPC results passed with the final runtime.

The [first campaign attempts](candidate-first-attempts.json) remain failures.
The first wrapper lacked the explicit endpoints required by local deployment
mode and stopped before sending anything. The first actual wave then encountered
`quote_limit` while startup backlog still exceeded the posting window; the
existing builder barrier had only established one inclusion. Its parallel sender
accepted two ordinary transfers before the supervisor stopped it. Both were
subsequently checked for canonical receipts and exact account costs. No
cross-chain source was submitted. The explicit continuation waits for canonical
nonce2, an empty queue and safe lag at most12, then runs the same generators,
limits and assertions. This changes the preparation precondition, not admission
or the first result.

## Completed baseline results

The [compact result](baseline-load.json) records the exact runtime, seven binary
hashes, archived test, configuration, workloads, measurements and limits.
No build ran during these measurements. All accepted ordinary transfers passed
canonical receipt, nonce, gas-price and exact sender/recipient balance checks.

| Phase | Scheduled offers | Submitted and accepted | Expired schedule slots | Withheld at pending cap | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| Sequential 5/s for 300s | 1,500 | 1,191 | 309 | 0 | Canonical settlement and accounting passed |
| Sequential 15/s for 300s | 4,500 | 1,284 | 3,216 | 0 | Canonical settlement and accounting passed |
| Sequential 30/s for 300s | 9,000 | 1,327 | 7,673 | 0 | Canonical settlement and accounting passed |
| Separate pipeline 30/s for 180s | 5,400 | 4,415 | 0 | 985 | Canonical settlement and accounting passed |
| Separate signer recovery | 24 | 24 | 0 | 0 | Canonical settlement and accounting passed |

The sequential sender waited for each RPC response; its median response time was
117 ms and p95 was 495 ms. It therefore did not issue most scheduled offers.
The separate pipeline allowed sixteen concurrent RPCs and reached 24.53 accepted
transfers/second over its planned 180 seconds, with 985 offers withheld by the
128-pending cap. No RPC rejected a submitted transfer and neither sender retried.
These are observed rates with explicit generator limits, not a capacity claim.

All twelve cross-chain waves passed: 96 source operations, 48 in each direction,
covering returned calls, calls without returns, wrapped calls, deposits and
withdrawals. The existing destination state/value oracle passed. Additional
checks bound every source to terminal `Committed` ownership, the canonical L1
post and native payload, and checked all 24 wrapper return events exactly.
The 27 excluded bundle attempts in the node log all occurred during bootstrap,
17:39–17:44 UTC, before the first measured wave. There were none during the
measured campaign or the later controls. They remain in the record.

Across L2 blocks 416–1291, receipt gas totals, parent hashes and two-second header
timestamps matched. The beneficiary received exactly 12,608,373,615,199,563 wei
in non-native tips. The baseline burned 1,880,079,368,795 wei in base fees; its
FeeVault address had no code and no balance change. This is the baseline's
intentional contract. The candidate must credit its FeeVault instead. The 8,241
ordinary transfers cost exactly 1,732,423,779,786,000 wei in total.

Submission-to-canonical-safe latency was p50/p95 13.79/25.41 seconds in the mixed
interval and 26.08/53.02 seconds in the pipeline. These use hash-bound canonical
advancement events. A separate [read-only inclusion analysis](baseline-inclusion.json)
matched all 8,241 final receipt block hashes to their first local commit events:
p50/p95 2.07/7.59 seconds in the mixed interval and 1.40/6.94 seconds in the
pipeline. This is local execution observation, not L1 settlement. Canonically
included signer windows took p50/p95 20/33 ms
and 57.5/60 ms, respectively. Ten-second Docker samples observed node CPU maxima
of 49.05% and 9.63% of one logical CPU, and node memory up to 2.30 and 2.366 GiB.
Sampling misses short peaks; signer samples rounded to zero do not mean no work.
The minimum sampled host availability during the measured intervals was
14.34 GiB, above the safety threshold. No maximum-capacity or improvement claim
is inferred from these shared-host observations.

During the separate 72-second signer pause, L1 advanced six blocks and L2
advanced 36. Twenty-two of the 24 transfers executed before restoration; all 24
settled afterwards. The maximum observed head age was 2.003 seconds and the
largest observed commit gap was 6.026 seconds. Future header timestamps cover
intentional intervals between commits; a two-second timestamp interval is not
a four-second wall-clock SLA. The signer was always restored and retained its
process identity. This test concerns signer unavailability, not missing L1 data.

The evidence collector authenticated 124 full canonical blob claims with KZG
and native payload decoding. The [unchanged claim index](baseline-batches.json)
covers every height 1–1308 exactly once. A fresh follower using the baseline's
verified archived binary then passed the sole selected replay test in 15.02
seconds: 1,308 derived blocks and 8,289 selected receipts, ending at
`0xe6a02fa7caa605f0cdaab0b8a4f3fe8b07aa9a28d7b68a56bdb4fe0a9cfcb6a2`.
The old harness cannot decode native `0x76` receipts, so those 96 receipts and
their canonical post links were checked separately. The follower executed their
transactions to reach the endpoint; the harness did not directly compare those
96 receipts. Its selected-receipt assertions compare status and canonical block
inclusion, not all receipt logs or gas fields. The separate accounting and wave
oracles provide those additional checks within their stated scope.

One operational anomaly remains explicit: the pipeline's outer shell reported
exit 143 after the child had exited 0 and the supervisor had recorded complete
semantic assertions. Its cause is unknown. The original result is preserved
and is not represented as a clean outer-shell exit.

After replay, only the baseline node and builder stopped cleanly with exit 0.
Eight complete archives, checked by hash, gzip CRC and full tar readback, retain
their data, embedded L1, bundle captures and configuration. The final recorded
safe/latest heads were 1596/1607. Canonical L1 and the signer stayed active.
Two read-only shutdown preflights failed before any lifecycle command: an
incorrect queue method name and an unavailable `stat` executable in the builder
image. The supported queue method and Docker archive API resolved them without
changing validation or container contents. The shutdown is maintenance after
measurement, not a continuity result.

## Completed final candidate and comparison

[The final result](candidate-load.json) binds the immutable final images, seven
runtime hashes and archived test to CI `40123208325`, source `28049ff7` and the
unchanged protocol pin. No build or topology change ran during either measured
load phase. The mixed campaign completed all twelve waves: 96 sources, 48 in
each direction, correct destination states and transferred values, 24 exact
wrapped return events, canonical ownership and signer attestation. All 27 unique
[discarded bundle attempts](candidate-dropped-bundles.json) occurred before the
measured interval; none occurred during it or the wave completion. This is a
timestamp classification, not a claim about the cause of PBS exclusion.

| Final phase | Scheduled | Accepted | Expired schedule | Withheld at cap |
| --- | ---: | ---: | ---: | ---: |
| Sequential, 900.955 s | 15,000 | 3,890 | 11,110 | 0 |
| Pipeline, 180.952 s | 5,400 | 4,561 | 0 | 839 |
| Three separate recovery controls | 72 | 72 | 0 | 0 |

No submitted transaction was rejected or retried in those completed phases.
All 8,523 accepted transfers passed canonical receipts, gas, nonce and exact
sender/recipient accounting. The two transfers accepted by the aborted
preparatory attempt are checked separately and enter replay, giving 8,525 total
ordinary transfers. They do not enter the measured denominators or latencies.
The maximum authorized gas reserve, including preparation, is 0.0429954 test ETH
against 0.05 test ETH funding; actual total actor gas was
1,808,922,848,964,000 wei. The original preparation failures remain failures.

| Observed metric | Baseline `a50c326d` | Final `28049ff7` |
| --- | ---: | ---: |
| Sequential accepted transfers | 3,802 | 3,890 |
| Pipeline accepted transfers | 4,415 | 4,561 |
| Sequential local inclusion p50 / p95 | 2.073 / 7.587 s | 2.236 / 7.445 s |
| Pipeline local inclusion p50 / p95 | 1.400 / 6.938 s | 1.425 / 6.891 s |
| Sequential canonical settlement p50 / p95 | 13.790 / 25.415 s | 14.092 / 31.599 s |
| Pipeline canonical settlement p50 / p95 | 26.078 / 53.019 s | 25.798 / 52.264 s |

The sequential settlement p95 was **slower**, 31.60 s versus 25.41 s. One campaign
on a shared host and independently bootstrapped histories does not identify its
cause, establish performance parity or rule out a timing regression. These
measurements do not establish maximum capacity or an SLA. The
[local inclusion analysis](candidate-inclusion.json) uses exact committed block
hashes and remains separate from canonical L1 settlement timing.

The final fee oracle checked all 96 retained quotes against their actual signed
transactions, gas limits, effective prices and receipts. The 48 inbound posts
paid the measured 90% net refund correctly, including payout gas and exact
recipient balance changes. No measured post had a shortfall in this campaign;
subsidized batching still permits shortfalls generally. Across L2 blocks
300–1307, every non-native receipt accounted exactly for
18,705,932,080,051 wei credited to FeeVault and 13,712,676,299,263,350 wei of
beneficiary tips. The baseline intentionally burned base fees. Quotes are
advisory: this checks their applied fields and actual charges, not equality of
estimated gas with actual usage or a proof of the whole quote algorithm. The
maximum-tip source inferred from receipts does not authenticate submitted
`refundTxHashes` or distinguish tied or rounded payer identities.

The [three controlled interruptions](candidate-faults.json) all restored the
same service process and recovered exact canonical accounting:

- Signer paused for 72 seconds: L2 advanced 36 blocks and executed 23 transfers
  during the pause; all 24 were later canonical.
- L1 observation paused: external L1 advanced six blocks while embedded L1
  stayed fixed; L2 advanced 36 blocks and executed all 24 transfers. Six native
  imports repeated the available origin and PREVRANDAO. All captured block
  hashes survived recovery unchanged.
- L1 validator paused: the measured 72 seconds had no L1 block after the
  24-second settling period; L2 advanced 35 blocks and executed all 24 transfers.
  Six imports repeated the origin/PREVRANDAO, then history and settlement caught
  up. All captured block hashes survived unchanged.

[Timing samples](candidate-fault-timing.json) retain actual header lead, head age
and the largest observed commit gaps: 5.021 s for signer interruption and about
8.05 s for each L1 control. They provide the exact timestamp/hash sequences;
there is no artificial two- or four-second acceptance SLA or unproven attribution
to host scheduling. Live signer configuration contains no L1/L2 RPC source;
this campaign does not assert a container firewall. Separate source processes
verify outbound-connect denial.

The [124 unchanged complete claims](candidate-batches.json) cover every block
1–1332 once, with adjacent pre/post hashes and at most 24 blocks per request.
The cold follower passed **1/1 in 13.77 s**, using the test and runtime from the
same final producer, and explicitly compared **8,669 receipts**: 8,523 completed
workload/control transfers, two preparatory transfers, 96 native `0x45` receipts
and 48 ordinary outbound sources. The terminal hash is
`0x417657f031008c4a0ec6e4a01271f8b5945887302029343ffe195176b192f236`.
The harness directly compares status and canonical inclusion; the terminal hash
forces complete execution, while separate oracles check logs, fees and values.
The baseline archive excluded native `0x76` from its older receipt parser, so the
receipt selections differ explicitly.

One optional metrics reader initially failed with `KeyError: span`: final
`window_signed` events contain canonical hashes but no request span or window
duration. The preserved read-only continuation attributes windows by their exact
pre/post hashes and reports signer duration/context as unavailable. Composer
roundtrip timings remain measured; no missing timing is inferred from proximity.

Finally, [all three owned enclaves were stopped](enclave-closure.json), preserving
all original containers and volumes: 41/34 for baseline and 42/35 for each
candidate enclave. Fresh datadirs remain in their stopped original containers
and volumes; no separate full fresh-network database archive was created.
Earlier snapshots, fixture keys and logs remain retained. No enclave, volume,
datadir or foreign service was deleted or reset. Available host memory after
closure was 23.33 GiB. This shutdown occurred after every oracle and the replay,
so it is outside the load and liveness measurements.

## Evidence and current limits

Raw records are local-only beneath `artifacts/extended-validation/kurtosis/`:
`preflight.json`, `owned-resource-inventory.json` and `missing-proxy/`. Their
sanitized result and digests are in [missing-proxy.json](missing-proxy.json).
The prior CI runtime identity and the original image/installed-byte distinction
remain as recorded in the earlier audit.

Baseline bulk logs, receipts, data and inputs remain local-only under
`artifacts/extended-validation/baseline/`; their digests and compact results are
committed here, but another checkout cannot retrieve the bulk data from Git.
The archived runtime/test payloads remain retrievable by the recorded public
image digests. Final candidate bulk evidence is retained separately under
`artifacts/extended-validation/candidate-fresh/`, with hashes in
[candidate-load.json](candidate-load.json). The negative admission checks above
remain attributed to `319ebc2d`; they do not become final-runtime live negatives.
All three owned enclaves are now stopped with their containers, data and volumes
preserved. The final candidate tests and replay have completed.

The first additional pending-debug observation reproduced a public RPC gap:
`debug_traceCall` on `pending` failed at a stable pre-import parent while the
latest trace and `eth_call` pending passed. See [the original observation](pending-debug.json).

The baseline control passed all three calls on the first stable parent #338,
using the exact same transaction object and no broadcasts; see
[baseline pending-debug control](baseline-pending-debug.json). This establishes
the regression against `a50c326d`. The same stable-parent probe passed on the
final `28049ff7` network; [candidate-network.json](candidate-network.json) retains
that result, separate from the exact archived process regression.
