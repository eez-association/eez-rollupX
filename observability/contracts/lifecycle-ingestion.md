# Lifecycle ingestion (schema 4)

Indexer mode accepts an optional `lifecycle` array. It defaults to empty, leaving
existing receipt/exporter behavior unchanged. Each entry names `service`,
`instance` and an absolute `directory` for the node-host spool. Environment and
deployment come from the enclosing indexer config. At most eight entries are
accepted; identities and paths must be distinct. Unknown fields, invalid identity
characters, relative paths and parent-directory components fail configuration
validation. The directory and identity are persisted together; changing that
binding in place is refused. This prevents importing unrelated files into an
existing event namespace.

For example, after provisioning the read-only group and local filesystem:

```json
"lifecycle": [
  {"service": "eez-composer", "instance": "node-1", "directory": "/var/lib/eez/lifecycle/composer"},
  {"service": "eez-proof-signer", "instance": "signer-1", "directory": "/var/lib/eez/lifecycle/signer"}
]
```

Use the exact identities emitted by the [native configuration](native-startup.md).
The observer stays on the node host and reads local files; no remote filesystem
or node/signer database connection is introduced. The writer must retain its own
exclusive instance/epoch locks. The observer requires only file read/traversal
access and its dedicated database writer role.

Each configured source has an independent asynchronous worker and dedicated
bounded PostgreSQL connection. Successful cycles wait five seconds; failures use
15/30/60-second backoff. Filesystem operations and logical-payload assembly run
on blocking workers. A missing spool or quarantined event cannot stop receipt
scans or another source. In `--once` mode each source gets one bounded cycle after
receipt/correlation work; errors are recorded in SQL/logs. As with optional
correlation, a successful receipt scan's exit code does not prove lifecycle health.

## Storage and recovery

A complete bounded inventory registers immutable epoch/segment IDs. Comparing
its persisted generation under the source lock rejects stale concurrent
inventories. A first absent inventory records uncertainty; a second successful
absent inventory marks the segment missing. Failed or over-limit enumeration
never proves retention expiry. Inventory caps are in the [transport contract](lifecycle-v1.md#bounded-reader-api).

Each cycle performs at most 16 file passes, selecting the oldest scan each time.
One pass reads at most 32 complete records and one bounded tail. An incomplete
active file may use later passes, while timestamps rotate work fairly across
other files. Complete segments are rechecked after five seconds. All descriptors
close before database work begins. A persisted checkpoint is compared under the
source/segment locks. Raw observations, staged chunks, control evidence and the
next complete-line offset commit in the same transaction. A killed importer
cannot leave an advanced cursor without its data, or committed chunks without
that cursor. Repeated byte-identical chunks are idempotent, including across
segments and `.active`/`.closed` renames.

Header identity, version and immutable digest are checked on every pass. Epoch
headers must agree on process ID and start timestamp. Unknown/invalid records
retain bounded raw bytes, digest and original segment/offset in
`eez.lifecycle_records`; Grafana cannot read that table. Oversized records retain
only their bounded prefix and stop advancement at that position. A changed
header is quarantined rather than interpreted as a replacement file.

Logical events remain incomplete until every expected chunk passes the shared
wire validator, completion marker, SHA-256 and typed-payload decoder. Assembly
visits at most 512 candidates and 16 MiB of hex payload per cycle, with at most
one additional event fetched to discover that byte boundary. Each event remains
capped at 8 MiB decoded. Pending assembly is visible separately from missing
chunks. Conflicting chunks never overwrite their original bytes; they retract
an already complete event from the readable history. An assembly finishing after
a concurrent conflict cannot publish stale success over that conflict.

Raw evidence is immutable. Its quarantine reason may be added after full-payload
validation, while original bytes, location and first observation time are retained.
The initial implementation retains lifecycle evidence in PostgreSQL; database
capacity and eventual evidence compaction must be measured with the actual
workload. File retention alone does not bound database size or the physical spool.

## Liveness and gaps

The observer uses read-only shared-lock probes, not PID existence or wall-clock
age, to distinguish live, ended and unknown epochs. End evidence is monotonic.
A missing/unreadable lock remains unknown. Probe work rotates across at most 16
unended epochs per cycle; a live result older than 90 seconds is shown as unknown.
Raw UTC timestamps and random filename order never establish restart causality.

Sequence gaps are derived from persisted observations as intervals, without allocating one row per
missing sequence. Full-u64 high-water values are supported. A hole or incomplete
event is provisional immediately, then suspected after 60 seconds of observation.
Late valid chunks/events can close it. An ended epoch whose retained files have
been fully scanned can establish `unrecoverable_from_spool`. Unknown/invalid
schema evidence remains quarantined; opaque records may explain a hole and do
not prove irrecoverable loss. Conflicting/invalid events have a quarantine state.

A partial line from a live/unknown writer keeps its offset and gets a stale warning
after 60 seconds. Proven writer end records its truncated bytes as a gap. It does
not fabricate a complete event. Missing segments with unread bytes remain visible
as retention gaps. An ended epoch without a latest uncontested `stopped` control
has an unknown trailing crash window; sequence/drop counters cannot bound work
that was never recorded. Drop minima/maxima are reported uncertainty, not an
assertion that every intervening event was lost.

## Reader views and metrics

| View | Meaning |
| --- | --- |
| `eez.lifecycle_event_history` | Complete, typed transport observations and source freshness; payload assurance/outcome fields remain emitter claims |
| `eez.lifecycle_gaps` | Sequence intervals or incomplete/invalid/conflicted events, detection time and classification |
| `eez.lifecycle_coverage` | Completion/backlog, quarantine, retention, unreadable/partial files, unknown liveness and trailing-window counts |
| `eez.lifecycle_epoch_status` | Epoch start/process metadata and observed lock-liveness evidence, including quiet startup |
| `eez.lifecycle_writer_status` | Latest control and cumulative counters/drop uncertainty; contested controls have no accepted stats |

These views are available to the read-only Grafana role. No view here establishes
canonical source completion, cross-chain success or queue durability. The required
anchor/membership/outcome checks are the separate R19 integration gate.

Metrics use bounded `source_service`/`source_instance` labels from the configured
sources, leaving the exporter's target identity intact. They include
`eez_observer_lifecycle_collection_success`, last-success timestamp and bounded
error reason. Fresh successful collections expose counts for `complete_events`,
`pending_events`, `pending_segments`, `gap_intervals`, `quarantined_records`, `lost_segments`,
`stale_partial_segments`, `unreadable_segments`, `unknown_liveness_epochs` and
`unknown_trailing_epochs`, under the same `eez_observer_lifecycle_` prefix.
Counts disappear on failure/staleness; last-success time remains. Successful
collection can still contain evidence gaps, which must be displayed beside counts.

## Quarantine investigation and replay

Stop the affected source's importer before an administrative replay. Preserve a
backup and select the exact source, epoch, segment and byte offsets from raw
records; do not clear quarantine merely to make a count zero. Unknown versions
require an explicitly compatible decoder first. Replay original bytes, not
reserialized JSON, because duplicate identity includes the raw record digest.
Retained source files may be replayed by an administrator resetting the selected
segment's offset to its earlier complete-line boundary and `scan_complete=false`.
Requeue an invalid logical event for assembly only after the decoder has been
corrected and reviewed; never resolve conflicting evidence by choosing a winner
without investigating its source. Existing quarantine records remain audit history.

If files have expired, original complete raw lines remain available in the
restricted database for a reviewed reconstruction. A captured oversized prefix
or truncated line cannot reconstruct missing bytes. This release has no automatic
repair, quarantine deletion or protocol-state recovery command. Native admissions
and canonical outcome reconciliation remain separate requirements.
