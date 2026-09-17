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

At the admission/readiness checkpoint, attempt membership, repricing/dispatch links,
exact reconciliation evidence and other terminal dispositions remained open.
Admission/readiness history does not establish canonical cross-chain completion.
Native metrics/traces, remaining dashboards/alerts, fault and performance drills,
clean restore and production rollout/baseline remain open in the requirement ledger.
The release-artifact build and full CI/Kurtosis matrix were not run at this
checkpoint. The recorded artifacts are local source builds, not promoted images.

## Attempt capture follow-up

`85a178f` adds complete native attempt membership, parallel manifest coordinates,
candidate sizing/nonce facts, explicit repricing links and submission observations.
It keeps native ownership in the existing ledger and emits after successful
mutations. Bounded capture failure produces a counted gap while protocol ownership
continues. Preparation identities and canonical reconciliation/terminal evidence
remain open; submission inclusion alone is not source completion.

The corrected test source is `bd0e03b`. Its three source-built canaries passed:
lifecycle under legacy policy, lifecycle under enabled policy, and the disabled
baseline. All retain the original bidirectional semantic/proof assertions; enabled
cases additionally validate membership-to-dispatch links and nonce/blob count
against mined transactions. 21 other cross-chain tests were filtered out.
The [attempt report](../../../observability/validation/native-attempts.json) records
exact binary hashes, selection and logs, with the
[requirement mapping](../../../observability/validation/native-attempts.md).

The first selection passed legacy capture and the baseline but the new policy
fixture failed before RPC startup because a required setting was absent. `bd0e03b`
adds the existing policy fixture's general-trigger/speculative-depth settings;
runtime validation was preserved. First-failure logs and executable hashes are
retained and identified in the report. The corrected selection passed all three
tests; this is a fixture correction, not a retry-only flaky pass.

214 Composer tests, 27 observer tests and 21 all-feature telemetry tests passed.
Final workspace all-target Clippy and formatting/document checks passed. No
dependency versions or protocol pins changed. Database transport storage remains
schema 4; optional details preserve old payload decoding, while old strict readers
quarantine new fields/variants. Upgrade the observer before enabling the writer.
Existing quarantined records require explicit replay, not automatic reinterpretation.

The database process suite and full release/CI/Kurtosis matrix were not rerun for
this capture checkpoint. The result does not establish canonical outcome
validation, independent follower replay, production overhead or rollout acceptance.

## Pre-signing preparation follow-up

`6baebfc` adds distinct preparation IDs, candidate admission membership, typed
finishes and monotonic elapsed duration around post-batch construction/proving/signing.
The original future retains ownership and returns its unchanged success/error
result. Unpolled drop emits nothing; cancellation and panic unwinding each finish
once. Missing/oversized membership produces a counted start gap while work continues.
Successful preparation links into the later native attempt without implying dispatch
or canonical completion.

The initial wrapper increased the enclosing minimal-dispatch future to 16,544 bytes,
which workspace Clippy rejected. Boxing the builder at the shared boundary fixed
the size regression without suppressing the lint or modifying the many callers.
The original failure is retained at `/tmp/eez-observability-preparation-clippy.log`;
the same component tests and full workspace lint passed after the change.

219 Composer tests, 27 observer tests, 21 all-feature telemetry tests and three
source-built canaries passed. No tests were ignored; the canary selection filtered
21 other cross-chain tests. Enabled legacy/policy canaries require matching
preparation endpoint, chain identity and source membership, followed by exactly one
successful finish for each registered attempt. They retain the semantic/proof and
mined-dispatch assertions; the disabled baseline also passed.

The [preparation report](../../../observability/validation/native-preparation.json)
records exact executable hashes and scope, with a
[requirement mapping](../../../observability/validation/native-preparation.md).
Cancellation/failure evidence is component-level; these canaries do not force the
historical fallback branch. Formatting, whitespace, local links and shell-example
syntax checks passed. No dependencies, migrations or protocol pins changed.
Canonical terminal/reorg capture and validation, native metrics/traces, full fault
workloads, release/CI/Kurtosis validation and production acceptance remain open.

## Native reconciliation and reorg evidence

Source `4b3acb89df209245d466af63f95091b8a33c76f5` records exact native
reconciliation facts, actual changed admission dispositions and observed watcher
reorg references. It preserves the distinction between scan snapshots and receipt
inclusions, bounds whole snapshots, and publishes after the pool/ledger locks.
The compatible APIs retain their existing protocol behavior. Height-only
invalidation cannot fabricate an ancestor hash.

226 Composer tests, 22 all-feature telemetry tests and workspace all-target Clippy
passed. The source-built legacy-policy, enabled-policy and disabled canaries all
passed, retaining semantic/proof assertions and adding source/post receipt and
applied L2 endpoint comparisons with RPC. The
[validation record](../../../observability/validation/native-reconciliation.md)
maps requirements to checks, preserves first failures and lists limits; its
[artifact report](../../../observability/validation/native-reconciliation.json)
identifies the exact executables. These are native claims and process validation,
not completed observer outcome assurance, independent replay or production
acceptance. Later local-disposition edits require their own validation.

## Explicit local admission dispositions

Source `c7c2782` captures actual local reservation removal before admission
metadata expires, including pre-submission eviction, recovery conflicts and
unsubmitted nonce-gap victims of a submitted root. It preserves canonical owners
and emits no second terminal on repeated release. Missing/oversized capture
remains a diagnostic gap, never an eviction limit.

232 Composer tests, 27 observer tests, 22 all-feature telemetry tests and workspace
all-target Clippy passed. Five source-built native tests passed: three
bidirectional lifecycle canaries and both Ethereum/Gnosis queue-liveness/follower
compatibility cases. The [validation map](../../../observability/validation/native-local-dispositions.md)
and [artifact report](../../../observability/validation/native-local-dispositions.json)
state exact scope, first failure and remaining limits. This evidence predates the
subsequent observer source-receipt work.

## Lifecycle-referenced source receipt costs

Source `48c748b` adds schema-5 source receipt enrichment while retaining the
original configured selection for existing cost views. It projects complete
admissions with chain/genesis/rollup/context checks, verifies exact source receipt
inclusions inside the indexed interval, and shares immutable receipt storage.
Repeated admissions cannot multiply costs; reorgs and disputed lifecycle links
retract the appropriate views. Null receipts, accounting gaps and failed lookups
remain distinct. Per-chain deadlines/backoff preserve the other chain's progress.

30 observer tests, workspace all-target Clippy and 34 disposable database check
groups passed. The [validation map](../../../observability/validation/source-receipts.md)
and [artifact report](../../../observability/validation/source-receipts.json)
include migration compatibility, injected errors, timeout recovery, context fencing,
exact sums and first failures. These are independently fetched receipt facts for
lifecycle-referenced hashes, not completed companion allocation or canonical EEZ
outcome assurance. No production deployment was performed.

## Independent receipt protocol-log evidence

Source `1be022c` adds bounded L1 receipt-log collection with separate gaps and
immutable conflict history. Schema 6 preserves fee facts, exposes exact root and
consumption occurrences, and retracts canonical log views on reorg. It does not
promote node outcomes or assign source/companion ownership.

36 observer tests, 37 disposable database checks and workspace all-target Clippy
passed. The [validation map](../../../observability/validation/protocol-logs.md)
and [artifact report](../../../observability/validation/protocol-logs.json)
record exact source/binary identities, schema-4/schema-5 upgrade tests, capture
limits, duplicates, conflicts, restart backlog, permissions and first failures.
Native protocol execution and live deployment were not performed in this slice.
