# Self-hosted EEZ observability

Implementation branch: `feat/network-observability`. This first slice implements
the database-free WP3a RPC exporter. The [reviewed plan](../docs/proposals/network-observability.md)
covers the later Grafana/Prometheus/Loki stack, canonical ledger and tracing.
Those services are not deployed by this change.

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
producers. Host metrics, latest/safe/finalized comparisons, full completion
progress and the actual Prometheus/Grafana deployment remain future work.

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
