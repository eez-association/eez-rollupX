# Native tracing implementation evidence

Date: 2026-09-17. Candidate is the uncommitted `feat/network-observability`
worktree based on `5183f56acd0903a10ad085e97d03aef4070e82a3`, with protocol gitlink
`3c0cdf4955941a5c23639346932b55b33babebbd`. No deployment or live service was changed.

This implements part of R23/R24. The [contract](../contracts/tracing.md) describes
configuration, scope, exporter limits and remaining acceptance gates.

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Disabled default, one identity source and validated optional config | `native::Config`, `traces::Config`, `prepare.py` | Native trace-only loader regression; seven preparation tests including absent/default/invalid/nondefault values |
| Preserve existing subscribers and logging | Reth `CliApp::access_tracing_layers`; signer registry with independently filtered JSON layer | Node/common compile and CLI conflict regression; exporter test excludes arbitrary log events |
| Finite spans and retry identity | Preparation, proving episode, numbered retry attempt, client and signer spans | Component parent-context test and real gRPC fixture; native workload acceptance remains open |
| Bounded source links and persistent diagnostic identity | Preparation membership links; optional lifecycle `Payload.trace` on publication | Two-member link test with exact admission identity; existing lifecycle transport tests remain green |
| W3C propagation before stream consumption | `traces::inject` and `attach_remote_parent`; no protobuf change | Production `RemoteProver` → actual tonic `ProveSvc` → real OTLP HTTP fixture; invalid/duplicate/oversized metadata unit regression |
| No effect on validation/signing | Original signer RPC body retained in `prove_request`; client keeps exact signature/public-input verification | 283 existing signer, four control-RPC and 15 prover-client tests; new successful connected RPC test verifies the production client's attestation check |
| Bounded export and visible loss | One thread, nonblocking bounded channel, 128-span batches, explicit HTTP timeout, SDK limits and closed loss categories | Collector outage/zero-budget shutdown; deterministic queue-full/contention/shutdown counters; actual protobuf export/resource assertions |
| Budget-aware post-drain flushing | Guard waits at most min(2 seconds, remaining/4); Reth uses conservative launcher-drop timing | Budget boundary/error/exhaustion tests; signer still passes zero because it has no finite total deadline |

The connected RPC test uses two separate tracer providers/resource identities
(`eez-composer` and `eez-proof-signer`) and real loopback gRPC and OTLP/HTTP
connections. It asserts composer → production client → signer parent IDs and a
shared trace ID, plus distinct span IDs. Execution uses the signer's existing
stub backend fixture; stream admission, settlement validation, signing and client
attestation verification execute their production paths. It is not a native
Anvil/devnet execution or a Tempo persistence/navigation test.

Commands and local logs:

- `cargo +1.94.0 test --locked -p eez-telemetry --features native --lib`:
  35 passed; `/tmp/eez-observability-tracing-telemetry-reviewed.log`.
- `cargo +1.94.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client`:
  283 signer, four control-RPC and 15 client tests passed before adding the new
  connected test; `/tmp/eez-observability-tracing-rpc-tests.log`.
- `cargo +1.94.0 test --locked -p eez-proof-signer remote_prover_and_tonic_signer_export_connected_spans -- --nocapture`:
  one passed, 283 filtered; `/tmp/eez-observability-tracing-connected-rpc-final.log`.
- `cargo +1.94.0 clippy --locked -p eez-telemetry --features native -p eez-node-common -p eez-node -p eez-follower -p eez-proof-signer -p eez-prover-client --all-targets -- -D warnings`:
  passed; `/tmp/eez-observability-tracing-clippy-reviewed.log`.
- `python3 -m unittest discover -s observability/tests -p test_prepare.py -v`:
  seven passed; `/tmp/eez-observability-tracing-prepare-tests-reviewed.log`.
- `cargo +1.94.0 check --locked -p eez-node -p eez-follower --bins`:
  passed with shipping feature selection; `/tmp/eez-observability-tracing-production-check.log`.
- `cargo +1.94.0 test --locked -p eez-node-common telemetry::tests`:
  three passed, six filtered; `/tmp/eez-observability-tracing-startup-tests.log`.
- Cargo metadata was regenerated offline only to add direct/dev edges to existing
  pinned packages. No external package version, Reth pin or protocol pin changed.

First failures were retained:

- `/tmp/eez-observability-tracing-tests.log`: attaching a remote parent after
  entering the span lost its parent in the pinned bridge. The production signer
  now attaches before instrumenting the RPC future; the regression passes.
- `/tmp/eez-observability-tracing-connected-rpc.log`: test module name shadowed
  `tracing`, and the fixture used its checked wrapper instead of ABI batch value.
  Only the test's imports/fixture conversion changed.
- `/tmp/eez-observability-tracing-connected-rpc-reviewed.log`: the generic signer
  fixture used vkey `0x42…42`, while the production client derives its registered
  vkey from the attester address. The fixture now configures that same identity;
  the client's public-input verification was preserved and caught the mismatch.
- `/tmp/eez-observability-tracing-clippy.log`: strict numeric-conversion and
  large-error lints; fixed without weakening checks. The metrics lane fixed its
  own population conversions. An earlier production node check also caught that
  lane's pinned task-executor signature; it was corrected separately.

The subsequent [Tempo backend validation](tempo-backend.md) adds the optional
profile, actual navigation and collector-failure alert delivery, native JSON trace
IDs, startup health counters and conservative Reth post-drain budgeting.

Remaining R23/R24 scope: a connected native devnet request/batch/retry capture,
actual retention expiry and clean-host recovery, measured native workload
sampling/overhead, native exporter alert delivery, and a defined finite signer
application shutdown budget. These component/backend checks do not prove those
gates or production deployment readiness.
