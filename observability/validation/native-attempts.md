# Native attempt capture validation

This extends the [admission/readiness checkpoint](native-lifecycle.md).
The [capture contract](../contracts/native-lifecycle.md) defines implemented events;
the [requirement ledger](../requirements.md) retains the remaining full-goal gates.
No production deployment or performance acceptance is claimed.

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Capture only committed ownership transitions | Held-pool wrappers call the existing ledger, then reserve snapshots; publish after pool/ledger release | Failed registration and invalid replacement leave sequence unchanged; superseded callbacks emit no event |
| Exact many-to-many admission/attempt membership | `attempt_prepared` members and parallel manifest plans, producing-effect hashes and chain identity | Mixed inbound/outbound fixture preserves source actions/group coordinates; retries reuse the admission across distinct native generations |
| Repricing does not duplicate ownership/payment | Explicit previous/replacement IDs, preserving native generation | Changed candidate links once; identical replacement emits nothing; a dispatched candidate cannot be repriced |
| Actual dispatch occurrence, not a polling counter | Accepted existing dispatch callback, resolved target and original signing nonce | Duplicate dispatch rejected; two synchronized competing callbacks produce exactly one event matching the retained target |
| Uncertain observations do not complete sources | Typed included/excluded/uncertain observations remain diagnostic | Uncertain callback retains the pending attempt/users; late callbacks after reconciliation remain harmless |
| Bounded capture with explicit loss | Member/effect/action limits before copies; missing context rejects the whole capture | Oversized manifest and missing admission each preserve protocol ownership and produce one counted sequence gap |
| Candidate DA facts stay distinct from paid costs | Preparation returns already computed stream length/blob count/allowance/nonce beside the envelope | Component fixture checks copied facts; process assertions check capacity arithmetic and mined dispatch nonce/blob count |
| Existing wire records remain readable | Optional details omitted when absent; strict new variants | Existing chunk replay fixture still round-trips its original payload without details; upgraded readers are required for new fields |

The final Composer suite passed **214 tests**, none ignored or filtered, including
four new attempt regressions. Telemetry's default-feature suite passed **18 tests**.
The initial check reported one obsolete import after rerouting submission through
the pool; it was removed. Workspace all-target Clippy then passed with `-D warnings`.
Later process-test additions still require the final lint/check recorded below.

Process validation is in progress. The existing source-built bidirectional suite
now selects three cases: lifecycle with legacy policy, lifecycle with enabled
settlement policy, and the unchanged disabled baseline. Both enabled cases retain
the original semantic/proof assertions and require each source to have a complete
prepared membership leading to a dispatched candidate, directly or through its
explicit repricing link. They also verify nonce/blob count against a mined L1
transaction. The policy fixture uses its existing `always` mode and explicit fee
caps; it does not depend on an operator deployment.

The first process selection passed legacy capture and the disabled baseline,
but the new policy fixture exited before RPC startup: it omitted the required
`EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS`. The fixture now uses `10000` and disables
the incompatible speculative-depth guard with `EEZ_MAX_SPECULATIVE_DEPTH=0`, as
the existing policy fixtures do. No runtime configuration validation was weakened.
That first run is not evidence for enabled-policy execution; the identical
three-test selection is being repeated with the complete fixture configuration.
Its log is preserved at `/tmp/eez-observability-attempt-e2e-first-failure.log`,
with executable hashes at `/tmp/eez-observability-attempt-first-binaries.sha256`
and process logs under `artifacts/native-observability/2026-09-17-attempts/logs`.

Run from the isolated worktree with the pinned initialized contracts and tools:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
CARGO_BUILD_JOBS=4 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/native-observability/attempts/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-observability/attempts/datadirs" \
cargo +1.94.0 test --locked --offline -p eez-node-e2e \
  --test cross_chain bidirectional_cross_chain_smoke -- --test-threads=1
```

Local logs are `/tmp/eez-observability-attempt-tests-final.log`,
`/tmp/eez-observability-attempt-clippy.log` and
`/tmp/eez-observability-attempt-e2e.log`. Bulk logs and fixture datadirs are not
committed and have no promised retention.

Pre-signing preparation IDs, canonical reconciliation/terminal evidence, observer
outcome validation, SQL attribution, native stage/queue metrics and traces remain
open. These tests establish diagnostic attempt history, not independent source
effect replay, finalized accounting, telemetry failure tolerance under a full
workload, or the provisional overhead budget.
