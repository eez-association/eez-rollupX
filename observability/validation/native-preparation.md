# Pre-signing preparation capture validation

This extends [native attempt capture](native-attempts.md) with a separate identity
for post-batch construction/proving/signing, before native ledger registration.
The [capture contract](../contracts/native-lifecycle.md) defines its exact scope;
source selection and earlier simulation are outside this duration.
Source `6baebfc` is verified by the [artifact/scenario report](native-preparation.json).

| Requirement | Implementation | Behavioral evidence |
| --- | --- | --- |
| Preparation exists before a signed attempt | First-poll wrapper at rich, minimal and historical preparation callers | Successful result links its preparation ID through facts into `attempt_prepared.details` |
| Failed preparations remain attributable | Started event carries complete candidate admission membership | Error result remains unchanged; its preparation retains the source admission and a typed finish |
| Exactly one finish for owned work | Move-only guard with completion fence | Unpolled drop emits nothing; polled cancellation and panic unwinding each emit one distinct finish |
| No text-based classification or secret error export | Exhaustive match on existing `PreparePostBatchError` variants | Identical error text in different variants produces different typed categories; error/envelope marker is absent from decoded events |
| Optional/lost capture never cancels work | Original future/result is retained; absent writer skips capture | Disabled and closed writer preserve success/error values; missing/oversized membership leaves a counted start gap while work finishes |
| Bounded asynchronous state | One boxed builder future at the shared wrapper boundary | Workspace Clippy checks all enclosing futures; cancellation/result regressions repeat after boxing |
| Exact native wiring | Three bidirectional canaries retain semantic/proof checks | Expanded assertions require matching preparation endpoint, chain identity and members, and exactly one successful finish before each captured attempt |

219 Composer tests and 18 default-feature telemetry tests passed, including five
new preparation regressions. The first workspace Clippy pass found that wrapper
state increased the enclosing minimal-dispatch future to 16,544 bytes. Boxing the
builder at the shared boundary corrected that size regression without suppressing
the lint or changing the many callers. The same component suite and workspace
all-target Clippy then passed with `-D warnings`.

Process validation passed **3 tests**, none ignored and **21 filtered out**, for
the expanded legacy-policy, enabled-policy and disabled canaries.
The fixture reader allows 64 bounded passes per segment
to cover the additional idle preparation records; its assertions are unchanged
or strengthened. Observer and all-feature telemetry suites also passed **27** and
**21** tests; empty binary/doc-test targets are not additional coverage. These
canaries verify successful preparation links for observed candidates; they do not
force historical fallback or replace component cancellation/failure evidence.

Local logs are `/tmp/eez-observability-preparation-tests-fixed.log`,
`/tmp/eez-observability-preparation-clippy-fixed.log` and
`/tmp/eez-observability-preparation-e2e.log`. The first lint failure is retained at
`/tmp/eez-observability-preparation-clippy.log`. Bulk logs and fixture state are
local-only, with no promised retention.
Final observer/telemetry results are in
`/tmp/eez-observability-preparation-observer-tests.log`. Formatting, diff whitespace,
edited local links and the shell example passed checks. Dependency versions,
protocol pins, migrations and storage queries did not change; the database process
suite and full release/CI/Kurtosis matrix were not rerun at this checkpoint.

Reproduce the native selection from the isolated worktree after the pinned
integration contract build, using Rust 1.94.0 and Foundry 1.7.1:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
CARGO_BUILD_JOBS=4 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/native-observability/preparation/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-observability/preparation/datadirs" \
cargo +1.94.0 test --locked --offline -p eez-node-e2e \
  --test cross_chain bidirectional_cross_chain_smoke -- --test-threads=1
```

A successful preparation is not canonical completion, dispatch or paid spend.
Crashes/aborts may lose the finish, and missing history stays unknown. Native
canonical terminal/reorg capture, observer outcome validation, stage/queue metrics,
traces, mixed fault workloads and production/performance gates remain open in
the [requirement ledger](../requirements.md).
