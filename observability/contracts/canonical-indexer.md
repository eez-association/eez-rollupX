# Canonical receipt indexer

`eez-observer indexer --config FILE` runs the existing RPC exporter and two
independent canonical scanner workers. `--once` imports at most one configured
new-block batch per chain, retries up to four historical receipt-gap blocks,
and exits without starting HTTP. A reorg can require walking more history than
one batch; discovery yields for 15 seconds after each batch of ancestors.
The observer submits no transactions and needs no signing keys.

The [example](../indexer.example.json) deliberately contains invalid placeholders.
Verify endpoint chain IDs, genesis hashes, rollup identity, poster wallets,
contracts and deployment bootstrap heights before replacing them. Keep
`environment` and `deployment` identical to the stack's target labels. The nested
`exporter` object has the same defaults and validation as exporter mode. A relative
`database_config` path resolves relative to the indexer config file, not its
process working directory. Keep credential files mode 0600.

Selection is explicit per chain: configured sender wallets, recipient contracts,
or `include_all_transactions: true`. Empty implicit selection is rejected.
Addresses are case-insensitive, must be exactly 20 bytes, and duplicates within a
list are rejected. At most 256 selection addresses are accepted. Both the
bootstrap and normalized selection are persisted and cannot be changed for an
existing chain namespace. Expanding selection requires a separately reviewed
rescan into a new deployment namespace; silently changing it would misrepresent
historical completeness. `batch_blocks` defaults to 16 and accepts 1–64.

A transaction matched by both sender and recipient is collected once. Selection
alone does **not** classify a receipt as a settlement or companion. Those protocol
relationships need separate evidence. L1 and L2 totals remain separate.

Each cycle verifies RPC chain ID and genesis before reading history. The namespace
includes deployment, chain role, chain ID, genesis and L2 rollup identity. Before
committing a block, the scanner checks transaction/receipt inclusion identities,
parent linkage, RPC identity again, and both the new block and prior cursor
against canonical RPC hashes. The first bootstrap block also checks its parent
when it is not genesis. Complete block/receipt writes and the cursor commit in
one transaction. Concurrent workers lock the chain before its cursor and compare
the expected cursor; a stale worker retries without advancing another worker's
snapshot. A database disconnect after commit is safe to retry.

Unfinalized forks retract canonical flags and affected receipt views atomically,
retain original inclusions, and permit the same transaction's later reinclusion.
Orphaned receipt gaps stay historical rather than being labelled repaired.
An exact contradiction of a previously stored finalized L1 anchor freezes that
chain. An inconsistent in-flight fetch is a retryable collection failure, not a
finality alarm. L2's `finalized` RPC tag is deliberately unused: exact applied
settlement hashes and finalized L1 anchors are required for L2 finality.
That settlement integration remains an open acceptance gate.

Missing/pruned blocks stop advancement at that height and persist a chain gap.
Missing or incomplete receipts persist receipt gaps without blocking subsequent
headers. Up to four gap blocks rotate fairly on each cycle. A service-level
receipt failure stops additional calls for that block and records each remaining
selected transaction as unavailable. Later complete data repairs gaps. Complete
receipt costs and inclusion facts cannot be overwritten by contradictory data;
conflicting immutable facts freeze the chain for investigation.

The SQL billing base is one canonical inclusion per logical transaction. Exact
U256 receipt quantities, U512 individual products, and unconstrained integer
`NUMERIC` sums preserve precision; canonical reverts incur charges. Fee caps,
gas limits and simulations are never substituted. A missing field produces a
NULL cost plus an explicit gap. Receipt bytecode/input and signed envelopes are
not stored; transactions retain a four-byte method selector for later decoding
integration.

The reader can access `eez.canonical_receipts`, `eez.canonical_cost_totals`, and
`eez.index_coverage`. Cost views retain known historical facts even when collection
fails. Consumers **must** show `source_valid`, indexed bounds, incomplete blocks
and receipt coverage alongside totals; a known subtotal is not a complete bill.
`source_valid` requires no error/freeze and last success within 90 seconds,
allowing at most five seconds of future clock skew. Liveness and receipt coverage
are separate: a healthy scanner can still report historical gaps.

Metrics in indexer mode:

| Gauge | Bounded labels | Meaning |
| --- | --- | --- |
| `eez_observer_index_collection_success` | `chain=l1/l2` | Last scan succeeded and is fresh; otherwise 0 |
| `eez_observer_index_last_success_timestamp_seconds` | `chain` | Last successful scan; preserved after failure |
| `eez_observer_index_error` | `chain`, enumerated `reason` | Current error; disappears after recovery |
| `eez_observer_lag_blocks` | `chain` | Observed head minus validated cursor |
| `eez_observer_index_open_gaps` | `chain` | Open/frozen persisted gaps |
| `eez_observer_index_finalized_number` | `chain` | Highest verified stored finalized block, if any |

Lag/gap/finality gauges are absent when the scan is stale or failed; default
exporter mode has no indexer metrics. RPC requests share a four-call semaphore
with the exporter and retain five-second deadlines, no redirects, and bounded
responses (8 MiB for full blocks, 1 MiB for other calls). A block above 10,000
transactions is explicitly rejected. Workers wait 15 seconds on success and
back off 15/30/60 seconds on failure. DB connection, statement, lock and client
request deadlines are respectively 5/10/5/15 seconds. HTTP drain is capped at two
seconds and scanner cancellation closes its dedicated database connections.

## Wallet and posting method evidence (schema 2)

`eez.wallet_receipts` and `eez.wallet_cost_totals` select receipts whose sender is
in that chain's configured wallet list. Wallet identity establishes spend only.
`eez.posting_receipts` additionally requires L1, a configured recipient contract,
and a complete canonical decode of the current `postAndVerifyBatch` ABI. A reverted
posting call still incurs its exact charge. Neither view asserts application of
an L2 range or ownership of a companion/source. These populations overlap and
must not be added together. All L2 receipt fees remain outside L1 totals.

The decoder reuses `eez-protocol::entries::decode_postbatch` and the generated ABI
types. An allocation-free preflight checks array lengths and contiguous, ordered
tails using those token types, before the normal decoder allocates. A full canonical
argument round trip then checks primitive values and padding. The input cap is
8 MiB with a finite token budget. Full transaction backfill uses the same 8 MiB
response bound as full block fetching. Only a calldata hash and classification
are retained; calldata, proofs and signed envelopes are not persisted.

`eez.posting_coverage` shows decoded, invalid, other-method and unknown method
counts, method collection failures, and last attempt time. `other` means a selector
other than the current posting method; it includes unsupported historical ABI
selectors and is not proof that no older protocol posting call occurred.
`unverified` includes noncanonical/overlapping ABI layouts, trailing bytes or
preflight-budget rejection. Such a call can still have a valid paid receipt;
its charge stays in wallet/general totals while it is excluded from the narrower
posting view. Do not report the posting subset as complete historic settlement
spend when unsupported or unverified method evidence exists.

Migration 2 leaves migration 1 unchanged, preserves all receipt costs, and seeds
old transaction classifications as `pending`. The ordinary scanner retries at
most four pending method records per chain/cycle in oldest-attempt order. A failed
transaction fetch leaves an explicit pending/error state and does not block
header/receipt progress. Decoding never reconstructs source ownership from sender
or transaction order. Decoded classifications are immutable for the same logical
transaction; a contradictory later observation is an inconsistency.
