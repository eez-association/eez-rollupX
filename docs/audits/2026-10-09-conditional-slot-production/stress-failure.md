# Candidate-A continuous ordinary load: failed before 15 minutes

2026-10-09 UTC. **Release blocker; not a completed or passing 15-minute test.**
This follows the smaller [live canary](live-canary.md), without changing images,
configuration or services. Candidate A remains active; this is not a test of B.

## Scope and identity

The user requested several transactions per block for 15 minutes and settlement
verification. Selected 5 ordinary zero-value self-transfers/second, up to 4500
transactions, with sync boundaries excluded by the current design. No cross-chain
semantic workload or independent follower replay was requested/run in this phase.

Exact runtime: node digest
`sha256:0eeb551755a060d7cd845c0cd4dbe871eb33dec1c55eba924d0e58e21da9a85c`,
source `c3061cb1883eee253890d8310e5393deeb7b4335` (tree-identical to A head
`9ca75baed975d4fd9aef560ea359ac6a800dd3af`), protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. Matching signer is unchanged from
the prior record. P=2, delay=0; Chiado 10200 / L2 906969. Reth pool dependency
is v2.7.0 `3d592ece6de8c4559987416a544fc215fd6d6921`.

Used the same existing funded workload account, distinct from the poster, while
holding both known checkout wallet locks. Sender initial nonce 51; no funding,
deployment, service restart, policy change or replacement submission. Each
transaction has gas 21000, max fee 100000000 wei, priority fee 10000000 wei;
aggregate maximum exposure 0.00945 native tokens. RPC admission initially accepted
the rate, and all observed base fees were far below the cap. A 48-outstanding
guard stops further sending when progress stalls. Signed intents are retained
before submission; ambiguous submissions are not retried.

The local helper exists because the existing staged contract workload does not
provide fixed-duration ordinary arrivals or receipt-to-slot occupancy accounting.
It uses existing Cast/RPC tools and does not change application code.

## Results

Start: 20:02:27.094 UTC. First transaction still unmined at the final check was
submitted 128.213 seconds later. The guard stopped further admission at 689
accepted transactions. At 20:06:35.919 UTC (248.824 seconds), SIGINT stopped only
the owned load process after the persistent nonce gap was confirmed. The helper's
`finally` block preserved its records; original metadata `loadError=null` reflects
KeyboardInterrupt not being caught as Exception, **not** successful completion.
The final settlement summary explicitly records `failed-stopped-early`.

| Check | Result |
| --- | --- |
| Accepted source transactions | 689 |
| Successful canonical receipts | 641, contiguous nonces 51–691 |
| Confirmed canonical settlement coverage | All 641, across two successful L1 settlements |
| Unmined | 48, nonces 692–739 |
| Pool at failure | 0 pending/executable, 44 queued |
| Latest and pending sender nonce | Both 692 |
| Receipt-derived gas cost | 134610659589000 wei; exactly matches sender balance reduction |
| Independent configured L1 receipt/block agreement | Passed for both settlements |
| Full Beacon blobs | Both versioned-hash-selected blobs retrieved |
| Resubmissions / automatic recovery | None |
| Service restarts during test | None; node/signer remain running, L1 synchronized |

Settlement transactions:

- `0x4816f332e4acf7040ce8026554193cf5f0e5209f52f4931c84ffe571373fb249`: first 625 test transactions.
- `0xe15bc0a25f410a202b6f241fc1751371eb7a6e184c19ad84e4b514b370c9f32c`: remaining 16 successfully included test transactions.

These settlement results do not establish loss-free queue/recovery behavior:
the same run leaves 48 accepted transactions unmined. No 15-minute occupancy,
capacity or performance-parity claim is supported. Other live ingress activity
appears in the shared node logs, so this is not an isolated throughput benchmark.

## Recovery-path evidence and working hypothesis

At 20:04:36.003, node logs recorded ordinary block **182696** with **20 transactions**
and hash `0x16d6ddb81d745e88cb4138bdbaf7637cf2b3be389e82ce0a215f3f8936a2cf1d`.
At 20:04:36.069–.077, settlement reconciliation replaced it with an empty sibling
`0x866db1c646e4a90471cbf030459de50e930c71e68d3b1c2de34a34ebaf3c092b`.
At 20:04:37.007, canonical block **182697** included **16** test transactions,
nonces **676–691**. The pool reinsertion metric reports 20 attempts; it does not
count successful insertions. Nonce 692 onward then stopped progressing.

The pinned Reth `crates/transaction-pool/src/maintain.rs:430` explicitly loses
local/external origin information during reorg recovery, calls
`add_external_transactions` and discards returned outcomes. Its default sender
capacity is 16 (`config.rs:12`); `pool/txpool.rs:1992` applies that capacity to
non-local insertions while locally admitted transactions are normally exempt.
This strongly fits the 20-to-16 transition and missing four transactions blocking
44 later queued transactions. **A controlled reproduction of the exact rejection
outcomes is still required before claiming the root cause confirmed.**

The replaced block lived for approximately 73 ms, below the helper's one-second
block sampling frequency. Its body was no longer available by hash at inspection.
Do not claim that the sampler or recovered body independently identifies all 20
transactions; the canonical 16 nonces and the log counts are verified separately.

## Evidence and next steps

Local-only private evidence:
`data/slot-production-canary-20261009.rTwtCl9Z/stress-15m-a/`.
Signed intents, accepted hashes, sampled blocks, health, receipts, metrics and
scoped logs remain retained. Do not publish the surrounding operator directory.

| Evidence | SHA-256 |
| --- | --- |
| `metadata.json` | `2ac35802145647212838023a721b4fb5ea86054ca9273141250c9d3bf796cb5e` |
| `events.jsonl` | `640534d1cb887fd164b9b488f6ac385f45d580d155d2f24b7acc98f4df535ca4` |
| `node-failure.log` | `3d62f3ba054de017cb23eb51db073ca10bba140752894511ce2e608e43d9ad26` |
| `final-settlement-summary.json` | `8312f8ada30187759344e1926298755b54a95259680343feaa53ab8b4c2724eb` |
| `final-receipts.json` | `eb5b565d8e6090b9c385ba8befcd4688efb0232b392d4bc6a3ce7e6515d1425a` |

Next: reproduce recovery of more than 16 transactions from one sender, including
concurrent arrivals; preserve ordinary ingress safeguards while restoring ownership
of all accepted transactions. Then deliberately reconcile/recover the retained
test requests and repeat the full 15-minute test. No application fix or recovery
resubmission was performed during this investigation. Keep PR #79 draft and do
not infer that candidate B fixes this shared recovery path.

Follow-up: the [bug review](bug-review.md) reproduced the sender-cap rejection
with the pinned pool, and the [repair record](bug-fix.md) records the failing
20-transaction process regression and subsequent candidate validation. The live
failure above remains unchanged; no recovery or successful 15-minute rerun is
implied by the local repair.
