# Private canonical database

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

Backup/clean-host restore, settlement/source attribution, authenticated blob
measurement and production calibration remain release gates in the
[requirement ledger](../requirements.md). The isolated tests establish canonical
receipt persistence/recovery, not those later capabilities.

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
