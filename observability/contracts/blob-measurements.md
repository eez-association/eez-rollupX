# Authenticated canonical blob measurements (schema 12)

This extends the [canonical indexer](canonical-indexer.md). Blob availability
never controls receipt indexing, fee arithmetic, canonical cursor advancement or
source outcome classification. Measurement requires a known configured-contract,
current-ABI L1 posting receipt of type 3. Reverted posts remain in that paid
population; a successful receipt alone does not prove that its effects applied.

## Configuration and lifecycle

Existing indexer configurations leave collection disabled. An omitted or null
`blobs` value produces no Beacon requests and no fabricated measurements. Enable
explicitly in the private indexer JSON:

```json
"blobs": {"beacon_url": "http://127.0.0.1:5052"}
```

A nonempty configured `l1.contracts` selection is required. The URL must be HTTP(S)
without a fragment; a base path, query credentials and HTTP authentication survive
request construction. The config's debug representation and diagnostic errors
exclude the URL. No credentials, hostnames, blob bodies or calldata are exported
as labels or stored as monitoring metadata. Receipt selection, bootstrap range,
chain ID and genesis identity remain the indexer's existing configuration.

The optional worker has a dedicated database connection and independent lifecycle.
In continuous mode it waits five seconds after a successful pass; database/worker
failures use the existing capped 15/30/60-second backoff. A pass has a 30-second
outer deadline and at most three new posts plus one persisted retry, with the
retry serviced first. Individual unavailable/pruned/invalid measurements retain a
closed state and rotate through that retry lane without stalling new work.
`--once` runs one bounded pass and retains the original receipt-scan exit semantics;
inspect blob states/health rather than treating a zero exit as complete coverage.

HTTP requests share the observer's four-request semaphore, five-second timeout
and no-redirect policy. Beacon timing responses have a 1 MiB cap; full-blob
responses have a 20 MiB cap and at most 64 physical blobs. These are resource
budgets, **not active-fork allowances or protocol limits**. Larger supported future
networks produce an explicit limit gap pending a reviewed budget change. KZG and
codec work uses one process-wide blocking-worker permit. Cancellation cannot
release that permit until the blocking work exits, so timeouts do not accumulate
unbounded background crypto tasks. The existing 10-second database statement
budget bounds each query.

## Authentication and exact length

For each unmeasured post, independently fetch its transaction and verify the known
inclusion hash/height/position, immutable sender/recipient/nonce/type/selector and
calldata hash. Preserve its ordered versioned hashes, original blob indices and
calldata-tail hash. If a known receipt's purchased-blob count contradicts complete
transaction metadata, fail explicitly through the existing immutable-fact/freeze
path. Missing fee fields do not fabricate a purchased count or prevent retaining
otherwise authenticated bytes.

Use Beacon genesis time and seconds per slot to locate the execution timestamp;
reject a zero interval, timestamp before genesis or nonaligned timestamp. Fetch
`eth/v1/beacon/blobs/{slot}` first. Only 404/405 permits the existing pre-Fulu
`eth/v1/beacon/blob_sidecars/{slot}` fallback. HTTP 410 is an explicit `pruned`
observation. A 404 without a successful fallback is `unavailable`: it cannot
reliably distinguish historical pruning from an unknown slot. Other transport or
HTTP failures do not silently switch endpoints.

Recompute KZG commitments/versioned hashes from the physical bodies using the
existing `eez-blob-codec` transport helper and match the execution transaction's
ordered versioned hashes. Full-block responses can contain other applications'
blobs: the physical KZG helper accepts all valid EIP-4844 field scalars, and EEZ
packing/stream rules apply only after selecting the expected transaction hashes.
Legacy responses must additionally agree with their advertised commitments. The bytes' authenticity comes from the expected EL
content hashes; a response from a different Beacon endpoint cannot substitute
another body. No provider-advertised byte length is trusted.

The supported deployed carrier selects all purchased blobs exactly once in their
transaction order. Other blob-index profiles retain metadata with an explicit
`unsupported_carrier` measurement. A valid calldata tail is supported and checked,
but its bytes are excluded from blob utilization. The shared version-00 decoder
validates field packing, brackets, message fields, terminator and zero padding.
`validated_blob_len` returns the original byte offset immediately after
`CloseBlobStream`, including version and closing marker. It validates the whole
stream and tail without retaining the decoded message list. This preserves legal
nonminimal varints (pinned blob spec §5.8), which a re-encoding could shorten;
trimming zeros would also miscount real zero-valued payload bytes.

The new-network derivable operations format uses inner tag 3/profile 1; it does
not change this outer version-00 stream, KZG authentication, 126,976-byte purchased
blob capacity, or receipt charge arithmetic. The measurement's `codec_version=0`
refers to that outer stream. Inner operation bytes are counted as opaque payload:
measuring them neither validates the derivation recipe nor authorizes settlement.
The node/signer's inner decoder and execution checks retain that responsibility.
Historical measured content and costs are preserved; this distinction does not
provide a legacy-network replay or mixed-format migration path.

The collector rechecks EL identity, cursor and exact inclusion hash after retrieval.
It locks the current scan cursor and requires that inclusion to remain canonical
before atomically committing metadata, measurement and attempt state. It does not
insert historical blocks or extend the indexed interval. An interrupted pass can
leave an `interrupted` attempt; retry uses the same unique keys. Metadata and
measurements have immutable SQL guards. Once measured, a transaction's bodies are
not fetched again. Concurrent duplicate workers cannot multiply measurements or
fees; an unacknowledged commit can cause redundant network work, not a second
logical measurement.

## SQL populations and metrics

- `blob_metadata`: immutable chain/transaction identity, calldata hash, ordered
  versioned hashes and indices, calldata-tail content hash. These identifiers
  support later independent re-fetch; the monitoring database does not archive DA.
- `blob_attempts`: bounded-state latest lookup diagnostics and attempt time.
  `retrieval_duration_ns` is monotonic duration of the latest actual Beacon and
  authentication attempt, including timing lookup, body retrieval, CPU-permit wait
  and KZG/codec validation. It is null if that work never began or was interrupted.
- `blob_measurements`: authenticated version, original encoded byte length,
  purchased count, versioned usable capacity, retrieval route and measurement time.
- `posting_blob_measurements`: canonical posting receipt population plus metadata,
  measurement and gap state. `source_valid` must gate current-state consumers;
  provenance is `unknown` while the indexed source is stale, failed or frozen.
  `has_applied_settlement` requires existing independently checked canonical applied
  L2 range evidence. It does not assert all bytes or every source applied.
- `posting_blob_totals`: exact sums, reverted-post count, posts with applied
  settlement evidence, measured subset and coverage. Utilization is
  `sum(measured encoded bytes) / sum(purchased blobs for those same posts × 126976)`.
  Unknown receipt counts exclude their measurements from both sides of this ratio;
  `receipt_count_unknown` remains visible. Empty/zero denominators are null, never
  fabricated zero. The physical blob size is 131072; neither size is an allowance.

Unknown, not measured, unsupported carrier/codec, invalid stream/response,
authentication failure, metadata unavailable, unavailable, pruned, interrupted and
resource-limit states remain distinct. Event-only composer measurements are not
promoted or mixed into this authenticated population. Active allowance remains
separately recorded native attempt data; no fixed six-blob limit is assumed.

`eez_observer_blob_` metrics expose worker success/last-success/error, bounded
per-state post counts, measured-subset utilization and measurement coverage.
`retrieval_last_duration_seconds` is a last-sample gauge paired with
`retrieval_last_attempt_timestamp_seconds`, not a histogram or a claim that a
later empty poll performed a new retrieval. Failures or snapshots older than
90 seconds hide data gauges. Exact accounting stays in SQL. Reorgs immediately
remove orphaned receipts from these views while retaining content facts; reinclusion
reuses one measurement and presents only the new canonical receipt charge.

## Compatibility and validation

Use stop/backup/explicit `migrate`/start. Schema 12 adds tables/views without changing
existing receipt selection or fees. Older binaries refuse its schema/checksum.
Rollback requires a compatible binary or separately restored backup. Historical
posts begin `not_measured` and enter the bounded worker only when enabled; absent
archive bytes stay unknown. Disabling the worker retains known facts.

| Requirement | Implementation | Verification |
| --- | --- | --- |
| R12 exact authenticated length | Existing KZG helpers plus shared count-only decoder, ordered metadata | Codec nonminimal/zero/cross-blob/tail/version tests and independent Ethereum KZG oracle; observer auth/legacy tests |
| R12 measured-subset weighting and coverage | `posting_blob_measurements`/`posting_blob_totals` | Real RPC/Beacon fixture with one- and two-blob posts, partial coverage and receipt gaps |
| R12 missing/pruned data never blocks fees | Independent worker with bounded gap states and retry lane | 404/410/authentication/metadata recovery, unchanged exact fees and reverted population |
| R09/R12 reorg/restart/cost uniqueness | Current canonical views plus immutable content keys | Reorg/reinclusion, source invalidity, restart/no-refetch, writer-guard tests |
| R01/R02 safe optional configuration and bounded work | Explicit disabled default, URL redaction, shared semaphore/byte/time/CPU limits | Config absent/invalid/valid tests; HTTP size/pruning tests; no Beacon requests when disabled; message-retention stress test |

See [the validation record](../validation/blob-measurements.md). Production archive
coverage and workload/resource budgets still require deployment calibration; these
fixture checks are not a claim that a particular live Beacon retains old blobs.
