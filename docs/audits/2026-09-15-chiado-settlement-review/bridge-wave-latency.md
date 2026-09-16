# Bridge-wave latency with prior history cleared

On 2026-09-15, **24 real Chiado L1-to-L2 bridges** were submitted through the
public UI ingress, one every **2.1 seconds**, after a separate warm-up bridge
completed. All 24 succeeded, with the exact native amounts delivered in canonical
L2 safe state. Their observed end-to-end latency was **24.9 seconds median** and
**32.2 seconds p95**, including queueing. The queue drained completely.

| Measurement | Samples | Median | p95 | Minimum–maximum |
| --- | ---: | ---: | ---: | ---: |
| Broadcast to L1 receipt and L2 safe balance observed | 24 bridges | 24.865 s | 32.212 s | 8.108–33.410 s |
| Broadcast to composer canonical reconciliation | 24 bridges | 24.675 s | 32.433 s | 6.695–32.603 s |
| Broadcast to bundle dispatch, including waiting and preparation | 24 bridges | 20.510 s | 27.949 s | 1.335–28.819 s |
| Bundle dispatch to composer canonical reconciliation | 9 posts | 4.649 s | 5.360 s | 3.614–5.360 s |
| Public-ingress acknowledgment | 24 bridges | 7.2 ms | 15.0 ms | 4.7–168.2 ms |

Percentiles use nearest rank. Transactions sharing a post are correlated, so
these are observations from one short wave, not an independent 24-post sample
or a latency guarantee. The two end-to-end measurements use different endpoints;
do not subtract their percentiles to estimate an individual stage's duration.

## Removing the prior-history delay

The queue was initially empty, but **1,095 unsettled L2 blocks** remained from
the preceding idle period. The separate
[warm-up bridge](https://gnosis-chiado.blockscout.com/tx/0xa7abbfc17edfe798f75a47865dde7c40b7ffc3e48116d0d7bdb9eea7319e4420)
took **51.994 seconds**. It required four successful historical posts and its
own bridge post. One historical attempt targeted a skipped L1 slot; two scheduler
deadline warnings also occurred during warm-up. None of those warm-up latencies
enter the wave statistics.

The measured wave began at **11:34:06.004 UTC**, with only **five unsettled L2
blocks**. Its last request was sent at approximately 11:34:54.305, and every
receipt and safe destination balance had been observed by **11:35:09.216**.
The unsettled range peaked at **20 L2 blocks** during measurement. Every measured
post included user bridges; no separate historical-prefix post was needed.

## What still contributed latency

The automatic admission estimator initially selected one request per opportunity,
even while additional requests were queued. Successful post sizes subsequently
grew to **2, 3, 5, and 9 bridges**. The final post contained the one remaining
request. This confirms that multiple sequential nonces from the same wallet can
be included together. The earlier live impression that wallet ordering imposed
a one-transaction-per-post cap was incorrect.

The estimator used for this measurement in
[`AutomaticAdmission::limit`](../../../crates/eez-composer/src/composer/admission.rs)
scales the preceding complete preparation time quadratically with group count
and allows at most a doubling per attempt. One-group preparation took about
**182–200 ms**, against approximately **775–786 ms** available on ordinary
opportunities. Near this ratio, the integer square-root estimate remains one
until the measured budget can support a two-group probe. That conservative
ramp-up explains the initial small selections despite spare gas and blob space.
Later, nine bridges were prepared in **207 ms** and fit in one blob. The held plus
in-flight queue peaked at **15 requests**.

There was also an early scheduling interruption:

| UTC | Observation |
| --- | --- |
| 11:34:12.492 | Lighthouse imported the first wave post's fully available beacon block. |
| 11:34:12.699 | Composer reconciled it, then began preparing the next request with only 101 ms remaining before its proof-start cutoff. |
| 11:34:12.882 | Preparation had taken 182 ms; the attempt was retained for retry because the proof-start cutoff had passed. No bundle was dispatched for this attempt. |
| 11:34:15 | Chiado produced no canonical block for this slot, independently established by consecutive block timestamps jumping from 11:34:10 to 11:34:20. |
| 11:34:22.002–22.036 | The sequencer caught up; the historical 11:34:20 slot used `Empty` mode, and the upcoming 11:34:25 slot used `Catchup` mode. Neither admitted the queued bridges. |
| 11:34:27.348 | The next user bundle was dispatched for the 11:34:30 slot. |

The missed proof-start deadline and missed network slot overlap; they should
not be counted as two independent five-second penalties. The subsequent
`Catchup` opportunity also matters when explaining the full gap between posts.
All **nine dispatched wave bundles were included successfully**; there were
no excluded or reverted wave bundles.

Across the 11 canonical L1 blocks observed during the wave, execution payloads
arrived **303 ms median / 758 ms maximum** after their slot timestamps. Payload
arrival was followed by data-availability verification and canonical processing.
For example, the first post's fully available beacon import occurred 2.492 seconds
after its slot start. Payload arrival alone does not measure the time at which
the composer can safely use the new head.

## Canonical settlement evidence

The wave's nine one-blob posts covered **65 consecutive L2 blocks**,
134,651–134,715. Each row's native destination balance was independently checked
at the canonical L2 endpoint against the exact cumulative bridge amount.

| L1 block | User bridges | Canonical L2 endpoint | Post |
| ---: | ---: | ---: | --- |
| 23,067,869 | 1 | 134,660 | [fe0cc8…f76ca](https://gnosis-chiado.blockscout.com/tx/0xfe0cc8e82be7121675a5d3d86817024d0ca584ab6b13548757634f25c52f76ca) |
| 23,067,872 | 1 | 134,680 | [2e28d5…18bd9](https://gnosis-chiado.blockscout.com/tx/0x2e28d580c19bad86d6332e3ce1a3e0763298f786021bdcac6a1f079129f18bd9) |
| 23,067,873 | 1 | 134,685 | [5ab0a1…e5d59](https://gnosis-chiado.blockscout.com/tx/0x5ab0a11c3ceb7614f7f4ec1f90e089c4a6c6e615ec183c72838dcd33532e5d59) |
| 23,067,874 | 1 | 134,690 | [0d07a3…aeded](https://gnosis-chiado.blockscout.com/tx/0x0d07a39f2958957a08100a312155c37622b055eda49351e14a21b6d1f91aeded) |
| 23,067,875 | 2 | 134,695 | [f6fe12…dec35](https://gnosis-chiado.blockscout.com/tx/0xf6fe1216b41b5f6b0573c01dcf5b1ead8a89fd4d19f61e2541eeb0f2dddedc35) |
| 23,067,876 | 3 | 134,700 | [6798e5…67bf6](https://gnosis-chiado.blockscout.com/tx/0x6798e5118cdecd34b913f1c300ee7a6f36717968698e911c28ff8e0430267bf6) |
| 23,067,877 | 5 | 134,705 | [f322dc…04e82](https://gnosis-chiado.blockscout.com/tx/0xf322dc1f714a8d2a7b2f8518a3e9e904acf189cef5df1c45a525dc8806604e82) |
| 23,067,878 | 9 | 134,710 | [f94164…468a6](https://gnosis-chiado.blockscout.com/tx/0xf94164139dbe9382d873182fd0261f71d4dd786473e6df47252b773273d468a6) |
| 23,067,879 | 1 | 134,715 | [3556f6…3debc9](https://gnosis-chiado.blockscout.com/tx/0x3556f6d0407759c29fabfbf390128bd56b4eeac0f27a9861c7478620703debc9) |

The dedicated test account `0x24436d6c505865d057e88DB2940858b7201584AE` was
both sender and recipient. The 24 measured values were 1,000,000,000,000 wei
plus the zero-based request index. Verification established:

- All 24 canonical source receipts succeeded, each with exactly one registry
  `ExecutionConsumed` event and the expected bridge recipient, rollup, and value.
- The safe destination balance increased by exactly **24,000,000,000,276 wei**.
  The sender's L1 nonce advanced from **871 to 895**; its L2 nonce stayed **812**.
- Source balance debits equaled the exact receipt execution charges plus value.
- Every measured source hash appeared in its successful post's L1 block; each
  post's canonical L2 endpoint and balance were independently re-read.
- The held queue and in-flight ownership were empty after completion and on a
  later independent check. There were no manual resubmissions.

This test measures safe L2 delivery and canonical L1 inclusion, not L1 consensus
finality. It uses one sender, one direction, and native-value bridges. Polling
sleeps were 200 ms, with additional HTTP/RPC delay; the table separately includes
server reconciliation timestamps to expose observation overhead. Public ingress
was called from the deployment host, so acknowledgment timings do not describe
remote browser network latency.

## Reproduction and follow-up

Scripts, signed-request journals, per-request measurements, canonical receipts,
raw composer/beacon logs, executable identities, and SHA-256 evidence hashes
are retained under `artifacts/chiado-bridge-wave-latency-20260915/`.
`measure.cjs` refuses to overwrite an existing measurement or automatically
resubmit requests; it performed one warm-up and 24 measured submissions.
`summarize.py` independently verifies the receipts and per-post state, and
produces `summary.json`. Both scripts passed syntax checks and completed their
runtime assertions. No production code, runtime configuration, or container was
changed for this test.

The final health check at **11:42:55 UTC** found the queue still empty, both
sender nonces unchanged since verification, and **17 connected beacon peers**.
The beacon was fully validating, execution was online, and sync distance was
zero. Composer and beacon container identities and start times were unchanged.
This snapshot is retained in `final-health.json`.

The measurements isolate prior-history settlement but retain real queueing and
scheduling effects. Useful follow-up work is to distinguish fixed preparation
overhead from growth with group count in the admission estimate, and review
whether a future Sync opportunity can safely admit work after catch-up finishes.
Neither behavior was changed or bypassed during measurement.
Both were addressed in the subsequent
[admission and scheduling correction](bridge-latency-fix.md), which records the
candidate deployment, repeated workload, correctness checks and remaining
latency limits.
