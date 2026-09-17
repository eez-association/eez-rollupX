# Lifecycle transport v1

The executable schema is [`eez-telemetry::schema`](../../crates/eez-telemetry/src/schema.rs).
Serde rejects unknown fields and unknown variants. Schema version is `1`; an
observer must retain unknown/invalid records with their original segment/offset
for quarantine and operator-directed replay, never interpret them as success.
The transport and [PostgreSQL ingestion](lifecycle-ingestion.md) have isolated
recovery tests; [native capture](native-lifecycle.md) covers admission,
replacement, readiness, attempt membership, repricing and dispatch observations.
It also captures post-batch preparation before signed-attempt registration.
Remaining transition wiring and canonical outcome validation remain
open in the [requirement ledger](../requirements.md).

## Identity and records

Every JSONL record has `record`, `version`, `identity` and `epoch`. Identity contains
`environment`, `deployment`, `service`, `instance`, each 1–64 ASCII letters,
digits, dots, underscores or hyphens, matching the deployment target identity.
They are diagnostic fields, not application-added metric labels.

Epochs and segment IDs are independent random 128-bit values rendered as 32
lowercase hex digits. Hashes are 64 lowercase hex digits. Neither uses an `0x`
prefix. UTC times are integer Unix milliseconds. Process ID is diagnostic only;
PID reuse does not prove writer identity. All sequences start at 1 and never wrap.

| Record | Fields and interpretation |
| --- | --- |
| `header` | `segment_id`, `process_id`, `started_at_unix_ms`, `segment_created_at_unix_ms`. First record of each segment; present before any request arrives |
| `data` | `sequence`, `chunk_index`, `chunk_count`, `payload_hex`, `complete`, `digest`. A logical event may span multiple data records |
| `control` | Separate `control_sequence`, `observed_at_unix_ms`, `status`, `stats`. Written directly, bypassing the data channel |

The raw data identity is `(deployment, service, instance, epoch, sequence,
chunk_index)`. Environment also remains available for deployment routing. Capture
one move-only ticket at the committed state transition under existing ownership;
serialize and publish after releasing protocol locks. A ticket allocated for an
admission is its admission ID `(epoch, sequence)`. Capture/publication can reorder;
a later sequence does not prove an earlier event was lost. An unused ticket can
leave a hole, including an unknown trailing crash window.

The complete logical payload is UTF-8 JSON containing `observed_at_unix_ms`,
optional `trace` (`trace_id`, `span_id`) and a typed `event`. Encode it once, divide
its bytes into at most 6 KiB fragments, and hex-encode each fragment. Every encoded
line, including newline and metadata, is at most 16 KiB. The last chunk alone has
`complete=true` and `digest=SHA256(concatenated original payload bytes)`. Other
chunks have `complete=false`, `digest=null`. An event is accepted only after all
chunks agree on scope/version/count, every index is present, and digest matches.
Byte-identical duplicates are harmless; conflicting duplicates are quarantined.
An 8 MiB logical-payload cap bounds serialization and reassembly memory. Oversized
events are a counted encoding loss; they never change the protocol decision.

## Typed events

| Event | Meaning and identity |
| --- | --- |
| `admitted` | Source identity, optional previous admission for explicit readmission, optional replaced admission. Repeated ingress of an already owned hash creates no admission |
| `readiness` | Admission/source member, exact L1/L2 source-state block references, optional blocking reason (`nonce`, `fee`, `balance`, `escrow`); null means ready |
| `preparation_started` | L2 chain/genesis/rollup, proposed Sync block and candidate members. The record's epoch/sequence is its distinct preparation ID |
| `preparation_finished` | Preparation ID, monotonic elapsed nanoseconds and outcome: `prepared`, `construction_failed`, `capacity_limited`, `prover_failed`, `validation_rejected`, `cancelled` or `panicked` |
| `attempt_prepared` | Existing native generation+post hash wrapped in epoch, sync height, exact member list, candidate encoded bytes/blob count and active blob allowance; optional detailed manifest evidence described below |
| `attempt_repriced` | Explicit previous/replacement attempt identities; preserves native generation while a never-dispatched candidate's signed hash changes |
| `dispatched` | Attempt identity, target block number/timestamp, signed transaction hash and nonce. The future target has no known block hash. Each event is a dispatch occurrence, not another paid transaction |
| `submission_observed` | Exact attempt with prepared/included/excluded/uncertain node observation. Included carries a reported receipt block; excluded preserves the existing charge-attempt flag. Neither establishes canonical source completion. |
| `reconciled` | Attempt identity and canonical L1 anchor reported by the node |
| `terminal` | Admission/source member, optional attempt, native outcome, claimed assurance, optional anchor, local monotonic elapsed nanoseconds |
| `reorg` | Last common L1 ancestor observed by the node |

Sources contain chain ID, genesis hash, rollup ID, transaction hash and direction.
Native attempts keep their existing `generation` and `post_batch_hash`; telemetry
does not allocate protocol ownership. Admissions distinguish a readmitted hash
from an earlier admission. Many sources may belong to an attempt, and a source
may participate in multiple attempts. Relationships never authorize duplicate
receipt charging.

`attempt_prepared.details`, when present, carries L2 chain/genesis/rollup identity,
the proposed Sync hash, signing nonce, complete effect endpoint hashes and plans
parallel to `members`. Each plan carries source sender (20-byte lowercase hex),
nonce, already charged failure episodes, optional outbound group, half-open
producing-effect interval and source-action hash/index pairs. No signed envelope
is present. Native capture bounds the complete copy before allocation; the
transport's payload/chunk limits still apply after encoding.
Its optional `preparation` links the separate pre-signing operation; omission in
older records means unknown. Successful preparation means the candidate was built,
not that native ownership was registered or a transaction was sent. Candidate
memberships in the start and signed-attempt events must agree before a consumer
claims that relationship. A missing start or finish is incomplete history.

The v1 extension preserves decoding of existing events: omitted `details` remains
unknown and serialization omits it when absent. New variants and populated details
require an updated observer. Upgrade the observer before enabling the new native
writer. Older strict readers quarantine unfamiliar payloads with retained raw
positions; upgrading does not automatically reinterpret those quarantined records.
They require explicit operator-directed replay. SQL transport storage needs no
migration for these additive payload fields. Lack of details never becomes an
empty verified manifest.
The same additive compatibility rule applies to optional preparation IDs and the
new preparation variants; older strict readers quarantine unfamiliar fields/events.

Outcomes preserve `committed`, `source_reverted`,
`source_completed_with_missing_effects`, `source_completed`, `source_nonce_spent`,
`attempts_exhausted`, `nonce_gap`, `pool_discarded`, and local `replaced` history.
Assurance is `receipt_verified`, `node_reconciled`, `local_policy`, or `unknown`.
An emitter's assurance is a claim: the observer validates the required canonical
anchors and the proposal's per-direction evidence matrix before exposing a
verified/reconciled view. A receipt fact does not prove cross-chain effects.
`unresolved_restart` belongs to observer history, not the native terminal enum.
No event accepts raw signed envelopes, keys, arbitrary error strings or baggage.

## Delivery, storage and liveness

This is unsampled, best-effort telemetry, independent of `RUST_LOG` and traces.
It is not an admission journal and never acknowledges durable protocol state.
`publish` uses a nonblocking bounded queue. Default limits are 4096 records,
64 MiB segments, 24 hours and 2 GiB retained segment files. Configurations may
reduce those limits, but must fit two segments; absolute spool path is required.
Reserve a separate physical 3 GiB filesystem budget because open-unlinked files,
metadata and crashed-writer files are not bounded by visible retained bytes alone.

The writer holds exclusive OS locks on `instance.lock` and `<epoch>.lock` for
its lifetime. The directory is dedicated to one configured process identity.
A collision is an explicit exporter-start error; application wiring must disable
and report the conflicting exporter while preserving settlement. Files are
created with mode 0640 and directories with 0750, subject to the process umask.
Provision the operator-managed read-only group and directory traversal permissions
before launch. Group-writable or publicly accessible spool directories are rejected.

Segment paths are `<epoch>-<segment_id>.active`, renamed to `.closed` on rotation.
The header ID remains unchanged. A partial write is never followed by more data
in that segment; after a five-second I/O backoff the writer opens a new ID. Closed
and abandoned segments expire by age and bytes; rotation reserves active-segment
headroom. The observer must close descriptors after each bounded read pass.
Alloy needs explicit patterns for both suffixes; the diagnostic `*.jsonl` pattern
alone does not collect this spool.

Controls are written at startup, close, and every five seconds while the writer
can perform I/O. `stats` reports cumulative attempted/dropped/written data records,
highest allocated logical sequence, dropped sequence min/max, and I/O errors.
These atomic counters are a concurrent observation, not a transactional snapshot.
Drop bounds describe uncertainty: they do not say every enclosed sequence is
missing. Encoding rejection counts as one attempted/dropped record when its exact
chunk count cannot be established. I/O failure or a crash can lose controls too.

Reader liveness probes open epoch locks read-only and attempt a shared advisory
lock. Would-block means live; acquired means ended; missing/unreadable/error means
unknown. Only proven end/new epoch/independent process-start change permits
abandoned-epoch classification. Time alone is not proof of death. A partial line
in a live/unknown epoch is unresolved; after 60 seconds warn stale. Holes are
provisional immediately, suspected after 60 seconds, and unrecoverable only when
relevant retained sources are gone or an ended epoch has been fully scanned. Late
valid events can resolve holes. None of this blocks canonical receipt scanning.

Shutdown stops accepting records and drains the bounded queue on the writer
thread. After protocol drain, the caller waits at most the lesser of two seconds
or 25% of its remaining shutdown budget. A blocked filesystem worker is detached;
crash-window uncertainty is preferable to delaying protocol shutdown.

## Local metrics

The library uses the existing `metrics` facade and installs no global recorder.
It increments `eez_telemetry_dropped_events_total{signal="lifecycle",reason}` for
`queue_full`, `writer_closed`, `encoding`, `capture_limit`, `capture_context`,
`io`, `shutdown` or `clock`, and
`eez_telemetry_writer_errors_total{reason="io"}` for writer failures. Native
Reth prefix/exposition remains an integration gate; no dashboard selects these
candidate names before it is verified. Epochs, paths, hashes, and errors are not
metric labels.
Capture rejection consumes the transition ticket and counts one attempted/dropped
record because no encoded chunk count exists; its sequence remains a gap.

## Bounded reader API

`eez-telemetry::reader` provides the filesystem boundary for the importer. Its
inventory accepts canonical epoch/segment filenames with either suffix, deduplicates
IDs across a racing rename, and sorts for deterministic enumeration only. It caps
successful inventories at 4,096 segments and 65,536 total directory entries. An
unreadable or over-limit inventory is a collection error, never evidence that
missing files expired. Scheduling must rotate across persisted scan timestamps;
always taking the first filenames would starve later segments.

A read pass captures the file length, rechecks a bounded header, and reads at most
32 complete records plus one bounded partial/oversized prefix. It never reads
past that length, so concurrent appends wait for the next pass. The next offset
advances only through complete lines of at most 16 KiB. Raw bytes and original
offsets remain available for quarantine; unknown JSON is not discarded by the
filesystem layer. The caller must check header identity/schema and compare the
header digest with its persisted checkpoint before importing records.

Partial lines stay at their start offset. An oversized line returns at most
16 KiB plus one byte as evidence and also leaves its checkpoint unchanged; it
requires explicit investigation/replay rather than an unbounded scan to another
newline. Truncation before the checkpoint and invalid line boundaries fail
explicitly. `.active` to `.closed` rotation resumes from the same ID and offset.
A NotFound read must be reconciled with another complete inventory before assigning
a retention gap. No file descriptor survives the pass or a subsequent SQL commit.

On Unix, segment opens and epoch-lock probes use no-follow/nonblocking flags and
verify regular files, including when a path changes after inventory. Failed lock
access remains unknown liveness. These are blocking filesystem APIs: the async
observer must run them on a bounded blocking worker. The [database importer](lifecycle-ingestion.md) uses this API for transactional
checkpoints and gap classification. Complete transport alone does not validate
canonical lifecycle outcomes.
