# Signer trace shutdown allowance

The exporter regression suite passed **43/43**, including the actual reachable
and deliberately stalled collector cases below. Signer **287**, prover-client
**15**, and control-RPC **4** tests also passed. These checks preserve the existing
validation, signature verification and detached-worker drain regressions.

The signer has no overall application shutdown deadline. Its existing tonic
request drain and `wait_until_idle` remain authoritative, including detached
blocking validation that still owns the request permit. Optional telemetry must
not bound or interrupt that work.

The chosen interpretation of R24 is an explicit **at most two-second allowance
after that protocol drain**. `Guard::shutdown_without_application_deadline`
implements this case separately from `shutdown(remaining)`, which still uses
`min(2 seconds, remaining / 4)`. Unknown/exhausted finite budgets remain zero;
dropping the guard adds no wait. No fictitious eight-second application budget or
new deployment option is introduced. If a signer-wide deadline is added later,
its caller must use its actual remaining budget instead.

This assumption follows the requested default two-second cap when there is no
finite remaining application budget. The optional clarification received no
answer; implementation proceeded within the authorized observability scope.

The added tests use actual loopback HTTP sockets. The reachable case decodes the
exported protobuf and verifies resource identity and exclusion of arbitrary log
events. The stalled case confirms an accepted request, queues a second batch,
and requires the independent total flush allowance to expire while the collector
still supplies no response. Existing finite-budget/exhaustion tests and disabled
configuration coverage remain in place. These are exporter-component tests, not
a claim of forcefully bounding total signer process shutdown.

Commands (Rust 1.94.0, locked dependency graph):

```sh
cargo +1.94.0 test --locked -p eez-telemetry --features native --lib
cargo +1.94.0 test --locked -p eez-proof-signer -p eez-prover-client -p eez-control-rpc
```

Local logs: `/tmp/eez-native-trace-shutdown-telemetry-tests.log` and
`/tmp/eez-native-trace-shutdown-rpc-tests.log`. The complete telemetry suite took
2.02 seconds; the stalled test returned a failed flush within its 2.5-second test
wall-clock ceiling, after the independent two-second budget. No complete signer
application deadline, forceful worker termination, or production deployment is
claimed by this component result.

The combined current-source native batch/retry pilot also passed (see
[native-trace-pilot.md](native-trace-pilot.md)). Final strict all-target Clippy
passed for telemetry, signer, client, testkit and native E2E crates; log:
`/tmp/eez-native-trace-shutdown-final-clippy.log`.
