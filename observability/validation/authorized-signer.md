# Authorized signer response evidence

The R22 critical signer alert now has an affirmative producer distinct from
ordinary invalid-proposal warnings. `ProveSvc::finish_authorized_response` accepts
only `AttestationMaterial` returned by the complete existing validation and
settlement pipeline. The caller retains its exclusive request permit across this
synchronous helper. Original pre/post-sign deadline checks, signing operation,
logs and response bytes are unchanged.

`AuthorizedRequest` records exactly one finite local response outcome, including
errors and unwind, only when native metrics are enabled. Startup initializes all
five outcome counters. Invalid or merely admitted work never enters the guard;
success means local response construction, not client delivery or L1 settlement.
No request identity, address or untrusted text becomes a metric label.

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Full validation before valid-work evidence | Typed `AttestationMaterial` argument at the service response boundary | Real tonic fixture: malformed input and a settlement-rejected request emit no authorized outcomes; a valid request returns the same recoverable signature and increments success once |
| Preserve deadlines and signing authorization | Existing checks remain inside the guarded response helper | Material produced by the full pipeline reaches the helper with an elapsed deadline: one deadline observation, unchanged public status, no signing-stage observation |
| Exact-once and disabled instrumentation | Local guard disarms before recording, finite enum, drop distinguishes panic/cancellation | Telemetry tests cover double finish, disabled guard, success, internal error, deadline, cancellation and panic |
| Page only on known authorized failures | Repeated internal/deadline/panic outcomes, no success in the same window, signer and upstream health gates | Exact PromQL tests cover each failure, excluded cancellation, historical success/failure aging, unknown success, signer/downstream suppression and recovery |
| Operable notifications | Stable bounded UID, owner/severity, runbook link and strict configurable count threshold | Actual Grafana/Prometheus/webhook fixture fires the page and resolves it after an authorized success without resetting counters; configuration tests cover default, custom and invalid thresholds |

Executed validation:

```bash
cargo +1.94.0 test --locked -p eez-telemetry -p eez-proof-signer \
  --features eez-telemetry/native
python3 observability/tests/test_protocol_rules.py \
  --report /tmp/eez-signer-authorized-rules-reviewed.json
python3 observability/tests/smoke-alerts.py \
  --report /tmp/eez-signer-authorized-alerts-reviewed.json
```

Results: **287 signer tests and 41 telemetry tests passed**; **68 deterministic
rule scenarios / 2600 expression assertions / 24 policy hold assertions passed**;
**nine actual alert acceptance groups passed**, with owned-resource cleanup
verified. Local Rust log: `/tmp/eez-signer-authorized-tests-loopback.log`.
Machine-readable alert evidence is linked from
[protocol alert validation](protocol-alerts.md).

The first Rust attempt preserved its failure log: the filesystem sandbox denied
52 local RPC/OTLP listener binds (`Operation not permitted`); 235 non-network
signer tests passed. The unchanged suite passed with permission for its private
loopback listeners. No assertions were disabled or changed. The new signer test
dev dependency uses the already locked `metrics` 0.24.6 package; all package
versions, sources and checksums were compared and remain unchanged.

Limits: internal-error/panic/cancellation injection tests exercise the outcome
guard, not a fictitious production signer backend. The existing attester remains
a concrete in-process ECDSA signer with a validated key. The real RPC tests cover
successful signing and rejection boundaries, while the helper test covers a real
post-validation deadline. Work that never returns authorized material to the RPC
remains unclassified by this counter. The critical condition does not establish
causality for every stalled queue, canonical settlement, or transport delivery.
Signer shutdown still has no finite total application deadline; this change does
not alter worker ownership or the separately documented zero telemetry wait.
