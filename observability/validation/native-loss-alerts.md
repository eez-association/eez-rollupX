# Native trace loss notification and recovery

`native_trace_loss_alert_delivery_and_recovery` passed **1/1 in 114.26 seconds**
with Rust 1.94.0 and locked dependencies. The [bounded report](native-loss-alerts.json)
records the actual test/runtime/source hashes and notification results. This is a
source E2E execution, not a shipping-artifact or production-rollout claim.

The native test uses the generated Composer telemetry configuration, the production
native trace-loss expression, and digest-pinned Prometheus/Grafana containers.
An owned loopback OTLP endpoint initially returns HTTP 503. Prometheus scrapes
the real Composer endpoint; Grafana must deliver an actual firing webhook for
the correct deployment and native rule. No replayed metrics drive this test.

While export fails, the existing bidirectional workload checks successful source
receipts, both destination values, exact committed-root/safe-hash convergence and
the posted signature against independently recomputed public inputs. After the
notification and those semantic checks, the collector starts accepting exports.
A second wave uses fresh nonces and must produce one additional event at each
destination, so the previous values cannot substitute for new execution.

Recovery requires an actual resolved notification, successful native exports,
continued scrape health and a cumulative drop counter that has not reset. Both
waves also check for process death, divergence and poison eviction. The final run
recorded **41 rejected and 17 accepted OTLP requests**, totaling 30,804 bytes.

The controller limits requests to 2 MiB, total received trace bytes to 32 MiB,
and saved notifications to 1,000. Prometheus has 256 MiB/one CPU; Grafana has
512 MiB/one CPU and a 384 MiB Go memory target. Both remained running without an
OOM kill. Linux host networking is used solely for explicitly bound loopback
listeners, with leased ports and unique container names. Cleanup checks that all
owned container names are absent; the Rust owner has bounded failed-test cleanup.
Only selected diagnostics are retained, excluding generated secrets.

Reproduce from the matching checkout, using the normal native test prerequisites:

```sh
cargo +1.94.0 test --locked -p eez-node-e2e --test cross_chain \
  native_trace_loss_alert_delivery_and_recovery -- --exact --nocapture
```

The native-suite preflight pulls both pinned backends. The controller and its
runtime inputs are read from the matching checkout, as with the existing native
builder/genesis fixtures; no runtime compilation of the controller occurs.

Limits: test alert timing is a 30-second loss window, 10-second hold/evaluation,
and shortened notification grouping. This verifies the real delivery/recovery
path, not production timing or an independent off-host notification service.
The final combined rerun verifies the delivered dashboard/rule/runbook URLs
against the actual leased Grafana origin and confirms the provisioned runbook
contains the native telemetry diagnosis. The earlier 113.85-second pass remains
recorded separately. Strict Clippy passed for the native cross-chain test after
a behavior-preserving quota-helper simplification; formatting, Python syntax,
native-suite shell syntax and diff checks passed. Performance, final shipping
artifacts and deployment acceptance remain separate gates.
