# Optional native telemetry startup

`EEZ_TELEMETRY_CONFIG` is an explicit path to a strict JSON file, at most 64 KiB.
Unset means existing startup, CLI settings and protocol behavior are preserved.
The telemetry loader does not discover `.env` or read signing keys. Node startup
retains its existing dotenv/deployments loading before resolving this variable;
the signer retains its existing configuration precedence. Invalid JSON, unknown
fields, wrong service identity and invalid endpoint/storage limits fail startup
before protocol work begins. Runtime lifecycle storage/lock failures instead
disable that exporter with an explicit diagnostic and leave settlement available.

Generate native files and matching scrape identities from the existing stack
configuration by adding `native_processes`:

```json
{
  "native_processes": [
    {
      "service": "eez-composer",
      "instance": "node-1",
      "metrics_target": "private-node:19101",
      "metrics_listen": "127.0.0.1:19101",
      "lifecycle_directory": "/var/lib/eez-lifecycle/composer"
    }
  ]
}
```

This is an additional section in `stack.example.json`, not a complete stack
configuration. The services accepted are `eez-composer`, `eez-follower` and
`eez-proof-signer`. Omit `lifecycle_directory` for metrics-only startup. Each
service/instance and scrape target must be unique. The example listener requires
a private forwarding path from `private-node:19101` to its loopback socket. For
a private interface bind, set its actual IP and explicitly add
`"allow_non_loopback": true`. Preparation makes no network calls or firewall changes.

`prepare.py` writes `native/<service>-<instance>.json` in the private runtime
directory. Place the generated file on the node host and set its path in the
existing runtime environment. The same source configuration generates Prometheus
target labels and the process lifecycle identity; do not add duplicate application
labels for environment/deployment/service/instance.

The generated process JSON has `identity`, optional `metrics` (listen and
allow_non_loopback), optional `lifecycle` ([spool limits](lifecycle-v1.md)), and optional
`traces` ([trace pilot](tracing.md)).
At least one signal must be enabled. Existing Reth `--metrics` takes precedence
over the file's listener; the selected listener must still satisfy the explicit
non-loopback opt-in. Existing CLI usage without this new file is unchanged.
The signer has no prior metrics flag and uses the configured private listener.

## Recorder ownership and effective names

The embedded node calls the pinned Reth custom-builder hook before CLI startup.
It populates the same singleton used by the CLI and both embedded clients.
Reth owns the HTTP endpoint and upkeep; EEZ installs no competing recorder or
subscriber. Existing upstream metrics without chain labels remain process-wide.
A second endpoint or target label cannot recover already merged L1/L2 metrics.

The standalone signer installs the compatible Prometheus recorder once and owns
its listener task. Existing JSON logging and `RUST_LOG` initialization remain
unchanged. The library's default feature set does not include native networking.

| Logical metric | Composer/follower exposition | Signer exposition |
| --- | --- | --- |
| Build version gauge | `reth_eez_build_info{version}` | `eez_build_info{version}` |
| Start timestamp gauge | `reth_eez_process_start_timestamp_seconds` | `eez_process_start_timestamp_seconds` |
| Lifecycle configured/running gauge | `reth_eez_lifecycle_writer_enabled` | `eez_lifecycle_writer_enabled` |
| Lifecycle drop counter | `reth_eez_telemetry_dropped_events_total{signal,reason}` | `eez_telemetry_dropped_events_total{signal,reason}` |
| Lifecycle I/O error counter | `reth_eez_telemetry_writer_errors_total{reason}` | `eez_telemetry_writer_errors_total{reason}` |

Writer-enabled indicates successful initialization, not storage health; inspect
errors, controls and collection freshness. No epoch label is added on restart.
Build version is a bounded release label. Metric samples still require actual
process scrape/devnet evidence before declaring WP0/WP1 complete.

Builder buckets are configured before registration for the forthcoming native
`eez_stage_duration_seconds` and `eez_request_duration_seconds` histograms (same
Reth prefix rule). Stage boundaries in seconds are
`.001,.005,.01,.025,.05,.1,.25,.5,1,2.5,5,10,30,60,120,300,600,1800`;
request boundaries are `.1,1,5,10,30,60,120,300,600,1800,3600,21600,86400`.
Both include the exporter-generated `+Inf` bucket. Existing upstream summary
configuration is untouched. Transport-layer tests verify classic bucket output
with both names; duration producers belong to WP4. Current queue recording rules
continue selecting only the RPC exporter until side-by-side native validation.

## Shutdown and remaining integration

The entrypoints signal lifecycle shutdown after the existing node/service drain
with zero additional wait. Composer/follower tracing computes a conservative
remaining budget from the launcher's drop boundary, which precedes the pinned
Reth runner's graceful-drain timer. This application uses its default runner
configuration; time spent in both protocol drain and runtime teardown consumes
that budget. The trace guard waits at most 25% of the remaining time, capped at
two seconds. Startup errors, an unstarted launcher, or an exhausted deadline
permit no wait. The signer still has no finite total drain deadline and passes
zero after `wait_until_idle`; protocol-worker ownership is unchanged. See the
[trace contract](tracing.md) for pilot limits and remaining acceptance gates.

This startup foundation emits process/segment metadata. Composer also wires
[admission, replacement and readiness capture](native-lifecycle.md) before ingress.
It does not authorize protocol-success claims.
[PostgreSQL transport ingestion](lifecycle-ingestion.md) is implemented with
isolated recovery evidence. Remaining attempt/outcome transitions, trace export, service-log adapters,
devnet neutrality/performance evidence and live rollout remain in the requirement
ledger. The initialization change does not modify consensus encoding, signed
inputs, ingress decisions, attempt ownership or the signer's request drain.
