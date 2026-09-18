# Private canonical database

For the single-machine test setup, use the [Compose observer overlay](../compose.observer.yaml)
and [local testing guide](local-testing.md). It uses generated `admin-container.json`
and `writer-container.json` files with `postgres:5432` inside the ledger network.
The host connection files below remain available for a separately reachable host
or tunnel connection; internal-only Docker networks may not expose published ports.

Set `database_enabled: true` in the stack input and rerun `prepare.py`. Optionally
set `postgres_port` (default 19432; 1–65535). The generator preserves separate
admin/writer/reader passwords across reruns, writes private connection files under
`database/`, and provisions Grafana's `eez-postgres` data source with the reader
identity. The enclosing runtime directory stays mode 0700. Do not print or commit
runtime files. Disabling the option removes the provisioned SQL data source;
it does not erase an existing database volume.

Start only the optional database before applying its schema:

```bash
docker compose --project-name eez-observability \
  --env-file .runtime/observability/compose.env \
  -f observability/compose.yaml --profile database up -d --wait postgres

cargo run --locked -p eez-observer -- migrate \
  --database-config .runtime/observability/database/admin.json
```

The pinned PostgreSQL 18 image initializes unprivileged login identities, not the
observer schema. Only the explicit administrative `migrate` command applies DDL.
It locks migrations transactionally, verifies every existing version/checksum,
and applies pending forward migrations in order. A newer, edited or unknown
schema is refused. Normal indexer startup verifies the exact supported schema
and refuses administrative or incompatible credentials. Migrations are append-only
once released; do not edit a deployed migration to evade a checksum mismatch.

The database uses its own internal Docker network with Grafana. Its host port
binds only to loopback. On a separate node host, provide a private authenticated
tunnel to that loopback port; copy only the writer connection file to the observer
and adjust its host/port for the local tunnel endpoint. The current Rust driver
uses Unix sockets or explicitly opted-in plaintext TCP inside that private
transport. It refuses TLS modes rather than silently ignoring them. It does not
provide encryption for traffic outside the tunnel. Never expose this port on a
public interface.

Prepare the verified [indexer configuration](../indexer.example.json), set its
`database_config` to the writer file, and follow the [indexer contract](../contracts/canonical-indexer.md).
For example, after replacing all placeholders and bootstrap values:

```bash
cargo run --locked -p eez-observer -- indexer --config /private/eez/indexer.json --once
cargo run --locked -p eez-observer -- indexer --config /private/eez/indexer.json
```

The continuous indexer owns the exporter's metrics port; stop a separate exporter
on that same port before switching. Database-free exporter mode remains available
if canonical indexing is intentionally disabled. `--once` is restart-safe bounded
backfill, not a claim that the full historical interval is caught up.

The database container has a 512 MiB memory limit, one CPU, 128 MiB shared buffers
and 30 connections; Grafana opens at most five. Its persistent volume is mounted
at PostgreSQL 18's `/var/lib/postgresql` parent. These are initial budgets, not
measured production capacity. Keep disk space/growth monitored and preserve the
volume across ordinary restart/upgrade. Changing secret files does not rotate
roles in an existing cluster: use an explicit coordinated PostgreSQL credential
rotation and then update generated credentials. Never delete the volume to fix
a login error.

For a frozen finalized contradiction, stop relying on affected accounting and
investigate endpoint identity and canonical anchors. Preserve the database and
source evidence. The scanner will not automatically unfreeze or rewrite finality.
Restore into a separate instance or perform an explicitly reviewed repair; do not
clear a flag merely to make a dashboard green. Missing historical blocks require
a source that retains that interval; changing bootstrap on the existing namespace
is refused. Receipt gaps retry automatically without blocking new headers.

The [restore runbook](restore.md) and [isolated restore evidence](../validation/restore.md)
cover SQL/configuration and retained diagnostic data on a fresh separate instance.
The drill checks exact receipt totals, cursors, migration checksums and resumed
ingestion without duplicate charges. Production recovery targets and deployment
calibration remain release gates in the [requirement ledger](../requirements.md).

References checked for this packaging: [Docker PostgreSQL guide](https://docs.docker.com/guides/postgresql/)
and [Grafana provisioning](https://grafana.com/docs/grafana/latest/administration/provisioning/).

Schema 2 adds wallet/posting views and method-coverage backfill. Upgrade the
observer database explicitly with the new binary's `migrate` command before
starting its indexer. The old schema-1 binary refuses the upgraded schema rather
than attempting a downgrade. Keep a backup of the database and original runtime
credentials before migration; reverting only the executable is not a schema
rollback. Existing exact charges survive migration while method coverage can
initially be unknown. The scanner fills that evidence in bounded passes.

Schema 3 adds independent applied-range collection and exact L2 finality views.
The same stop/backup/explicit-migrate/start order applies to schema 1 or 2 upgrades;
older binaries reject schema 3. Receipt costs and selections are preserved.
Configure the node's correlation RPC capability before expecting range coverage.
Absent/unsupported history remains a visible gap while receipt scans continue.
Do not treat a successful `--once` receipt scan as proof that correlation is healthy;
check `eez.settlement_coverage` and the settlement collection metric explicitly.
A derived L2 finalized contradiction freezes the correlation source, separately
from the receipt chain's freeze state. Preserve both records when investigating.
See the [applied-range contract](../contracts/canonical-indexer.md#applied-ranges-and-l2-finality-schema-3)
for source assurance, bounded replay and view semantics.


Schema 4 adds optional lifecycle imports. Stop the old observer, preserve a backup,
run the new binary's explicit `migrate`, then start it with the same receipt
configuration and optional `lifecycle` entries. An omitted/empty array disables
file ingestion. Provision local read-only spool access before enabling entries;
a missing directory remains a visible collection failure and does not stop chain
indexing. All prior schema binaries refuse schema 4. Reverting the executable
alone is not a database rollback. See the [ingestion contract](../contracts/lifecycle-ingestion.md)
for bounded replay, quarantine investigation, freshness and writer-liveness limits.


Schema 5 adds lifecycle-referenced source receipt lookups and separates configured
selection from receipt dependencies. Use the same stop/backup/explicit-migrate/start
order; schema-4 and earlier binaries refuse this version. Existing `lifecycle`
entries enable source lookups without changing receipt selection or bootstrap.
Check both per-chain source-receipt collection signals, pending admissions and
`eez.source_receipt_coverage`; null receipts are unobserved, not zero-cost success.
Source and wallet cost populations can overlap and must not be added together.
See the [source receipt contract](../contracts/source-receipts.md) for exact scope,
work budgets, gap states and rollback boundaries.

Schema 6 adds independent receipt protocol-log evidence. Use the same
stop/backup/explicit-migrate/start order; schema-5 and earlier binaries refuse the
new schema. There is no executable-only downgrade. Existing receipts, exact costs,
selections and lifecycle records remain intact. A nonempty L1 contract selection
enables bounded log backfill from those receipts automatically; no rescan or cursor
reset is required. Inspect `eez.protocol_log_coverage` and the protocol-log health
metrics before relying on stronger evidence. See the
[protocol-log contract](../contracts/protocol-logs.md) for limits and conflict policy.

Schema 7 retains each decoded post's verification-rollup table. Follow the same
explicit upgrade order; older executables refuse the new schema. Existing decoded
rows keep their method/fee facts and acquire the missing list through the bounded
method-repair lane. Inspect nullable `verification_rollups` and repair errors in
`eez.posting_rollup_evidence`: an unknown list is not an empty list. The
[verification-table contract](../contracts/posting-rollups.md) explains immutable
binding, full-width IDs and the separate receipt-success requirement.

Schema 8 adds admission-history and local-policy evidence views. Existing events
and fees are preserved; use the same explicit upgrade order. Query
`eez.lifecycle_admission_status` to distinguish recorded local dispositions from
unresolved histories and positively observed process exits. These statuses do not
assert queue membership or cross-chain success. Inspect
`eez.lifecycle_local_policy_evidence` for missing admissions, inconsistent ownership
or incomplete parent decisions; see the [status contract](../contracts/admission-status.md).

Schema 11 disables PostgreSQL JIT for the dedicated reader/writer roles in the
observer database; short expanded evidence queries otherwise spend longer than
the query deadline compiling. Observer sessions also set it explicitly. For a
custom Grafana login that inherits `eez_observer_reader`, configure `jit=off` on
that login for this database and reconnect its pool: role settings are not
inherited by membership or `SET ROLE`. Verify `SHOW jit` returns `off` on a fresh
reader connection. The existing statement/lock deadlines remain unchanged.
Schema 13 also gives fresh dedicated reader connections a 10-second statement
budget, 5-second lock budget and 10-second idle-transaction budget. Apply the same
settings to any custom Grafana login alongside `jit=off`; reconnect its pool after
an upgrade. A failed/expired dashboard query must remain unknown, not display zero.

Schema 15 adds a persisted per-chain canonical revision for funding snapshots.
Ordinary cursor append preserves valid historical burn; orphaning or deleting
canonical blocks invalidates it transactionally. Follow the normal explicit
migration order. Administrative block changes must lock the chain row before
cursor/block rows, matching the scanner; never edit/reset the revision to retain
a stale funding result. See the [funding contract](../contracts/funding.md).

Schema 14 adds native cost-population projection, companion attribution and exact
integer shared-overhead estimates. Use the same backup/stop/explicit-migrate/start
sequence; schema-13 binaries refuse the new version. Receipt fees and existing
canonical outcome checks are preserved. Configured lifecycle sources enable the
bounded cost worker automatically; absent or conflicting historical membership
stays unallocated. Check `eez_observer_cost_projection_success` and
`eez.posting_allocation_status` before using per-source estimates. The
[cost contract](../contracts/cost-allocation.md) distinguishes native provenance,
receipt-level charges, allocation estimates and overlapping wallet populations.

Schema 16 retains declared cost claims independently of asynchronous projection;
late conflicts or missing parents cannot silently restore an allocation. Its
one-time backfill needs a representative database-size timing check before rollout.

Schema 17 records first observed finalization for future canonical finality
transitions. Already-finalized historical rows keep unknown observation times;
migration time is not a substitute for finality latency. See the
[finality timing contract](../contracts/finality-timing.md).

Schema 18 adds settled-transaction cost views without new configuration or a new
worker. Exact denominators require full-history all-transaction L2 receipt scans
and verified applied ranges; selective scans and historical gaps remain visible
as unknown coverage. Existing namespaces keep their immutable selections and
bootstrap points. Do not change them in place to make a dashboard populate; a
separate full-history namespace/rescan needs its own capacity plan. None of these
coverage gaps removes independently recorded fees. Older binaries refuse the new
schema; rollback requires a compatible binary or a separately restored database.

Schema 19 bounds repeated canonical/finality query expansion and exposes the
already-selected canonical context on admission status. These statement-local
query boundaries retain immediate conflict/reorg/freshness checks; they are not
persisted caches. The earlier schema17/18 migrations are unchanged. Apply the same
explicit upgrade procedure and verify reader query latency on representative data;
small-fixture timings do not establish production capacity.

Schema 21 adds indexes matching admission identity and settlement block lookups,
and reuses canonical settlement evidence within each outcome query. It preserves
all receipt amounts, event identities, view columns and grants. Reorgs, conflicts
and freshness are evaluated within the same query snapshot, with no persisted
cache. Stop the observer and back up before the explicit migration; building the
indexes takes table locks and must fit the existing migration deadline. Measure
that operation on a representative restored database before production rollout.
Versions through schema 20 refuse schema 21; reverting only the executable is not
a rollback. Keep a compatible binary or restore a backup into a separate database.
The normal statement deadlines and memory limits are unchanged.


Schema 22 expands applied-range storage to 65,536 blocks and ordinals 0–65,535
for the new-network derivable operations format. It changes only those two check
constraints; existing receipt charges, range evidence, columns and grants remain
intact. The migration does not transform an existing chain into the PR26 network
or make historical operation formats replayable. Follow that network's fresh
genesis and mandatory settlement-policy requirements independently.

For an observer database upgrade, stop the observer and preserve a database and
configuration backup. Provision the matching node with
`eez_getSettledL2RangePage` before starting the new observer, apply its explicit
administrative `migrate`, then restart indexing with the writer identity. There
is no fallback to the old full-range method. An old node produces a visible
correlation collection error rather than fabricated or truncated coverage.
Versions through schema 21 refuse schema 22; executable-only rollback is not
supported. Restore the backup into a separate instance or use a compatible
binary. The forward upgrade fixture accepts a retained schema-21 executable via
`smoke-database.py --previous-v21-binary` and verifies unchanged historical costs
and outcome anchors plus explicit downgrade refusal.

The 65,536-member SQL boundary and paged parser regressions establish correctness
bounds, not native workload capacity. Existing statement deadlines, 512 MiB
PostgreSQL/128 MiB shared-buffer guards and observer resource limits are unchanged.
Measure representative maximum-range ingestion and recovery before claiming
production capacity or memory headroom; earlier release measurements used the
previous runtime and do not establish that new maximum.
