# Protocol alert validation

This R22 slice builds on checkpoint `c36bbd6` in the
`feat/network-observability` worktree. It provisions rules from actual metric
contracts, with opt-in capabilities and explicit owners. It does not deploy
services or establish production thresholds. Native instrumentation and the
independent follower producer are reviewed/tested in their own lanes.

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Disabled compatibility, explicit configuration | Optional `protocol_alerts` requires database profile and owner; strict fields, bounds and capability/target combinations | Preparation tests cover absent, enabled, custom thresholds, invalid values, dead combinations and removal on reconfiguration |
| Canonical handling progress | Valid canonical outcome gauge plus ready/in-flight current queue and advancing L1; configurable 120/300-second thresholds | Exact PromQL tests cover warning/page, boundary, quiet workload, stalled upstream, missing history, retraction and recovery; actual Grafana notifications |
| Index and receipt coverage | Fresh scanner lag/open gaps plus independently configured source-receipt worker gaps | Threshold and upstream-suppression tests; actual notification/recovery |
| Lifecycle capture health | Explicit source availability/coverage; native writer enabled/drop/error counters independent of tracing | Missing/stale/gapped source tests and actual gap/loss delivery/recovery without resetting counters |
| Blob availability scope | Historical measurement warnings stay distinct from typed canonical required-current-blob retry pages | Coverage/failure and retry/expiry/retraction expressions; actual warning/page/recovery delivery |
| Authorized signer response failures | Separate counter only after full validation and settlement, repeated failure/no success in the same window | [Authorized signer producer tests](authorized-signer.md), exact expressions and actual firing/recovery cover errors, cancellation exclusion, unknown/down gates and historical-window aging |
| Signer rejection symptoms | Fixed invalid/rejected request outcomes, configurable count over five minutes, native/L1/L2 availability gates | Repeated rejection and missing/failed upstream tests; actual firing/recovery |
| Policy-aware pure-L2 age | Explicit operator inclusion grace; actual candidate-to-anchor policy age, enabled/valid/fresh and pending-work gates | Inactive/quiet/boundary/stale/missing tests; actual warning/page and recovery |
| Independent follower mismatch | Optional comparison-known gate and root-agreement=0; 30-second normal hold | Known mismatch, missing/stale comparison tests; actual page and recovery |
| Funding | Configured observer-account runway and actual native-poster affordability remain independent | Unknown/zero/stale/NaN expressions; actual funding delivery/retraction/recovery; source tests in funding.md |
| Trace/lifecycle loss | Existing trace/native loss rules plus writer capture health | Actual collector/native failure evidence and dedicated synthetic native loss delivery/recovery |
| Alert operation | One Grafana route, owner/severity, stable UIDs, explicit error/no-data policy and runbook/dashboard links | Provisioned rule inspection and real local webhook callbacks; [operator runbook](../runbooks/protocol-alerts.md) |

`test_protocol_rules.py` uses the pinned Prometheus `promtool` without network to
evaluate the exact generated expressions. **68 scenarios, 2600 expression assertions and 24 alert-state assertions
passed.** It exercises absence and upstream failure explicitly;
tests do not wrap unknown values in `or vector(0)`. Report:
[protocol-alert-rules.json](protocol-alert-rules.json).

The final actual Grafana/Prometheus fixture passed **nine acceptance groups**,
including the corrected pure-L2 holds and authorized signer response alerts;
[protocol-alerts.json](protocol-alerts.json) records the result and verified
cleanup of its owned containers/volumes. It delivered and recovered canonical
progress warning/page, unknown-history and index-parent alerts, receipt/lifecycle/
historical blob coverage, follower mismatch, policy warning/page, signer rejection
funding/current-blob alerts, and native trace/lifecycle-loss alerts. The fixture preserves cumulative loss and
rejection counters on recovery. Command (exit 0):

```bash
python3 observability/tests/smoke-alerts.py \
  --report /tmp/eez-signer-authorized-alerts-reviewed.json
```

The latest exact Prometheus alert-state tests use the actual configured holds
and prove that work arriving after a long idle period and settling within grace
never fires; sustained due work fires at grace and grace plus critical delay;
evaluation loss resets the hold. The nine-group backend report above also validates delivery/recovery with the
corrected timing configuration; deterministic tests establish exact grace boundaries.

The adjacent local `.log` contains each actual delivery/recovery assertion. No
operator process, chain state or funded account was used.

Other executed checks:

- `python3 -m unittest discover -s observability/tests -p 'test_*.py' -v`:
  **29 passed**, including the concurrent queue-source configuration tests.
- `python3 -m unittest scripts.ci.tests.test_workflow_contract.ObservabilityWorkflowContractTest -v`:
  **2 passed**. Exact rules and the disposable alert stack are required CI gates,
  with five- and fifteen-minute step budgets. The combined job budget is 90 minutes.
- Python compilation of provisioning/fixtures and `git diff --check`: passed.
- The initial disposable Grafana test passed all five original groups, including
  actual progress/index/coverage and trace/lifecycle-loss notifications and
  recovery. `/tmp/eez-protocol-alerts.{json,log}` preserves that first result.

The expanded rule test initially caught a malformed policy vector-match
expression (`group_left` needed explicit empty grouping parentheses before the
right-hand expression). The corrected exact expressions passed; the original
failure remains at `/tmp/eez-protocol-rules-expanded.log`. No assertion or producer
condition was weakened.

Remaining limits: rates are synthetic fixture inputs, not native fault injection;
thresholds need workload/policy calibration. Aggregate queue/progress alerts can
mask a single blocked direction while another advances. Authorized-attestation failure metrics establish failure of fully validated
responses; they do not establish global queue causality or validate a pending
request that never completed the authorization pipeline. Funding source correctness is recorded
separately in [funding validation](funding.md); this alert fixture uses synthetic
metric inputs. Optional
signals never become verified healthy merely because they are absent.

The first expanded authorized-signer backend run failed Grafana provisioning
because one generated rule UID exceeded its 40-character bound. The UID was
shortened and the all-rule configuration test now enforces that bound. Initial
evidence is retained at `/tmp/eez-signer-authorized-alerts.{json,log,logs.txt}`.


The later delivered-link check found and fixed Grafana's default localhost:3000
notification origin. The private runbook dashboard and configured-origin links
passed actual notification capture and authenticated browser navigation for both
operations and protocol runbooks; see [alert link validation](alert-links.md).
Native loss delivery/recovery also checked the actual URL/API contract separately
in [native loss validation](native-loss-alerts.md).
