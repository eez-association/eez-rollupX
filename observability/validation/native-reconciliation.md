# Native reconciliation and reorg capture validation

Source `4b3acb8` extends the [native capture contract](../contracts/native-lifecycle.md).
This is native decision evidence, not observer assurance or an independent replay
verdict. The full observability goal remains open in the
[requirement ledger](../requirements.md).

| Requirement | Implementation | Behavioral checks |
| --- | --- | --- |
| Distinguish scan watermark from inclusion | Composer copies already validated post/source receipt references and actual applied endpoint into reconciliation facts | Component fixture uses L1 scan 105 and post/source block 100 at indices 3/4; proposed and applied L2 hashes differ |
| Emit only committed ownership transitions | Held-pool wrapper captures after ledger resolution and reservation transfer, under the existing owner | Invalid partial partition, spent-nonce retry and stale generation preserve ownership and allocate no decision; repeated resolution emits once |
| Preserve retry/admission identity | Changed resolutions reference existing admission and actual owner | Retry increments once, next generation retains admission, exhausted root and older-owner nonce suffix retain separate local-policy outcomes |
| Keep capture failure outside protocol decisions | Bounded complete snapshots, counted loss, publication outside locks | Missing admission and excessive consumed coordinates lose one parent event while canonical ownership still resolves |
| Handle concurrent callbacks deterministically | Existing pool mutex and ledger exact-ID fence | Competing resolutions produce one parent and one matching terminal for the actual committed outcome |
| Preserve reorg history | Exact watcher ancestor/old/new references; existing invalidation behavior | Same-height reorg reopens ownership, reuses admission/attempt and records both reconciliation observations; height-only audit invents no hash |
| Preserve older payloads | Optional reconciliation/reorg details and terminal parent link | Legacy JSON decodes without evidence fields; omitted fields remain omitted when reserialized |
| Verify runtime wiring | Expanded source-built legacy/policy/disabled bidirectional canaries | Native parent/child agreement, source/post RPC receipt references and exact applied L2 block hashes, plus existing destination-state/proof checks |

The component suite passed 226 Composer tests, including seven new reconciliation
regressions. All-feature telemetry passed 22 tests, including legacy-payload
compatibility. Workspace all-target Clippy passed with `-D warnings`; formatting
and diff whitespace checks passed. The three source-built canaries passed, none ignored and 21 filtered, in
346.90 seconds including the harness signer build. The outer build took 8 minutes
26 seconds. Exact artifacts and scenario scope are in the
[process report](native-reconciliation.json). This timing is not a performance
measurement. Subsequent local-disposition edits are not covered by these binaries.

The first workspace lint found the reconciliation payload enlarged the event enum
from the prior maximum to 560 bytes and two old reorg fixtures lacked the optional
field. Boxing only reconciliation details retained the JSON representation and
avoided inflating unrelated events; the fixtures use absent details. The corrected
component suite and lint passed. The first all-feature telemetry run additionally
hit sandbox restrictions for existing listener/FIFO tests; the authorized rerun
outside that sandbox passed all 22 tests. First failures are retained separately.

Local logs, with no promised retention:

- `/tmp/eez-observability-reconciliation-check.log`
- `/tmp/eez-observability-reconciliation-tests-fixed.log`
- `/tmp/eez-observability-reconciliation-telemetry.log` (restricted sandbox)
- `/tmp/eez-observability-reconciliation-telemetry-sockets.log`
- `/tmp/eez-observability-reconciliation-clippy.log` (first failure)
- `/tmp/eez-observability-reconciliation-clippy-fixed.log`
- `/tmp/eez-observability-reconciliation-contracts.log`
- `/tmp/eez-observability-reconciliation-e2e.log`

Reproduce process validation after building the pinned integration contracts:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
CARGO_BUILD_JOBS=4 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/native-observability/reconciliation/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-observability/reconciliation/datadirs" \
cargo +1.94.0 test --locked --offline -p eez-node-e2e \
  --test cross_chain bidirectional_cross_chain_smoke -- --test-threads=1
```

These tests do not validate the observer's outcome/anchor views, source/companion
cost allocation, independent follower replay, native metrics/tracing, performance
or production rollout. Direct local evictions outside the ledger partition still
need their own capture. Native reorg events supplement independently checked
canonical indexing; a lost native event cannot authorize retaining an orphaned
outcome as canonical. No protocol rule, dependency version, database migration,
operator configuration or live service was changed by this slice.
