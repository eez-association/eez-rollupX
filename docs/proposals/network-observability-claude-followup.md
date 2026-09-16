Review provenance: second proposal-only Claude review, 2026-09-16, Claude Code 2.1.273. All tools were disabled; no repository source files were supplied. CLI-reported models: claude-haiku-4-5-20251001, claude-fable-5-1. Reviewed input SHA-256: `1919b4a849dd5a1b42c017c1a258a6a20d36438968df9da4a2c6625ce850b611`. Preserved verbatim below. The later-stage findings have since been addressed in the [proposal](network-observability.md); see [dispositions](network-observability-review-disposition.md). This review predates those final corrections and is not runtime validation.

---

## 1. Can WP0 / WP3a / WP2 start?

Yes. Nothing in the stage‑1 path depends on an unresolved decision. WP0 is bounded (capability matrix, contracts, pins), WP3a is database‑free with explicit invariants (`ready+blocked+unknown=queued`, `active=queued+inFlight`, stale‑with‑success stays stale, no manufactured zeros), and WP2's acceptance is self‑contained (clean devnet start, dependency‑stop alert, dead‑man on external host). The `eez_observer_*` vs native naming split plus recording‑rule single‑source selection removes the earlier dual‑producer ambiguity. The WP2→WP3a dependency is correctly limited to live acceptance, so WP2 packaging can proceed in parallel. Remaining issues below sit in stage 2/3 and do not block starting.

## 2. Remaining contradictions and missing decisions

**Blockers (must be decided before WP3b/WP4 start; none block stage 1)**

**B1 — Drop reporting cannot be reconciled with hole classification (WP4, "Default limits…").** Drops are detected on the *producer* side when the 4,096‑record channel is full, and surfaced only as `eez_telemetry_dropped_events_total`. The observer, however, is told to distinguish "provisional holes" from "gaps persisting beyond replay/retention" purely from sequence numbers in the file. Since sequences are assigned *before* enqueue, a dropped event and a reordered late event look identical in the stream, and the metric carries no sequence ranges. Minimal fix: have the writer task emit an in‑band `dropped` record (`epoch`, `dropped_count`, and the min/max dropped sequence, tracked by an atomic on the producer side) whenever it observes the drop counter advance, plus a fixed provisional window (e.g. a hole becomes a confirmed gap once a later record with `sequence ≥ hole + N` or a later segment close has been read). Without this, the WP4 acceptance "queue overflow … create visible gaps" has no defined mechanism.

**B2 — Chunked membership records have no sequence rule (WP4, same paragraph).** Large membership is split into chunks with a completion marker, but it is unspecified whether each chunk consumes a sequence number or shares one with a chunk index. Combined with reordering and per‑chunk drops, gap detection and idempotent upsert keys differ. Minimal fix: one logical sequence per event, `chunk_index/chunk_count` fields, completion marker = last chunk; the `lifecycle_events` unique key becomes `(epoch, sequence, chunk_index)`.

**B3 — Poster spend vs. the sender‑address prohibition (WP3b "Index source/companion ownership…" and "Reuse existing protocol accounting").** WP3b must reconcile saved audits "including companions" and publish poster spend, yet ownership must "never [be inferred] solely from … sender address" and per‑source attribution "requires the recorded exact attempt membership" — which only arrives in WP4. As written, WP3b can either not compute poster spend or must violate the rule. Minimal fix: state explicitly that *operator/poster spend* is keyed by canonical receipts from configured poster/system accounts (sender identity is the definition, not an inference); that *companion* classification in WP3b uses a named chain‑derived rule (contract/log signature, to be confirmed in WP0) with `evidence_class`; and that *per‑source allocation* views are a WP4 deliverable. The WP3b acceptance sentence should drop "companions" or qualify it accordingly.

**B4 — Stage‑2 "no canonical progress" alert has no fallback producer (alert table row "2 / native lifecycle metrics + verified reconciliation" vs. "Recorder integration").** If the Reth recorder hook cannot be reached, native histograms/counters are deferred, but the alert row names native metrics as producer. Minimal fix: name the observer's lifecycle‑event‑derived terminal‑progress metric (from `lifecycle_events` with validated anchors) as the authoritative producer for this rule; native counters are an optional cross‑check. This matches the single‑producer rule in requirement 1.

**B5 — "Writer is live" is undefined for the trailing‑line rule (WP4).** The observer ignores an unfinished trailing line "while the writer is live" and marks it missing "after recovery," but liveness is only inferable from a *later* epoch appearing. A node that crashes and stays down, or restarts before emitting any event, leaves the hole provisional indefinitely. Minimal fix: define liveness as *either* a newer epoch observed in the stream *or* the exporter's process‑start timestamp advancing *or* a bounded age (e.g. segment older than retention); any of these converts the partial line to `missing`.

**Minor contradictions / clarifications (fix in text; not blocking)**

- **Segment naming:** "immutable segment IDs and rename‑based rotation" are in tension. Simplest: the writer opens `seg-<epoch>-<n>.jsonl` directly and never renames; checkpoints key on segment ID, not path. If rename is retained, state that the segment ID is embedded in the record header so a checkpoint survives the rename.
- **Disk bound vs. open handles:** the observer (and Alloy) holding a deleted segment open defeats the 2 GiB bound. Add "reader closes a segment within one poll after finishing it; retention accounting is by writer‑visible files."
- **Lifecycle writer implementation surface:** "independent of `RUST_LOG`" implies it is *not* a `tracing` layer. State that it is a dedicated `eez-telemetry` channel API so subscriber filters cannot drop it.
- **Head‑stale threshold quantisation:** 3 Chiado slots (~15 s) equals the 15 s poll interval; either rely on subscriptions for head observation in WP3a or express the rule as ≥ N poll intervals. Thresholds are already declared provisional.
- **Restart without events:** requirement 4's epoch logic only fires on an observed event; add the process‑start timestamp as a second restart trigger for `unresolved_restart`.

**Optional improvements**
- Record file ownership/permissions for the lifecycle directory (node writes, observer/Alloy read) in WP2's deployment manifest.
- Evidence table: "leaves the stronger claim unresolved" is correct but terse; one sentence saying the receipt fact is retained while the cross‑chain claim goes to `unknown` would prevent misreads.

## 3. Validation gates acceptable to leave as implementation work

- Recorder hook reachability, effective metric names, histogram exposition (WP0 acceptance with stated fallback).
- Reth/Lighthouse structured file logging availability; "logs unavailable" fallback is explicit.
- Blob exact‑length recovery fixtures; unknown/composer‑reported states already defined.
- Finality/reorg replay invariants, duplicate‑delivery cursor tests, reinclusion history — well specified in WP3b/WP6.
- Trace‑context propagation over gRPC, flush budget behaviour, sampling policy after pilot.
- Overhead budget (<5 % provisional) and lock‑hold measurement.
- Alert calibration, retention sizing, RPO/RTO after the baseline week.

Net: the document is internally consistent for stage 1. Resolve B1–B5 in the contracts/schema text before WP3b/WP4 begin; the rest are wording fixes or ordinary implementation gates.
