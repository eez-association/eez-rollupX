# Staged protocol workloads: transaction flow and timing

The requested staged run completed on the existing Chiado deployment on
2026-09-15. All **30 source transactions from 28 scenarios** had successful
canonical receipts and terminal committed dispositions. The source nonces,
L1 receipt ordering, exact L1/L2 settlement correlations and all settled block
hashes were checked. The cross-chain queue was empty afterward, and the previous
safe state was preserved. The composer, signer and beacon were not restarted.

The user's requested scope is transaction inclusion, layer placement, packing
and processing time. Protocol semantic verification was deliberately skipped.
No claim that `--no-verify` establishes protocol correctness is made.

## Prepared command

From `eez-core-protocol/`:

```bash
bash script/e2e/run/network-staged.sh --no-verify all
```

The upstream parser requires flags before targets. Its original source was not
modified. The ignored local `chain.env` links to
`.runtime/chiado-settlement-20260913/protocol-staged/chain.env`. That profile
binds the local Chiado L1/L2 RPCs and ingress endpoints, registry, manager and
existing test funding key. It uses four prepare workers, 28 send workers, two
threads per Foundry process and one-second receipt polling. Worker funding is
0.02 native units, with a 0.01 refill threshold. The funding key is read from its
existing private file; it is not copied into the profile or this report.

The live policy remained **300 L2 blocks for general consolidation**, pure-L2
`always`, and automatic cross-chain admission. Protocol commit:
`3c0cdf4955941a5c23639346932b55b33babebbd`. Composer SHA-256:
`321025bcd368c0dfc8f11d24f79e677d5ea1f98db41eae1d2629deb0ce398840`.

## Measured run

The runner funded 28 wallets, prepared all 28 jobs, sent 122 deployment
transactions and checked code at 114 predicted contract addresses. It then sent
the 30 triggers: **17 inbound and 13 outbound**. All ingress acceptances occurred
within 40 ms, from 14:55:57.488661 to 14:55:57.528133 UTC.

| Measurement | Result |
| --- | --- |
| Total upstream run | 61.947 s |
| Deployment planning | 6.732 s |
| Deployment receipt monitoring | 4.595 s |
| Trigger sending phase | 0.085 s |
| Trigger receipt monitoring | 43.417 s |
| Acceptance → canonical L2 safe, minimum / median / p95 / maximum | 23.035 / 33.150 / 43.041 / 43.043 s |
| Acceptance → canonical reconciliation, median / p95 | 33.461 / 43.449 s |
| Bundle dispatch → canonical L2 safe, median / maximum | 1.889 / 1.983 s |

Latency comes from node ingress, dispatch, safe-advance and reconciliation
events linked to canonical receipts. The monitor duration includes its polling
granularity. L1 block slot timestamps are not used as transaction receipt times.

| L1 block | L2 sync block | Inbound / outbound source requests | Blobs | Meaningful stream bytes | Share of two-blob capacity | Bundle share of L1 block gas |
| --- | --- | --- | --- | --- | --- | --- |
| 23,070,037 | 146,790 | 0 / 3 | 1 | 20,003 | 7.88% | 7.08% |
| 23,070,038 | 146,795 | 2 / 1 | 1 | 18,094 | 7.13% | 12.15% |
| 23,070,039 | 146,800 | 1 / 9 | 1 | 26,070 | 10.27% | 22.14% |
| 23,070,040 | 146,805 | 10 / 0 | 1 | 33,334 | 13.13% | 29.82% |
| 23,070,041 | 146,810 | 4 / 0 | 1 | 13,288 | 5.23% | 8.72% |

These are **source-request counts**, distinct from the L2 transaction count:
cross-chain processing can create several L2 transactions per request. The five
posts were included in consecutive L1 blocks, with no intervening skipped
inclusion opportunities. Ten is the maximum observed in this wave; the run
does not establish the network's maximum capacity.

## What limited performance

The wave arrived while the runner's **62 pure L2 deployment transactions were
still awaiting settlement**. A setup-related attempt had encountered a skipped
slot. Another prepared attempt was deferred because its target parent or
submission window changed before final pricing. The deployments settled in
L1 block 23,070,036, slot 14:56:15, before any of the 30 cross-chain requests
could settle. Thus the observed end-to-end latency includes setup settlement
waiting; it is not an isolated steady-state bridge latency measurement.

Once cross-chain processing began, the admission estimator cut the first
selection from 30 requests to three and a later selection from 24 to ten. Both
events recorded `preparation_time` as the constraint. The following selections
were temporarily limited to the previously accepted count, as required by the
existing admission backoff logic. Measured preparation took **188–246 ms**,
including **158–190 ms** of blob preparation, against **494–496 ms** available.
The estimator reserved more time than these attempts ultimately consumed.

Neither blob bytes nor measured L1 gas saturated in this run. The useful next
investigation is to improve admission estimation, early bundle gas accounting,
and the handoff from pending pure-L2 settlement to queued cross-chain work,
while preserving deadline, FIFO, gas and indivisible-group checks. The unused
time and capacity are evidence of an optimization opportunity, not proof that
all 30 requests can safely fit in one preparation window.

## Why the entire wave did not enter one sync block

The two observed cuts were predictions of insufficient preparation time.
`Sample::tail_for` scales the non-blob portion of its last measured finishing
cost quadratically with the proposed group count. KZG cost scales separately
with blob count. `Attempt::can_start_group` and `can_finish_group` reserve this
predicted tail before accepting another complete group. A prediction can stop
selection even when the eventual measured preparation finishes well before its
deadline, as happened here.

`AutomaticAdmission::finish` then applies a count backoff after a constrained
attempt. The recorded selection/acceptance pairs were **30/3, 3/3, 24/10, 10/10,
4/4**. This explains both the small initial selection and the extra similarly
sized batch after each cut. The backoff also protects source ordering and
progress with outbound-first execution; it should not simply be removed.

A separate early gas bound would also prevent admitting this complete wave
under the existing rules, even if preparation were faster. All **17 inbound
user transactions declare 1,000,000 gas each**. `PostBatchGasBudget` sums those
limits, then adds its postBatch projection and margin. The companion limits
alone total **17,000,000 gas**, and the absolute minimum projection plus margin
adds another **1,151,000**, before pricing any of the actual cross-chain entries.
This already exceeds the approximately 17.0–17.1 million L1 block budget seen
during the run.

This is the composer's conservative admission bound, not a claim that all
declared transaction limits must add up below the consensus block gas limit.
The final ordered simulation already charges actual cumulative execution and
checks each next transaction's declared limit against remaining block gas; its
regression test explicitly covers a valid bundle whose summed declared limits
exceed the block limit. The early bound prevents this candidate reaching that
more accurate check.

For scale, the 17 inbound receipts actually consumed **2,672,889 gas**. The five
post receipts plus their inbound companions consumed **13,639,588 gas** in total.
Their five encoded streams totalled **110,789 bytes**. These are measured sums
of separate executions and encodings, not a simulation or encoding of a merged
candidate. They support investigating a single-sync batch; they do not prove
that it meets all execution, ordering, proof and deadline constraints.

The earlier per-batch gas-utilization percentages therefore describe physical
usage only. They do not show that the existing planner would accept all 30
together. [Additional accounting evidence](single-sync-analysis.json) records
the exact source gas limits, receipts and relevant implementation locations.

## Evidence and validation

[Machine-readable results](protocol-staged-wave.json) include every source
transaction, exact posts, per-layer placement, latency, capacity, admission cuts
and executable identities. Raw receipts and logs are retained locally in
`artifacts/chiado-protocol-staged-20260915/`. The original upstream run is
`eez-core-protocol/tmp/e2e-staged-net/20260915-145538/`.

Checks completed: offline Foundry build/cache warm-up, profile shell syntax,
the unchanged upstream `--no-verify all` run, and independent read-only receipt,
nonce, settlement-correlation and timing analysis. Existing Foundry lint
warnings remain; no protocol or rollup source code was changed for this run.
