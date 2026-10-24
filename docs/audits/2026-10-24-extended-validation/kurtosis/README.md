# Extended Kurtosis validation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-24 UTC; MissingProxy admission negatives and baseline load/recovery/replay complete; final candidate campaign pending |
| Scope | Negative admission, sustained mixed traffic, canonical settlement, fees, recovery and independent replay |
| Checkout at resumption | `b0195b0bc1ca90d71792fcfbc8613e8b00b00473`, branch `codex/l1-history-offline-proving` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Existing runtime | CI `40104687331`, source `319ebc2dbfdace3d5e708a2c9881cc741529aec4`; the dependency update being prepared is not deployed |
| Owned enclave | `eez-native-v1-review-20261024`, UUID `78c4b5eee3a844b99f4864b33c0d0643` |
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

The candidate campaign waits for verified final runtime artifacts. The baseline
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
advancement events. Canonically included signer windows took p50/p95 20/33 ms
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
image digests. The final candidate campaign and its replay are still pending.
The negative checks above validate the `319ebc2d` runtime, not the upcoming
dependency/RPC update. Both owned nodes/builders are currently stopped with data
preserved while the final candidate is prepared.

The first additional pending-debug observation reproduced a public RPC gap:
`debug_traceCall` on `pending` failed at a stable pre-import parent while the
latest trace and `eth_call` pending passed. See [the original observation](pending-debug.json).

The baseline control passed all three calls on the first stable parent #338,
using the exact same transaction object and no broadcasts; see
[baseline pending-debug control](baseline-pending-debug.json). This establishes
the regression against `a50c326d`, while the candidate correction remains pending.
