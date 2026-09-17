# Native observability integration evidence

This is an implementation checkpoint, not production acceptance. The source is
`e1a098c` on `feat/network-observability`.
Main remains the user-frozen `5469c88860408daf776ee812156dd280c1a9e262`;
the protocol remains `3c0cdf4955941a5c23639346932b55b33babebbd`.
No live service, operator configuration, wallet or chain datadir was changed.

The [proposal](../../proposals/network-observability.md),
[requirement ledger](../../../observability/requirements.md) and
[native capture contract](../../../observability/contracts/native-lifecycle.md)
define scope and remaining gates. Earlier indexing/transport evidence is in the
[previous checkpoint](../2026-09-16-network-observability/README.md).

## Admission and readiness

Optional native capture now records successful admissions, explicit replacement
links/local disposition, retained readmission links and exact readiness source
states. It derives chain/genesis/rollup identity from the embedded specifications.
Sequence allocation follows accepted ownership under the existing pool mutex;
serialization/publication runs after release. Internal retries retain admission
identity, and a closed writer does not reject protocol ingress.

| Check | Result / scope |
| --- | --- |
| Composer library suite | 210 passed, none ignored or filtered; includes five new lifecycle regressions |
| Node / native telemetry library suites | 33 / 21 passed, none ignored or filtered |
| Affected Composer/node/E2E all-target Clippy | Passed with `-D warnings` |
| Workspace all-target Clippy | Passed with `-D warnings` after the final code changes |
| Workspace formatting / diff whitespace | Passed |
| `cargo deny check advisories bans sources` / `cargo audit` | Passed; audit retains 14 existing allowed warnings; lockfile changes add only local dependency edges |
| Integration contract build | Passed with Foundry 1.7.1, Solc 0.8.34 and the unchanged protocol pin; existing compiler/lint warnings retained |
| Source-built bidirectional canaries | Passed: two tests, none ignored, 21 filtered; same selection after the legacy-readiness fix |

Unit/component evidence covers duplicate and rejected ingress, replacement,
both directions, readiness deduplication and same-height hash changes, recovery
without readmission, explicit readmission after local termination, late callbacks,
ended-writer behavior and policy-disabled queue RPC compatibility.

The process test reuses the existing bidirectional canary's assertions: source
receipts, destination values, safe-state convergence and independently recomputed
proof signature. The enabled variant additionally checks lifecycle admission IDs,
both readiness observations and chain identity against actual RPC genesis hashes.
It uses a dedicated temporary spool and the testkit's isolated processes/ports.
This is a correctness comparison, not a throughput or preparation-latency benchmark.
The exact successful Composer/signer/test executable hashes are in the
[process report](../../../observability/validation/native-lifecycle.json);
[validation notes](../../../observability/validation/native-lifecycle.md) map
requirements to assertions and provide the reproduction command.

### First failure and correction

The first source run selected two tests: the disabled baseline passed; the
enabled test passed protocol semantic/proof assertions but timed out waiting for
native readiness. Successful readiness was captured only inside the enabled
settlement-policy branch. The legacy success path now records the diagnostic
observation without changing its existing queue RPC fields or scheduling.
A component regression checks that compatibility explicitly. The first failure
is not classified as a flaky pass or hidden by a retry.

Initial executable SHA-256 values:

| Executable | SHA-256 |
| --- | --- |
| Composer | `48bc70f84155d184f8999d874a04b810607cffe1d413a7277f0bd4d33efd3848` |
| Signer | `a45279229eef4c9dae4b55f456185318f7ce7a263c1f4d7400c5da205b57b1aa` |

Local diagnostics are retained under
`artifacts/native-observability/2026-09-17-admission/`, including the failed
node datadir. The first test's temporary lifecycle spool was not retained;
the revised test preserves it on a lifecycle assertion timeout. Detailed logs
are `/tmp/eez-observability-native-admission-e2e-first-failure.log`,
`/tmp/eez-observability-native-composer-tests.log`,
`/tmp/eez-observability-native-admission-clippy.log` and
`/tmp/eez-observability-native-contract-build.log`. These local paths have no
promised retention and bulk logs are not committed.

Final component/static commands were:

```bash
cargo +1.94.0 test --locked --offline -p eez-composer --lib
cargo +1.94.0 test --locked --offline -p eez-node -p eez-telemetry --lib
cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
cargo deny check advisories bans sources
cargo audit
git diff --check
```

The final node/telemetry and workspace lint logs are
`/tmp/eez-observability-native-admission-node-tests.log` and
`/tmp/eez-observability-native-admission-workspace-clippy.log`. Edited local
documentation links resolved and the reproduction shell example passed `bash -n`.

## Remaining evidence

Admission/readiness history does not establish canonical cross-chain completion.
Attempt membership, repricing/dispatch links, exact reconciliation evidence and
other terminal dispositions still need native capture and observer validation.
Native metrics/traces, remaining dashboards/alerts, fault and performance drills,
clean restore and production rollout/baseline remain open in the requirement ledger.
The release-artifact build and full CI/Kurtosis matrix were not run at this
checkpoint. The recorded artifacts are local source builds, not promoted images.
