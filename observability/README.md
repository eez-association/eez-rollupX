# Self-hosted EEZ observability

Implementation branch: `feat/network-observability`. The database-free RPC exporter
and the self-hosted Prometheus/Grafana/Loki/Alloy backend are implemented. Two
provisioned dashboards cover network and collection health. See the
[reviewed plan](../docs/proposals/network-observability.md) for canonical accounting
and tracing, which remain later work. The backend has been exercised with local
fixtures; it has not been connected to the live Chiado deployment.

The shared lifecycle transport now has a [versioned contract](contracts/lifecycle-v1.md)
and [fault-test evidence](validation/lifecycle-transport.md). Native instrumentation
and durable ingestion are still in progress. The [requirement ledger](requirements.md)
tracks each implementation and acceptance gate without treating partial work as a
completed release.

The exporter reads L1/L2 identity and latest heads, the EEZ queue RPC, and an
optional independent L1 reference. It never submits transactions, reads `.env`
files, loads node signing keys, or modifies either chain. Its metrics listener
is loopback-only unless a non-loopback binding is explicitly enabled in config.
The listener has no authentication: use a private network/tunnel for scraping.

Run from this worktree:

```bash
cp observability/config.example.json /tmp/eez-observer.json
# Replace both genesis placeholders and verify chain IDs for this deployment.
cargo run --locked -p eez-observer -- exporter --config /tmp/eez-observer.json
```

Read `eth_chainId` and `eth_getBlockByNumber("0x0", false)` from independently
verified deployment endpoints when preparing the configuration. Genesis identity
is deliberately required, never learned automatically from a potentially wrong
endpoint. The example cannot start until placeholders have been replaced.

`GET /metrics` serves Prometheus text on `127.0.0.1:19100` by default.
`GET /healthz` proves only that the exporter process/HTTP server is running;
it does not claim node readiness or successful cross-chain execution.

Each source is polled independently, waiting 15 seconds after a completed
successful collection. Failures back off 15, 30, then at most 60 seconds. Each
HTTP call has a five-second timeout, all sources share a four-request limit,
redirects are disabled, and responses are capped at 1 MiB. Each collection
rechecks chain ID and genesis, so a reset/repointed RPC cannot keep exporting
previous successful data. The optional reference uses the expected L1 identity.

A failed sample immediately removes its data metrics but preserves the last
successful timestamp. A snapshot older than 90 seconds is omitted even if a
collector becomes stuck. Queue-RPC success is separate from source-state
readiness evaluation freshness. Unknown ages stay absent; restarted/empty queue
counts can legitimately be zero while readiness remains unknown.

Prometheus target configuration owns `environment`, `deployment`, `service` and
`instance` labels. No URLs, hashes, addresses or free-form error messages appear
in metric labels. The [metric contract](contracts/metrics.md) describes the
initial signals; [recording rules](prometheus/recording-rules.yaml) gate raw
values on collection/evaluation validity. Do not sum exporter and future native
producers. Optional host metrics use an existing private node-exporter target. Native
client instrumentation, safe/finalized comparisons and full completion progress
remain future work.

Checks:

```bash
cargo test --locked -p eez-observer
cargo clippy --locked -p eez-observer --all-targets -- -D warnings
cargo fmt -p eez-observer --check
cargo build --locked -p eez-observer
python3 observability/tests/smoke-exporter.py --binary target/debug/eez-observer
```

Tests use ephemeral loopback mock RPC servers only. A sandbox that forbids
binding sockets needs permission for these local tests. The smoke test starts
and stops its own exporter and mock server, verifies HTTP exposition, and
checks failure/recovery across real polling cycles (roughly 30 seconds).

See [initial integration evidence](validation/initial-slice.md) for implemented
scope, findings and outstanding gates. No native Reth recorder, composer,
signer, live Chiado configuration, or protocol data is changed by this slice.

**Start the monitoring backend.**

1. Keep the exporter running with verified chain identities. Its target must be
   reachable from the monitoring containers: a host loopback listener is not
   reachable through `host.docker.internal`. Bind an approved private interface
   with `allow_non_loopback=true`, or supply a private tunnel. Check the actual
   host firewall path; this setup does not modify firewall rules.
2. Create a dedicated approved JSON-log directory and copy
   `stack.example.json` to a private configuration file. Fill the deployment
   labels, exporter target, operator webhook, and independently hosted heartbeat
   receiver. Optional node-exporter/public-RPC/signer targets remain null until
   configured. Supplying a webhook config makes it the notification destination
   when you start the stack; `prepare.py` itself makes no network calls.
3. Prepare and start a separate Compose project:

```bash
python3 observability/prepare.py \
  --config /tmp/eez-monitoring-stack.json \
  --runtime .runtime/observability

docker compose --project-name eez-observability \
  --env-file .runtime/observability/compose.env \
  -f observability/compose.yaml up -d --wait
```

Grafana defaults to `http://127.0.0.1:19300`, Prometheus to
`http://127.0.0.1:19090`, and Loki to `http://127.0.0.1:19310`. Use an SSH tunnel
or authenticated private access for remote viewing. Grafana's initial user is
`admin`; read its generated password from
`.runtime/observability/secrets/grafana-admin-password`. The preparation tool
preserves that file on reruns and never prints its value. Keep the runtime
parent directory mode 0700: generated backend configs can contain webhook
credentials. The password file is readable by the container UID through its
individual secret mount.

Open **EEZ · Network overview** or **EEZ · Infrastructure and collection**.
Both select environment/deployment/instance and display missing stat values as
**Unknown**. Optional reference-head and host panels remain unknown when not
configured. Dashboard files, data sources, recording rules, alerts, contact
points and routing are provisioned from version-controlled inputs.

Loki/Alloy collect only the explicitly selected JSONL files. This Compose profile
mounts those files from the backend host; a separate-node topology requires an
agent on that node and a private transport to Loki, not an assumed shared path.
The packaged blackbox exporter runs on the backend too; configure a separate
probe exporter to test availability from an independent network location.

The notification and storage procedures, heartbeat receiver requirements,
missing-data semantics and recovery limitations are in the
[operations runbook](runbooks/operations.md). A newly configured receiver still
needs a delivery test, and production readiness requires a separate-host
heartbeat receiver and backup/restore drill. The current test proves container-replacement
persistence, not recovery after loss of the monitoring disk.

**Validate the complete backend without live services.**

```bash
python3 observability/tests/test_prepare.py
cargo build --locked -p eez-observer
python3 observability/tests/smoke-stack.py
```

The integration test uses a unique `eez-obs-test-*` project and the pinned
[test-only fixture image](tests/images.lock.json). It runs the real Rust
exporter against fixture RPCs inside that project's Docker network, sends all
notifications to its fixture receiver, and removes only its own containers and
volumes. It checks Compose, `promtool` config/rules, Alloy config, both dashboards'
queries, file-to-Loki ingestion, failure/recovery notifications, container-replacement
persistence and missing heartbeats after stopping the backend. Timings are
shortened only inside the test; normal Grafana alerts evaluate every 30 seconds
with two-minute failure periods and one-minute heartbeats. The final JSON report
is written under `.runtime/` without credentials.

Images are pinned by tag **and** digest in [Compose](compose.yaml) and
[the image lock](images.lock.json). The test mounts the locally built Linux
exporter into its Linux fixture image; build a matching-architecture binary
first. The normal backend deployment uses your separately managed exporter.

The dedicated [Observability CI workflow](../.github/workflows/observability.yml)
runs exporter checks and the isolated backend test on relevant pull requests.
It uploads only the sanitized validation report, not runtime credentials or logs.
