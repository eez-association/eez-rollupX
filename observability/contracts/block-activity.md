# Canonical block activity (schema 23)

`canonical_block_activity` exposes full-block observations, independently of the
wallet/contract receipt selection. The scanner already fetches each full block;
it captures the complete transaction-array length and optional header fields
before filtering transactions. No additional RPC polling or configuration is
introduced. Metadata, receipt observations and cursor advancement commit in the
same existing chain-locked transaction. Receipt retries can fill unknown fields
from the same exact canonical block without moving the cursor.

| Column | Meaning |
| --- | --- |
| `deployment_id`, `chain_instance_id`, `chain` | Existing deployment/context identity; `chain` is `l1` or `l2` |
| `block_hash`, `block_number`, `timestamp_seconds` | Exact currently canonical block |
| `transaction_count` | All transactions in its observed body, including unselected transactions; zero means an observed empty body |
| `gas_used`, `gas_limit` | Full-block execution gas from the header, not selected receipt gas |
| `blob_gas_used`, `excess_blob_gas` | Header observations of whole-chain blob demand and accumulated fee-market pressure |
| `interval_seconds` | Timestamp difference from its exact canonical parent at height minus one; null if missing, nonconsecutive or time moves backwards |
| `source_valid` | Unfrozen, error-free chain collection within 90 seconds and at most five seconds in the future |

Old rows and absent/null optional fields remain SQL NULL. Missing blob fields do
not establish zero usage or a fork activation date. Gas quantities retain exact
u64 values as SQL numeric; malformed non-null quantities, gas above the observed
limit, or blob gas not divisible by 131072 fail explicitly. Once a field is known,
its value is immutable for the block hash. Repeated observations are idempotent;
missing fields on a later receipt retry do not erase known values. Conflicting
facts use the existing chain freeze behavior. Reorged blocks leave the canonical
view immediately, and same-height replacement blocks retain distinct facts.

Consumers must require `source_valid`, retain coverage counts, and use only
known transaction counts in the empty-block fraction denominator. Divide gas
used by gas limit only when both are present and the limit is positive. Blob gas
divided by 131072 yields observed blob count. The header does not provide the
active fork's maximum blob allowance; this view does not invent a capacity
percentage or turn excess blob gas into an assumed fee. Block cadence is an
observed timestamp difference, not measured wall-clock production latency or
maximum achievable throughput. Receipt gaps do not remove independently observed
header/body facts.

The reader role receives SELECT on the view only. Existing raw-table grants and
indexer selection are unchanged. Upgrade uses stop/backup/explicit migrate/start;
old observers refuse schema 23. No automatic history rescan is added: pre-upgrade
rows stay unknown except when an already-existing bounded receipt retry revisits
them. Previously stored timestamps still support exact parent-linked cadence.
Rollback requires a compatible binary or the pre-migration backup.

Coverage: `indexer` Rust tests check missing/zero/full-width and malformed header
values. `tests/block-activity.sql` checks actual PostgreSQL unknown/zero values,
parent gaps/backwards time, reorg replacement, health, immutable facts and reader
access. `tests/smoke-database.py` exercises the actual scanner against RPC header
fixtures and checks stored values across receipt failure/retry. Synthetic SQL
rows alone do not establish RPC ingestion or protocol execution.
