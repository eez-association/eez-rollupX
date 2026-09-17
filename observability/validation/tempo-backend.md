# Optional Tempo backend evidence

Date: 2026-09-17. Candidate is the uncommitted observability worktree based on
`5183f56acd0903a10ad085e97d03aef4070e82a3`, preserving protocol gitlink
`3c0cdf4955941a5c23639346932b55b33babebbd`. No operator deployment was changed.

[Machine-readable report](tempo-backend.json) records the final disposable project,
exact image digests and eight passed check groups. Tempo 3.0.3 is pinned to
`sha256:0296560ac66f8a3600d7fb3014a52c189d4d9c3549ad6ff441bf2409855d68d5`.
The existing Alloy, Grafana, Prometheus and Loki pins are unchanged.

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Opt-in private self-hosted trace backend | `compose.yaml` traces profile, `tempo/config.yaml`, separate `alloy-traces` service | Pinned config validators; unique Compose project and ephemeral loopback ports |
| Seven-day persistent trace storage | Tempo local WAL/blocks/scheduler named volume; scheduler/worker retention 168h | Effective `/status/config` assertion and same trace queried after container replacement |
| Bounded collector and visible failure | Four-MiB request limit, memory limiter, 2048-item queue, one worker, one-second exports, ten-second retries | Queue overflow rejects admission and exports failure counters; brief outage recovers queued spans; longer failure expires retries; fresh spans recover |
| Grafana/log/trace navigation | Optional `eez-tempo`, Loki derived field, environment/deployment trace-to-log mapping | Actual OTLP fixture queried through authenticated Grafana proxy; provisioned regex extracts its ID; the same ID retrieves original JSON from Alloy→Loki |
| Missing trace is explicitly unavailable | Optional-link label plus Tempo's not-found behavior | Missing ID returns HTTP404 while retained diagnostic logs remain queryable |
| Alert delivery and recovery | Backend scrape alerts and recent collector-loss rule, enabled only with profile | Actual fixture receiver gets firing and resolved notifications for both Tempo outage and trace collection loss |
| Preserve default deployment | Strict prepare flags, absent default, port validation, datasource pruning and config removal | Python tests cover disabled/enabled/nondefault/invalid/re-disabled configuration |

Command:

```bash
python3 observability/tests/smoke-traces.py \
  --report /tmp/eez-observability-tempo-backend-final.json
```

Result: all eight check groups passed. The script removed its own containers,
volumes and network and verified no owned containers/volumes remained. It did not
restart or inspect configuration/data of any existing operator service.

Supporting checks:

- Pinned Tempo `-config.verify=true` and Alloy `validate` passed with networking
  disabled during validation.
- `python3 -m unittest discover -s observability/tests -p test_prepare.py -v`:
  ten passed in `/tmp/eez-observability-tempo-prepare-tests-final.log` (includes the
  concurrent dashboard lane's configuration cases).
- `python3 -m py_compile observability/prepare.py observability/tests/smoke-traces.py`
  and `git diff --check` passed.
- Full source test command for the associated log-ID/startup/shutdown changes:
  `cargo +1.94.0 test --locked -p eez-node-common -p eez-proof-signer -p eez-telemetry --features eez-telemetry/native`:
  11 node-common, 284 signer and 36 telemetry tests passed;
  `/tmp/eez-observability-tempo-native-tests.log`.

The connected production-client/signer fixture now verifies an actual JSON log
contains the exported trace ID. The remote parent is still attached before that
context is materialized. Native startup installs the signer's metrics recorder
before trace gauges/counters; zero loss counters exist before the first failure.
Composer/follower use a conservative remaining grace budget after Reth drains;
unit tests cover fast/exhausted drain, errors/no launch, the quarter-budget rule
and two-second cap. Signer `wait_until_idle` remains unchanged and still permits
zero telemetry wait because it has no finite total application deadline.

First failures/diagnostics retained outside the repository:

- `/tmp/eez-observability-tempo-config.log`: Tempo's verification flag requires
  `=true`; the corrected pinned-validator call passed.
- `/tmp/eez-observability-tempo-backend.{json,log,logs.txt}`: the initial fixture
  cached an ephemeral host port across `docker compose start`; Docker reassigned
  it and the probe received connection-refused. The fixture now rediscovers that
  port after start/recreation. No backend settings or assertions were weakened.
- `/tmp/eez-observability-tempo-backend-reviewed.{json,log}`: the six initial groups
  passed before adding effective-retention and alert-delivery assertions.
- `/tmp/eez-observability-tempo-backend-final.{json,log}`: final eight-group run.

Scope limits: this validates a low-volume disposable backend and synthetic OTLP
fixtures, not native devnet neutrality or a measured production ingest budget.
Seven-day configuration was inspected; seven real days did not elapse. The
unavailable-ID test is not a physical expiry test. Persistent-volume replacement
is not clean-host backup restoration. Native connected workload capture,
performance/retention calibration, native alert delivery and signer deadline
integration remain separate R23/R24/R27/R29/R30 gates. SQL accounting independence
is architectural here; the trace-backend test does not execute PostgreSQL queries.

[Operator procedure and limits](../runbooks/traces.md) reference the primary Tempo
and Alloy configuration documentation used for this profile.
