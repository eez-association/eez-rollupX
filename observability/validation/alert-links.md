# Delivered alert links and private runbooks

The original pinned Grafana notification fixture exposed a real operator-facing
bug: although Grafana was published on an allocated loopback port, delivered
`generatorURL`, `dashboardURL` and `panelURL` used `http://localhost:3000`.
The old relative `runbook` annotation was not a clickable standard runbook link.
The retained reproduction is `/tmp/eez-alert-links-before.json`.

`grafana_public_url` now supplies a validated browser-facing HTTP(S) origin to
Grafana's root URL and standard `runbook_url` annotations. The default uses the
configured port (19300 normally). An unresolved diagnostic port zero has no
inferred operator origin; fixtures must supply their actual allocated origin for
usable links. Credentials, non-origin paths, queries and fragments are rejected.
Authentication, loopback binding and the existing backend resource limits remain
unchanged.

The managed `eez-runbooks` dashboard uses Grafana's built-in Markdown text panels
and the checked-in operations/protocol runbooks. It requires no metrics or SQL
datasource and no external documentation service. Both alert classes link to the
corresponding private panel; other repository paths remain explicit checkout
references. Preparation tests cover defaults, custom origins, invalid values,
profile changes and unresolved dynamic ports.

The actual browser fixture receives real operations and protocol notifications
from the pinned Grafana/Prometheus stack. It requires unauthenticated runbook API
access to fail with 401, checks the origin of every delivered generator/dashboard/
panel/runbook URL, logs in, clicks each delivered runbook link, verifies its actual
instructions, and follows each matching dashboard link. It uses synthetic alert
inputs; the separate [native loss gate](native-loss-alerts.md) establishes real
native producer delivery/recovery and checks the same URL/API contracts.

The first browser attempt failed because the fixture replaced a live Grafana SPA
document with receiver HTML while retaining its navigation interception. The
captured failure URL had changed, but its entire body was still `Open alert
runbook`. Moving the receiver to a separate `about:blank` document corrected the
fixture; no production URL, content predicate or assertion was relaxed. Initial
reports remain `/tmp/eez-alert-links-reviewed*` and
`/tmp/eez-alert-links-diagnostic*`, including bounded failure text and screenshot.

Command:

```bash
browser_work=/path/to/isolated-playwright
NODE_PATH="$browser_work/node_modules" \
PLAYWRIGHT_BROWSERS_PATH="$browser_work/browsers" \
python3 observability/tests/smoke-alert-links.py \
  --report /tmp/eez-alert-links-final.json
```

The dedicated browser gate is required in the observability workflow, with a
five-minute step budget and retained failure reports/screenshots. Python checks
passed: 62 observability cases and 17 workflow contract cases. JavaScript/Python
syntax and `git diff --check` passed. The final browser gate passed **three backend groups and five browser checks**,
with zero page errors and all owned resources removed; see [alert-links.json](alert-links.json).
A subsequent unit-covered input check also rejects empty `?`/`#` delimiters, which
otherwise put the appended runbook path inside a query/fragment. That input-only
follow-up preserves valid-origin output and is recorded separately from the
browser source snapshot. The final report records pinned images,
before/after source identity and owned-resource cleanup. Real deployment origin,
receiver ownership and remote CI execution remain operator acceptance items.

Final input-validation follow-up: 16 preparation tests passed. Concurrent overhead
work expanded discovery to 72 cases: **71 passed, one optional Docker fixture was
skipped**. The first sandboxed run rejected sockets in two new loopback HTTP
tests; the unchanged suite passed with loopback access. Logs are
`/tmp/eez-alert-links-origin-unit.log` and `/tmp/eez-alert-links-combined-unit.log`.
