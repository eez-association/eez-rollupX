# Operate the current Chiado observability deployment

This runbook covers the existing single-host Compose deployment: Chiado L1
**10200**, L2 **6291**, registry `0x0ca34c68324dd5ef4bd6813af661d7817e4793b8`,
rollup **1**, deployment block **23118328**. It is an operations/handoff procedure,
not a fresh-install script. Use the [Chiado quick start](chiado-quickstart.md) for a
new network and the [monitoring deployment guide](../../observability/runbooks/deployment.md)
for reusable monitoring assets, or the [fresh single-host testing guide](../../observability/runbooks/local-testing.md) for a new local Compose stack.

The [dated deployment audit](../audits/2026-09-18-chiado-observability-deployment/README.md)
records exact genesis hashes, protocol pin, source/image provenance, functional
canaries, schema upgrades and retained failures. Live native binaries remain at
source `68e5da389b0df815a0b013dea7915735f544aa93`; the schema-23 observer is from
`345f003`. Subsequent branch integration or a merged PR does **not** deploy those
changes. Check image digests against the audit and private build manifests before
any replacement. The live protocol pin is
`3c0cdf4955941a5c23639346932b55b33babebbd`.

## Select the existing resources

The private deployment root is in the **main checkout**, not relative to the
observability worktree. Set its absolute path explicitly:

```bash
set -euo pipefail
: "${MAIN_CHECKOUT:?Set the absolute path to the existing main checkout}"
DEPLOYMENT_ROOT="$MAIN_CHECKOUT/.runtime/chiado-observability-20260918"
MONITORING_ROOT="$DEPLOYMENT_ROOT/monitoring-preparation"
test -f "$DEPLOYMENT_ROOT/compose.sh"
test -f "$MONITORING_ROOT/compose.sh"
test -f "$DEPLOYMENT_ROOT/datadir/genesis.json"
bash "$DEPLOYMENT_ROOT/compose.sh" config --quiet
bash "$MONITORING_ROOT/compose.sh" config --quiet
bash "$DEPLOYMENT_ROOT/compose.sh" ps
bash "$MONITORING_ROOT/compose.sh" ps
```

These helpers/configurations are **host-specific private files, not shipped in
this repository**. Missing files are a stop condition; do not create replacements
from an old conversation or copy an example `.env` over this installation. The
monitoring helper embeds its configuration checkout path. Moving/removing that
worktree requires reviewing this reference first.

| Resource | Selection / ownership |
| --- | --- |
| Native Composer, signer and Lighthouse | `$DEPLOYMENT_ROOT/compose.sh`; project `eez-chiado-observability`; includes fresh names and native telemetry overlay |
| Observer, PostgreSQL, Prometheus, Loki, Grafana and Alloy | `$MONITORING_ROOT/compose.sh`; project `eez-chiado-observability-20260918` |
| Private loopback relay | `public-preparation/host-relay/compose.json`; project `eez-chiado-observability-host-relay` |
| Fresh explorers | Project `eez-chiado-observability-explorers`; private `public-preparation/explorers.env` and `explorers.compose.json` |
| Existing public UI/monitor | Private `public-preparation/public.compose.json`, a **subset** of project `eez-chiado-public`; gateway and TLS state are separate |

Keep `.env`, deployment bindings, keys, genesis, writable datadirs, spools,
monitoring secrets and retained backups private and intact. Use `config --quiet`;
full Compose rendering, `.env` dumps and unrestricted container inspection can
expose credentials. Never use `down --volumes`, `--remove-orphans`, broad cleanup,
or the copied historical deploy/teardown scripts for routine operations. In
particular, `down` against the partial public definition is not a gateway procedure.

The old L2 **6290** and simulated monitoring volumes remain stopped and preserved.
Do not start old and new rollup writers together: they share a funded posting
wallet and ports. Do not run two lifecycle observers against the same recovered
namespace. A fresh namespace or changed genesis is not a repair for missing data.

## Connectivity and access

Use the configured public origin, written here as `https://your-public-host.example`:

| Public path | Purpose |
| --- | --- |
| `/` and `/monitor/` | Existing UI and original monitor |
| `/grafana/` | Grafana, with the preserved administrator account |
| `/rpc/l1`, `/rpc/l2` | L1/L2 RPC; expect chain IDs 10200/6291 |
| `/composer/l1`, `/composer/l2` | Cross-chain submission fronts; health checks must not submit transactions |
| Same configured hostname on ports 4445 / 4443 | L2 Blockscout / Blobscan |

Private host listeners are L1 RPC `127.0.0.1:18645`, L2 RPC `127.0.0.1:18688`,
Beacon `127.0.0.1:6152`, Composer metrics `127.0.0.1:19101` and signer metrics
`127.0.0.1:19102`. Backend loopback ports are Grafana **33000**, Prometheus **33001**,
Loki **33002**, PostgreSQL **33003**. The observer's container port 19100 is unpublished.

The two-service relay forwards through a named-volume Unix socket to these fixed
loopback upstreams. Its bridged alias `eez-private-host-relay:8080` joins
`eez-chiado-explorers_default`; it exposes no host TCP port. Observer/Prometheus
use `/l1`, `/l2`, `/beacon/`, `/metrics/composer` and `/metrics/signer` as appropriate.
The database uses the private ledger network. Do not expose the relay publicly,
add a firewall rule to bypass it, or infer upstream health from its `/healthz` alone.

Grafana's credential is stored only in
`$MONITORING_ROOT/runtime/secrets/grafana-admin-password`. Do not paste it into
commands, documentation or public API requests. The private
`check-monitoring.py` reads that file internally, refuses redirects for its HTTP
checks, and sends authentication only to localhost Grafana. Public health/login
checks remain unauthenticated. A copied password file does not rotate an existing
Grafana database's account.

## Routine checks and start/stop

Run these checks without changing service state:

```bash
curl --fail --silent --show-error http://127.0.0.1:33000/grafana/api/health
curl --fail --silent --show-error http://127.0.0.1:33001/-/ready
curl --fail --silent --show-error http://127.0.0.1:33002/ready
python3 "$MONITORING_ROOT/check-monitoring.py"
```

The private checker records `monitoring-runtime-check.json`. It verifies local
Grafana authentication, datasource health, expected dashboards, the three
observer/Composer/signer scrapes, collection metrics and Loki lifecycle data.
Inspect its reported collection values too: scrape `up=1` alone does not prove
successful indexing, source freshness or cross-chain completion. Expected
namespace is `environment=chiado`, `deployment=eez-chiado-6291-20260918`.

Check both RPC identities directly; expected results are `0x27d8` and `0x1893`:

```bash
curl --fail --silent --show-error -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
  http://127.0.0.1:18645
curl --fail --silent --show-error -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
  http://127.0.0.1:18688
bash "$MONITORING_ROOT/compose.sh" exec -T postgres \
  psql -X -v ON_ERROR_STOP=1 -U postgres -d eez_observer \
  -c 'SELECT max(version) AS schema_version FROM eez.schema_migrations;' \
  -c 'SET ROLE eez_observer_reader; SELECT chain,source_valid,count(*) AS recent_observed_blocks FROM eez.canonical_block_activity WHERE timestamp_seconds>extract(epoch FROM now())-300 GROUP BY chain,source_valid;'
```

Schema version must be **23**. Confirm genesis hashes against the audit before
accepting a replaced endpoint; chain ID alone is insufficient. In Grafana verify
fresh L1/L2 collection, decreasing/zero indexing lag after catch-up, canonical
outcome validity, source/receipt/blob coverage and lifecycle writer health. Registry
escrow is separate from poster balance. Unknown old header metadata, pruned blobs,
an unexercised direction and incomplete 24-hour burn history must remain unknown.

To start the existing services or apply authorized configuration changes, use the
same pinned files (`up -d` does not restart unchanged running containers):

```bash
bash "$DEPLOYMENT_ROOT/compose.sh" up -d --pull never --no-build
bash "$MONITORING_ROOT/compose.sh" up -d --wait --pull never --no-build
```

For an authorized observer process-only restart with unchanged configuration, use
`bash "$MONITORING_ROOT/compose.sh" restart observer`. Changed environment settings
require `up -d` to recreate affected containers; `restart` alone does not apply them.
Do not treat either operation as initial contract deployment. Before a native restart, inspect
the current valid queue and drain accepted work where possible: native pending
queues are not durable. An observer-only restart does not require restarting the
node/signer or redeploying contracts. To stop monitoring or native services, use
the corresponding selected helper's `stop`, preserving its volumes. After restart,
repeat health/identity checks and compare retained receipt identities and fees;
new process epochs must not duplicate canonical charges. Read bounded logs privately,
for example `bash "$MONITORING_ROOT/compose.sh" logs --tail=80 observer`.

## Empty infrastructure panels

The infrastructure overview lists configured host metrics, Lighthouse targets,
public RPC probes and node-host diagnostic sources. Configuration alone does not
prove a producer is healthy. In this deployment these optional targets/collectors
are currently unconfigured: the corresponding host/consensus/probe panels are
unknown, not zero or failed measurements. A running blackbox exporter is not a
configured public RPC probe. Alerts, tracing and follower replay remain deliberately
disabled; none is implicitly enabled by viewing or regenerating dashboards.

Composer process CPU/RSS uses already-scraped `reth_process_*` families alongside
monitoring backend process metrics. The dashboards prefer the unprefixed family
if both aliases exist for one target, avoiding double counting. These are process
measurements, not separate L1/L2 attribution; absent signer/observer process families
are not manufactured. Host memory/disk panels honor the Client / host selector
and successful-scrape gate, just like the other host panels.

Current Loki streams contain Composer/signer lifecycle events. The dedicated
application-log directory is empty and the optional sanitized node-host log
collector is not configured; lifecycle availability is not full diagnostic log
coverage. See the [node-agent runbook](../../observability/runbooks/node-agent.md)
for existing supported capabilities before scheduling any service/configuration
change. Do not enable a follower, tracing, node-exporter or a Beacon restart merely
to remove a No data label.

Native scrape generation drops the unnecessary `process_cli_args` and
`reth_process_cli_args` series. Pinned Reth strips argument values and emits flag
names only; this change is not evidence of leaked secrets. Previously ingested
series remain until normal retention expires. Do not enable the Prometheus admin
API or delete monitoring history to remove them.

## Sparse history and cost coverage

Cross-chain, settlement, gas and blob dashboards now open the last 24 hours.
With four-hour settlement, a one-hour view can contain no posts; select a window
spanning a complete cycle before diagnosing a collection failure. Empty queues
show zero/idle when their complete fresh count is known, while readiness and
oldest ages can remain unavailable. Latency/disposition counters need actual
qualifying events; quiet operation is not an exercised canary.

Funding runway stays unknown until its configured 24-hour history is covered.
The advanced per-settled-transaction cost panels report `prior_history_unavailable`
on this namespace: their exact denominator requires full history from both chain
bootstraps, whereas L1 indexing begins at deployment block 23118328. Waiting alone
does not change that immutable coverage boundary. Actual receipt fees and posting
costs remain available independently. Do not reset bootstrap or weaken coverage
guards merely to populate an estimate.

## Current settlement policy

The verified host configuration is:

| Setting | Value |
| --- | --- |
| `EEZ_SETTLEMENT_PURE_L2_MODE` | `interval` |
| `EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS` | `14400000` (four hours) |
| `EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS` | `14400` |
| L2 block time | 1000 ms |
| `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS` | `16384` |
| `EEZ_SETTLEMENT_BLOB_FULLNESS_BPS` | off |
| `EEZ_MAX_SPECULATIVE_DEPTH` | `0` |

These are deployment settings, not new repository defaults. Cross-chain work
retains priority at the next eligible L1 opportunity. Gas/blob/fee/proof capacity
and safety limits can trigger earlier publication or defer work. The general
threshold counts blocks, not wall time; the pure-L2 interval retains its existing
L1-anchor timer. Do not lower the signer request ceiling beneath the intended
window and then expect the four-hour policy to remain the only periodic trigger.

The [September 19 audit update](../audits/2026-09-18-chiado-observability-deployment/README.md#first-complete-four-hour-observation-and-operations-handoff)
records an actual four-hour interval between successful canonical L1 posting
receipts and 14,400 applied L2 blocks. It is bounded deployment evidence, not a
maximum-load/proving benchmark or a promise that every post waits four hours.

## Observer upgrade and rollback

Use the [schema contract](../../observability/contracts/block-activity.md),
[database runbook](../../observability/runbooks/database.md) and
[restore procedure](../../observability/runbooks/restore.md). Prepare the verified
compatible observer image first; keep native and observer source identities separate.
The private monitoring helper currently stores its **observer image** in
`bindings.json` under the historical key `node_image`. Updating this binding is
not authorization to replace the native Composer image.

For an authorized observer upgrade, checkpoint only after stopping its writer:

```bash
umask 077
BACKUP="$DEPLOYMENT_ROOT/validation/observer-upgrade-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -m 700 -- "$BACKUP"
bash "$MONITORING_ROOT/compose.sh" stop observer
cp -p -- "$MONITORING_ROOT/bindings.json" "$MONITORING_ROOT/service.env" \
  "$MONITORING_ROOT/indexer.json" "$BACKUP/"
cp -a -- "$MONITORING_ROOT/runtime" "$BACKUP/runtime"
bash "$MONITORING_ROOT/compose.sh" exec -T postgres \
  pg_dump -U postgres --format=custom --no-owner --dbname=eez_observer > "$BACKUP/ledger.dump"
test -s "$BACKUP/ledger.dump"
bash "$MONITORING_ROOT/compose.sh" exec -T postgres \
  pg_restore --list < "$BACKUP/ledger.dump" > "$BACKUP/ledger.contents"
sha256sum "$BACKUP/ledger.dump" > "$BACKUP/ledger.sha256"
```

Stop on any failure; do not continue from a partial backup. This is the database
and configuration checkpoint for an observer-only upgrade. A complete diagnostic
volume/spool restore needs the separate quiescing and archive steps in the restore
runbook; copying configuration is not a volume backup.

Stage the reviewed, locally available observer image for the migration first.
Do **not** regenerate dashboards against views that have not been migrated yet.
The shell override below takes precedence over this private helper's env files
and affects only the explicitly selected migration service:

```bash
: "${NEXT_OBSERVER_IMAGE:?Set the verified locally available observer image digest}"
EEZ_NODE_IMAGE="$NEXT_OBSERVER_IMAGE" bash "$MONITORING_ROOT/compose.sh" \
  run --rm --no-deps --pull never observer-migrate
```

Stop on failure and keep the observer stopped; do not advance provisioning or
rewrite migration history. After a successful migration, update **only** the
reviewed observer image binding, preserving chain/genesis/selection and secrets,
then regenerate and start the matching observer:

```bash
python3 - "$MONITORING_ROOT/bindings.json" "$NEXT_OBSERVER_IMAGE" <<'PY_IMAGE'
import json, pathlib, re, sys
path = pathlib.Path(sys.argv[1])
image = sys.argv[2]
assert re.fullmatch(r'(?:sha256:[0-9a-f]{64}|[^\s]+@sha256:[0-9a-f]{64})', image)
bindings = json.loads(path.read_text())
bindings['node_image'] = image
path.write_text(json.dumps(bindings, indent=2) + '\n')
PY_IMAGE
python3 "$MONITORING_ROOT/prepare-monitoring.py"
bash "$MONITORING_ROOT/compose.sh" config --quiet
bash "$MONITORING_ROOT/compose.sh" up -d --no-deps --pull never --no-build observer
python3 "$MONITORING_ROOT/check-monitoring.py"
```

Confirm the expected migration version/checksums, fresh collectors and retained
receipt fees before declaring success. Schema 23 preserves older costs/cursors;
old metadata is null, not zero. Never reset bootstrap, schema checksums or producer
identity to fill missing charts. Seven early malformed native admissions are
intentionally retained as unknown; their payloads are not repair targets.

An old observer refuses a newer schema. Executable-only rollback cannot undo a
migration: restore the pre-upgrade backup into a separate verified instance with
its matching binary/configuration, retain the upgraded database and account for
new observations since the backup. Starting the retained simulated demo restores
only a demo, not real-chain monitoring. Do not present it under the live namespace.

## Handoff and limits

Keep private build manifests, image digests, deployment bindings, backup checksums,
service start times, receipt/fee snapshots and verification reports together under
the existing deployment root. Append sanitized dated evidence to the audit; preserve
initial failures. Do not copy private `.env`, keys or bulk databases into a PR.

Alerts remain paused; distributed tracing and an independent follower are not
configured. RPC/correlation agreement is not independent replay. Performance
comparisons, production capacity and off-host disaster recovery remain outside
this functional single-host deployment. A frozen identity/finality conflict is a
stop-and-investigate condition, not a reason to clear a guard or reset the old rollup.
