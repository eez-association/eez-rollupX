# Observer capacity follow-up

The frozen release overhead pilot failed. Its original plan/results remain
unchanged: first A was swap-contaminated; first B completed native workload
semantics but PostgreSQL had four OOM kills and canonical observer coverage did
not catch up. A diagnostic B repeat reproduced three OOM kills and retained the
logical database dump. No overhead or production-capacity acceptance is claimed.
Exact identities and local artifact locations are in [the record](observer-capacity.json).

The native fixture stored PGDATA on a 256 MiB tmpfs inside a 512 MiB PostgreSQL
cgroup. At peak, sampled memory included about 288 MiB shared/tmpfs pages and
188 MiB anonymous memory. Both ANALYZE and a canonical snapshot were OOM victims;
the evidence does not attribute every kill to a single query. Deployment already
uses a persistent disk volume. Restoring the retained data onto an owned disk
bind at the same memory/CPU limits produced no OOM in the diagnostic query run;
that excludes concurrent native ingestion and is not a live capacity pass.

Schema 21 retains the canonical accounting/evidence rules while adding two
indexes and statement-local query boundaries. The indexes match admission's
`eez.u64` cast and hash/number-first settlement block lookup. Canonical settlement
facts are reused only within each SQL snapshot. Native digest validation keeps
its exact per-reference key lookup instead of letting the planner scan every
chunk from a producer for each terminal. Existing migrations remain unchanged.

The new SQL fixture uses 3,000 distinct admissions on one producer plus 137 in
another deployment with overlapping hashes/chain numbers. It deliberately reuses
transactions and synthetic coherence caches; it measures SQL selection, not
native ingestion or unique-transaction throughput. The old query exceeded the
normal 10-second limit and a separate 60-second diagnostic limit. After the
lookup fix, the primary runtime snapshot took 303 ms, the other context 273 ms,
and the reader admission lookup 1,287 ms. All assertions completed in 9.8 seconds:
counts/cursors/progress, equivalent duplicates, unprojected contradictory claims,
late parent conflicts, context-local reorg retraction and rollback restoration.
No query deadline or memory limit was raised for acceptance.

The continuous native worker now waits 100 ms after a fully used new-work batch,
otherwise five seconds. Its 24 new/8 repair limits, repair-first interleaving,
attempt reservation before reads, 30-second pass deadline, error backoff and
one-shot behavior remain intact. The new process regression requires a burst of
120 malformed-lineage claims to drain while older checks are retried, preserving
unknown evidence and deduplicating after restart.

Native PostgreSQL fixtures now use a separate owned target directory, retaining
the short Unix-socket directory. They explicitly verify the deployment's 128 MiB
shared buffers, 30 connections, 512 MiB memory and one CPU. Functional fixtures
can use RAM-backed workspaces; performance measurements reject them. Failure
capture runs before cleanup for returned errors as well as panics, retaining
bounded local diagnostics. The complete database suite passed 58 groups, including a real schema-20
binary upgrade and downgrade refusal, the 120-claim burst and all existing
accounting regressions. A follow-up also asserts the reader returns exactly
3,000 canonical admissions. The native returned-error regression passed in
76 seconds: its dump parses with pg_restore, expected diagnostics are retained,
and its container/data/socket directories are removed in order. Observer unit
checks (78), affected Clippy and overhead adapter tests (21 input, 9 runner) pass.
The bidirectional native observer canary also passed in 123 seconds, including
semantic/signature checks and canonical/lifecycle reconciliation. A newly frozen
release performance experiment remains an open gate. Native checks here use the current optimized-development
observer/test harness with unchanged verified release node/signer executables;
they do not establish a new sealed release candidate.

Requirement mapping: R13 → append-only schema upgrade and prior-binary rejection;
R16/R19 → immediate evidence retraction, duplicate/context regression and fair
bounded native projection; R27 → representative storage and preserved failure
evidence; R28 → database/process integration tests. R27's comparative overhead
and R30's deployment/baseline gates remain open.

The schema-21 restore drill passed all six groups with all 21 migration checksums:
backup 32.77 seconds, restore through acceptance 68.91 seconds. Exact costs,
cursors, roles, replay/gap handling, dashboards, retained diagnostics and actual
heartbeat/fault/recovery delivery were verified in fresh owned instances on the
same Docker host. Cleanup was verified. This adds R29 upgrade recovery evidence;
it does not establish physical-host/off-host recovery or a production RPO/RTO.
