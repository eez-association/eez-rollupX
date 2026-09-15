# Oversized Sync interval: cause and recovery

The data-wave stall was caused by admitting more encoded data between valid
settlement boundaries than Chiado could accept. It was separate from the
historical-post targeting delay. The test sent three 100164-byte calls in
successive L2 blocks, incorrectly assuming each block could be posted separately.

## Exact reproduction from the preserved database

The last settled block was 39500. The matching runtime's native codecs were
run against the preserved canonical blocks, including their original headers
and signed transactions. The diagnostic also packed the resulting stream into
physical blobs and checked that the measured count agreed.

| L2 endpoint | Contents of this block | Encoded bytes since 39500 | Blobs needed | Valid historical terminal? |
| --- | --- | ---: | ---: | --- |
| 39501 | First large call | 101150 | 1 | No |
| 39502 | Second large call | 202276 | 2 | No |
| 39503 | Third large call | 303402 | 3 | No |
| 39504 | Empty | 304167 | 3 | No: outside the Sync grid |
| 39505 | Empty Sync | 304933 | 3 | Yes, but over capacity |
| 39510 | Next empty Sync, after the fourth call | 409124 | 4 | Yes, but over capacity |

Two blobs carry at most **253952 stream bytes** with the current codec
(126976 per blob). The first valid terminal exceeded that allowance by 50981
bytes. Every later prefix includes the same oversized interval, so splitting
later or retrying cannot repair it. An empty intermediate block such as 39504
does not qualify merely because it is empty.

Each call used 1022970 execution gas and fit its L2 block's 30000000 gas limit.
Execution gas and encoded data therefore impose different constraints. Passing
the ordinary EVM gas checks does not prove that the interval fits in blobs.
The L1 post does not execute all of those ordinary L2 transactions again;
raising its execution-gas allowance would not increase the network blob budget.
See exact size evidence (`artifacts/audit-evidence/2026-09-14-settlement-targeting/stalled-prefix-measurement.json`, local evidence).

The relevant code paths are:

- `crates/eez-node-common/src/pool.rs`: the existing Ethereum validator checks
  individual transactions, wrapped by the system-address gate. It has no
  cumulative data budget for the interval between valid Sync terminals.
- `crates/eez-node-common/src/payload.rs`: the native payload builder limits
  execution gas, without budgeting settlement bytes across that interval.
- `crates/eez-composer/src/composer.rs::empty_chunk_boundaries`: historical
  anchor-only batches require an empty, scheduler-aligned terminal. A terminal
  containing ordinary transactions cannot be attested by the existing
  anchor-only proof path.
- `ActivityIndex::historical_stream_bytes` and `emit_historical_chunk` price
  complete canonical prefixes. No valid prefix fits, so they retain the range
  and report `unpostable_range` instead of producing an invalid post.

This is the unsplittable-range condition described in proposal section 6.2.
That proposal permits retaining and reporting it; it does not implement an
admission guarantee that every accepted workload remains postable. The timing
fix changes neither those boundaries nor those checks.

## Recovery and evidence

After explicit approval, the node was stopped at 09:44:53 UTC. A fresh stopped
database snapshot captured all 2545 unsafe signed transactions through block
41705, authenticated against the stored block transaction roots. There were
no transactions in the shutdown pool backup. Both the full backup and the
original database are retained privately.

Only a copy was unwound to the L1-validated safe endpoint at 39500. Before
promotion, its block hash, state root, head and reconciliation checkpoint were
checked. The verified copy was promoted at 09:44:56 UTC, and the node resumed
at 09:44:59 UTC. L1 data and the original settled history were not rewound.

The four original signed probes subsequently settled at L2 blocks 41961,
41966, 41976 and 41981. Recovery waited for each probe to become safe before
submitting the next, preserving their signatures and transaction hashes.

The initial recovery drain authenticated seven posts covering every L2 block
39501–41780, **2280 contiguous blocks in 14 blobs**:

```text
L1: 23051176 → 23051178 → 23051179 → 23051181 → 23051183 → 23051185 → 23051186
gap:          2           1           2           2           2           1
```

Every receipt succeeded, every post used both available blobs, KZG verification
passed, all decoded L2 block hashes matched the canonical node, and the final
endpoint matched the registry at the corresponding historical L1 block.
This proves consecutive historical posts are possible with the timing fix.
It does not promise inclusion in every consecutive block: actual preparation,
head arrival, skipped slots and builder acceptance still matter. See
authenticated drain (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-post-verification.json`, local evidence).

Those live checks authenticate data and canonical hashes; they are not an
independent execution replay. The native integration suite covers independent
follower execution separately.

## Replay complications and operational lesson

Public admission was not quiesced during recovery. Live traffic resumed and
some senders used rewound nonces for different transactions before all old
signatures could be replayed. Both signatures cannot execute at the same
sender nonce. Recovery records those conflicts, preserves the old signed data,
and does not replace current transactions or claim the conflicted originals
were restored. Remaining compatible transactions are replayed in original
block order and their receipts are checked against the snapshot.

Reth can also report `txpool is full` when an account has exhausted its pool
slots. The same RPC error covers global eviction and per-sender capacity;
the message alone does not prove global saturation. The replay uses bounded
retries of the identical transaction, without increasing fees or changing
signatures. Completed receipts are retained across replay-script restarts.

Future recovery procedures should pause transaction admission and coordinate
transaction producers before rewinding unsafe nonces, keep admission paused
while replaying, and capture pending ownership explicitly. Preserving the
database is necessary but does not itself prevent nonce conflicts after a
rewind. The replay inventory and final outcome must be read alongside the
network-recovery result.

### Completed replay outcome

All 2545 captured originals are accounted for: **2488 were replayed and settled;
57 could not be replayed because their sender nonces had been consumed by
different live transactions**. Every conflict belongs to
`0xb392eed5f2cdb7b352507f539c07276ae069b365`. Their original signatures remain
in the private export and database backup. The recovery does not claim that
those 57 originals executed on the replacement history.

The conflicted nonce-56 transaction had funded a later proxy-deployment account
with 499999394944296 wei. That missing transfer caused replay to stop at the
account's first contract transaction with insufficient funds. Recovery restored
the exact amount from the existing validation wallet in a separate, recorded
transaction:
`0xcbb9a1980e80b1eef989caf7909c69e1942d13ba531afca882c9b8c2fc28d4af`.
This is a compensating gas-funding transfer from a different sender, and must
not be represented as the original funding signature being replayed. See
funding evidence (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-gas-funding.json`, local evidence).

The account queue limit was temporarily increased to 4096 to accommodate the
replay alongside higher-nonce traffic, then the exact original Compose
configuration was restored. Settlement mode, thresholds, proof budget,
submission slack, builder lead and network blob limits were unchanged.
For the remaining ordinary transfers, adjacent EOA-only original blocks could
be submitted together after checking recipient/sender code and sufficient
balances for all outgoing debits without relying on incoming transfers.
Their original submission order was preserved. The raw-byte budget was one
eighth of the resolved two-blob stream allowance, with settlement required
between groups. Contract interactions retained separate original groups.

After the final restart, every restored canonical receipt was checked again.
All 2488 matched their original success status, per-transaction gas usage and
ordered log addresses, topics and data. Two provisional log mismatches were
caused only by JSON object key order; the comparison was corrected and all
receipts were revalidated. No semantic mismatch was excluded. Provisional
evidence is retained in comparison correction (`artifacts/audit-evidence/2026-09-14-settlement-targeting/receipt-log-comparison-correction.json`, local evidence).

See final recovery outcome (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-outcome.json`, local evidence) and
restored configuration (`artifacts/audit-evidence/2026-09-14-settlement-targeting/replay-pool-restore.json`, local evidence). Matching replayed receipts
does not imply that the complete new ledger equals the old unsafe ledger:
the conflicting transactions, compensating funding and independently arriving
traffic make that claim incorrect.

The complete L1 data audit through the replay checkpoint also passed:
**193 posts, 200 blobs, and 4310 contiguous L2 blocks (39501–43810)**.
It verified KZG, every covered canonical L2 block hash, receipt attribution,
the final historical registry endpoint, and the presence of all 2488 restored
transaction hashes in the authenticated payloads. No conflicted original hash
appeared in those payloads, and no transaction or covered block was duplicated.
See complete post verification (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-all-post-verification.json`, local evidence).
Independent posts were checked concurrently; the earlier completed checks
were reused only after matching their recorded input/output hashes and fresh
canonical receipts. The final runtime check (`artifacts/audit-evidence/2026-09-14-settlement-targeting/final-runtime-check.json`, local evidence)
confirmed the deployed image, restored settings, continuing settlement and
unchanged original safe block 39500.

## Regression coverage and prevention

The new deterministic unit test
`individually_fitting_blocks_can_exceed_capacity_before_the_first_sync_boundary`
constructs three individually fitting data blocks followed by empty blocks,
and checks that the authoritative prefix index agrees with actual encoding:
the first Sync terminal exceeds two blobs. It passes alongside the existing
prefix-index tests.

The existing native test
`oversized_single_block_reports_no_prefix_and_preserves_data_until_cap_recovery`
also passed with the deployed runtime binaries in 97.99 seconds. It verifies
retention, no invalid post, recovery after an operator cap is relaxed within
the native chain's protocol allowance, and independent follower replay.
It does **not** justify increasing Chiado's protocol blob limit.

Preventing recurrence generally needs a data budget enforced **before unsafe
block commitment**, covering the whole interval to the next valid terminal.
The builder should reserve encoding and terminal overhead, defer ordinary
transactions that do not fit until a later interval, and reject a single
transaction that can never fit. The budget must use the resolved network and
fork allowance and preserve cross-chain atomicity, reorg and restart behavior.
A soft fullness trigger or a smaller consolidation interval alone cannot
guarantee this. Alternatively, changing the allowed terminal boundary would
require separate proof and derivation validation.

Neither preventive protocol change is silently included in this timing fix.
The current behavior is fail-closed but still permits this liveness failure
under sufficiently large admitted traffic.
