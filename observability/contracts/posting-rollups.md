# Posting verification-table evidence (schema 7)

The [canonical indexer](canonical-indexer.md) retains the exact
`rollupIdsWithProofSystems[].rollupId` list from an ABI-decoded posting call. This
is needed when validating same-block application windows: the pinned native
scanner and EEZ contract replace queues only for the rollups that a successful
post lists. A post for another rollup cannot be treated as that boundary.

`posting::Analysis` uses the existing allocation-bounded ABI preflight, decoder,
canonical re-encoding check and calldata hash. Only `state='decoded'` produces a
list. IDs retain wire order, duplicates and the full uint64 range; the observer
does not silently normalize the payload or reimplement contract validity checks.
For example, an unsorted verification table can be ABI-decoded but would fail the
contract's structural validation. A receipt's success remains a separate fact.
No calldata, proof bytes or signed envelope is stored.

Migration 7 adds nullable `posting_analysis.verification_rollups`, a one-dimensional
array of the existing exact `eez.u64` domain. NULL means the list is unknown;
an empty array means the call decoded to an empty table. Arrays with null,
out-of-range or multidimensional elements are rejected. Once populated, the
existing fact guard makes the list immutable alongside method state and calldata
hash. Reorg and reinclusion cannot change a transaction's decoded calldata.

Old decoded rows keep their existing method classification, calldata hash and
fees while the list is missing. The existing fair method-repair lane now also
selects those rows, at most four per chain cycle. It refetches and checks the
transaction using the existing identity and immutable-classification checks.
Unavailable calldata leaves an explicit `collection_error` and retries; it does
not turn NULL into an empty list, remove a known posting call or erase its fees.
New observations and source receipt dependencies populate the list through the
same shared writer. There is no additional configuration or native RPC method.

Reader views:

- `posting_rollup_evidence` extends the original canonical posting-receipt
  population with its nullable verification table and repair diagnostics.
- `posting_rollup_membership` exposes one row per table entry, preserving duplicates
  and one-based SQL `table_position`. Cast `verification_rollup_id` to text in
  clients that cannot represent uint64 integers exactly.

Both views retain receipt success and `source_valid`. They include reverted calls
for diagnostics and accounting; a listed rollup is not proof that it was verified
or advanced. Consumers must require the correct contract, canonical successful
inclusion and relevant membership/occurrence evidence before using the list as a
queue boundary. Missing decoded lists leave that association unknown. This slice
does not independently classify source completion or allocate companion charges.

Use the normal stop/backup/explicit-migrate/start upgrade order. Older executables
refuse schema 7; rolling back only the executable is unsupported. Existing
configuration and fee populations remain compatible. Historical metadata backfill
requires neither resetting receipt cursors nor changing protocol/node state.
