# Live bridge latency after the discovery correction

A single **0.000001 xDAI** bridge from Chiado L1 to L2 completed in
**25.284 seconds**, measured from the start of broadcast to the destination
balance being visible at the L2 `safe` head. The test used the deployed UI bridge
and the public `https://eez.asuscomm.com/composer/l1` ingress. The sender and
recipient were the dedicated test account
`0x24436d6c505865d057e88DB2940858b7201584AE`, separate from the poster.

Source transaction:
[`0x95e4af…bd9e2a`](https://gnosis-chiado.blockscout.com/tx/0x95e4af65462bd3b98eb76e40d13060e8fbd8c2f35940c7de627ced25adbd9e2a).
All times are UTC on 2026-09-15.

| Event | Time | Elapsed from broadcast |
| --- | --- | --- |
| Broadcast started | 11:14:31.702 | 0 |
| Composer admitted request, from server log | 11:14:31.707386 | 5.4 ms |
| Public ingress acknowledged hash | 11:14:31.709 | 6.8 ms |
| First bridge bundle dispatched | 11:14:47.753227 | 16.051 s |
| That bundle excluded from target block | 11:14:50.423656 | 18.722 s |
| Retry dispatched | 11:14:52.416074 | 20.714 s |
| Successful inclusion block's slot timestamp | 11:14:55 | 23.298 s |
| L1 receipt first observed locally | 11:14:56.984 | 25.283 s |
| Destination balance verified at L2 safe head | 11:14:56.986 | 25.284 s |
| Composer recorded canonical completion | 11:14:57.005369 | 25.303 s |

The balance was initially visible in provisional L2 state at 11:14:47.700,
15.998 seconds after broadcast. That observation was not counted as settled
delivery because the first bridge bundle was subsequently excluded.

## Why this took 25 seconds

The held cross-chain queue started empty, but there were **581 unsettled L2
blocks** in the preflight snapshot and **585** when the next Sync slot evaluated
the request. The bridge triggered immediate settlement, which first had to
publish the older history within Chiado's two-blob capacity.

| Submission | Target L1 block | Result | Covered L2 range |
| --- | --- | --- | --- |
| Historical prefix, 2 blobs | 23,067,655, 11:14:35 | Included | 132,901–133,230 |
| Remaining history, 2 blobs | 23,067,657, 11:14:45 | Included | 133,231–133,490 |
| Bridge bundle | 23,067,658, 11:14:50 | Excluded | No canonical advancement |
| Bridge retry, 1 blob | 23,067,659, 11:14:55 | Included | 133,491–133,505 |

The captured eight L1 blocks, 23,067,653–23,067,660, reached the embedded execution
client **261 ms median / 403 ms maximum** after their slot timestamps. Lighthouse
reported seven peers during the test. Slow arrival of the block payload was not
the main delay in this sample.

For the three included settlement blocks, the beacon still had to collect all
64 custody columns before importing a fully available head:

| L1 block | Execution payload received | 64/64 columns available | Canonical L1 head committed |
| --- | --- | --- | --- |
| 23,067,655 | 11:14:35.283192 | 11:14:37.356 | 11:14:37.361195 |
| 23,067,657 | 11:14:45.265738 | 11:14:46.940 | 11:14:46.958701 |
| 23,067,659 | 11:14:55.273303 | 11:14:56.750 | 11:14:56.756027 |

After the first historical post became canonical, its batch notification reached
the composer at 11:14:37.755922. Reconciliation and L2 catch-up ran, and the
scheduler reached the Sync deadline check at 11:14:37.861272. That was about
**61 ms later** than the configured 11:14:37.800 preparation cutoff for the
11:14:40 target. One opportunity was skipped. No allowance was shortened or
data-availability validation bypassed for this measurement.

The first actual bridge bundle was excluded from the 11:14:50 target, adding
one five-second retry cycle. Its target L1 block was empty. The canonical receipt
and captured logs establish exclusion, but do not establish why that proposer
did not include the builder's bundle.

## Correctness and measurement scope

- The source receipt succeeded in canonical L1 block 23,067,659 and contains
  exactly one registry `ExecutionConsumed` event.
- The destination's balance at safe L2 block **133,505** increased by exactly
  **1,000,000,000,000 wei**. The sender's L1 nonce advanced by one; its L2 nonce
  remained unchanged. The block hash was independently re-read.
- The bridge bundle's post succeeded, and the excluded candidate has no
  canonical receipt. Three posts were included across the full operation.
- The cross-chain queue was empty after completion and on a later recheck.
- Exactly one user transaction was signed and broadcast. Retries were handled
  by the composer; the measurement script never resubmitted it.

This measures a single bridge with accumulated history, not a steady-state
latency percentile or L1 consensus finality. Receipt and balance polling used a
200 ms interval plus RPC duration. The public ingress was called from the same
host as the deployment, so its 6.8 ms acknowledgment is not a remote browser
round-trip benchmark. Application events use the host's clock; block timestamps
denote slot starts rather than exact publication times.

Reproducible measurement, canonical receipt checks, raw runtime log windows,
and file hashes are under
`artifacts/chiado-bridge-latency-20260915/`. The primary files are `measure.cjs`,
`measurement.json`, `summarize.py`, and `summary.json`. No production code or
runtime configuration was changed for the test.
