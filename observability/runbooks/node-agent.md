# Node-host client metrics and log collector

This optional profile keeps the observer and Alloy next to their readonly local
sources. The monitoring host receives HTTP scrapes and Loki pushes over a private
network or authenticated tunnel. It does not mount the node's filesystem. This
profile does not start or reconfigure existing node/client processes.

Add the following optional fields to the existing stack JSON, replacing the
example addresses, directories and identities with the deployment's values:

```json
{
  "client_targets": [
    {"service": "lighthouse", "instance": "consensus", "metrics_target": "consensus-private:5054"}
  ],
  "node_agent": {
    "instance": "node-one",
    "metrics_target": "node-private:12346",
    "listen": "127.0.0.1:12346",
    "loki_url": "http://monitor-private:19310/loki/api/v1/push",
    "read_gid": 10001,
    "run_uid": 10001,
    "run_gid": 10001,
    "sources": [
      {"service": "eez-composer", "instance": "composer", "kind": "reth_json", "directory": "/srv/eez/diagnostic"},
      {"service": "eez-composer", "instance": "composer", "kind": "lifecycle", "directory": "/srv/eez/lifecycle"},
      {"service": "lighthouse", "instance": "consensus", "kind": "lighthouse_json", "directory": "/srv/lighthouse/diagnostic"}
    ]
  }
}
```

All fields are optional at stack level. Absent `client_targets` means no client
scrape; absent/null `node_agent` means no generated node-host profile. An explicitly
configured unavailable target alerts; an unconfigured capability stays unavailable.
Do not describe a running collector as proof that every configured producer writes
files. Verify each source's emitted records and local `loki_source_file_*` diagnostics;
per-file path series are deliberately dropped from the remote Prometheus scrape.

Run the normal `prepare.py` command. It generates `runtime/node-agent/config.alloy`,
`compose.yaml` and a capability manifest. Place those generated files on the node
host and regenerate there (or update only the generated configuration bind source
to its actual node-host path). Every configured source directory must already exist.
The generator does not create/chmod/chown producer directories. The resulting
Compose file is independent from the monitoring stack:

```sh
docker compose -p eez-node-agent -f runtime/node-agent/compose.yaml config --quiet
docker compose -p eez-node-agent -f runtime/node-agent/compose.yaml up -d
```

Only dedicated source directories are mounted, readonly. There is no Docker
socket, host PID namespace or key mount. Alloy uses a persistent `positions` volume,
256 MiB memory limit, half a CPU, no capabilities and no privilege escalation.
`read_gid` supplies a supplementary group for 0640 files. The default process user
remains the pinned image's `alloy` account. Optional paired `run_uid`/`run_gid`
select a nonzero numeric producer owner for 0600 files. Its one-shot initialization
service has only `CHOWN`/`DAC_READ_SEARCH`, no network, and access only to the dedicated positions
volume. It updates that volume recursively so an explicit owner change can retain
existing checkpoints. Stop the collector before changing its process owner, then
regenerate and start the same Compose project. It never touches host files. Root-owned 0600 producer logs are unavailable under the current nonzero-owner
profile; this tool does not change their owner or permissions. Use separate agents
for incompatible file owners; do not grant broad filesystem access or make diagnostic files public.

File discovery runs every five seconds. A rotated path discovered for the first
time can replay retained records, including after a restart; diagnostic transport
is not exactly-once. Known-path checkpoints persist across graceful restarts.
Lifecycle accounting deduplicates its independent typed sequence identities.

The listener defaults to loopback. Expose it through a private tunnel or explicitly
set a private address and `allow_non_loopback: true`; configure firewall access
outside this tool. `metrics_target` is the address reachable from Prometheus,
which can differ from the local listener. `loki_url` must include the push path;
embedded credentials/query tokens are rejected. Provision authenticated transport
outside this minimal profile if the private network alone is insufficient.

## Producer capabilities

Pinned Reth supports `--log.file.format json --log.file.name eez.jsonl` with
`--log.file.directory`, `--log.file.filter`, `--log.file.max-size` and
`--log.file.max-files`. Preserve the operator's existing filter configuration.
Its default file filter is **debug** and `RUST_LOG` target directives also apply;
a global `info` setting does not guarantee sensitive debug records never reach
the local file. Keep that dedicated local storage private and bounded. The
collector exports only [reviewed event names](../contracts/native-log-events.json)
and individually constrained scalars; it never exports arbitrary message/error
bodies, raw signed transactions or debug/trace records. Unexpected or malformed
fields become null; unreviewed event names are dropped. Hashes and trace IDs stay
inside JSON, never indexed Loki labels. These logs explain decisions; complete
request/attempt history still comes from the separately indexed lifecycle SQL.

Both pinned Lighthouse 8.1.2 and 8.1.3 support `--metrics --metrics-address
127.0.0.1 --metrics-port 5054`, and `--logfile-dir /srv/lighthouse/diagnostic
--logfile-format JSON --logfile-debug-level info --logfile-max-size 10
--logfile-max-number 3`. These are explicit deployment settings, not changes made
by this generator. Metrics are disabled by default. Its default `beacon.log`
permissions are 0600: match the collector's explicit owner. **Do not use
`--logfile-no-restricted-perms`** for this profile: the pinned binaries force 0644,
even under umask 027. The collector reads only `beacon.log*`; auxiliary libp2p and
discv5 files are not included. Reviewed static messages and finite numeric fields
are exported; opaque upstream errors and peer identifiers are omitted.

The proof signer currently supports its existing JSON stdout subscriber and the
separate lifecycle file writer. It has no configured diagnostic file adapter in
this profile; `signer_json` is rejected. Signer lifecycle files may be collected.
Do not claim diagnostic log coverage from TCP reachability or a lifecycle source.

## Identity, retention and annotations

Embedded execution metrics share a process-global recorder. Unlabelled pool
counters mix events; gauges set by either embedded instance can overwrite one
another. These are **mixed/unattributable client observations**, not per-chain
series and not necessarily meaningful combined totals. Use the observer's
identity-verified L1/L2 head metrics for chain views. Process CPU/RSS remain valid
process-wide signals. Lighthouse uses an explicit service/instance scrape identity;
no L1/L2 labels are invented after collection. The reviewed scrape subset excludes
hash-labelled gossip families. Native scrape jobs likewise drop complete upstream
`hash`/`address`-labelled samples; the raw private Reth registry still exposes its
upstream families. Use the generated scrape policy rather than removing those
labels and creating collisions. Missing optional samples remain missing.

Only environment, deployment, service, instance and job are emitted as log labels;
Loki may derive bounded `service_name` and `detected_level`. Policy/startup markers
can be queried using the existing Loki datasource, for example:

```logql
{environment="chiado",deployment="eez-chiado",service="eez-composer"}
  | json | event_name=~"eez.composer.(settlement.policy|emission.limits)"
```

These are emitted at actual Composer initialization; they identify startup and
active settings rather than a release revision or an observed hot reload. Current
settings also have native gauges. Loki's configured retention is 14 days, so older
annotations can expire. Trace navigation is optional and expires independently;
SQL lifecycle/canonical records remain the durable lookup. Do not label package
version `0.1` or upstream Reth's build SHA as the EEZ release revision.

See [client/collector validation](../validation/client-capabilities.md) for measured
capabilities, failure evidence and remaining deployment checks. Physical lifecycle
storage uses its separate [quota procedure](lifecycle-storage.md); diagnostic
rotation limits are not proof of a physical disk quota.
