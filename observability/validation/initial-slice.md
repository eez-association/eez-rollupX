# Initial implementation evidence

Base: main at `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`.
Worktree: `target/worktrees/network-observability`, branch `feat/network-observability`.
Date: 2026-09-16. This is an implementation start, not completion of stage 1.

| Area | Evidence / decision | Status |
| --- | --- | --- |
| WP0 source inventory | Reth's pinned recorder is process-global and prefixes names; the locked exporter defaults to summaries unless configured with buckets | Source-verified; native devnet metric capture remains pending |
| Existing UI collector | Inspected separate `eez-rollup-ui/network-observatory` README and implementation entrypoints. It is a Python service with bounded canonical windows and its own UI/decoding responsibilities | Reuse behavior/fixtures where useful; no runtime dependency from the Rust RPC exporter |
| Identity | Configured chain ID and full genesis hash required per deployment endpoint; verified on every source collection | Implemented and mock-tested |
| Queue semantics | Disjoint counts, checked sums, absent unknown ages, independent source-evaluation freshness | Implemented and mock-tested |
| Endpoint failures | Remove failed data immediately; preserve last-success time; unsupported queue does not disable heads | Implemented and mock-tested |
| Resource bounds | Shared four-call semaphore, five-second HTTP timeout, 1 MiB response cap, 15/30/60-second retry delays | Implemented; concurrent calls and response cap tested |
| Metric contract | `contracts/metrics.md`, bounded names/labels and readiness-gated recording-rule examples | Exporter contract implemented; rule deployment/evaluation belongs to WP2 |
| Build dependencies | Existing locked axum 0.8.9, reqwest 0.12.28, Tokio and serde; no Reth/native dependencies in observer | Offline build supported; no dependency versions upgraded |
| WP1/native telemetry | No changes to composer/signer subscribers or Reth recorder | Deferred until native startup integration is demonstrated |
| WP2 | No monitoring backend or alert route deployed | Pending |
| WP3b and later | No database, cost ledger, lifecycle event writer, distributed traces or live canary | Pending |

Validation for this slice:

- `cargo test -p eez-observer --offline`: nine tests passed, including loopback mock RPCs.
- `cargo clippy -p eez-observer --all-targets --offline -- -D warnings`: passed.
- `cargo fmt -p eez-observer --check`: passed.
- `cargo build --locked --offline -p eez-observer`: passed.
- Executable smoke test: passed HTTP metrics/health, absent null ages, isolated queue RPC failure, preserved last-success timestamp, unaffected L1 head, automatic recovery and graceful SIGTERM shutdown.
- Markdown local links, YAML parsing, example JSON and `git diff --check`: passed. Prometheus rule evaluation is not yet validated with promtool (not installed); WP2 owns that gate.

The initial sandbox run passed the six pure tests but prohibited the three
loopback listener tests (`Operation not permitted`). The complete suite passed
with permission for local socket tests; no public RPC was contacted. The native
Reth/Lighthouse metric sample, lifecycle schema and complete WP0 capability
matrix remain outstanding, so WP0 is not marked complete.
