# Implementation checkpoint

This checkpoint prepares the implementation for review and release CI. It does
not close the [acceptance ledger](../requirements.md). The branch includes the
agreed frozen main revision `5469c88860408daf776ee812156dd280c1a9e262`; the
protocol pin remains `3c0cdf4955941a5c23639346932b55b33babebbd`.

## Local validation

| Check | Result and scope |
| --- | --- |
| `cargo +1.94.0 fmt --all --check` | Passed |
| `cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed, 11.38 seconds |
| `cargo +1.94.0 test --locked -p eez-composer -p eez-observer -p eez-telemetry -p eez-proof-signer -p eez-prover-client -p eez-control-rpc --features eez-telemetry/native` | 665 passed, zero failures or ignored tests: Composer 241, observer 75, telemetry 43, signer 287, client 15, control RPC 4 |
| CI tooling discovery | 239 selected, 234 passed, five explicit opt-in skips; 97.730 seconds. The separate real relocated producer/consumer check is recorded in [runtime packaging](runtime-packaging.md) |
| Observability Python/configuration | 109 passed, zero skips, 13.258 seconds. Source-bound results, including the real PostgreSQL resource regression and subsequent benchmark contract checks, are recorded in [Python validation](python-config-final.md) |
| Complete staged `git diff --cached --check` | One whitespace finding: migration 0019 ends with an extra blank line. Its already-validated immutable bytes/checksum are preserved; earlier unstaged checks did not include this new file |

The affected Rust command and workspace checks precede the final test-only
benchmark capacity changes. Their focused build/tests are recorded separately
in [the overhead harness evidence](overhead-harness.md): the probe measurement
executable built successfully in 4m26s and both Rust budget validation tests
passed; the final Python suite includes the elapsed-budget regression. These results do not
claim the remote release matrix has run.

The local raw logs are `/tmp/eez-observability-final-affected-tests.log`,
`/tmp/eez-observability-final-workspace-fmt.log`,
`/tmp/eez-observability-final-workspace-clippy.log`, and
`/tmp/eez-observability-final-ci-reviewed.log`. Local paths are diagnostic
locators, not portable artifacts; the committed evidence documents describe the
assertions, identities and limits of each integration gate.

## Integration evidence

- [Canonical database](database.md): 64 groups, including populated upgrades,
  concurrency, recovery and exact accounting.
- [Native observer](native-observer.md): legacy/policy operation, restart and
  actual reorg reconciliation with source/post receipt and population checks.
- [Dashboards](protocol-dashboards.md): 88 real backend queries and browser
  navigation; [trace expiry](trace-retention.md) preserves durable SQL history.
- [Native traces](native-trace-pilot.md): connected Composer/client/signer
  ancestry, retries, source links and independently checked signature.
- [Disk exhaustion](native-enospc.md): actual bounded-volume exhaustion,
  semantic progress while full and fresh persisted lifecycle evidence after
  recovery.
- [Native loss alerts](native-loss-alerts.md) and [delivered links](alert-links.md):
  firing/recovery and private runbook/dashboard navigation against real backends.
- [Restore](restore.md): fresh application instances and volumes on the same
  Docker host, with explicitly measured recovery and lifecycle loss.

Each document limits its claims to the exercised fixture or native scenario.
Earlier failures and their corrections remain in the relevant records.

## Open acceptance gates

1. Live lock-probe calibration and sufficiently sampled, paired release-profile
   overhead measurements. The development-profile pilot establishes harness
   operation; it does not establish the proposed under-five-percent target.
2. Full application release images/archives and the remote CI matrix, followed
   by consumption of the exact validated revision and artifacts.
3. The private deployment manifest, actual independent heartbeat receiver,
   provisioned host/storage budgets and deployment-specific backfill capacity.
4. Physical-host/off-host recovery, operational RPO/RTO, seven days of baseline
   observations, threshold calibration and operator handover.

The [deployment runbook](../runbooks/deployment.md) prepares the last two gates;
it is not evidence that a live rollout occurred. No funded canary or production
service operation is implied by this checkpoint.
