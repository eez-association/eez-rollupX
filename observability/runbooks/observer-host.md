# Observer on the node host

For single-machine testing, the [Compose observer overlay](../compose.observer.yaml)
and [local testing guide](local-testing.md) manage this same executable and explicit
migration through private Compose networks. The standalone procedure below
requires a separately working database network/tunnel: a published port on an
internal-only Docker network is not sufficient on all Docker versions. The local
Compose setup avoids that host-port dependency.

The runtime producer now builds `eez-observer` with the shipping binaries,
includes it in the checksummed runtime archive and freezes its identity in the
native test payload. The existing node image also contains the same executable;
its normal Composer entrypoint is unchanged. This avoids a second build or a
separate unverified observer image. These packaging changes require a new tested
runtime candidate; older node images do not contain the observer.

Select the exact tested node-image digest through the repository's existing
runtime manifest verification procedure. A mutable `latest` or commit tag alone
does not establish executable identity. The archive consumer
`scripts/ci/prepare-runtime-artifacts.sh` checks source/protocol/lock identity,
archive checksum, expected regular files and executable permissions. Preserve the
manifest and validation evidence with the deployment record. Do not rebuild a
missing executable in the deployment step.

## Node and database compatibility

This observer requires the matching new-network node's
`eez_getSettledL2RangePage` method and database schema 23. The previous full-range
RPC is not a fallback. Stop the old observer, back up its database/configuration,
make the matching paged node RPC available, explicitly migrate with the new
observer binary, then start indexing. Schema-22 and older binaries refuse the
upgraded database. Follow the [database runbook](database.md) for rollback limits.
This observer upgrade preserves historical accounting; it does not migrate a
legacy chain to PR26's tag-3/profile-1 genesis or remove its mandatory settlement
policy requirement.

Pages are bounded to 1,024 members and assembled/verified atomically up to 65,536
applied blocks per receipt. Incomplete, unsupported or inconsistent responses
remain visible collection gaps. Existing memory and request/database deadlines
remain enforced; the format ceiling is not a measured capacity or headroom claim.

## Inputs and boundaries

- Prepare the [indexer configuration](../indexer.example.json) with explicit
  deployment, chain/genesis/rollup, immutable bootstrap/receipt selection and
  lifecycle producer identities. Invalid placeholders must fail before startup.
- Apply the explicit database migration using a separate administrative identity
  on the monitoring host; follow [the database runbook](database.md).
- Give the observer only its writer connection file, indexer configuration and
  read-only lifecycle directory. Use a dedicated host user/group that can traverse
  the spool directory and read its files; match those numeric IDs in the
  container. Do not grant access to node signing keys or unrelated host paths.
- Configure the database through the private authenticated network/tunnel. Native
  RPCs remain private. Host-network mode below permits existing loopback RPC/tunnel
  endpoints; it does not publish or change their bindings. The metrics listener
  remains the explicitly configured address. Check that Prometheus can reach it
  over the intended private path before enabling alerts.
- Stop a standalone exporter using the same metrics port before replacing it
  with indexer mode. The indexer already owns that exporter's collection tasks.

The following example expects an already reviewed configuration directory whose
indexer file refers to `/config/writer.json` and whose lifecycle entries refer to
`/lifecycle`. The numeric observer UID/GID must have the required read permissions.
All variables are operator-supplied deployment inputs, not generated credentials:

```bash
docker run --name eez-observer --restart unless-stopped \
  --network host --user "${EEZ_OBSERVER_UID:?}:${EEZ_OBSERVER_GID:?}" \
  --read-only --cap-drop ALL --security-opt no-new-privileges:true \
  --memory 256m --cpus 1 --pids-limit 128 \
  --mount "type=bind,src=${EEZ_OBSERVER_CONFIG_DIR:?},dst=/config,readonly" \
  --mount "type=bind,src=${EEZ_LIFECYCLE_DIR:?},dst=/lifecycle,readonly" \
  --entrypoint /usr/local/bin/eez-observer \
  "${EEZ_NODE_IMAGE:?use the verified immutable digest}" \
  indexer --config /config/indexer.json
```

The memory/CPU values are initial resource guards, not measured production
capacity. Validate them with the deployment's capped backfill and lifecycle load.
Inspect collection success, cursor freshness, source gaps and database permissions;
`/healthz` alone proves neither indexing coverage nor cross-chain execution.

Receipt cursors and lifecycle offsets live in PostgreSQL; the container requires
no writable local data volume. Restarting it replays retained inputs idempotently.
Do not remove database or lifecycle volumes during observer rollback. Use a
schema-compatible binary or restore into a separate database before switching.
Loki/Alloy and the independent heartbeat receiver remain separate services;
installing this observer does not implicitly configure those host agents.

This procedure is prepared for deployment review. It has not been executed on
the live Chiado hosts. Local runtime-fixture validation and remaining release
gates are recorded in [the requirement ledger](../requirements.md).
