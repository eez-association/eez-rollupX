# Native progress with unavailable telemetry

The complete `unavailable_telemetry_bidirectional_cross_chain_smoke` test passed
**1/1 in 90.76 seconds**. It reuses the existing bidirectional native semantic
workload. No production behavior changed for this test.

The lifecycle directory is deliberately a fixture-owned regular file, producing
storage unavailability regardless of process UID. This is not a permissions or
ENOSPC claim. The OTLP target is an owned TCP listening socket that never accepts
or serves a request; the socket remains reserved through all assertions. Native
sampling is 100%, its export queue is 16 and its timeout is 20 ms. Existing
process-owned metrics remain available independently of both failed transports.

The test requires successful source receipts in both directions, the intended
values on both destination chains, L1 committed-root/L2 safe-hash convergence,
and a posted proof signature checked against independently recomputed public
inputs. Valid work must not suffer poison eviction or divergence. One admission,
committed terminal and completed-duration observation per direction must remain
visible while all current queue states return to zero. The failed lifecycle writer
must expose `writer_enabled=0`; the stalled collector must produce positive trace
export-failure drops with no exported spans. The startup failure appears exactly
once, and private fixture body/path sentinels and transaction metric labels do not
leak. The node remains alive through the final checks.

## Build and retained evidence

The initial source-based command was:

```sh
env -u EEZ_TEST_PREBUILT -u EEZ_TEST_RUNTIME_BIN_DIR -u EEZ_TEST_PROOF_SIGNER_BIN \
  EEZ_TEST_LOG_DIR=/tmp/eez-observability-native-telemetry-failure-logs \
  EEZ_TEST_DATADIR_DIR=/tmp/eez-observability-native-telemetry-failure-datadirs \
  cargo +1.94.0 test --locked -p eez-node-e2e --test cross_chain \
  unavailable_telemetry_bidirectional_cross_chain_smoke -- --exact --nocapture
```

That run reached and passed protocol and loss-metric assertions, then failed the
final log count: the existing testkit helper parses JSON events and correctly
ignored the actual plain stderr notification before subscriber initialization.
The fixture now counts that exact raw line. No runtime behavior or semantic/loss
assertion was changed. The initial failure, logs and datadir are retained; this
was an assertion bug rather than a retry-only protocol pass.

For the explicitly authorized test-only correction, the freshly built Composer,
signer and follower executables were copied into a private immutable runtime
directory and their full SHA256 identities checked. The corrected test target
was compiled normally with `cargo +1.94.0 test --locked -p eez-node-e2e --test
cross_chain --no-run --message-format=json`, then itself copied and hashed. The
complete exact test ran against those snapshots using the testkit's existing
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` overrides. This avoided
rebuilding an unchanged shipping runtime after the test feature graph changed.

[The compact manifest](native-telemetry-failure.json) retains full executable and
source digests, commands' log locations, initial failure details and the
repository-relative snapshot location. The production digest covers the recorded
Rust/manifest/proto input map, retained inside that local snapshot. It is not a
claim of validation against subsequent signer changes; later source checkpoints
need their own integration results. The private snapshot and logs are local-only,
not published artifacts. The checked-in test is included by the existing full
`cross_chain` CI suite.

## Boundaries

This proves canonical workload progress with startup lifecycle-storage failure
and a stalled trace collector. The independent [physical spool drill](spool-quota.md)
proves a disk-backed 3 GiB cap, explicit ENOSPC loss and reader descriptor behavior.
Neither result alone proves native protocol progress during a dynamically filled
spool or deliberately lagging reader. Comparative telemetry overhead, lock hold
times, sustained saturation and deployment-host quota enforcement remain separate
gates. The 90.76-second canary duration is not an overhead measurement.
