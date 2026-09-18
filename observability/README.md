# Self-hosted EEZ observability

**Start here:** the current target is a [single-machine Docker Compose test setup](runbooks/local-testing.md).
It includes the observer and PostgreSQL, with optional paused alerting so no
webhook receivers or separate hosts are required to try it. Production rollout
and operational hardening are later milestones.

Implementation branch: `feat/network-observability`, integrated on top of
[PR #26](https://github.com/eduadiez/rollupX/pull/26). The
[PR26 integration record](validation/pr26-integration.md) tracks current
compatibility and validation; earlier release records apply only to their named
pre-integration binaries. Settlement policy is mandatory on this baseline while
telemetry remains optional.

The database-free RPC exporter
and the self-hosted Prometheus/Grafana/Loki/Alloy backend are implemented. Two
stage-1 dashboards cover network and collection health; the database profile
adds cross-chain, settlement, blob and gas dashboards. See the
[reviewed plan](../docs/proposals/network-observability.md) for canonical accounting
and tracing. The optional canonical receipt indexer and PostgreSQL profile are
now implemented, along with durable lifecycle transport and native lifecycle
capture. [Source-receipt enrichment](contracts/source-receipts.md) has isolated SQL/RPC
validation. [Protocol-log evidence](contracts/protocol-logs.md) adds separate gap
and conflict coverage. [Admission-history views](contracts/admission-status.md)
distinguish local dispositions and unresolved process exits.
[Canonical outcome checks](contracts/canonical-outcomes.md),
[Admission to observed finality](contracts/finality-timing.md),
[companion/shared-fee attribution](contracts/cost-allocation.md) and optional
[tracing](contracts/tracing.md) now have fixture evidence. The backend has been
exercised with local fixtures; it has not been connected to the live Chiado deployment.

The shared lifecycle transport now has a [versioned contract](contracts/lifecycle-v1.md)
and [fault-test evidence](validation/lifecycle-transport.md). Native admission, preparation, attempt, reconciliation, reorg and local-disposition
capture have component/process evidence. The
[historical schema21 checkpoint](validation/release-runtime.md#current-schema21-checkpoint)
records pre-PR26 native, database, dashboard, network and disk-full checks,
retained strict-CI failures and separate passing corrections. Timing/performance
comparisons are postponed at the user's request until a suitable window or host
is available; deployment acceptance remains open. The
[requirement ledger](requirements.md) maps implementation to evidence and remaining
gates. The earlier [implementation checkpoint](validation/implementation-checkpoint.md)
is retained as historical evidence, not the current release status.

[Network health](contracts/network-health.md) documents optional verified safe-head
and explorer probes, finality age, and the client/host infrastructure panels.

Native process endpoints can now be configured explicitly through generated
`EEZ_TELEMETRY_CONFIG` files; see [startup and precedence](contracts/native-startup.md).
The composer/follower reuse the pinned Reth recorder and signer uses the compatible
standalone recorder. Stack `queue_source` defaults to `rpc`; explicit `native`
requires one configured Composer and uses current sample/evaluation/scrape gates
without RPC fallback. Validate both sources side by side before switching; see
[the native metric contract](contracts/native-metrics.md). Optional
[finite tracing](contracts/tracing.md) and the private [Tempo pilot](runbooks/traces.md)
are disabled until explicitly configured.
The indexer also supports an optional [independent follower comparison](contracts/follower-comparison.md):
configure `follower.url` and `follower.eez` only for an L1-only follower. Reused or
unavailable execution evidence is unknown, including after a follower restart.
[Current required-blob diagnostics](contracts/required-blobs.md) separately confirm
a native derivation dependency; historical archive gaps alone cannot page.

The runtime artifact producer now includes `eez-observer` in its verified archive
and node image. Use a newly built and validated candidate; existing deployed tags
do not acquire the executable automatically. The [node-host procedure](runbooks/observer-host.md)
describes read-only mounts, private connectivity and schema-compatible rollback.
All dashboards include scoped [release annotations](runbooks/annotations.md).
Operators record verified deployment changes in Grafana; configuration generation
does not claim a release occurred. Configured Composer diagnostic sources also
supply startup/active-policy markers through the node-host collector.

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
values on collection/evaluation validity. Do not sum exporter and native
producers. Optional host metrics use an existing private node-exporter target.
Embedded upstream client metrics remain process-wide where chain attribution is
unavailable. Canonical progress/finality have separate observer evidence and must
not be inferred from native attempt counters.

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
scope, findings and outstanding gates for that earlier slice. The optional native
startup described above is subsequent work; live Chiado configuration and protocol
data have not been changed.

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

Set `grafana_public_url` to the HTTP(S) URL reached by the operator’s browser
(including a prefix such as `/grafana/` when using the [HTTPS gateway](runbooks/gateway.md))
when it differs from `http://localhost:19300`. This controls delivered alert and
private **EEZ operator runbooks** links; authentication and loopback binding remain
in place. See [notification links](runbooks/operations.md) for private origins and
dynamic-port fixtures.

Open **EEZ · Network overview** or **EEZ · Infrastructure and collection**.
Both select environment/deployment/instance and display missing stat values as
**Unknown**. Optional reference-head and host panels remain unknown when not
configured. Dashboard files, data sources, recording rules, alerts, contact
points and routing are provisioned from version-controlled inputs.

Loki/Alloy collect only the explicitly selected JSONL files. This Compose profile
mounts those files from the backend host; a separate-node topology requires an
agent on that node and a private transport to Loki, not an assumed shared path.
The optional [node-host collector](runbooks/node-agent.md) generates that separate
readonly Alloy profile and explicit pinned Lighthouse scrape targets; absent
configuration preserves the existing backend behavior.
The packaged blackbox exporter runs on the backend too; configure a separate
probe exporter to test availability from an independent network location.

The notification and storage procedures, heartbeat receiver requirements,
missing-data semantics and recovery limitations are in the
[operations runbook](runbooks/operations.md). A newly configured receiver still
needs a delivery test, and production readiness requires a separate-host
heartbeat receiver and an operator-owned backup/restore drill. The
[isolated restore test](validation/restore.md) proves recovery into fresh volumes,
including SQL checkpoints and retained diagnostic history. Off-host transfer,
physical-host loss and production recovery objectives need deployment validation.

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
It records successful checks in the job summary and retains sanitized failure
reports for three days. Runtime credentials and logs are not uploaded.

**Optional canonical receipt indexing.**

Enable `database_enabled: true` and start the `database` Compose profile using the
[database runbook](runbooks/database.md). Apply the explicit migration with the
admin connection, then run `indexer` with its separate writer identity. Read the
[canonical indexer contract](contracts/canonical-indexer.md) before choosing the
bootstrap and tracked wallet/contract selection. Indexing does not change the
existing database-free exporter mode. The [database validation](validation/database.md)
covers real PostgreSQL recovery fixtures, not live deployment acceptance.


## Browser validation

CI requires real Chromium navigation in addition to datasource query checks. For
an isolated local run, use the same disposable Playwright installation as the
[existing explorer verifier](../deploy/explorers/README.md):

```bash
browser_work="$(mktemp -d /tmp/eez-observability-browser.XXXXXXXX)"
npm install --prefix "$browser_work" --no-audit --no-fund playwright@1.63.0
PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  "$browser_work/node_modules/.bin/playwright" install chromium
NODE_PATH="$browser_work/node_modules" \
  PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
  python3 observability/tests/smoke-protocol-dashboards.py --browser \
    --report /tmp/eez-dashboard-browser.json
```

This needs Docker, Node/npm, Python with PyYAML and Chromium's system libraries;
CI installs those libraries through Playwright's `install --with-deps chromium`.
The fixture creates its own internal network and temporary credentials, publishes
no service ports, and uses Linux host access to the owned Grafana bridge address.
It exercises synthetic accounting/native observations, all generated queries,
and actual request links. It does not access a live deployment. Reports preserve
failure diagnostics; see [dashboard evidence](validation/protocol-dashboards.md)
for verified scope and remaining deployment gates.

Before live rollout, complete the private [deployment record](runbooks/deployment.md).
It collects host/owner/endpoint inputs, ordered rollout checks, backup recovery
targets and the required seven-day baseline; the template is not live deployment
evidence. Native trace-loss delivery and semantic recovery are covered separately
by the [real native alert test](validation/native-loss-alerts.md).
