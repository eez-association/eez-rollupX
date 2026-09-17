# Isolated backup and restore evidence

The reviewed fixture passed all six acceptance groups on 2026-09-17. The
[machine-readable report](restore.json) records pinned images, all 13 migration
checksums, archive sizes and exact timings. Both test projects, including their
containers, volumes and networks, were removed successfully. No operator
resources were used. This is a new instance with empty volumes on the same
Docker host, not a drill on a separate physical host.

Command (exit 0):

```bash
python3 observability/tests/smoke-restore.py \
  --binary target/debug/eez-observer \
  --report /tmp/eez-observability-restore-reviewed.json
```

The executable SHA-256 was
`9de5c53e16545e8e72c5001eb3e8360ccdcf5adb63da9d36eea6b006a6c4f86e`.
Schema 13 was
`45cd1d96a2443a0984e5c2752b2856707cec34e8a6e9f2a5ce22a843b1cab2c3`.
The source snapshot used actual observer migrations and receipt/lifecycle
ingestion against the existing RPC fixture. The destination used a copied
executable and archived bind assets, with no source-directory mounts.

| R29 acceptance | Implementation and observed assertion |
| --- | --- |
| Consistent database/configuration backup | Custom `pg_dump`; inactive fixture indexer; cold archives of six diagnostic volumes, private runtime, logs and lifecycle spool; checksummed complete set |
| Invalid backup fails explicitly | Missing SQL dump and incorrect digest both rejected before destination directory or resource creation |
| Clean instance and identity | Entirely new volumes; source stopped; empty EEZ schema verified; standard `pg_restore --create`; exact migration/chain/genesis/rollup/bootstrap/selection/checkpoint equality |
| Least-privilege access survives | Actual reader/writer authentication; nonadministrative role flags; approved view allowed, raw lifecycle read and receipt deletion denied; fresh reader settings `off`, `10s`, `5s`, `10s`; writer JIT off |
| Exact accounting and ingestion recovery | Restored L1 selected fee total `738216`; repeated healthy replay unchanged; fork catch-up to `804216` with matching two-chain cursors/receipt history; repeated catch-up unchanged |
| Lifecycle checkpoint and data-loss visibility | Original imported sequence/segment offset/header digest preserved; one deliberately unbacked record remains absent; later sequence3 exposes gap2:2 |
| Diagnostic history and navigation | Six dashboard UIDs, original Grafana admin credential, reader datasource, exact contact points/policy; old Prometheus sample, Loki trace log and Tempo trace through authenticated Grafana proxy; SQL lifecycle trace ID retained |
| Notification recovery | Actual restored heartbeat delivery and observer-unavailable alert firing/resolved deliveries |
| Isolation and cleanup | Unique project resources, rebased private paths, no old mounts, source stopped before verification; both projects' containers/volumes/networks absent afterward |

The cold backup took **32.81 seconds**. Its canonical snapshot was **43.87
seconds** old when restoration began. Restore through all assertions, including
real alert delivery and recovery, took **77.49 seconds**. These figures include
small fixture data and locally available images, not installation, off-host
transfer or a production dataset.

Controlled recovery-point evidence: one post-backup fork affecting two chain
cursors was recovered from RPC history without duplicate charges. One
post-backup lifecycle record was deliberately unavailable and correctly remained
missing. This is a measured nonzero loss example, not a zero-loss guarantee or a
configured production RPO. No production backup interval or RTO target is implied.

The first full fixture also passed (five initial groups), with evidence in
`/tmp/eez-observability-restore.json`; the reviewed run added explicit corrupt/
missing-backup refusal, copied-binary identity, migration hashes and destination
mount isolation. The shared `lifecycle_wire.py` extraction was independently
checked byte-for-byte against the prior database fixture functions over 24 cases
and headers. Existing database smoke assertions are unchanged.

The [runbook](../runbooks/restore.md) describes backup contents, role initialization,
database-scoped settings, safe destination ordering and acceptance checks. A
deployment still needs operator-owned off-host storage, authenticated/encrypted
backup transfer and key recovery, scheduled backup freshness monitoring, a
full-size physical-host-loss drill and independent heartbeat receiver recovery.
Retention expiration and recovery of custom cluster roles/tablespaces are not
covered by this packaged fixture. R29 has reproducible application-level restore
evidence; deployment-specific recovery promises remain unestablished.
