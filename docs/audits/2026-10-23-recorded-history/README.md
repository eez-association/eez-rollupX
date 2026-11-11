# Recorded history transactions and independent L2 production

| Field | Value |
| --- | --- |
| Status | Local validation complete: all seven selected final native scenarios, affected libraries, signer, protocol oracles, formatting and required signer tooling pass. Full CI/external interoperability limits are stated below. |
| Source | Dedicated `codex/l1-history-offline-proving` worktree, parent `3e23955dc477c9e675364776f118b1cecda75774`, plus the uncommitted implementation. |
| Worktree | `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`; the original checkout and its unrelated work are preserved. |
| Integration base | `8621b0368f4f42dc871dd84c0aaf5b6c4599bfbd` merges the upstream 206/205/198 backport; subsequent commits add offline history, the published edge-case protocol and portable proving. This change extends that existing dependency chain. |
| Protocol | Clean published `feature/e2e_edge_cases`, gitlink `84a5b5207880f192a6a5896cbfe0bac5249db47f`. |
| Dependencies | Rust 1.95.0, locked workspace dependencies, Foundry 1.7.1/Solidity 0.8.34. No dependency or core gitlink update. |
| Contract | [Proposal](../../proposals/l1-origin-and-offline-proving.md), [history](../../../contracts/L1_HISTORY_SPEC.md), [signer](../../../crates/eez-proof-signer/SPEC.md). |

## Scope and implementation

The selected origin is an actual native `0x76` history transaction in the public
block body and V0 ordinary column. The existing envelope carries chain ID, shared
system nonce, history destination, zero value and exact bounded import calldata.
There is no origin metadata in extraData and no invisible history import. The
executor requires exactly one first-position import at each scheduled update,
checks its exact ancestry, delay, runtime and state, meters its gas and requires
successful execution. Header randomness and the PREVRANDAO opcode use that choice.

An empty import repeats the inherited origin. Later eligible evidence cannot
reinterpret earlier blocks. Acquisition, ordinary production and settlement have
separate progress conditions: one owned preparation worker can wait without
occupying the local clock. Remote reconciliation reads occur outside the commit
lock; generation and predecessor fences protect the subsequent state changes.
An older replay must not discard a matching, more recent prepared ancestry.

The four same-parent replacements and speculative-depth cap still bound
conditional preparation. They do not bound ordinary block production. Genuine
canonical reorgs still invalidate evidence and require reconciliation; local
production independence does not authorize continued execution on an orphaned
origin or promise irreversible unsafe receipts.

V2 protobufs remain unchanged. The existing header witness supplies offline
validation, including unused bridge headers. The fresh-network profile domain is
V2 and portable validation specification is 2 (identity schema remains 6).
Composer, follower, signer, genesis and deployed manager must agree. There is no
legacy-network activation or migration implementation.

## Evidence to date

All commands run at the isolated worktree root. Rust commands use `--locked`,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, LLVM 22 on PATH and a worktree-local TMPDIR.
Detailed logs below are **local only** in ignored `artifacts/validation/` and are
not available in a new checkout. Older audits validate their own source revisions.

[Final source and evidence identities](validation-results.json) retain hashes of
the changed tracked files, local logs and tested executables. They identify the
observed artifacts; hashes alone do not make the local-only logs retrievable.

| Check | Result / limitation |
| --- | --- |
| Pinned core contract suite with prescribed script/test exclusions | 778 passed, 0 failed, 3 skipped (`testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`, `testForkRejectsWrongSigningKey` require fork configuration). Explicitly excluded `script/*`, `test_Doc_OriginalValue_SeedInSetup`, `test_PostCost_Incremental`; no coverage claimed for them. Log `continuity-core-contracts.log`. |
| `forge test --root contracts` | 120 passed, 0 failed, 0 skipped; context modes, profile binding, history and storage-proof tests. Log `continuity-contracts.log`. |
| Affected Rust libraries: primitives, protocol, EVM, driver, deriver, L1, payload codec, Composer | 980 passed, 1 explicitly ignored pre-existing diagnostic; log `continuity-unit-3.log`. The subsequent evidence-retention/generation regression passed with all 195 L1 tests in `continuity-evidence-rpc-units.log`. |
| Final affected library suite, including node-common, blob codec, control RPC and prover client | 1,061 passed, 0 failed, 1 pre-existing ignored diagnostic across 12 packages after the recovery fixes; log `continuity-unit-4.log`. Includes both new correlation persistence regressions. |
| `bash scripts/update-eezl2-genesis.sh` and `--check` | Passed, root `0x44d653c875106c56352e80715aa23b3823b52a6d43c45b11b4ddeb7ae496b94a`. Logs `continuity-genesis.log`, `continuity-genesis-check.log`. |
| Native source-based tests with network-denied signer | Bidirectional smoke, caught-revert observation and changing-static observation all passed, one selected test each. The smoke checks both application states, canonical safe/root agreement and the independently recomputed on-chain proof digest; the observation scenarios also check fresh follower replay. Logs `continuity-native-smoke-3.log`, `continuity-native-lookup-2.log`, `continuity-native-static.log`. The earlier setup failure remains documented below. |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed after the witness-pacing/correlation fixes and fixture corrections; log `continuity-clippy-11.log`. Earlier lint/type failures are retained separately. |
| Driver reconciliation/Sequence regressions | 3 passed, including the new actor-time stale-parent rejection before any Engine call; log `continuity-actor-race-2.log`. The other selected packages supplied the established feature combination and had no matching tests. |
| Focused L1/RPC/client libraries | 195 L1, 7 control-RPC and 34 prover-client tests passed, including the new stale-generation/older-replay regression. Log `continuity-evidence-rpc-units.log`. |
| Independent history-repeat gas oracle | Owned Anvil 1.7.1/Osaka executed the exact canonical genesis runtime at cutoffs 104/116, timestamps 110/122, with cold storage and an empty import. Both consumed 41,077 transaction gas, advanced `nextCutoff` and the sender nonce, and emitted no logs. [Inputs and results](history-gas-oracle.json). The synthetic signer fixtures use this fixed external result, independently assembled state tries and hand-built one/two-leaf transaction/receipt tries; their execution passed in `continuity-signer-unit.log`. |
| Frozen native fixtures and portable v2 vectors | Three fresh captures copied; 29 cases regenerated and replayed against real TCP/Stateless services. Manifest check passed: 679 client requests, 678 correlated responses, 24 proofs, 1,436 events. Logs `continuity-vector-generation.log`, `continuity-vector-manifest.log`, `continuity-vector-final-check.log`. The initial full signer run had 325 passes, 2 obsolete assertion failures, 2 ignored artifact utilities; the corrected full repeat passed as recorded below. |
| Seven combined native scenarios | 5 passed: consecutive skipped slots, two composers/two followers, imported-history reorg with ordinary transactions, Ethereum RPC outage, Gnosis RPC outage. 2 failed: expired multipart recovery and same-block partial-consumption resync. Log `continuity-native-combined.log`; mechanisms and corrections below. |
| Repeated recovery scenarios after fixes | Both exact tests passed: expired multipart/cold offline signing with public replay, and same-L1-block partial consumption with follower resync. Log `continuity-native-recovery-2.log`: 2 passed, 0 failed, 0 ignored. The 675-second invocation includes Cargo contention and rebuilding cold signer dependencies; it is not a recovery performance measurement. |
| Final combined process regression | All five remaining scenarios passed with the corrected executables: skipped slots, four nodes, imported-history reorg, Ethereum outage and Gnosis outage. Log `continuity-native-final.log`: 5 passed, 0 failed, 0 ignored, 454.48 seconds including fixture setup. Together with the recovery repeat, all seven selected scenarios pass. |
| Final signer suite | `cargo +1.95.0 test --locked -p eez-proof-signer`: 327 passed, 0 failed, 2 ignored artifact utilities (native-fixture inventory and vector regeneration), 0 filtered out. Log `continuity-signer-unit-3.log`. Includes all 29 frozen real TCP/Stateless vector cases and the corrected independent synthetic fixtures. |
| Protocol and codec integration oracles | `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec`: 261 passed, 0 failed/ignored, including the KZG compatibility oracle, public-input/rolling-hash Solidity vectors, causal evidence and issue35. This total overlaps library tests above; empty doctest selections are not counted. Log `continuity-protocol-combined.log`. The separately selected five Solidity vector tests also passed in `continuity-protocol-vectors.log`. |
| Formatting and artifact manifest | Rust workspace format, integration-contract format, `git diff --check`, and the 29-case fixture/schema manifest passed. Logs `continuity-final-format.log`, `continuity-contract-format.log`, `continuity-vector-final-check.log`. |
| Signer contributor checks | `cargo +1.95.0 check --locked -p eez-proof-signer --all-targets`; `cargo +1.95.0 clippy --locked -p eez-proof-signer --all-targets --all-features -- -D warnings`; `RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked -p eez-proof-signer --no-deps`: all passed. Logs `continuity-signer-check.log`, `continuity-signer-clippy.log`, `continuity-signer-doc.log`. |

Genesis generation used the existing unchanged pure genesis-state-root helper
source and local binary, with both SHA-256 identities in
`continuity-genesis-helper.sha256`. That helper computes a trie root from the
supplied genesis and does not validate the new node runtime. Its reuse is not
reported as a new runtime build. The first redundant helper compilation was
stopped before rerunning generation with the explicit helper on PATH.

Initial compiler and unit failures are preserved in `continuity-all-targets*.log`
and `continuity-unit*.log`. They exposed old test APIs and fixtures that omitted
the now-visible transaction, its nonce and its metered gas; fixtures were updated
while retaining the independent receipt, checkpoint, rollback and gas assertions.
This is an implementation correction, not a retry-only pass of unchanged code.
The earlier production-stall mechanism was established by source and prior
expectation tests; no process-level before/after comparison is claimed yet.

The gas-oracle setup first rejected a different source-path metadata artifact,
then rejected incomplete bootstrap storage with a missing endpoint hash. The
corrected fixture executes the exact embedded runtime and installs a nonzero
bootstrap hash. These fixture corrections are not retries of an unchanged
passing scenario. Only the owned disposable Anvil was stopped afterwards.

## Requirement coverage

| Requirement | Implementation | Relevant regression |
| --- | --- | --- |
| Ordinary clock survives stalled/closed L1 preparation and depth exhaustion | `eez-driver/src/sequencer.rs`, `slot.rs`; Composer's local `block_attributes` | `ordinary_clock_crosses_missing_slots_and_speculative_cap_without_l1_context`, owned stalled-worker/closed-channel test; final Ethereum and Gnosis RPC-outage process scenarios passed. |
| Recorded advance/repeat remains stable with late evidence | Shared `l1_history`, EVM history validation, follower inputs and witness capture | EVM `repeat_is_valid_after_late_headers_arrive_and_advancement_resumes_at_next_update`; L1 immutable-snapshot and older-replay regressions. |
| Exact calldata, ordering, bounded import and delay | Shared decoder, block executor and native envelope | Missing/duplicate/misplaced update, chain/nonce/value/target, over-64, newer/regressing origin, malformed ABI and wrong RANDAO tests. |
| Real gas/receipt/nonce and application environment | EVM and Composer draft/prefix execution | Metered history/opcode test; full rebuild versus prefix checkpoints, native gas-cap and rollback tests. |
| Public replay and conditional proof agree | V0 codec, driver witness, deriver, signer streaming/execution/settlement | History byte round trip; final four-node native nonce/history checks, cold multipart signers/follower and refreshed native v2 corpus passed. |
| Recovery preserves required local evidence | Composer parent witness gate; deriver correlation capture, schema and reauthentication | Reproduced failures, corrected same exact multipart and partial-consumption tests, distinct checkpoint histories after reopening, invalid-origin rejection. |
| Both anchors and bounded multipart recovery | Existing OfflineRollup and v2 pipeline with recorded transactions | Contract anchor tests and native expired-source multipart recovery passed, including cold signers, unused-bridge rejection and independent follower state/history replay. |

## First process failure and correction

`cross_chain::minimal_bidirectional_cross_chain_smoke` reached the native node
and outbound-network-denied signer. Setup then timed out after 90 seconds waiting
for `compose_sync_slot` after ordinary deployments. With no queued work and an
inactive settlement timer, the independent clock correctly produces ordinary
blocks without invoking composition. The setup helper now verifies deployed
code/proxy bindings and waits a complete Sync interval before admitting work.

The same preserved log exposed competing ordinary/preparation Sequence requests
admitted against one completed parent. The enqueue-time check alone allowed the
second command to use attributes captured for the earlier parent. Sequence now
carries its exact expected parent into the actor and rechecks it before any
Engine call. A deterministic actor regression simulates the first producer
completing after the second was admitted, checks the typed stale-parent error,
and requires the accepted head to remain unchanged. This is a correctness fix,
not suppression of an Engine error. The corrected smoke passed and none of the
three fresh native Composer logs contained an ERROR event.

An unnecessary separate all-feature unit build was stopped before running tests;
its log is `continuity-actor-race.log`. The regression uses the existing affected
workspace feature combination in `continuity-actor-race-2.log` instead.

The passing smoke built `target/debug/deps/node_e2e-969bf1dbfc9e03d4` and the
current-source Composer/follower binaries. The next Cargo invocation started
rebuilding the alternate dependency feature graph after the signer build. It
was stopped before running a test (`continuity-native-lookup.log`). Subsequent
process filters invoke that exact E2E executable directly with
`--exact --test-threads=1`; runtime/test-harness sources are unchanged. Prebuilt
overrides remain cleared and the harness still builds the shipping signer.
Executable SHA-256 records are `continuity-runtime-smoke-3.sha256` and
`continuity-runtime-native-three.sha256`. No older runtime was substituted.

## Recovery failures and corrections

The combined run reproduced two production failures before the fixes:

- Multipart catch-up produced past block 1,100 while capture was still at 794.
  Reth could no longer anchor the historical trie overlay, and the existing
  durable-capture health gate correctly rejected further sequencing. Composer
  now uses the existing local witness source to persist its exact non-genesis
  parent before building the next ordinary block. Capture remains independent
  of remote proving and L1 RPC. The failed canonical witness is never skipped.
- The fresh partial-consumption follower derived the correct final root, then
  its post-subscribe resync reopened an earlier same-height checkpoint whose
  state had been replaced. Each correlation now retains the history state read
  from that exact executed checkpoint before acceptance. Recovery checks the
  canonical L1 occurrence and source header using this record. The derived
  index has a new explicit schema; no legacy migration or fallback is added.
  Focused storage tests cover distinct same-height histories after reopening
  and reject malformed origin records before persistence.

The first failure logs and owned datadirs remain under
`artifacts/native-e2e/continuity/`; they are local-only diagnostics. The repeated
exact process filters used newly compiled node/follower binaries, not the earlier
passing binaries. Both passed in `continuity-native-recovery-2.log`; identities
are in `continuity-runtime-recovery-2.sha256`. The multipart node exceeded 2,000
blocks with capture tracking production and no ERROR events; its test then
completed cold proofs, settlement and independent application/history replay.

The final 12-package unit invocation completed successfully. Its following signer
test command was stopped while still waiting for Cargo's artifact lock, before
compiling or running any tests, to let the cold process signers finish without
alternating dependency feature builds. `continuity-signer-unit-2.log` records that
wait only and is not test evidence; the completed repeat is
`continuity-signer-unit-3.log` (327 passed).

## Final review and limits

The final review traced the recorded transaction from immutable producer choice
through payload identity, gas/receipt/nonce handling, ordinary DA, follower
reconstruction, cold execution and conditional signature. It checked exact ABI
decoding, off-schedule/duplicate updates, unchanged v2 schemas, both anchor modes,
unused bridge coverage, partial-group checkpoints, reorg invalidation and the
single-owner preparation/commit fences. The two process failures found during
review were reproduced and their exact tests passed after correction.

An isolated `cargo test --locked -p eez-blob-codec --test
ethereum_kzg_compatibility` failed to compile: its unchanged manifest does not
enable Alloy's signer-recovery feature itself (`recover_signer` unavailable).
The repository-prescribed combined protocol/codec invocation supplies that
feature and executed the KZG oracle successfully. This standalone-package build
limitation remains; no dependency or lockfile changes were made to conceal it.
The original failure is `continuity-kzg-vector.log`.

The complete workspace/native/Kurtosis and release-artifact CI matrix was not
run locally; selected relevant libraries, real process scenarios and pinned
contract suites are the coverage reported here. Core fork tests need their
external configuration and retain the exclusions listed above. The optional
four-day stress test was not run: the required expired-`BLOCKHASH` mechanism was
covered with bounded historical parts instead. Nethermind/C# integration and
cross-implementation parity require that team's implementation; portable vectors
do not establish those results. No throughput or catch-up performance claim is
made from these runs.

Ordinary continuity assumes usable local execution/storage and an already
bootstrapped chain. Real canonical reorgs may replace unsafe history; disk
exhaustion fails closed. Evidence/session quotas still bound a single proof,
and the proposal does not specify recovery beyond its ancestry limit. The new
correlation index rejects old formats; this is a fresh-network profile, with no
legacy-network migration or rollback path.

The separate [specs-draft audit](../2026-10-23-specs-divergences/README.md) records
its own earlier source snapshot and compatibility analysis. The final capture
and correlation fixes preserve the consensus choices classified there.
No deployment, operator service restart, wallet funding or external publication
was performed. All implementation and validation stayed in the dedicated worktree.
