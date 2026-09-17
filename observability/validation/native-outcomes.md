# Native decision coherence and runtime projection

2026-09-17. Worktree `feat/network-observability`, based on `5183f56` with the
uncommitted implementation described below. Main baseline `5469c88` and protocol
pin `3c0cdf4955941a5c23639346932b55b33babebbd` are unchanged. Rust 1.94.0 and
Foundry 1.7.1. This is partial R19 evidence, not canonical outcome acceptance.

## Executed checks

The pure checker was tested before the parallel metrics/tracing integration:

- Observer: 46 unit tests passed, including nine new outcome cases.
- Composer: 232 unit tests passed after extracting its existing replay-coordinate
  validation into `eez-protocol` without changing native conditions/messages.
- Protocol and blob/payload codec suites passed, including the independent public
  inputs, rolling-hash and Ethereum KZG compatibility oracles.
- Workspace all-target Clippy, formatting and diff checks passed at that checkpoint.
- Two source-built same-L1-block middle-group resumption tests passed (default
  policy and configured policy). Log: `/tmp/eez-observability-outcomes-native-resumption.log`.
- Two source-built bidirectional lifecycle process tests passed with the observer
  checker consuming actual captured native terminal/parent/prepared/repricing
  evidence. Existing independent receipt/hash/position/effects/proof-signature
  assertions remain. Log: `/tmp/eez-observability-outcomes-native-capture.log`.
  Result: 2 passed, 22 filtered, 229.73 seconds. These are lifecycle/default-policy
  canaries, not the entire `cross_chain` target.

After adding the projection and schema 10, 47 observer tests passed. The first
sandboxed invocation had four mock-socket permission failures; the same suite
passed with socket access. Logs are respectively
`/tmp/eez-observability-source-references-tests.log` and
`/tmp/eez-observability-source-references-tests-socket.log`. The observer binary
build also passed. Direct isolated PostgreSQL checks applied migrations 1–10,
source extraction reader-role tests and existing admission-status regressions.

The full disposable process suite passed **45 check groups**, including the new
native-outcome importer/projection assertions and actual saved schema-4/5/6/8
binary upgrade paths. The tested observer binary SHA-256 is
`f9ae02bf9753c4893cfb4ccc66ec081faf79bdba3803da77b556c4528fd4bf3c`.
Report: `/tmp/eez-observability-source-references-database-reviewed.json`;
log: the adjacent `.log` file. All nine Rust coherence cases plus the real importer
case passed, including the 129-record lineage rejection and immediate cache
invalidation. Full-tree formatting also passed after the parallel source edits.

The first process attempt stopped earlier at a source-recovery fixture that
incorrectly encoded a large manifest as one transport chunk. The fixture was
corrected to respect the existing chunk limit; no importer validation was weakened.
That failure did not reach the native-outcome process assertions. Both original
and reviewed diagnostic logs are retained.

All `/tmp` logs and retained native datadirs are local-only artifacts. They are not
available to another checkout unless separately archived. No live deployment,
operator configuration, wallet funding or service restart was performed.

## Coverage and limits

The pure checker covers exact source/member/owner identity, producer/epoch/order,
all five canonical native outcome labels, missing/ambiguous evidence, bounded
membership/lineage, repricing including recurring signed hashes, whole outbound
groups, source receipt/consumption facts, actual applied prefixes and competing
inclusion. Native successful test cases demonstrate that these checks accept the
runtime's captured decisions while preserving independent semantic assertions.

The new database worker and view bind derived native checks to current complete
transport digests, retain context-specific gaps, invalidate changed dependencies
and newly recovered lineage, and expose bounded projection health. The process
fixture additionally exercises late parent recovery, absent admission history,
wrong context isolation, repeated passes and the 128-record lineage limit.
See [the contract and mapping](../contracts/native-outcomes.md).

Coherent native records still expose `unknown` canonical assurance. Independent
chain-anchor validation, canonical outcome selection/reorg retraction, exact
companion allocation, native restart canaries and finality/progress publication
remain open. The final combined metrics/tracing/observer source tree still needs
coordinated validation; earlier tests do not establish that later revision.
