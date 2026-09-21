# EEZ settlement indexer

A read-only settlement indexer. It indexes the pinned registry's
`L2ExecutionPerformed(uint64,bytes32)` events for one rollup. Emitted roots can be intermediate commitments that do
not identify a materialized L2 block; explorer links use correlated range endpoints. Optional details add transaction blob hashes, L2 ranges using the
existing correlation RPC, and Beacon blob retrieval. It does not index individual
cross-chain user effects or inspect Composer's scheduling state.

## Contract and implementation checklist

- WebSocket `newHeads` notifications wake one sequential RPC scanner. Periodic
  head checks and persisted checkpoints recover missed notifications/restarts.
- Require matching chain ID and genesis on HTTP and WS; bind the database to
  that identity, registry, rollup and initial block. Reject incompatible reuse.
- Store all scanned block headers, matching settlement events, and one receipt
  charge per transaction. Multiple logs/transactions in one block stay distinct.
- Validate block/receipt/log attribution and recheck canonical hashes before
  committing each block. Commit data and progress atomically in PostgreSQL.
- Detect head regression and same-height replacements; remove reverted blocks
  and their dependent rows transactionally, then replay the replacement branch.
- Serialize writers with a PostgreSQL advisory lock. Never modify Composer data.
- Grafana reads a restricted PostgreSQL role. Show time, L1 block, exact transaction
  links, L2 commitment hashes, and execution/blob fees separately. L2 range
  endpoints link by hash to Blockscout; blob hashes link to Blobscan.
- Optional enrichment runs separately from the L1 checkpoint writer. Pending L2
  correlations or unavailable Beacon data cannot block canonical indexing.
  Enrichment rows reference canonical receipts and are deleted on reorg.
- Unit/component tests cover malformed data, receipt costs, duplicates, multiple
  events, partial failures, restarts, reorgs, WS reconnect and chain mismatch.
- Keep this manifest/lockfile independent of the node workspace: monitoring-only
  dependency updates must not alter the Composer/proof-signer validation identity.

## Accounting boundaries

Records describe successful commitment events in the current canonical L1 chain,
not finalized history. Administrative `RootUpdated` events and failed submissions
are outside this increment. Receipt costs are whole-transaction costs, including
any work for other rollups; never sum them once per emitted settlement event.
The existing Composer cross-chain completion card remains an internal confirmation
observation. It cannot be replaced by a settlement receipt alone.

The protocol reference is `eez-core-protocol` revision
`3c0cdf4955941a5c23639346932b55b33babebbd`, `EEZ._applyRollupUpdates`, also mirrored
in `crates/eez-protocol/src/abi.rs`. The previous observer's strict log/receipt
attribution and transaction-level charge separation inform this smaller indexer.

## Start on the same machine

First set up the [base monitoring stack](../README.md#start). From `monitoring/`:

```bash
cp indexer/config.example.json config.indexer.json
```

Edit the config with the L1 HTTP/WS endpoints, chain ID, genesis block hash,
registry, rollup ID and registry deployment block (`start_block`). The example's
placeholders deliberately fail validation. Obtain the genesis hash using
`eth_getBlockByNumber("0x0", false)` on the intended L1 endpoint. Choose the
deployment block to retain the whole rollup history, or a later block for an
explicitly limited history. The scanner needs headers, logs and receipts from
that block onward. Nothing is collected before it.

The optional `details` object configures `l2_http_url`, `l2_chain_id`,
`l2_genesis_hash` and `beacon_url`. Use `eth_chainId` and genesis block zero from
the intended L2. The database binds enrichment to that L2 identity; URLs may
change, but silently switching L2 networks is rejected. The L2 endpoint needs
`eez_getSettledL2RangesByL1Block`, block headers and `eth_getBlockReceipts` for the
indexed L2 history. Remove the entire object for L1-only operation;
existing configs without it still work and the extra panels show pending data.

Create credentials once; preserve existing files when restarting:

```bash
umask 077
printf 'INDEXER_DATABASE_PASSWORD=%s\nINDEXER_GRAFANA_PASSWORD=%s\n' \
  "$(openssl rand -hex 32)" "$(openssl rand -hex 32)" > .env.indexer
docker compose --env-file .env --env-file .env.indexer \
  -f compose.yaml -f compose.indexer.yaml config --quiet
docker compose --env-file .env --env-file .env.indexer \
  -f compose.yaml -f compose.indexer.yaml up --detach --build
docker compose --env-file .env --env-file .env.indexer \
  -f compose.yaml -f compose.indexer.yaml logs --tail 20 indexer
```

The indexer uses HTTP for authoritative reads and WS to wake on new L1 heads.
It also checks every five seconds; a WS outage does not stop catch-up. Only one
scan runs at a time. RPC calls time out after ten seconds and failed scans retry
after five seconds. PostgreSQL loss exits the process and Compose restarts it.
The database is on localhost port 19432 (override `INDEXER_DATABASE_PORT` in
`.env.indexer`). Both credentials are required; use hex passwords as above to
avoid URL encoding when assembling `DATABASE_URL`. Private config and credential
files are ignored by Git.

Open `/d/eez-settlement-history` in Grafana. Set its L1/L2 explorer and Blobscan
URLs for your network. It shows the latest blob posting's commitment time,
block and transaction, blob count, execution/blob/total fees, posting result,
L2 ranges and status, versioned blob hashes and the Beacon whole-block blob
count in one **Latest settled commitment** table, plus recent events and indexing
progress. Extra rows list multiple ranges/blobs independently; receipt fees
appear once, without multiplying ranges by blobs. Links open only the L1/L2
explorers and Blobscan. Collection timestamps and Beacon retrieval diagnostics
are retained in PostgreSQL but omitted from the table.
**Settlement events (grouped by transaction)** uses the same columns and explorer
links for the latest 100 blob-posting transactions with commitment updates in the
selected range. Both tables exclude non-blob execution updates; filtering happens
before selecting the latest posting or limiting the history to 100 transactions.
Each transaction shows its total commitment-update count. **Last L2 block** uses
the correlated batch range endpoint and links to its exact block hash. An
intermediate commitment emitted by the posting may not correspond to a materialized
L2 block, so it is not used as a block-explorer link. Missing range data stays
blank. All raw events remain in PostgreSQL. Fees appear once per transaction.
Execution updates remain indexed and included in the fee chart and totals. Batch L2 ranges
belong to the posting and may include blocks completed by its companion
transactions; they are not ranges for individual commitment logs. Non-blob updates
have no batch correlation of their own and do not retry that lookup. An empty
time range produces an empty table.
Both tables include transaction gas used, gas limit and utilization, plus blob
stream bytes used, usable capacity and utilization. Gas used comes from the
receipt and its limit from the exact transaction; a limit below actual use is
rejected. Blob capacity is **126976 usable bytes per blob**, compared with
131072 physical bytes. Used bytes measure the occupied EEZ stream through its
final close marker, including version/framing and meaningful zero bytes, excluding
trailing padding. The measurement validates packing/version/terminal marker; it
does not decode the inner operations or claim cryptographic KZG verification.
Beacon commitments are hashed to select only this transaction's blobs, in its
blob order, before measuring usage. Missing/pruned bytes remain blank, not zero.

Both commitment tables also show **L2 burned fees** and **L2 tip fees** for each
finalized L2 range. Burn is the sum of receipt gas used times that block's base
fee; tips use receipt gas used times (effective gas price minus base fee). These
are L2 execution fees, including system transactions and reverted transactions,
not L1 posting costs or poster profit. Values are stored as exact integer wei and
displayed in ETH/xDAI. Missing receipts or incomplete scans stay blank; a fully
collected empty range has zero fees. Receipt identities and gas totals must match
each block, and block hashes/parents must match the finalized range.

Collection advances at most 128 L2 blocks per posting per details pass and saves
progress in the existing details row, so retries/restarts do not double count.
Only finalized ranges are summed. Changed range identities start a new scan;
L1 reorg cleanup removes their details as before. Older history backfills gradually
without delaying canonical L1 event collection. There is no new Composer metric,
configuration option, database service, or CI job.

The fee chart stacks execution and blob fees; the combined height is the total.
It does not plot another total bar on top. The selected-range totals panel still
shows execution, blobs and total explicitly.
Until backfill reaches the head, the latest indexed event may be older than the
current settlement. Costs use ETH/xDAI; gas prices in the SQL view use gwei.
The existing Composer dashboards still show internal queue and policy state.

The details worker examines up to eight transactions every five seconds, newest
uncollected first. Incomplete details retry after a minute. A successfully
collected transaction with finalized L2 ranges is not continually re-fetched.
Beacon checks bind the slot to the L1 execution block, check blob byte structure
and compare count against whole-block blob gas. Successful retrieval is recorded
with its time; it is not a promise that Beacon still retains those bytes, nor
cryptographic KZG verification. Blobs themselves are not stored in PostgreSQL.
The Beacon count is for the whole L1 block, while the transaction blob list and
fees belong to the posting transaction. Missing data is shown explicitly.

Upgrading creates two additive tables (`details_source`, `details`), without
resetting or backfilling the canonical checkpoint. Existing transactions are
enriched automatically. To disable enrichment again, remove `details` from the
config and restart only the indexer; already collected details remain readable.
The L2-fee update requeues older enrichment once, retaining prior observations
and canonical history. Completed version-3 details stay cached across restarts.
No database reset is required. An older indexer cannot collect L2 fees and will
replace details when it retries them; re-upgrading requeues those older records.

Gas-limit utilization is not block fullness or gas charged. Posting normally
uses simulated consumption plus a 20% margin, but
[`posting.rs`](../../crates/eez-composer/src/composer/posting.rs) retries with its
validated full gas budget if the smaller signed transaction fails the effects
check. The latest audited posting used 179519 of a 16777216 limit (1.07%); its
preflight log confirms both values. Of 141 audited blob postings, 133 used roughly
83.33% of their declared limit and eight used the full ceiling. The exact rejected
smaller-limit error is not logged, so its cause cannot be determined from that
record. Fees use receipt gas consumed. Tightening that conservative fallback is
a separate Composer change, not a monitoring/database repair.

PostgreSQL and Grafana are optional additions; the base stack continues to work
without this overlay. This increment makes no node, signer, contract or root
Cargo.lock changes. Its container is built separately from the node image.

## Restart, reorg and recovery

Restarting resumes after the last committed block. The database binds itself to
the configured chain/genesis, registry, rollup and start block; changing these
requires a separate database, not reusing an existing volume. HTTP/WS URLs may
change if they serve the same chain. Headers and indexed history are retained
without pruning in this first increment.

Canonical replacements roll back the affected suffix and replay it automatically.
These are canonical, not finalized records; dashboard rows and totals may change
after a reorg. Missed WS notifications do not lose history. Do not remove volumes
with `down --volumes` unless intentionally discarding this rebuildable index.

For a small local backup, use the same Compose arguments as above:

```bash
docker compose --env-file .env --env-file .env.indexer \
  -f compose.yaml -f compose.indexer.yaml exec -T indexer-db \
  pg_dump -U eez_indexer -d eez_indexer -Fc > indexer.dump
```

Keep dumps outside Git. Restore only to a stopped indexer and a dedicated database
with the same source configuration. Grafana uses `eez_grafana`, a separate role
granted SELECT only; database writer credentials are not passed to Grafana.
Initialization creates that reader on an empty PostgreSQL volume. Changing a
password in `.env.indexer` does not change an existing database role password.

## Focused validation

From the repository root:

```bash
cargo +1.94.0 fmt --manifest-path monitoring/indexer/Cargo.toml --check
cargo +1.94.0 clippy --locked --manifest-path monitoring/indexer/Cargo.toml --all-targets -- -D warnings
cargo +1.94.0 test --locked --manifest-path monitoring/indexer/Cargo.toml
```

The default tests cover config validation, event/receipt parsing, fee arithmetic,
detail attribution, L2 range consistency, Beacon blob structure, L2 burn/tip
arithmetic and checkpoint recovery, and WS reconnection/identity rejection. The single database test is opt-in and
requires a disposable database named `eez_indexer_test`: it deletes that test
schema, then checks writer exclusion, atomic failure, repeated scans, restart,
multiple events per transaction, reorgs and incompatible configuration.
The reorg check also verifies deletion of associated enrichment.

```bash
INDEXER_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:19433/eez_indexer_test \
  cargo +1.94.0 test --locked --manifest-path monitoring/indexer/Cargo.toml \
  --test recovery -- --ignored
```

No new CI workflow or node E2E matrix is required for this standalone increment.

### Chiado validation (2026-09-21)

The five focused tests passed, including the opt-in PostgreSQL recovery test.
Formatting, Clippy, shell syntax, Compose rendering and the release container
build passed. A live backfill from registry deployment block 23118328 reached
the head with 188 events across 165 transactions. The latest five receipts were
independently compared with L1 for block hash/time, event roots and exact fees.
All five dashboard queries returned data through Grafana's read-only role.
No transactions were sent and neither Composer nor the signer was restarted.

On the current local deployment, the dedicated Compose project is in
`.runtime/chiado-observability-20260918/indexer/` (including private `.env`, config
and `compose.json`). Run `docker compose -f compose.json ps` or `logs indexer`
from that directory. Its database additionally joins the existing Grafana ledger
network; the checked-in overlay uses localhost with the base stack's host networking.
The live datasource was added using Grafana's API, which stores its password
encrypted, and survives restart in the existing Grafana volume. The dashboard
is file-provisioned in the EEZ folder at
`https://eez.asuscomm.com/grafana/d/eez-settlement-history`.
Local-only validation details are in that runtime directory's `validation.json`.
The new indexer runs independently; existing Composer metrics remain compatible.

The subsequent details extension passed six focused tests and all seven Grafana
queries. The latest live posting matched the network dashboard: L2 blocks
263251–263255 (five finalized blocks), one transaction blob, and two retrieved
blobs in Beacon slot 24923790 for the whole L1 block. Both L2 endpoint hashes
resolved in Blockscout and the versioned blob hash resolved in Blobscan.
Enrichment catches up in the background; unavailable older correlations or
pruned Beacon blobs remain explicitly pending. No node or CI changes were needed.
The subsequent presentation cleanup consolidates those details into one table
and removes collection diagnostics and network-dashboard/decoder links. All five
remaining dashboard queries passed; the collector and schema are unchanged.

The utilization extension passed seven focused tests, including requeueing old
enrichment while retaining current completed records. All 165 indexed receipt
totals matched execution plus blob fees; the fee chart now stacks only those two
components. The latest posting's gas use matched RPC (179519 / 16777216), and
2664 / 126976 occupied stream bytes matched the existing outer-message parser
with inner operations treated as opaque. All five Grafana queries passed with
the new columns. These checks do not claim inner-payload or KZG validation.

The duplicate-data review checked every indexed event against L1 `eth_getLogs`:
188 distinct logs in 165 transactions, with no missing/extra events. All receipt
charges, signed gas limits and transaction blob hashes matched RPC. All 141
event-bearing L1 block headers matched canonical RPC, and 49575 stored headers
were contiguous. All 141 collected L2 ranges and blob measurements matched the
live correlation API and independent outer-envelope parsing. The repeated rows
were real intermediate commitment logs, not database corruption. Grouping now
shows 141 blob postings and 24 non-blob execution updates, preserving all raw
logs and counting each transaction's fees once. No database reset was performed.
Local read-only audit evidence is in `indexer/review-20260921/` under the runtime
directory above. The reviewed change passed six default tests (database recovery
was not rerun; no persistence logic changed), Clippy, formatting and all five
Grafana queries, including whole-history, narrow and empty range checks.
