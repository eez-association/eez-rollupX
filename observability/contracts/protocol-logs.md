# Receipt protocol-log evidence (schema 6)

The optional L1 log collector supplies evidence for the outcome checks in R19.
It does not yet promote lifecycle outcomes or allocate companion costs. It reads
only receipts already identified by the [canonical indexer](canonical-indexer.md)
or [source receipt worker](source-receipts.md), within the configured history.
No extra blocks, source ownership, or recovered admissions are inferred.

A nonempty L1 `contracts` selection enables the worker; omitted/empty contracts
leave it disabled. Existing configuration, endpoint identity, credentials and
receipt selection remain unchanged. The worker has its own database connection,
30-second pass deadline, five-second success interval and 15/30/60-second failure
backoff. It shares the four-request RPC semaphore and five-second HTTP deadline.
`--once` runs one optional pass after source collection; its exit code still
reflects ordinary receipt scanning. Inspect log metrics and SQL coverage separately.

Each pass attempts at most 32 canonical L1 receipts in oldest-attempt order.
Attempts are recorded before RPC, so a missing, slow or interrupted response does
not monopolize the next pass. Persisted receipts from previous schema versions
are eligible automatically. Completed sets continue to receive rechecks. Each
response is processed and committed separately: memory does not accumulate a
block's log sets. Receipt responses now have an explicit 8 MiB cap in all three
receipt consumers, matching block/transaction retrieval; generic aggregate
responses retain their 1 MiB cap. Oversized responses remain explicit failures.

## Validation and storage

The returned receipt must match the stored transaction hash, inclusion block
hash/number, transaction index, status and type. Every log must match that
receipt's identity, have a strictly increasing log index, and valid address,
topics and data. Removed logs and nonempty reverted receipts are rejected. Absent
or null `logs` is a gap; an empty array is a complete observation of no events.

The fixed ABIs come from the pinned `eez-protocol` definitions:
`L2ExecutionPerformed(uint64,bytes32)` and
`ExecutionConsumed(bytes32,uint64,uint256)`. Topic counts, data widths and the
uint64 rollup bound are checked. Queue indices retain all 256 bits, and repeated
call hashes at different queue positions remain distinct. Other valid event
signatures are ignored. Up to 32,768 raw logs and 4,096 relevant events are allowed
per receipt; exceeding either bound rejects the entire log set. No prefix is
persisted as complete evidence.

Before saving, the worker rechecks chain/genesis, the remote scan cursor and
receipt block hash. It uses the scanner's chain/cursor lock and requires exact
canonical block membership. RPC failure or a changed cursor cannot commit a
new claim. Cancellation drops the dedicated connection; already committed sets
remain safe to reprocess. No node/protocol lock or protocol task is involved.

`protocol_log_claims` stores an ordered digest and fixed normalized event fields,
without calldata, pending envelopes or arbitrary error text. Claims are immutable
to the runtime writer. Under the shared chain lock, at most two different complete
claims are retained per inclusion. Two claims establish a permanent conflict for
that occurrence; repeats, later missing data and a third differing answer cannot
restore a stronger claim. Last-attempt diagnostics are separate from claims. The read-only database role
can inspect raw claims and attempts, including conflicts and orphaned history;
raw tables do not assert canonicality or an outcome.
Missing or conflicting logs never modify fee facts, freeze fee scanning, or
replace a known charge with zero.

## Views and metrics

- `protocol_log_coverage`: one row per known canonical L1 receipt, with `pending`,
  `gap`, `complete` or `conflict`, plus attempt/error timestamps and `source_valid`.
- `canonical_protocol_log_sets`: only unambiguous complete sets, including empty
  sets. Historical observations remain stored when their block is retracted.
- `canonical_protocol_logs`: decoded rows with exact emitter, rollup, receipt and
  log position. A later reinclusion is a separate observation.

Consumers must require `source_valid` and filter the exact configured EEZ emitter
and rollup. A matching signature at a different address is not EEZ evidence.
The log collector preserves observations for all emitters to avoid introducing
another mutable collection identity. These views alone establish neither batch
ownership nor cross-chain success. In particular, a historical applied L2 prefix
may be superseded at the same height: outcome validation must bind it to its
canonical L1 execution occurrence, not require its hash to own that L2 height now.

Metrics expose collection success, last success timestamp, bounded failure reason
and receipt counts by the four evidence states. Collection failure/staleness hides
counts rather than exporting stale zeroes; the last-success timestamp remains.
Successful collection can coexist with missing or conflicting log evidence.
