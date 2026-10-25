# EEZ settlement indexer

A read-only settlement indexer. It indexes the pinned registry's
`L2ExecutionPerformed(uint64,bytes32,uint256)` events for one rollup. Emitted roots can be intermediate commitments that do
not identify a materialized L2 block; explorer links use correlated range endpoints. Optional details add transaction blob hashes, L2 ranges using the
existing correlation RPC, Beacon blob retrieval, and unique successful cross-chain
source transaction counts. Optional structured posting logs add local submission
history. It does not validate individual user effects or control Composer scheduling.

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
`820e50f749f63165be6a5797d4c5fe52cbbbbe34`, `EEZ._applyRollupUpdates`, also mirrored
in `crates/eez-protocol/src/abi.rs`. The previous observer's strict log/receipt
attribution and transaction-level charge separation inform this smaller indexer.

This event ABI is incompatible with the previous two-field event. Keep the old
indexer with old-protocol deployments. Deploy this version with matching new
contracts and a separate network identity/database; no historical event migration
is provided. The ether-balance word is validated structurally but is not used as
a settlement or fee claim.

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

Reth can retain the genesis header while rejecting the full genesis block with
pruned-history RPC code `4444`. For that specific error, HTTP identity checks,
head sampling and WS subscription setup request `debug_getRawHeader("0x0")` and
compare its locally computed Keccak hash with the configured genesis. Missing,
oversized or malformed bytes and mismatched hashes fail identity validation;
other RPC failures do not activate this fallback. The node must expose that
read-only debug method to the indexer when using pruned history. Retained block,
log and receipt requirements from `start_block` remain the same.

Upgrade the indexer before switching its RPC to a Reth revision that prunes the
genesis block API. This fix changes no configuration, schema or checkpoints;
existing full-genesis endpoints remain supported. An older indexer can reuse the
database, but cannot resume against the pruned endpoint until that endpoint is
changed or the fix is restored. Preserve the database and posting-log volumes
through either upgrade or rollback.

The optional `details` object configures `l2_http_url`, `l2_chain_id`,
`l2_genesis_hash` and `beacon_url`. Use `eth_chainId` and genesis block zero from
the intended L2. The database binds enrichment to that L2 identity; URLs may
change, but silently switching L2 networks is rejected. Set `l2_http_url` to
the **Composer L2 front** (Chiado default `http://127.0.0.1:18998`), not the
regular node/follower RPC. The front provides
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

Both commitment tables also show **L2 collected base fees** and **L2 tip fees** for each
verified canonical L2 range. Ordinary transaction base fees credited to the genesis collector equal receipt gas used times
that block's base fee; tips are receipt gas used times (effective gas price minus
base fee). Ordinary reverted transactions still pay these fees. Native system
transactions of type `0x45` contribute receipt gas but pay **zero** base fees and tips,
including in blocks with a positive base fee. A nonzero native price, an ordinary
price below the base fee, or a missing/unsupported receipt type leaves collection
pending. The native exception follows the
[node's native transaction profile](../../crates/eez-proof-signer/SPEC.md#3-startup-configuration).

These are gross L2 execution fees, not L1 posting costs or poster profit. The
range records expose `baseFeeWei` and `tipWei` decimal strings in the
network's native base units, displayed in ETH/xDAI. Fee-accounting checkpoint
version 2 records:

| Field | Meaning |
| --- | --- |
| `ordinaryGasUsed` | Sum of ordinary receipt gas, as a decimal string. |
| `nativeGasUsed` | Sum of native system receipt gas, as a decimal string. |
| `tipsByBeneficiary` | Object mapping the actual L2 block beneficiaries to decimal priority-fee totals; its sum must equal `tipWei`. Zero amounts may be omitted. |

A changed beneficiary is accounted for using each block's `miner` field, never
the current operator configuration. A zero-address beneficiary is recorded when
it receives nonzero tips. Attribution does not prove treasury ownership or exclude
operator-funded transactions; these totals are not yet qualified external
customer revenue, redeemable L1 funds, or inputs that authorize posting.
Missing receipts or incomplete scans stay blank; a fully collected empty range
has zero fees. Receipt identities and cumulative/block gas totals must match
each block, including native gas, and block hashes/parents must match the
correlated canonical range.

The details worker processes up to four independent transaction rows concurrently,
from batches of eight. A slow Beacon/L2 request no longer serializes all eight rows.
Concurrency remains separate from canonical L1 indexing and uses existing RPC
connections; cancellation drops outstanding futures. A result orphaned by an L1
reorg cannot survive its database foreign key. The worker retries progressing scans
after five seconds and unavailable/unchanged data after sixty seconds. Canonical
results continue to be checked until finalized; the database contains the most
recent validated observation, not an instantaneous view of RPC reorgs.

Collection advances at most 128 L2 blocks per posting per details pass and saves
progress in the existing details row, so retries/restarts do not double count.
Canonical ranges are collected immediately; the table's L2 status distinguishes
provisional canonical results from finalized ones. Every retry revalidates a cached
canonical result against its endpoint hashes, including when promoting it to finalized.
Hash/state mismatches hide the total; changed range identities start a new scan;
L1 reorg cleanup removes their details as before. Older history backfills gradually
without delaying canonical L1 event collection. There is no new Composer metric,
configuration option, database service, or CI job.

The fee chart stacks L1 base fees, effective priority tips and blob fees in ETH;
the combined height is total L1 cost. Unconnected green dots on the same ETH axis show
**Estimated total at recent tip**: actual base cost + actual gas used × recent
reference tip + actual blob cost, summed across transactions in each bucket.
Each posting samples `eth_feeHistory` at the preceding 20 L1 blocks, excluding
its own block; we take the median of nonempty blocks’ gas-weighted 50th-percentile
tips. Zero-tip nonempty blocks count; empty blocks do not. Each dot is a
counterfactual price comparison with unchanged execution and inclusion-block
conditions, not a guarantee that the reference bid would get the same inclusion.
Missing reference data leaves a gap rather than a partial cost estimate.

Enrichment version 4 backfills these fields once without resetting L2 fee
checkpoints or canonical history. The split uses the inclusion block base fee
and receipt effective gas price. Historical fee-window reads check the canonical
posting hash before and after; reorg cleanup removes enrichment. Missing data
is blank, never substituted with zero. No Composer changes are needed.
It does not plot another total bar on top. The **Fees** panel shows L1 execution, L1 blobs and L1 total separately from
L2 collected base fees and L2 tips for posting ranges included in the selected L1 period.
It is a compact value card above Cross-chain transactions on the right of the
wider Settlement transaction fees chart. Both summaries use stat panels without
table borders or headers, preserving all six fee values and all three counts.
The Fees card uses automatic precision, including
`0 ETH` when the period contains no postings. The final row is **L2 tips − L1
total**, calculated before floating-point display conversion; positive values mean
L2 tips cover L1 costs. The difference stays Pending when the L2 total is incomplete. L2 totals stay Pending if any
selected posting lacks complete canonical-range fees; partial sums are not shown.
Until backfill reaches the head, the latest indexed event may be older than the
current settlement. Costs use ETH/xDAI; gas prices in the SQL view use gwei.
The existing Composer dashboards still show internal queue and policy state.

**Cross-chain transactions** counts unique successful source transactions for this
rollup, with separate inbound, outbound and total rows. Several top-level calls
in one source transaction count once. Inbound uses the configured L1 registry's
nonzero-key `ExecutionConsumed` events for this rollup, deduplicated by source
transaction hash. Zero-key entries execute outbound L2 transactions and are not
inbound sources. Outbound scans the verified canonical posting ranges for the L2 manager's
`ExecutionConsumed` logs, verifies their canonical receipts, and excludes inbound
system delivery receipts (`IncomingCrossChainCallExecuted`), including nested
callbacks. Static-only and reverted calls do not count as successful deliveries.
These are transaction counts, not call counts or attempted submissions. The Latest settled commitment and
Settlement events tables also show Cross-chain tx, Inbound tx and Outbound tx
after L2 blocks. These are totals for the displayed **L1 block**, across all of
its postings and inbound source transactions for this rollup, using the same
source identity rules as the period summary. They appear only on the posting's
first row. If a block contains several postings, those rows share the same block
totals; summing them would double-count that block. Continuation rows remain blank.
Missing collection leaves the affected count blank (not zero).


The selected period uses L1 settlement time: inbound source inclusion and outbound
posting inclusion. An outbound source executed earlier can therefore appear when
its range settles. Only history from the configured indexer start block is covered;
this is not a rollup lifetime total. Each direction stays Pending until every
selected record needed for it is complete; an empty period shows zero. Stored
source hashes are deduplicated across posting ranges and retries. Reorg deletion
of canonical receipts cascades to their counts. The indexer source record retains scanner freshness and the configured history
start; the dashboard omits the History coverage panel.

Outbound collection scans at most four 2,048-block log chunks per posting
per pass, checkpoints only whole verified chunks, and resumes after restart.
Finalized range identities and canonical block/receipt attribution are checked;
a changed range discards the old checkpoint. Completed scans are cached. This
adds bounded historical RPC reads to the standalone indexer, with no Composer
instrumentation or new configuration. An older indexer cannot maintain these
fields; upgrading again requeues its older records.


The details worker examines up to eight transactions every five seconds, newest
uncollected first. Incomplete fee or cross-chain scans that advance their checkpoint without errors resume after five seconds; stalled scans, RPC errors and other missing details retry after a minute. A successfully
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
Enrichment version 6 requeues older records once. It preserves canonical L1
history and reusable transaction/blob/cross-chain observations, but rescans old
L2 fee ranges: their totals alone cannot establish native gas or beneficiary
attribution. Collection still advances at most 128 blocks per posting/pass, and
new fee checkpoints resume across retries/restarts. Missing historical receipts
leave that range pending; no attribution is guessed. No database reset or new
configuration is required. Completed current-version finalized ranges stay
cached; provisional canonical ranges continue to be revalidated.

Consumers must read `baseFeeWei` in place of `burnWei`; the field is renamed
because native-v1 networks collect ordinary base fees instead of burning them.
Rolling the indexer back can overwrite enrichment on retry, lose the new fields
and restore the positive-base-fee/native-receipt bug. Do not rely on beneficiary
attribution while running an older collector. Re-upgrading requeues old-version
details and reconstructs the affected fee ranges again.

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

## Current network summary

The top of the settlement dashboard groups latest, safe and finalized RPC head
heights into separate L1 and L2 cards, with a third card for
**L2 unsettled = latest − safe**. The unsettled card shows `count / policy limit`
and a percentage, using the live Composer block-threshold metric from Prometheus.
Neither count nor limit is hard-coded. The threshold requires a healthy scrape
and a sample no older than 30 seconds; invalid inputs suppress the ratio.
The card requests a recent policy sample; absolute historical selections that
return an old policy sample suppress the ratio rather than mix it with current heads.
This is backlog relative to the block trigger, not a settlement countdown or the
Composer's exact candidate-to-anchor decision. Other policy triggers can post
earlier, and progress may exceed 100% while awaiting inclusion.
Unsettled counts produced
L2 blocks beyond the safe head, not queued transactions. Each height links to its
chain explorer. These cards always show current state, independent of the selected
historical time range and settlement backfill.

A separate indexer task samples both endpoints every five seconds using the
existing L1 configuration and optional `details` L2 configuration. No Composer
changes or additional settings are required. Each chain is identity-checked;
finalized ≤ safe ≤ latest and canonical height/hash checks reject inconsistent
reads. Reads are observations, not an atomic cross-chain snapshot. Each chain has
a ten-second collection timeout. A failed sample clears its values; Grafana also
hides samples older than 30 seconds, measured from the start of collection.
Unavailable safe/finalized tags are never replaced with latest or zero. Without
`details`, L1 is sampled and L2 displays Unavailable.

The additive `network_heads` table stores only the latest observation per chain.
Existing history and enrichment are preserved. The existing Grafana reader's
default SELECT privileges cover the new table; installations with custom grants
must grant SELECT on `eez_indexer.network_heads`. Older indexers leave snapshots
stale and the cards become Unavailable after 30 seconds.

## Focused validation

From the repository root:

```bash
cargo +1.95.0 fmt --manifest-path monitoring/indexer/Cargo.toml --check
cargo +1.95.0 clippy --locked --manifest-path monitoring/indexer/Cargo.toml --all-targets -- -D warnings
cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml
```

The default tests cover config validation, event/receipt parsing, fee arithmetic,
detail attribution, L2 range consistency, Beacon blob structure, L2 base-fee/tip
arithmetic, native zero-price receipts, beneficiary attribution, checkpoint
upgrades/recovery, and WS reconnection/identity rejection.
Cross-chain tests cover source deduplication, rollup/emitter filtering, exclusion
of inbound delivery callbacks, receipt/log attribution, resumable collection,
changed ranges, and a reorg during collection before checkpoint commit.
The database tests are opt-in and run serially against a disposable database
named `eez_indexer_test`: they delete that test
schema, then checks writer exclusion, atomic failure, repeated scans, restart,
multiple events per transaction, reorgs and incompatible configuration.
The reorg check also verifies deletion of associated enrichment.

```bash
INDEXER_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:19433/eez_indexer_test \
  cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml \
  --test recovery --test admission -- --ignored --test-threads=1
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

The L2-fee extension adds bounded, resumable receipt collection for finalized
ranges. All nine indexer tests passed (eight default tests plus the disposable
PostgreSQL recovery test), along with formatting, Clippy, Compose rendering and
the release image build. The latest posting's range 277291–277305 independently
matched L2 receipts: 8878205 wei burned and 1268314993420875 wei in tips. Both
Grafana tables expose the completed totals; older ranges continue backfilling.
These fees include all L2 transactions in the range, including system transactions.

The accompanying Composer fix sizes posting gas before storage refunds. Its
historical replay and unit regression cover the previous low-limit failure;
fee estimates still use post-refund receipt gas. After integrating main
`3f1c8cba`, 218 Composer and 47 node unit tests passed, with only the existing
manual timing profile ignored. Composer/node and indexer Clippy passed. The
indexer/dashboard changes are deployed locally; the Composer gas fix is **not**
yet deployed. Full native E2E and a post-fix live traffic wave remain release
validation, not claims made by these unit and historical-replay checks. Deploy
Composer and proof signer together from the same lockfile as documented in the
[parent monitoring guide](../README.md).

## Posting lifecycle history (optional)

Composer emits versioned `eez.composer.posting.v1` JSON events on tracing target
`eez_posting`. The indexer can replay a dedicated persistent log directory into
PostgreSQL. Composer has no database connection or telemetry disk waits. This
feature does not change settlement triggers, retry decisions or queue ownership.
See the [contract and coverage](../../docs/proposals/posting-lifecycle-history.md).

1. Create a persistent host log directory writable by the node and readable by
   the indexer (container UID 65532). Mount it in Composer at `/var/log/eez-posting`.
   Keep it separate from node databases and credentials.
   The Chiado base Compose file already mounts `POSTING_LOGS_DIR` (default
   `./data/posting-logs`) and supplies the options in step 2. Set that variable
   in the node's `.env` to preserve an existing log location. No node overlay or
   copied startup script is required; the old posting overlay remains compatible.
2. Append these existing Reth options to the Composer command, preserving its
   other arguments and stdout logging:

   ```text
   --log.file.format=json
   --log.file.filter=off,eez_posting=info
   --log.file.directory=/var/log/eez-posting
   --log.file.name=posting.log
   --log.file.max-size=10
   --log.file.max-files=20
   ```

   Reth appends the chain name/ID to the directory. After starting the updated
   Composer, locate `posting.log` below the mount. Use its containing directory
   in step 3. This changes the existing file sink's filter; stdout can retain
   general diagnostics. Do not enable another logger or parse human log messages.
3. Add `"posting_logs_dir": "/var/log/eez-posting/696990"` to the indexer JSON
   config, replacing `696990` with the actual directory from step 2. Set
   `POSTING_LOGS_DIR` to the absolute host log root in `.env.indexer`. Start with
   the optional overlay:

   ```bash
   docker compose --env-file .env --env-file .env.indexer \
     -f compose.yaml -f compose.indexer.yaml -f compose.postings.yaml \
     up --detach --build indexer
   ```

   The overlay mounts the source read-only. The configured directory must exist
   and be readable at startup. No configuration option means no collection worker;
   older configurations continue to work. New tables/view are additive, and the
   existing Grafana read-only role receives access through its default grants.
4. Open **EEZ Composer queue**. **Posting outcomes by target slot** counts distinct
   observed target slots, including slots with recorded preparation failures.
   Categories can overlap: a failed preparation followed by inclusion in one slot
   counts in both columns, and repeated failures in that slot count once.
   Preparation failures cover the proving boundary, not every construction or
   admission deferral. **Why Composer attempted a posting** explains the captured
   reasons and their counts; several reasons can apply to one attempt.
   **Posting submissions** contains one row per recorded
   attempt with its captured reasons, timings, transaction links and chain status.
   Update its `l1_explorer` and `l2_explorer` constants for another network.

`prepared` starts after proving. `repriced` records the final signed hash;
`dispatched` is emitted at the final submission gate, before sending the network
request. `included` is the observer's receipt verdict, `excluded` its conclusive
slot verdict, and `uncertain` an observer error. `preflight_failed` was never sent. New events retain the exact target from
preparation onward. `preparation_failed` records failure at the proof gate or
prover, with no signed transaction hash and a distinct attempt ID. Its preparation
duration covers construction through reaching the proof gate, excluding the failed
prover call; `proof_gate_ms` is when the deadline was checked, not proof start.
Earlier encoding/admission errors are still ordinary local logs.

**Posting timing & outcomes** and **Posting submissions** distinguish local deferrals,
preparation/prover failures, skipped network timestamps proven by adjacent canonical
headers, produced target blocks without observed inclusion, and successful inclusion.
Missing canonical history stays unknown. An omitted transaction does not reveal
whether the proposer used another builder, disabled the relay, or had connectivity
problems; those reasons require infrastructure evidence. Public Chiado logs are
investigation evidence only, never a runtime dependency.
An attempt ID survives repricing; its posting ID is the exact prepared Sync hash.
Recomposition can produce a different hash even at the same height, so rows are
not automatically combined into one retry chain. Reasons can be unavailable if
there was no applicable canonical policy observation for that target.

Durations use a monotonic clock within each attempt; wall-clock event timestamps
are retained separately. The `Canonical commitment` status requires a match in
the existing canonical index, and can disappear on reorg. It is not a claim that
all cross-chain effects completed. Off-chain observations survive reorgs. Before
chain catch-up, an observed receipt may not yet match the index.

**Posting timing & outcomes** refreshes every five seconds. Its colored outcome
markers use an independent hidden scale near the top (not counts) and the same expected
timestamp as the timing points. Hover the event markers below the plot for the
attempt ID, expected L1 block, L2 sync block, outcome, reason and transaction hash.
Retries remain distinct; simultaneous attempts can overlap visually but retain
separate annotation details. Attempts lacking target timestamps remain in the
submission table. Each
attempt has separate, unconnected prepared, dispatch, canonical-inclusion and receipt-observed
points relative to its captured target timestamp (zero). A negative dispatch offset
means it was sent before the target time. Prepared marks the recorded submission
after proving and before final repricing, not the start of proving. A missing
prepared event stays absent; the first available later event is never substituted.
The table includes expected L1 block/time,
actual canonical inclusion time, block difference (actual minus target), target
result and receipt observation delay. Compare block heights to determine whether
we hit the expected block: wall-clock observation delay does not answer that question.
Receipt observation delay is local event time minus the block-header timestamp;
it includes block propagation/observation and depends on host clock synchronization.
Receipt timing is hidden if the observed hash differs from canonical inclusion.
Missing target or canonical data stays unavailable, including after reorgs.
This reuses retained dispatch events and indexed block headers; no new Composer
instrumentation, RPC polling, worker or configuration is required. Apply the updated
`posting_timing` view before provisioning the dashboard. Existing rows gain timing
without replay or resetting checkpoints; the separate view allows earlier indexer binaries to restart unchanged.

The reader checks every two seconds, up to 256 KiB per retained file per pass,
with 64 KiB lines and at most 128 files. It imports complete lines only. Event IDs
and file offsets commit in the same transaction; replay, rename rotation and
indexer restarts do not duplicate events. Conflicting IDs, malformed events and
wrong network/rollup inputs retain the checkpoint and expose a collection error.
File reads run outside the async executor, separately from canonical indexing.

### Recovery and limits

Retain the log volume across Composer/indexer restarts. Size-based rename rotation
is supported; do not use `copytruncate`. Missing sequence numbers appear as
`Missing events` until retained logs fill the gap. Collection errors/staleness
appear in the History column of existing rows. If there are no rows, inspect:

```sql
SELECT checked_at, error FROM eez_indexer.posting_collection;
SELECT count(*) FROM eez_indexer.posting_events;
```

For a damaged/truncated source, stop the indexer, preserve the affected files,
repair the source/mount, and delete only its `posting_files` checkpoint to replay.
Never delete canonical tables or the node's data to repair telemetry. Identical
replays are safe; conflicting duplicates require investigation, not overwriting.
Keep imported logs outside the watched directory when archiving them.

Reth's nonblocking logger is not a transactional outbox: abrupt termination,
queue overflow, disk errors or removal of unread rotated files can lose events.
`Observed sequence intact` means the recorded sequence through its latest stage
has no gaps; it cannot prove no later event or whole attempt was lost. On-chain
facts can be backfilled, but missing off-chain reasons cannot. This deliberate
boundary preserves posting availability and avoids adding a new Composer storage
system. An indexer restart can recover all **retained, complete** source events.

To disable collection, remove `posting_logs_dir` and the optional mount, then
restart only the indexer. Keep the stored history. When rolling back to an older
indexer, remove the new option first because unknown fields are rejected. An old
Composer produces no new lifecycle history; existing queue metrics still work.

### Diagnostic and early-enrichment upgrade (September 2026)

Upgrade the indexer before the Composer: older indexers reject the new
`preparation_failed` stage and would stop advancing the affected file checkpoint.
Existing events without early target fields remain readable. Schema changes are
additive. Keep the updated indexer when rolling Composer back; a collector rollback
needs posting ingestion disabled or an explicit replay plan for retained new-stage
logs. Never delete retained logs or checkpoints to conceal parser failures.

Early canonical enrichment uses the existing details schema/checkpoints. Exact-range
caches survive state promotion; old pending rows without a checkpoint start from
zero. No rescan of completed finalized history or database reset is necessary.
For rollback, use the matching older dashboard and indexer together: the old worker
will defer due canonical rows again. Already stored observations remain in PostgreSQL.

The active deployment also retains the prior operator indexer fixes: canonical
block-receipt collection, an optional `details.l1_http_url` for transaction
enrichment, and the transaction-hash lookup index used by posting queries.
These changes preserve the event attribution checks and the new event ABI.


### Admission history

The optional posting collector also imports `eez.composer.admission.v1` into
`admission_events`, under the same transactional file checkpoint. Upgrade this
collector before the new Composer. The additive `admission_candidates` and
`admission_sources` views power **EEZ Cross-chain admission** (`eez-admission`).
The existing PostgreSQL datasource and read-only Grafana role are reused.

The dashboard shows all candidates, including successful no-cut batches;
per-source simulation, pin/discount/calldata and companion gas; full-calldata
projection; final preflight gas used/required; signed limit; and canonical
postBatch receipt gas. Filter by candidate ID or source hash and use
**Inspect → Data → Download CSV**. Rebuilt candidates have separate IDs and an
explicit predecessor. Repricing joins through the posting's final signed hash.
An orphaned receipt disappears from the canonical join while observations remain.
Source preflight and indexed canonical source receipt gas are separate columns.
New `source_preflight` records identify `execution_chain_id` and `preflight_basis`:
`native_l1_companion` measures the L1 companion; `native_l2_terminal_receipt`
measures an outbound source in the final candidate's L2 block. The latter reports
charged gas and the signed limit; its pre-refund required gas is null. It is
still speculative execution, not canonical L2 fee collection. Historical records
without these fields retain their original L1-only meaning. No exact per-source
share of the aggregate canonical postBatch receipt is claimed.

Final `preflight` admission and `repriced` posting details also carry an optional
`fee_observation` object (version 1 measurements; version 2 also supports shadow
pricing). Its `l1` component prices the complete final
postBatch's execution and physical blobs in L1 native base units, with the
fee-cap balance reservation reported separately. The `l2` component records
ordinary/native gas, collected base fees and gross tips at the terminal Sync block's
beneficiary in L2 native base units. Exact block/parent/target and transaction
identities bind the observation; the L1 cost includes shared historical DA,
while L2 historical-block fees are outside the reported scope. Per-source L2
fee details contain the signed sender and effective priority price. A zero
beneficiary remains visible and is not assumed to be controlled.
The reservation field describes the signed transaction's up-front affordability
bound; it does not create a durable financial reservation.

Missing or inconsistent evidence is `unavailable`, not zero. Without a pricing
policy, `pricing_status: unpriced` retains null recovery/tip fields. Version 2
can report `shadow_priced` under explicit
[operator assumptions](../../docs/configurable-settlement-policy.md#cross-chain-fee-configuration):
full cost, eligible/excluded tips, projected recovery and shortfall/surplus.
With simplified fee pricing, `comparison` and its shortfall/surplus still refer
to the desired profit target. `minimum_service_payment_l1`,
`minimum_service_payment_l2` and `cost_coverage` separately describe the enforced
cost-plus-risk floor. A target shortfall can therefore coexist with covered
costs; it is not by itself a settlement funding failure. Historical observations
can omit these added fields.
A minimum tip is available only for one eligible L2 source without companions;
it uses charged receipt gas and current execution, and is not a wallet quote.
Qualified revenue remains null in either mode. Proving/operations and conversion
are configured assumptions, not verified expenses or treasury redemption.
Gross tips may include operator-funded sources; observations and treasury
liquidity must not be equated. See the [shadow fee contract](../../docs/proposals/cross-chain-fee-recovery.md#third-development-increment-shadow-pricing-policy).
These optional details use existing event kinds and are retained in JSON by
current collectors, without a database migration. Older Composer records and
rollbacks simply omit the fields. Canonical L2 fees remain the separate
receipt-backed enrichment; log records do not authorize financial recovery.

Selected counts describe the finite snapshot. `selection_limit` is its effective
count cap; `selection_limit_reason` identifies an active configured, fairness,
submission, deadline, or preparation-probe cap. `admission_stop_reason` reports
the first resource cut or the count cap actually reached, and distinguishes
snapshot exhaustion from exhaustion of eligible sources. It remains separate
from a later proof/preflight failure (`outcome_reason` in Grafana). Old records
without these fields show **Unknown (legacy or incomplete)**, not an empty
queue. Queued counts exclude claimed
sources reused by a rebuild. Only sources reaching individual
checks receive source records; untouched and selection-filtered work is counted
in each summary. This keeps repeated backlog logging bounded by examined work.
The source timing column measures checks through the gas estimate, before append
and finalization. Historical records without that explicit boundary stay null.

Preparation, proving, native finalization, relay acknowledgement and dispatch
headroom are distinct observations. The timing table also shows the proof
prediction, its configured/covering/scaled basis and finalization reserve next
to measured durations. Historical predictions remain null. A relay
acknowledgement is not settlement.
The timing table also exposes prefix hashing, its executed transaction count,
blob sidecar preparation and witness preparation. These are subintervals of
preparation. Witness time includes blocking-worker scheduling. Older events
leave these columns null; the dashboard reads optional details from existing
event kinds, so this timing addition needs no collector or schema migration.
Sequence gaps are explicit; `finished` records the expected final sequence.
Missing historical estimates and candidates that never reached simulation remain
null. Check collector status and its last successful check before interpreting
coverage. Preserve retained JSON logs and existing database checkpoints across
restart. Collectors predating the whole admission family ignore it; an
admission-aware collector rejects unknown event kinds. Retain the upgraded
collector and additive tables/views on Composer rollback. Monitoring is operational history, not durable queue ownership.

Apply the additive views before loading the updated dashboard and Composer.
Keep the upgraded indexer when rolling back Composer: PostgreSQL rejects an old
`CREATE OR REPLACE VIEW` that would remove appended columns. Source histories
from `bounded-calls-2` expose entry reserve, L1 call count and nested result count;
the target simulation term separately covers execution. The entry reserve is
the proposed group's shape cost before any batch-size discount revocation.
For inbound sources, the call and result counts describe metadata stored by
postBatch; executing their companion transactions remains a separate charge.

From `deferred-storage-3`, `estimate_reasons` counts entries by pricing decision:
`deferred_storage`, `deferred_legacy`, `immediate_registered`, or a fallback
(`parent_state_unavailable`, `immediate_evidence_unavailable`, `deferred_old_slot`,
`deferred_reverted`, `deferred_shape`, `deferred_size_bound`,
`deferred_reverted_or_static_row`, `deferred_call_flags`, `general_reserve_lower`).
The immediate evidence reason includes unsupported execution and missing proxy
registration; it does not identify a specific failed eligibility check.
`deferred_old_slot` means the canonical-parent layout could not be bounded.
`general_reserve_lower` keeps the existing reserve when the new storage estimate
would be larger. Counts sum to the source's entry count. These explain proposed
per-entry pricing; `batch_discount_revoked` separately records when the proposed
prefix exceeds the measured 128-total-entry bound, restoring general reserves
for the entire prefix. A rejected group does not change accepted prefix pricing.
Both columns remain null for older events; do not infer a reason from missing
history. Upgrade the additive views/collector/dashboard before Composer and
keep them on runtime rollback, as described above.

From `companion-storage-4`, bounded reverted/flagged storage and results up to
256 bytes can use `deferred_storage`. Historical `deferred_reverted` and
`deferred_reverted_or_static_row` reasons remain readable; new events do not emit
them. `deferred_call_flags` now denotes unsupported value/routing for storage.
`companion_simulation_gas` remains the raw source simulation. New columns show
`companion_protocol_reserve_gas`, `companion_estimate_gas` (the amount used in the
bundle budget), and `companion_estimate_basis`: `bounded_protocol`,
`bounded_capped`, or `declared_limit`. A declared-limit fallback has no measured
protocol reserve. Candidate summary `companion_gas` now totals these consumption
estimates. Old estimates remain null in the new columns.

From `companion-execution-5`, `companion_target_simulation_gas` shows separately
measured L1 callback application gas, including reverted frames. Bounded estimates
sum source simulation, callback simulation and protocol reserve, capped at the
signed limit. Earlier rows retain null callback gas; do not infer zero. See the
[versioned admission profile](../../docs/configurable-settlement-policy.md) for
shape bounds and fallbacks.

Policy `packing-fifo-preference-2` continues past nonfitting independent groups.
The candidate table exposes `capacity_deferrals` counts per reason and the
`fifo_bypassed` diagnostic. Policy `strict-fifo-5` enforces FIFO across submitted
batches: `fifo_bypassed=true` describes discarded speculation, with stop reason
`fifo_prefix_rebuild` and a replacement linked by `rebuild_of`. A retained queue
boundary appears as `fifo_prefix_limit` on the next opportunity. Resource reasons
remain in `fifo_trigger` and `capacity_deferrals`; no learned FIFO count cap is
installed. Policy `strict-fifo-6` additionally clears that boundary after canonical
resolution of its first source, allowing the remaining queue to share the next
batch under the same FIFO checks. Receipt or relay observations do not clear it.
The event schema and collector remain compatible; version 5 histories retain
their original boundary behavior. `strict-fifo-7` superseded sparse proof scaling
when a slower, wider observation supplied the requested dimensions.
`strict-fifo-8` also accepts a faster wider observation for that comparison,
retaining every sample in the extrapolation's slow-duration floor. The existing
`proof_estimate_ms` and `scaled_sample` basis expose that prediction. Actual
deadlines are unchanged; no collector migration is needed. Source rows expose
`source_outcome_reason`, including a blob rejection after a successful gas check.
The final candidate stop reason and earlier individual deferrals can differ.
Old histories retain null fields and historical `fifo_fairness_limit` reasons.
Apply the additive schema and dashboard before the new Composer, keep retained
logs/checkpoints, and retain these views on rollback. No historical follower
encoding, node storage, signer profile or configuration input changes.

Policy `packing-minimum-check-3` adds `source_capacity_check` for capacity
refusals before full simulation. The basis `minimum_admission_cost` denotes a
lower bound on the current admission estimator, not measured gas. Its event
contains the minimum entry/L2 costs, accepted postBatch and companion prefix,
signed companion requirement, fixed drain reserve and active caps. Source rows
show **Capacity deferred before simulation**, the bound and constraint; full
simulation estimates remain null. `capacity_check_ms` measures selection through
this cheap decision. `source_checks_and_simulation_ms` remains null for that row.
Candidate `minimum_check_count` counts these refusals; `simulation_count` counts
actual source composition executions, including rejected/failed executions.
Neither count denotes unique sources across candidates. A rebuilt candidate has
its own counters and state.

Estimator `companion-storage-6` distinguishes zero, retained and fresh storage
writes and uses tighter measured protocol framing reserves. The separately
measured source and callback execution terms and declared-limit fallback remain.
See the [settlement specification](../../docs/configurable-settlement-policy.md)
for prices and shape bounds. Upgrade schema, collector and dashboard before
Composer: an older admission-aware collector rejects the new event kind and
cannot advance its checkpoint past it. Keep the upgraded collector and appended
view columns on Composer rollback. Retain the JSON volume and checkpoints;
no historical events are rewritten and missing estimates are not reconstructed.

Policy `confirmed-failure-backoff-4` adds optional `submission_failure_streak`
and `submission_backoff_threshold` to candidate `started` details. Grafana's
preparation table reads them directly; historical and numeric-admission values
remain null. The fifth consecutive qualifying failure first reduces selection;
complete canonical source settlement resets the streak. This is distinct from
per-source cumulative retry accounting. Estimator `companion-reserve-7` lowers
bounded execution framing reserves while retaining version 6's posting storage
model. Existing gas columns expose the changed values. These versions add no
new event kind or schema; existing collectors accept them. Upgrade the dashboard
for the new columns, preserve logs/checkpoints and keep it on Composer rollback.

The optional details worker also enriches accepted inbound sources independently
of registry events. It checks source receipts against the canonical posting's
block and transaction order, then rechecks the block before saving. This fills
receipts for calls whose registry logs reverted. `source_receipt_coverage` is
`available`, `absent`, or `before_postbatch`; null means not collected or not
applicable. Included sources with unavailable or mismatched RPC receipts retry;
a null RPC response never establishes absence. `source_receipt_success` is only
the receipt's outer status, not evidence that the protocol call succeeded.
These observations do not allocate postBatch gas to a source or complete queue
ownership. A source must be after the posting in the same canonical block;
its receipt in another attempt/block cannot fill this row.

Collection uses at most 32 sources per round with four concurrent RPC sequences
under the existing details worker. Cancellation drops pending requests. The
pending-work query starts from canonical posting links and reads source events
only for those candidates; it avoids repeatedly aggregating the full dashboard
views as retained history grows. Direction resolution still uses the latest
estimate, falling back to the earliest source-selection event. This query-only
change needs no schema migration or node upgrade; rollback preserves observations
but can restore the old timeout under a large retained history. The
separate `admission_source_receipts` table references the canonical posting;
reorg deletion removes observations and rejects late stale inserts. It does
not add no-log sources to settlement cost totals. Historical accepted sources
are enriched as the worker catches up, newest blocks first. Deploy the additive
schema/collector before the dashboard and Composer, preserve the existing log
checkpoints, and retain this indexer when rolling Composer back. No queue data
or protocol storage migration is involved.

Composer emits bounded-label `eez_composer_admission_events_total` and
`eez_composer_admission_duration_seconds`; hashes and candidate IDs only occur in
the retained event records. Logs use the existing nonblocking tracing sink.
Malformed/conflicting records stop checkpoint advancement and surface collector
errors. The [initial validation audit](../../docs/audits/2026-09-26-admission-throughput/README.md)
and [selection/shape follow-up](../../docs/audits/2026-09-26-admission-shapes/README.md)
record SQL/replay checks and live coverage.

Estimator `companion-input-8` replaces the maximum input-word allowance for
validated nested companion calls with their measured lengths, retaining a
four-word minimum for repeated table copying. Protocol reserve and total
companion estimate remain available in the source drill-down.

Estimator `candidate-framing-9` adds a per-candidate framing reserve, complete
admission estimate, bounded calldata gas, active calldata-floor flag, general
execution fallback flag and gas-margin percentage to the postBatch table.
Source drilldowns show each group's framing contribution and accumulated framing/
full estimates before and after the gas gate. After-values are provisional until
the source outcome confirms acceptance; later blob/timing checks can defer it.
`admission_gas` remains the original
incremental projection; compare `admission_including_framing_gas` with the full
planning cap for versions 9 and later. Historical rows keep null values for the new fields.
Queries read these additive details from retained events; no schema migration
or collector rebuild is needed. Static-only sources have a zero mutable-entry
minimum in cheap capacity checks, while their full estimates still price the
actual calldata and general execution projection. Version 10 requires both the
mutable minimum and static-only/general-pin alternatives to fail before a cheap
rejection. `minimum_mutable_entry_gas` and `minimum_static_general_fallback`
make that distinction visible without inventing simulation results.

The preparation/proof/delivery table also exposes relay bundle hash (when
returned as a valid 32-byte hash), target block/timestamp, expected parent,
transaction count and `ack_remaining_ms`. The latter measures the shared
delivery window remaining at acknowledgement, whereas dispatch remaining time
is recorded before the HTTP request. These additive JSON details require no
database migration; legacy rows remain null. A null relay identifier may also
mean a successful relay returned no identifier. Correlate by postBatch hash
and exact target in that case. These fields do not establish builder rejection
causes or canonical settlement. Dashboard provisioning can be updated before
or after the Composer; old collectors retain the existing delivery event.
With several configured relays, these fields describe the first
acknowledgement and the event adds a `builders` array of per-relay outcomes
(position, host, outcome class, identifier, elapsed time); the collector stores
it in the retained event JSON without a schema change. See the
[fan-out contract](../../docs/configurable-settlement-policy.md#builder-relay-fan-out).
