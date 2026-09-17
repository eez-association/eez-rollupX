# Backup and restore

Restore monitoring into a separate instance with new volumes. Preserve the old
instance and its evidence until the recovered instance passes the checks below.
Do not reset chain data, receipt cursors, lifecycle sequences or migration rows
to make recovery succeed. The [isolated restore evidence](../validation/restore.md)
exercises this procedure with the packaged PostgreSQL, Grafana, Prometheus, Loki,
Alloy and optional Tempo versions.

## Backup set

Stop the observer indexer before taking the checkpoint. Quiesce native lifecycle
writers before copying their spool, or use a filesystem snapshot whose
consistency and durability have been established. Preserve the original closed
segments and headers; a SQL checkpoint alone cannot recover unimported lifecycle
bytes. Do not run two observers against one recovered lifecycle namespace.

Create a PostgreSQL custom-format dump of `eez_observer` using the administrative
identity, for example `pg_dump --format=custom --no-owner --dbname=eez_observer`.
Keep credentials in the private runtime/secret files, not command-line passwords
or logs. Fail the backup if the command fails. This dump covers database schema,
data, privileges and database-scoped settings. The packaged dedicated login
roles are recreated by the normal initialization script using the backed-up
secrets. Additional custom cluster roles and tablespaces need a separately
reviewed global-object backup: they are outside `pg_dump`'s database scope.
See [PostgreSQL's dump documentation](https://www.postgresql.org/docs/18/app-pgdump.html)
and [global objects](https://www.postgresql.org/docs/18/app-pg-dumpall.html).

Stop this monitoring project's diagnostic services before filesystem archives,
then preserve all of the following as one backup set:

- Private runtime configuration, stack input, connection files, secrets,
  provisioning files, dashboards, alert rules, contact points and routing policy.
- `grafana-data`, `prometheus-data`, `loki-data`, `alloy-data` and, when enabled,
  `tempo-data` and `alloy-traces-data`, including WALs and tailer positions.
- Approved log files and the native lifecycle spool associated with the SQL
  checkpoint, including original epoch/segment identities.
- The exact observer executable, migration files, pinned image manifest,
  Compose/configuration assets and their SHA-256 checksums.

Keep numeric file ownership and modes on data-volume archives. Restrict the
backup and checksum manifest to the operator; generated config includes
credentials. Checksums detect accidental corruption but do not authenticate
an untrusted backup. Choose an authenticated, encrypted off-host storage and key
recovery procedure before production use. Record snapshot time, last complete
chain cursors and lifecycle checkpoints with each set. Configure backup cadence,
retention, ownership and recovery objectives explicitly; none is selected by
this test tooling.

## Restore order

1. Verify every required file and checksum before creating or changing the
   destination. Use the matching executable, migration set and pinned images.
   A missing/corrupt archive or incompatible version is a failure, not an empty
   replacement. Keep the destination private and use new project, network and
   volume names.
2. Restore runtime configuration and secrets together. Rebind host paths and
   loopback ports to the new machine. Inspect rendered Compose to ensure no
   bind points at the old host/project. Preserve logical environment, deployment,
   chain/genesis/rollup identity and configured selections. Preserve the
   lifecycle directory as seen by the observer (the fixture uses `/lifecycle`),
   even if its physical host path changes.
3. Create the empty destination volumes. Restore diagnostic archives with their
   numeric ownership while those services are stopped. Initialize a fresh
   PostgreSQL cluster with the packaged init script and original role secrets.
   Do not overwrite a running database or reuse an incompatible upgraded volume.
4. Restore the custom dump using the administrative identity against the fresh
   destination's maintenance database:

   ```bash
   pg_restore --dbname=postgres --create --clean --if-exists \
     --exit-on-error --no-owner /private/backup/ledger.dump
   ```

   This destructive database replacement is for the newly created destination
   only. `--create` restores database-scoped role settings as well as the named
   database; merely restoring into a precreated database omits those settings.
   See [PostgreSQL restore options](https://www.postgresql.org/docs/18/app-pgrestore.html).
5. Point the restored private admin/writer connection files at that destination.
   Run the matching observer's explicit `migrate` command. An exact restored
   schema makes this a checksum-verifying no-op. Stop on an edited, unknown or
   newer migration; do not repair the checksum table manually.
6. Verify reader/writer roles and the acceptance checks below before enabling
   one observer. Resume bounded ingestion with the same source identities and
   bootstrap selections. After catch-up, start diagnostic services and confirm
   that alert delivery uses the intended recipients before returning to service.

The Grafana SQLite volume and provisioning/configuration are a coordinated
restore. Changing a bootstrap password file does not rotate an existing Grafana
account. Preserve the original credentials or use Grafana's supported recovery
procedure; recreating an empty Grafana database loses UI-managed state.

## Acceptance checks

Compare installed migration versions/checksums, chain identities, bootstraps,
selections, cursor heights/hashes, exact decimal receipt totals and receipt
observation counts with the backup checkpoint. Compare lifecycle epochs,
sequence/state/payload and segment offsets/header digests. A repeated import
must not duplicate fees or events. Available canonical RPC history can recover
post-backup chain progress; lifecycle bytes that were never backed up cannot be
reconstructed from receipts. Later records must expose missing sequences as
gaps instead of manufacturing success.

Authenticate as the actual reader in a fresh connection. It must read approved
views and be denied raw lifecycle records and receipt mutation. Its `SHOW jit`,
`SHOW statement_timeout`, `SHOW lock_timeout` and
`SHOW idle_in_transaction_session_timeout` must return `off`, `10s`, `5s` and
`10s`, respectively. The writer also has `jit=off` and cannot acquire
administrative capabilities. See [database role configuration](database.md) for
custom logins, which do not inherit these settings through role membership.

Verify all six provisioned dashboards, the reader SQL datasource, contact
points and notification policies. Query a known historical Prometheus sample,
Loki log and retained Tempo trace through Grafana; trace absence after the
retention window is expected. Verify an actual heartbeat callback and an
operator alert's firing and recovery delivery. A green datasource health check
alone does not establish restored history or routing.

## Repeatable fixture drill

With Docker/Compose, Python 3.12+ and the matching built observer available:

```bash
python3 observability/tests/smoke-restore.py \
  --binary target/debug/eez-observer \
  --report /tmp/eez-restore-report.json
```

The test uses pinned images, synthetic receipts/logs/traces and fresh private
source/destination projects. It copies the executable first, stops its source,
restores into entirely new volumes, verifies the checks above, then removes
only its own resources. It never attaches to an operator instance. Reports
contain checksums, fixture timings and recovery-point counts; private archives
and credentials are temporary. On failure, same-stem project log artifacts aid
diagnosis and should remain private.

This measures application restore on one Docker host. It does not test a failed
physical host, off-host transfer, encryption/key recovery, a full-size dataset,
production backup scheduling or the independent heartbeat receiver's own
disaster recovery. Run an operator-owned drill with those conditions and record
its measured RPO/RTO before relying on production recovery objectives.
