# Optional native trace pilot

Tracing is disabled unless the native `EEZ_TELEMETRY_CONFIG` JSON contains
`traces`. The same `identity` supplies service, instance, environment and deployment
resource attributes; applications do not add these identities to metric labels.
`prepare.py` accepts this optional section on each `native_processes` entry and
copies it into the generated native file:

```json
{
  "traces": {
    "receiver": "127.0.0.1:14318",
    "sample_ratio": 0.1,
    "queue_capacity": 2048,
    "export_timeout_ms": 1000
  }
}
```

This is a configuration fragment. The receiver is an explicit IP and nonzero port
for a private OTLP/HTTP endpoint; export always uses `/v1/traces`. Non-loopback
addresses require `allow_non_loopback: true`. No endpoint credentials, arbitrary
headers, proxy environment, redirects, or `OTEL_*` configuration are consumed by
this exporter. The root sampling ratio is required and must be finite in `[0,1]`;
parent sampling decisions propagate. The value is an operator pilot choice, not
an already calibrated production default. Queue capacity defaults to 2048 and
must be 1–16384. Export timeout defaults to 1000 ms and must be 1–2000 ms.

Composer/follower add a filtered layer through the pinned Reth CLI application's
existing tracing-layer hook. Its existing stdout/file layers remain installed.
Native tracing plus Reth's separate `--tracing-otlp` exporter fails configuration
validation because two OpenTelemetry span layers would make propagation
ambiguous. The standalone signer adds the same layer beside its existing JSON
logging layer. `RUST_LOG` controls those logs independently; only explicitly
instrumented `eez_trace` spans are exported. Diagnostic log events and arbitrary
error messages are not copied into trace events.

The finite span chain is preparation → proving episode → numbered retry attempt
→ prover client → signer RPC. W3C `traceparent`/`tracestate` metadata is injected
on each complete new gRPC request. The signer extracts it before consuming the
stream and sets the remote parent **before entering** its span; the pinned bridge
materializes parent identity on entry. Invalid, duplicate or oversized metadata
is ignored and counted without changing RPC admission, payload validation,
statuses or deadlines. Baggage is never propagated. No protobuf, calldata,
signed envelope, proof or consensus commitment changes. The existing successful
`eez.prover_client.attested` event also carries an optional explicit `trace_id`
field: native Reth log filters can omit `eez_trace` span fields even while the
separate exporter records those spans. The node-agent's reviewed event allowlist
and scalar projection retain this field without exporting arbitrary log text.

The prepared ordinary-block path adds finite client/server span pairs:
`prover_capabilities` → `capabilities`, `prover_prepare_ordinary` →
`prepare_ordinary`, and `prover_retain_ordinary` → `retain_ordinary_ranges`.
Each RPC propagates the same bounded W3C context contract, including a final
`Prove` request containing cached ranges. Preparation workers inherit their
request span even after the await expires; trace lifetime does not change worker
ownership or cancel execution. Capabilities, prepared facts and leases are not
attestations and emit no `window_signed` event. The final client attestation
records the logical block count, not the smaller count of wire chunks.

Each captured preparation links finite source-reference spans carrying the exact
admission epoch/sequence and source chain/genesis/rollup/hash/direction. Links are
bounded to 128 and overflow is visible. These are observations at preparation,
not invented admission-to-finality durations. Existing lifecycle records receive
optional current trace/span IDs on publication, while their delivery remains
independent of sampling. SQL admission/attempt identities and canonical receipts
remain usable without any trace; an absent, sampled-out or expired trace is not
missing accounting.

## Bounds and health

The exporter has one dedicated thread, a bounded nonblocking channel, batches of
at most 128 spans and no application-side retry queue. SDK limits are 32
attributes, zero events, 128 links and four attributes per link per span. Callers
use reviewed finite span names and bounded fields. The worker handles encoding,
HTTP and bounded protobuf responses, and counts collector partial rejection.
Failures do not change protocol work or prevent startup. The optional [Tempo/Alloy profile](../runbooks/traces.md) supplies separately
bounded collector retries, seven-day retention and persistent trace storage.

| Logical metric | Meaning |
| --- | --- |
| `eez_trace_export_enabled` | Export worker initialized; not collector health |
| `eez_trace_sample_ratio` | Configured root sampling ratio |
| `eez_trace_sampling_decisions_total{decision}` | `sampled` or `sampled_out` SDK decisions |
| `eez_trace_exported_spans_total` | Collector accepted spans |
| `eez_telemetry_dropped_events_total{signal="trace",reason}` | `queue_full`, `contention`, `shutdown`, `export_failure`, `collector_rejected`, `invalid_context`, or `link_limit` |

Composer/follower use the existing `reth_` exposition prefix. Identifiers are
trace/SQL fields, never metric labels. The invalid-context and link-limit
categories count rejected context/link observations, not whole rejected RPCs.

Shutdown is signaled after protocol draining. The guard accepts a known remaining
application budget and waits at most `min(2 seconds, remaining / 4)`. Dropping it
or passing zero performs no wait. The worker owns no Tokio blocking task whose
join would extend runtime shutdown. Composer/follower use the pinned runner's default graceful duration and a
launcher-drop timestamp that precedes its drain timer. They subtract all elapsed
drain/teardown time before calling this guard; errors or missing/exhausted timing
permit zero wait. The signer explicitly has no total application shutdown deadline. After its
existing tonic and request-slot idle drain completes, it uses the distinct
`shutdown_without_application_deadline` path, allowing at most two seconds for
traces. This is an additional bounded post-drain allowance, not an invented
application deadline or a limit on protocol validation. Unknown/exhausted finite
budgets still permit zero wait, and Reth's remaining-budget path is unchanged.
The assumption is that the specification's two-second cap applies when there is
no finite application budget; if a signer-wide deadline is introduced later,
its caller must switch to the actual remaining-budget API. Trace export remains a pilot until clean-host recovery, workload performance
and deployment calibration gates have been exercised. The [native capture](../validation/native-trace-pilot.md)
verifies connected batch/retry/source identities and successful JSON correlation;
the [retention fixture](../validation/trace-retention.md) verifies physical expiry
and accounting independence. Disposable backend tests cover ingestion, navigation, collector
overload/outages and persistent storage across container replacement. Component and real gRPC evidence is
recorded in [the validation report](../validation/tracing.md).

## Mixed versions and rollback

Existing native JSON without `traces` keeps tracing disabled. Upgrade binaries
before adding the new field: older binaries deliberately reject unknown config
fields. Remove `traces` before rolling those binaries back. W3C gRPC metadata is
optional and older signers ignore it; a newer signer receiving no context starts
a local trace. Tracing itself does not change protobuf or signed data. The
underlying derivable operations rollout separately requires compatible producer,
signer and follower binaries and the validation-identity agreement described in
the [RPC specification](../../crates/eez-control-rpc/SPEC.md#21-validation-identity-and-capability-discovery).
Lifecycle v1 already defines optional trace/span IDs, so enabling them needs no
transport or PostgreSQL schema migration. Metrics must be configured/scraped to
observe exporter counters; generated native stack configurations include that
endpoint. A hand-written trace-only file intentionally does not create a new
metrics listener.
