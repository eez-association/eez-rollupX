# Native Composer-to-signer trace pilot

The `traced_bidirectional_cross_chain_smoke` native test implements the R23
connected request, multi-source batch, retry and JSON-log correlation gate.
The complete test passed **1/1 in 87.88 seconds** after correcting the native JSON
log-correlation gap. It captured 95 spans in 74 OTLP requests (40,587 protobuf
bytes), including fixture setup and ordinary historical/pure-L2 activity. Both
source requests share a preparation with distinct numbered retries and connected
Composer/client/signer ancestry. All 14 successful native client attestation
records carry their explicit trace ID. [The compact manifest](native-trace-pilot.json)
records compiler/source/executable identities and retained logs.

It uses the existing native bidirectional workload: successful inbound/outbound
source receipts, expected values on both destination chains, exact L1 committed
root/L2 safe-hash convergence, and the posted proof signature verified against
independently recomputed public inputs. No signer validation, deadlines, proof
bytes or protocol behavior are bypassed.

The Composer and signer receive separate role-validated telemetry configurations.
An owned loopback OTLP/HTTP receiver accepts their actual protobuf exports, with
2 MiB per request, 16 MiB total and 8192-span capture bounds. Raw captures are saved
only under the explicitly selected local native-test log directory.

The existing test proxy initially returns the closed retryable `Unavailable`
status. It recovers only when an exported numbered retry belongs to a native
lifecycle preparation containing both admitted source hashes. Its stream
reconstruction forwards only bounded `traceparent` and `tracestate` metadata;
baggage, arbitrary metadata and request payloads are not exported.

After semantic settlement, the test requires:

- One shared trace with preparation → proving episode → distinct numbered retry
  → production client → real signer spans and exact parent IDs.
- Composer and signer service/resource identities for the same fixture deployment.
- Both source links bound to their lifecycle admission epoch/sequence, source
  hash, chain/genesis and rollup; the shared preparation's lifecycle trace/span
  IDs must match the exported preparation.
- Existing JSON logs from both real processes containing that same trace ID,
  a successful signer observation, and no protocol divergence or poison eviction.

The target compiled with locked dependencies. The final combined strict Clippy
check passed for telemetry, signer, prover-client, testkit and all native E2E
targets (`/tmp/eez-native-trace-shutdown-final-clippy.log`). The
only new manifest edges are test dependencies on already pinned
`opentelemetry-proto` and `prost`; no external package version changed. First
compile failures (one remaining fixture caller and explicit bounded HTTP-body
error conversion) are retained in the local build logs.

## First native findings

- The first direct executable invocation did not set `RUSTUP_TOOLCHAIN`, allowing
  its nested signer build to select host Rust 1.96.1. That owned build tree was
  stopped before runtime startup. Subsequent commands explicitly select 1.94.0;
  the aborted invocation establishes no runtime result.
- `/tmp/eez-native-trace-pilot-reviewed-run.log`: failed after 154.30 seconds.
  Real exported retries existed, but the predicate expected OTLP `IntValue`.
  The pinned native Reth subscriber records these reviewed numeric fields as
  decimal strings. The predicate now accepts those exact numeric values in both
  representations; no protocol or exporter behavior changed.
- `/tmp/eez-native-trace-pilot-numeric-run.log`: failed after 73.67 seconds at the
  final Composer JSON correlation assertion. Both source effects, signature,
  connected ancestry, multi-source admission links and numbered retries passed.
  Native logging filters exclude the `eez_trace` span target, so the existing
  `eez.prover_client.attested` event now carries an optional explicit trace ID.
  The node-agent allowlist/projection already supports that event and field.
- `/tmp/eez-native-trace-node-agent.json`: all ten actual Alloy/Loki/Prometheus
  fixture groups passed, including a direct-field/no-span attestation record and
  existing secret/debug/unknown-event exclusions. These are synthetic diagnostic
  records through real backends, separately scoped from the native run.

This bounded 100%-sampled development workload is not a production sampling
recommendation, throughput/latency benchmark, Tempo persistence test, or evidence
of a seven-day operational baseline. Those retain their separate acceptance gates.

## Complete current-source execution

```sh
env -u EEZ_TEST_PREBUILT -u EEZ_TEST_RUNTIME_BIN_DIR -u EEZ_TEST_PROOF_SIGNER_BIN \
  RUSTUP_TOOLCHAIN=1.94.0 \
  EEZ_TEST_LOG_DIR=/tmp/eez-native-trace-combined-logs \
  EEZ_TEST_DATADIR_DIR=/tmp/eez-native-trace-combined-datadirs \
  target/debug/deps/cross_chain-7f117aa924f0a582 \
  traced_bidirectional_cross_chain_smoke --exact --nocapture
```

The exact executable was produced by `cargo +1.94.0 test --locked -p
eez-node-e2e --test cross_chain --no-run --message-format=json`; its filename is
an observed artifact, not a stable operator command. A separate locked build of
Composer/follower/signer with their production feature selection passed before
this target. The repository's native E2E target then builds its Composer/follower
wrapper executables from the same role entrypoints; their actual runtime hashes
are recorded separately from the shipping-role build hashes. Signer startup
performed its normal pinned-toolchain freshness build. This local source E2E
result does not claim execution of release-image/archive artifacts.

The successful native rerun includes the explicit deadline-free signer shutdown
API described in [its validation](signer-trace-shutdown.md). The native harness
normally kills fixture processes on cleanup; its success therefore does not
replace the separate reachable/stalled-collector shutdown component tests.
