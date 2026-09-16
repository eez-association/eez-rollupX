# First bridge after idle: 0x6bd0ae1b

The [Chiado source transaction](https://gnosis-chiado.blockscout.com/tx/0x6bd0ae1b17c38303f87bc918bd4a31db8c320ea85d0f8b62211b636b6c08369a)
succeeded and delivered exactly **0.001234 xDAI** to
`0xF35960302a07022aBa880DFFaEC2Fdd64d5BF1c1` on L2. It took **81.899 seconds**
from composer ingress to safe L2 delivery, and **82.053 seconds** to source
ownership reconciliation. This was a read-only investigation; runtime settings
and code were not changed.

## Timeline

All times are UTC on 2026-09-15.

| Time | Event |
| --- | --- |
| 13:44:59.119749 | Composer accepted the bridge into its held queue. |
| 13:45:03.305 | First selection saw 1,300 unsettled L2 blocks and one queued request. |
| 13:45:10 | First historical post included, settling L2 141,216–141,545. |
| 13:45:20 | Second historical post excluded from an empty L1 block. |
| 13:45:30 | Retry included, settling L2 141,546–141,875. |
| 13:45:40 | Chiado skipped the targeted slot. Consecutive canonical block timestamps jump from 13:45:35 to 13:45:45. |
| 13:45:55 | Third historical post included, settling L2 141,876–142,205. |
| 13:46:05 | Fourth historical post excluded from another empty L1 block. |
| 13:46:15 | Retry included, settling L2 142,206–142,535. |
| 13:46:18.668980 | Bridge bundle dispatched. |
| 13:46:20 | Source included in L1 block 23,069,299. |
| 13:46:21.018843 | Bridge delivery became safe in L2 block 142,590. |
| 13:46:21.173125 | Canonical source ownership reconciled as committed. |

L1 times without fractions are block slot timestamps, not precise publication
times. The final L1 payload arrived at 13:46:20.321641 and became canonical at
13:46:20.846968. Receipt and runtime details are in
[bridge-6bd0ae1b.json](bridge-6bd0ae1b.json).

## Why it waited

The preceding settlement was at 13:23:25. The live policy allows **1,800 empty
L2 blocks**, or 30 minutes at the current cadence, before general consolidation;
the fullness trigger is off. At the first selection the accumulated history was
approximately **996 KB including the bridge candidate**. The current two-blob
allowance holds **253,952 encoded bytes** per post. The history therefore had to
be posted before the fresh bridge could settle.

Four successful historical posts each carried **330 L2 blocks in two blobs**.
The last post carried the bridge plus the remaining 55 blocks in one blob:
**five successful posts and nine blobs in total**. Each historical post used
the full two-blob count, so another such post could not share its L1 block.
The two excluded attempts targeted blocks with zero transactions and zero
blobs. Occupancy did not cause those exclusions; the available evidence does
not establish the builder/proposer decision. The source bridge itself was
included on its first actual submission.

Historical scheduling also left avoidable waiting to address. The ordinary
Sync trigger now fires 1.7 seconds before the next slot. Historical target
selection reserved **990 ms for measured preparation plus configured proof**,
then **700 ms submission slack**. By the time range sizing reached target
selection, that allowance no longer fit the upcoming slot. For example:

- Selection at 13:45:03.518 required readiness after 13:45:05.208.
- The 13:45:05 target was consequently unsafe under the retained budget.
- The selector chose 13:45:10, dynamically producing an offset of two blocks.

The same pattern recurred during the drain. Actual measured historical encoding,
KZG and witness preparation was 423–449 ms; the historical allowance retains a
previous maximum and also includes 500 ms for proof. The target selector obeyed
that allowance. Historical work needs to begin early enough to accommodate it,
including the work preceding target selection. Relevant paths are
`EnabledPolicy::preparation_budget` in `composer/policy.rs`,
`dispatch_minimal_postbatch` / `emit_historical_chunk` in `composer.rs`, and
`select_policy_target` in `eez-l1/src/submitter.rs`.

The local L1 payloads in this window arrived within 0.42 seconds of their slot
timestamps. There were no preparation-deadline failures in this transaction's
window. Its delay should not be attributed to the earlier peer-starvation issue
or to a bridge execution failure.

## Correctness and scope

The source receipt and synthetic L2 delivery receipt both succeeded. The pinned
L2 balance difference across block 142,590 is exactly 1,234,000,000,000,000 wei.
The source nonce advanced from 47 to 48 across its inclusion block, and its
balance debit equals the bridge amount plus 872,144 wei of execution fees.
There is exactly one registry consumption event for this source. The settlement
correlation points to the canonical post
`0x256523bffbe893c9fdae1e6339cff242d8675eac83fc21618048d2fe12764917`
and the verified L2 block hash. The queue was empty when inspected.

The running composer hash matches the previously tested candidate. This check
used canonical receipts, pinned balances/nonces, runtime events and settlement
correlation; it did not launch another independent follower. Detailed evidence
is retained under `artifacts/chiado-tx-6bd0ae1b-20260915/`.

The [previous 6.012-second median](bridge-latency-fix.md) was measured after a
completed historical warm-up. It does not establish fast delivery for the first
bridge after a long idle interval. Improving this case requires both earlier
historical scheduling and a policy/encoding strategy that prevents a new bridge
from inheriting several posts of history. The timing guard must remain intact;
moving a target earlier without sufficient preparation time would not be a fix.
