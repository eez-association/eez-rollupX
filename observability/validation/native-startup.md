# Native startup validation

2026-09-16, Rust 1.94.0, branch `feat/network-observability`. The protocol
submodule was initialized from the existing local checkout at the unchanged
gitlink `3c0cdf4955941a5c23639346932b55b33babebbd`; no contracts were modified.

Passed commands:

```bash
cargo +1.94.0 check --offline -p eez-node -p eez-follower -p eez-proof-signer
cargo +1.94.0 test --locked --offline -p eez-telemetry -p eez-node-common -p eez-proof-signer -p eez-follower -p eez-node --all-features
cargo +1.94.0 test --locked --offline -p eez-telemetry --all-features
cargo +1.94.0 clippy --locked --offline -p eez-node -p eez-node-common -p eez-follower -p eez-proof-signer -p eez-telemetry --all-features --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
python3 observability/tests/test_prepare.py
git diff --check
```

The affected native package suites passed 336 tests: node 33, node-common 12,
follower 8, signer 283. The telemetry suite initially passed 13; after correcting
the shutdown/send race, its final run passed 14, including the new regression.
No tests failed, were ignored, weakened or removed in these successful runs.
Documentation tests passed. Five Python configuration/provisioning tests passed.

New acceptance evidence:

- Pinned Reth's custom builder and subsequent normal startup return the same
  singleton; rendered process metrics have the `reth_eez_` prefix.
- Configured stage/request histograms render classic buckets under both Reth
  prefixing and standalone names; no deployment/epoch labels are injected.
- A real loopback standalone HTTP listener exports build and histogram samples
  and rejects competing global initialization.
- Optional config validates executable role, private binding and strict fields;
  selected file address reaches the node command and explicit CLI address wins.
- Generated native JSON and Prometheus scrape labels have identical identity;
  invalid/duplicate targets fail before runtime files are written.
- A producer reserved before shutdown cannot enqueue behind the writer's final
  drain and silently lose an accepted record. Sequentially consistent send
  reservations fence that race without waiting on the protocol producer.

During review, the dispatch schema was corrected to native future target
number/timestamp, since no block hash exists yet. Native shutdown APIs do not
expose a remaining deadline: startup wiring conservatively passes zero remaining
budget after the existing drain. A budget-aware trace flush remains a WP5 gate.

The Docker provisioning/alert test passed and is recorded separately in
[native-startup-stack.json](native-startup-stack.json). The generated native scrape
target produced an outage notification and a recovery notification. Existing
queue/error/exporter alerts, all dashboard queries, file logs, container replacement
with retained data, and independent heartbeat expiry also passed; disposable
containers and volumes were removed. It uses fixture metrics to validate
scrape and notification routing, not a live signer or chain. Real embedded-client
metric samples, full devnet instrumentation neutrality/performance, lifecycle
transition coverage, tracing and the seven-day production baseline remain open.
