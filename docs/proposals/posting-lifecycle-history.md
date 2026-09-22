# Posting lifecycle history

Status: implemented and validated locally and on Chiado (2026-09-22).
See the [deployment evidence](../audits/2026-09-22-posting-history/README.md).

## Scope and invariants

Keep Prometheus for live queue/policy gauges and aggregate measurements. Add
versioned structured posting events to Composer's existing tracing output, and
optional replay of retained JSON files into the existing indexer/PostgreSQL.
No database, network telemetry client, new worker or blocking I/O in Composer.
No changes to submission, retry, ownership, reconciliation or settlement policy.

An attempt identity includes a process session, rollup and ledger generation;
final repricing must not change it. The posting identity is the exact prepared
Sync hash. Recomposing different contents creates a different posting; we do not
infer retry lineage from height. Record prepared, repriced, dispatched and the
observer outcome or preflight failure, with ordered event numbers and monotonic
elapsed times. Capture applicable trigger reasons before spawning the observer;
unknown policy context stays unknown. Prepared excludes preceding proving time.

Observation is not canonical completion. Join transaction hashes to existing
canonical settlement data at query time; retain off-chain history on reorg.
No receipt alone establishes successful completion of all cross-chain effects.

## Collection contract

Reuse Reth's filtered JSON file logger on persistent storage. Indexer mounts that
log directory read-only. Read complete bounded JSON lines, validate version and
network/rollup identity, and atomically store events with file checkpoints.
Duplicate replay must be harmless; conflicting duplicate identities must fail.
Retained files support restart, reader downtime and rotation replay. Missing or
invalid input must be visible, not silently classified as successful ingestion.

This is operational history, not a crash-proof transactional journal. Reth's
nonblocking logger can lose buffered events on abrupt termination/overflow;
rotation can remove unread files. Show incomplete attempt sequences explicitly.
An entirely lost attempt cannot be detected from its nonexistent events. A
strict no-loss guarantee would require a durable producer outbox and is outside
this minimal increment. Never change posting availability to guarantee telemetry.

## Configuration and compatibility

Collection defaults off. Existing node/indexer configurations still work. Opt in
with a dedicated log directory and indexer `posting_logs_dir`; reject empty or
unreadable configured directories. Use the existing Reth logging arguments.
Schema additions are additive. Old Composer versions produce no lifecycle events;
old indexers cannot use the new option (remove it for rollback). Canonical history
and checkpoints must remain untouched. No node or indexer dependency upgrades.

## Acceptance checklist

- Stable identity and ordering across repricing, different attempts and restarts.
- Every existing observer outcome, preflight error and actual dispatch covered.
- Immutable policy context, explicit unknowns, measured stage durations.
- Strict event validation, filtering, complete-line handling and bounded reads.
- Atomic import/checkpoint; duplicate, conflict, restart and rotation coverage.
- Read-only file access; independent collection cannot starve chain indexing.
- Dashboard: current unresolved state, trigger reasons, per-attempt events,
  timings, L1 links and canonical status, with incomplete history visible.
- Existing configuration, Composer tests, indexer tests, format/lint and SQL checks.
- Runbook: opt-in, persistence, retention, restart, rollback and loss boundaries.

## Requirement coverage

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Stable attempts, final hash replacement, ordered timings | `crates/eez-composer/src/posting_events.rs`; shared observer hooks in `composer.rs` | `identity_survives_repricing_and_attempts_stay_distinct`; Composer regression suite |
| Exact dispatch/outcome hooks without changing ownership | Existing preflight result, dispatch callback and observer result | Source review against submission ownership contract; earlier live success and latest excluded-attempt evidence distinguished in the audit |
| Snapshot reasons and explicit unavailable context | Canonical-generation-valid policy observation with matching target captured before spawn | Existing policy invalidation tests; two live attempts captured cross_chain |
| Version/network validation and optional configuration | `monitoring/indexer/src/config.rs`, `postings.rs`, runtime spawn/abort in `main.rs` | Posting schema/config tests; old config fixtures |
| Bounded complete-line replay, rotation/restart handling | `postings::read_batch`, persistent `posting_files` | Partial write, rename, fresh file and truncation regression |
| Atomic cursor/data, idempotency, conflict rejection | `postings::import`, `posting_events` primary key | Disposable PostgreSQL recovery test, including malformed second line and unchanged checkpoint |
| Off-chain history survives reorg; canonical state remains authoritative | `posting_attempts` joins current transactions/settlements/blocks | PostgreSQL canonical join and reorg assertions |
| Readable lifecycle dashboard and honest gaps | Queue panels 15–18; sequence completeness and collection freshness | Actual dashboard SQL fixture, live status expression, actual PostgreSQL chart/table queries and browser rendering |
| Operator setup, rollback and durability boundaries | Indexer README, optional `compose.postings.yaml`, AGENTS link | Compose validation, read-only live mount and indexer restart verified |

No protocol, signer validation, dependency lockfile or CI matrix changes are
required. Session-qualified attempts are local observations, not persisted queue
ownership. A recomposed Sync is a new posting, even if its height is reused;
linking arbitrary recompositions into a single logical retry chain is deliberately
not inferred. Full distributed tracing and a crash-proof producer outbox remain
outside this increment.

## Expected inclusion timing

Retain dispatch target height/time in the query view and compare them with canonical
inclusion height/time. Show a live-refreshing per-attempt point graph relative to
target time, keeping dispatch, inclusion and local receipt observation distinct.
Missing targets/canonical data are unknown, not zero. Reorgs remove canonical timing;
a receipt for a different block hash cannot measure observation of the replacement.
This is a SQL/dashboard extension using existing events, with no producer change.
The PostgreSQL recovery fixture covers on-target, earlier/later inclusion, missing
dispatch, reorg invalidation and the actual dashboard query.

## Follow-up: exclusion diagnostics and prompt settlement details

Implemented and deployed; live validation and the unsuccessful inclusion canary
are recorded separately in the audit.
The Chiado investigation found both skipped slots and produced blocks without the
posting, plus local parent/deadline deferrals. Retain exact target height/time before
dispatch, report typed local gate reasons without changing gate/retry decisions, and
record proof-gate/prover failures with distinct unsigned identities. Derive slot
outcomes from adjacent canonical headers; missing headers cannot prove a skipped
slot. External proposer intent remains unknown without its own evidence. The
public Chiado log viewer must not become a dependency of monitoring.

Settlement enrichment should not wait for L2 finality to begin. Collect complete
canonical-range receipt totals and unique source counts, preserving visible L2
state. Revalidate provisional cache hashes on every pass and before finality
promotion; changed ranges restart and incomplete sums remain hidden. Process at
most four independent details rows concurrently; keep the existing eight-row query,
128-block fee budget, log-scan bounds, retries and independent L1 writer. No new
configuration, Composer RPC, dependency or CI job is needed.

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Unsigned failure identity and early target | `posting_events.rs`, proof-gate hook | Producer identity/target test; parser failure fixtures |
| Typed parent/deadline reasons | `eez-l1::policy_submission_failure` | Fake-clock reason tests and unchanged boolean API regressions |
| Canonical evidence, explicit uncertainty | `posting_diagnostics`, queue panels 17/18 | PostgreSQL exact/skipped/missing-header and unsigned-failure fixtures |
| Prompt complete canonical fees/counts | `l2_fees`, `cross_chain`, settlement SQL | Resume/retry, legacy cache, state promotion and reorg mock-RPC tests |
| Bounded independent enrichment | `details::worker` | Indexer tests/lint, source review and live catch-up validation |
| Backward-reading compatibility | Optional target fields, additive views, same details storage | Legacy event/cache fixtures; documented indexer-first upgrade |
