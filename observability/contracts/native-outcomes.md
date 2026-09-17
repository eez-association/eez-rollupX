# Native outcome coherence (schema 10)

This is the native-evidence prerequisite for canonical outcomes in R19. It is
**not canonical assurance**. `lifecycle_native_outcomes.evidence_class` remains
`unknown`, including rows whose `native_state` is `coherent`. Schema 11 adds [canonical anchor validation and selection](canonical-outcomes.md)
and terminal-progress gauges as a separate consumer of this coherence evidence. Local-policy and actual
admission history retain their schema-8 semantics.

An existing nonempty `lifecycle` configuration enables the independent projection
worker. No native process connects to the database and no new configuration knob
is introduced. The worker uses the configured deployment, producer identities,
chain/genesis pair and rollup. In `indexer --once`, its bounded pass follows
lifecycle, source-receipt and protocol-log collection; the existing receipt scan
still determines exit status. Projection errors are visible in logs and the
`eez_observer_native_outcome_projection_*` health metrics. Neither successful
projection nor a zero CLI exit means canonical completion or an empty backlog.

## Evidence and identity

`outcomes::native_decision` checks a terminal claim against its exact reconciliation
parent and original prepared membership, including every retained repricing of
that native generation. Producer, boot epoch, logical sequence order, source
scope, owner, per-source outcome and duration must agree. The manifest and
reconciliation have bounded, unambiguous source membership; source nonces,
receipt status, consumption entries and whole replay groups must agree with the
native claim. Local-policy decisions stay in their existing separate view.

The native replay-coordinate rule is shared with Composer. It checks the exact
applied endpoint and group selection. The observer additionally rejects partial
intersections with indivisible outbound groups in untrusted telemetry. The
original native scanner's boundary-validation responsibilities are unchanged.
Signed candidates may recur after repricing (`A -> B -> A`): sequence order defines
causality and a repeated hash does not imply another paid receipt.

The scan watermark, posting inclusion, proposed full L2 hash, actual applied
prefix and later L2 snapshot remain distinct. Outbound inclusion through a
competing canonical batch does not require the original attempt to have applied.
These facts preserve native meaning; independently checking their canonical
anchors remains a separate requirement.

## Persistence and late evidence

`lifecycle_native_records` selects only complete events with their final transport
completion digest. SQL-only synthetic complete rows without chunks cannot pass
this gate. The importer already checks assembly/digest/schema before setting
`complete`; this view does not replace that importer validation.

`lifecycle_native_checks` caches bounded derived checks by producer, epoch,
terminal sequence and configured chain pair. The stored source-specific decision
references the terminal, parent and lineage by exact transport digest. It does
not duplicate the complete multi-source manifest for every terminal. Missing
admissions are never fabricated: explicit later membership can be checked while
`lifecycle_admission_status` still has no row for the unobserved acceptance.

`lifecycle_native_outcomes.native_state` immediately invalidates a coherent cache
when any referenced record stops being complete or changes digest. It also
invalidates the cache when another complete prepared/repriced record appears in
that generation before the parent. This prevents late evidence from leaving a
stale positive check visible between importer and projection passes. Readers
must use this view and its current state, not the raw cache's `state` column.
Receipt facts and costs remain independent of native conflicts.

The worker has a separate connection and a 30-second pass deadline. Each pass
checks up to 24 new terminals in oldest-import order and 8 previously checked
terminals in oldest-attempt order, interleaving the lanes starting with a repair.
It records an attempt before loading dependencies so cancellation or a
slow query cannot permanently monopolize the new-work lane. Successful passes
wait five seconds; failures use the existing 15/30/60-second backoff. Database
connections retain server statement/lock deadlines and close on cancellation.

Each dependency is fetched separately with a 16 MiB normalized JSON bound; the
whole check has a 32 MiB normalized-byte budget and at most 128 lineage records.
These normalized limits account for JSONB text expansion above the transport's
8 MiB payload limit. Oversized, missing or contradictory evidence produces an
explicit gap, never a truncated proof. These are initial work bounds, not measured
production capacity; R27/R30 performance acceptance remains open.

## Upgrade and validation

Stop the observer, back up, run explicit `migrate`, then start the matching binary.
Schema 10 appends derived views/cache to schema 9. Old binaries reject the newer
schema; rollback uses a compatible binary or a separately restored backup.
Original lifecycle records, selected receipt populations and paid costs remain
unchanged. Grafana's reader role can inspect the derived view; only the observer
writer can update checks.

Coverage mapping:

| Requirement | Implementation | Verification |
| --- | --- | --- |
| R14/R19 exact membership, ownership and native outcome coherence | `crates/eez-observer/src/outcomes.rs`; shared `eez-protocol` replay rule | Nine outcome unit tests; native bidirectional lifecycle assertions; Composer/protocol regression suites |
| R19 bounded runtime projection, retry and scope isolation | `indexer/outcomes.rs`; schema 10 | `tests/native_outcomes.py` importer/indexer process cases; metric freshness unit test |
| R16/R19 late conflicts/recovered lineage retract cached coherence | digest references and current-state view | Process fixture with rollback checks of importer/projection boundary |
| R13 explicit migration, role separation, compatibility | migration 10 and schema registration | Disposable database suite: 45 check groups, including real importer/projection and saved-binary upgrades |

Native canaries completed before the parallel metrics/tracing changes: two
bidirectional lifecycle tests passed with independent RPC/semantic checks in
`/tmp/eez-observability-outcomes-native-capture.log` (local-only artifact). That run
validates the pure checker against captured native decisions, not this later
runtime projection or the final combined tree. Final coordinated validation is
still required.
