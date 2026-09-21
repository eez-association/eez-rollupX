# Resource measurement for the native overhead pilot

Status: **measurement tooling verified; no native overhead result yet**.
`tests/overhead_resources.py` supplies the external sampler and an owned healthy
OTLP/HTTP transport receiver to the fixed-candidate workload runner. It does not
start, stop or reconfigure native roles or operator services.

The sampler records Linux process identity as PID plus start ticks, cumulative
CPU, minor/major faults, I/O, sampled RSS and the separately labelled lifetime
high-water mark. PostgreSQL uses its entire cgroup v2, including query workers,
CPU throttling, memory/current and lifetime peak, memory events and per-device
I/O. Native/sampler roles cannot also be PostgreSQL cgroup members; process or
cgroup identity changes invalidate the measurement. Missing counters are errors,
not zeros.

Host context includes aggregate CPU/steal/I/O-wait, memory and swap availability,
fault/swap counters, CPU/memory/I/O pressure and individual device counters.
Device additions/removals are explicit; the sampler does not sum partitions and
virtual devices into a misleading physical-disk total. Unattributed CPU retains
the signed host-minus-tracked residual. The reported `other_cpu_cores` is a
conservative upper bound including two ticks per observed CPU/process counter
and the timestamped sequential-read interval. A materially impossible negative
residual invalidates the bracket instead of becoming zero background load.
This residual includes unsampled fixture-driver, Beacon-helper and builder-stub
work as well as unrelated host services; it is not a measurement of only unowned
processes. Native roles, observer, PostgreSQL and the sampler/collector retain
their separate measured resource scope.

Each phase requires a complete sample before its start and after its end.
Resource deltas cover that wider interval, with leading/trailing slack recorded;
they are not exact phase-boundary CPU totals. Memory maxima are sampled values,
not proof of an exact peak. Every interior sample is checked for wall/monotonic
clock discontinuities, including steps that reverse before the outer endpoints.
The fixed maximum observed clock-offset drift is 50 ms. CPU population changes,
counter resets and observed PostgreSQL OOM invalidate the evidence.

The native arm must retain its processes and database until the runner has taken
its final sample, stopped the sampler and persisted its report. Ready/start and
evidence/finish barriers enforce this ownership. Final reports refuse a live sampler thread; shutdown failures remain errors and
snapshot lock waits are bounded. The overall bracket spans the first phase
through final observer catch-up exactly once. Individual phase brackets may
overlap and must not be summed into an overall CPU total.

Metrics scraping defaults to the packaged 15-second cadence, independently of
one-second resource sampling. Only explicit literal loopback addresses are
accepted, with no proxy or redirect handling. Connection and response deadlines
are one second each, and replies are capped at 2 MiB. The report keeps response
digests/byte counts and selected exact health/drop/export samples. These prove
HTTP collection, not Prometheus storage; the workload's separate semantic and
telemetry-health assertions remain required.

The serial OTLP sink discards bodies after bounded reading. It has a 2 MiB
request bound, 64 MiB cumulative byte budget and 50,000-request budget by default.
An absolute two-second connection timer also bounds trickled headers/bodies.
Shutdown closes any active connection; cancellation joins the timer so a late
deadline error cannot race a healthy snapshot. Rejections and acknowledgment
failures remain visible. This is a healthy transport sink, not Tempo ingestion
or retention evidence.

The sampler caps raw capture at 1,200 samples by default and 32 MiB overall.
The frozen experiment must record cadence, limits and actual backend scope.
Calibration and the native A/B result are separate from these tooling checks.

## Executed checks

All **18 tests passed in 4.782 seconds**, including an actual pinned PostgreSQL
container with a CPU-consuming query worker and verified owned-container cleanup:

```bash
EEZ_TEST_RESOURCE_DOCKER=1 \
EEZ_TEST_RESOURCE_REPORT=/tmp/eez-overhead-resources-overall-final.json \
PYTHONPATH=observability/tests \
python3 -m unittest observability/tests/test_overhead_resources.py -v
```

The regression set covers real process counters, unusual process names, units,
device changes, exact selected metrics and rejected redirects, PID reuse,
counter resets, impossible CPU accounting, interior clock steps, missing boundary
samples, OOM, reserved/duplicate attribution, finite sample budget, actual HTTP
success/rejection and bounded shutdown with incomplete/trickled requests.
The real database phase recorded 0.235104 CPU seconds across its cgroup; this is
a query-worker measurement check, not an EEZ benchmark. Full evidence remains
local at `/tmp/eez-overhead-resources-overall-final.{json,log}`.

Earlier runs and review findings are retained. The first database readiness check
could observe the image's temporary bootstrap server; one later query failed
with exit 2 (`/tmp/eez-overhead-resources-reviewed.log`). Readiness now requires
the final PID-1 postmaster before `pg_isready` and sampling. Review also corrected
the interior-clock, negative-residual and socket-shutdown cases described above.

The combined observability suite subsequently passed 83/83, including this real
database opt-in. Its log is `/tmp/eez-observability-final-python-integrated.log`.
The initial native diagnostic pair passed the disabled arm but rejected the
enabled fixture's insecure precreated spool. It is retained as diagnostic evidence,
not an overhead result. The sampler shutdown/report fence and overall bracket
were subsequently verified by the final 18-test run above; the corrected paired
pilot remains pending. No production capacity, quiet-host condition or under-5%
regression is established by this file. See the [bounded tooling record](overhead-resources.json).
