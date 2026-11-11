# Native v1 implementation and validation

Status: A1–A11 and repository-side E3 complete; local validation and independent
review complete. CI release/Kurtosis and external Nethermind execution remain
outside the local evidence below. The accepted contract is the [native specification](../../proposals/native-system-transactions.md)
and [history/branch scope](../../proposals/l1-origin-and-offline-proving.md).

## Baseline

- Branch: `codex/l1-history-offline-proving`.
- Worktree: `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`.
- Integration dependency: `8621b0368f4f42dc871dd84c0aaf5b6c4599bfbd` contains the
  upstream 206/205/198 integration; this work extends its subsequent offline-history
  and protocol changes rather than recreating them.
- HEAD: `3e23955dc477c9e675364776f118b1cecda75774`, plus existing recorded-history
  and specification changes, preserved before implementation.
- Core gitlink and initialized checkout: `84a5b5207880f192a6a5896cbfe0bac5249db47f`
  (`feature/e2e_edge_cases`). No core pin change is planned.
- Other branches/worktrees and operator state are outside this task.

## Requirement checklist

| Requirements | Required implementation/evidence | Status |
| --- | --- | --- |
| A1–A2 | 0x45/version 1, canonical envelope/RPC/receipt/storage, new sender, no native nonce, public admission rejection | Complete; focused tests and final four-node process check pass |
| A3 | Retain EEZL2 proxy/history addresses; regenerate sender-dependent runtime/genesis | Generated fixtures and renderer checks pass |
| A4 | Checked original positions, exact source hashes, both reconstruction directions, inline callbacks and partial replay | Complete; source/DA/partial-suffix unit and nested/partial process regressions pass |
| A5–A6 | Exact position gas; checked temporary funding, cleanup and rollback | Implemented; EVM/protocol/Composer tests pass |
| A7 | Required authenticated collector, per-transaction L2 fee credit, profile/config propagation | Complete; execution, configuration, contract, renderer and final four-node collector checks pass |
| A8 | Preserve recorded delayed history/PREVRANDAO, continuous production, both anchors and bounded catch-up | Complete; history, multipart/cold signer/public replay, reorg and Ethereum/Gnosis RPC-outage checks pass |
| A9/E3 | v2/offline proving, schema6/spec3/ProfileV3, language-neutral vectors and real transcripts | Complete within this repository; 320 signer tests, all 29 current-profile v2 cases, three network-denied captures and selected process regressions pass |
| A10–A11 | Retain compact V0/core grouped semantics and local Osaka calendar | Complete; protocol/codec/execution, nested process and pinned-core 64-scenario Anvil checks pass |
| Review gap | DELEGATECALL/CALLCODE code-context regression | 10 contract cases and inspector regression pass |
| Completion | Combined focused/workspace/contracts/process checks, independent review, exact requirement evidence | Complete locally; exclusions and external validation limits below |

## Requirement-to-implementation-to-test mapping

| Requirement | Production integration | Regression evidence |
| --- | --- | --- |
| A1–A2: canonical native transaction, sender and no nonce | `eez-primitives` envelope/receipt/Engine/database/RPC; `eez-evm` admission and native handler; public pool filters | Independent native reference bytes and receipt; malformed/trailing/noncanonical/version rejection; Engine and storage round trips; `native_never_checks_or_increments_the_account_nonce_and_ordinary_still_does`; zero-nonce native caller semantics |
| A3: fixed proxy/history deployment | Local history runtime, embedded runtime, generated Solidity code, all four genesis outputs and renderer | Local contract suite, `update-eezl2-genesis.sh --check`, actual positive rendering, required-collector negative cases; EEZL2 addresses/core pin unchanged |
| A4: exact occurrence identity | [Shared source domains](../../../crates/eez-protocol/src/native_source.rs), semantic reconstruction before filtering, Composer and driver plans, signer's exact DA reconstruction | Independent source vectors; `source_positions_count_original_brackets_and_failed_static_roots`; duplicate/root/context cases; `partial_suffix_retains_original_source_and_position_gas`; DA source mutation rejects; inline callback/group tests |
| A5: gas at the actual transaction position | `NativeTransactionPlan`, execution prefix receipts and shared materialization used by Composer/follower/signer | `native_bytes_commit_source_and_exact_remaining_gas_without_normalization`; receipt-derived plan tests; syntactically valid wrong gas rejects; real history gas oracle and budget limits |
| A6: temporary protocol credit and cleanup | EVM beginning-of-block cleanup, checked load/delivery funding and journal rollback | Balance cleanup versus L1 simulation, nested payable table overflow, rollback and no-native-nonce cases; core grouped call regressions |
| A7: fee collector | Required genesis field; L2 EVM final gas accounting; state checkpoints; ProfileV3; deploy renderer/config; optional indexer accounting v2 | Success/revert/halt/refund/floor; next-transaction visibility; sender/beneficiary aliasing; post-SELFDESTRUCT credit; absent account; overflow; zero-price advisory no mint; L1 fee distribution unchanged; config/preflight and profile vectors |
| A8: recorded history and ordinary liveness | Tx0 history import/repeat, clock/preparation separation, complete L1 ancestry witness, canonical recovery and parent/generation fences | History execution/opcode/delay/repeat/malformed/rollback/budget tests; pending RPC inherits executed choice; expired-source multipart catch-up uses cold offline signers and public replay; combined liveness/reorg scenarios below |
| A9/E3: offline v2 and external prover contract | Signer execution/settlement gates; ProfileV3, identity schema 6/specification 3; unchanged v2 protobuf; portable integration guide | 320 signer tests including 29 real TCP/Stateless transcripts/24 proofs; 27 independent native/profile encodings; three portable identity vectors; three network-denied source captures; no C# execution claimed |
| A10–A11: retained core and network execution rules | Existing compact V0 codecs, original whole-group/nested/static/reverted/partial semantics; local Osaka schedule | KZG/public-input/rolling-hash independent oracles; protocol/driver/codec suites; pinned core 778 unit tests and 64 semantic Anvil scenarios; changing-static and caught-revert captures; combined nested/partial process tests below |
| C09 review gap: code context | Pinned proxy dispatch, inspector leaves DELEGATECALL/CALLCODE to EVM | Ten local contract cases cover both chains, unauthorized contexts, authorized local delegation and CALLCODE self-probe; inspector regression |

## Executed evidence

Commands run at the isolated worktree root with Rust 1.95.0, `--locked`,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, LLVM 22 on PATH and worktree-local
`TMPDIR=$PWD/artifacts/tmp`. Logs under `artifacts/validation` are local-only,
ignored artifacts; they are not retrievable from a fresh checkout.

- Before changing primitives, added the independent corpus regression and ran
  `cargo +1.95.0 test --locked -p eez-primitives native_v1_reference_vectors_are_canonical`.
  **Expected baseline failure:** 1 selected test failed with
  `native-history-repeat: unexpected tx type`. The old decoder does not admit
  0x45. Log: `native-v1-primitives-before.log`.

## Implemented paths

- `eez-primitives` owns the canonical 0x45/version-1 envelope, sourceHash, fixed
  sender and zero RPC placeholders. There is no old native decoder or nonce path.
- `eez-protocol` owns checked source domains/original semantic positions and
  transaction plans resolved with the actual preceding receipt gas. Composer,
  deriver/follower and signer use these rules. The signer still independently
  reexecutes and compares all reconstructed bytes before signing.
- `eez-evm` preserves native zero-fee metering and temporary funding/cleanup,
  skips native nonce mutation, and credits ordinary base fees after final gas
  accounting and account deletion. The authenticated genesis collector reaches
  stateful execution, prefix checkpoints and Stateless replay.
- Local `OfflineRollup`, genesis rendering and portable validation identity bind
  the collector through ProfileV3 / identity schema 6 / validation specification 3.
  Proving remains v2; transport capability profile ID 2 is unchanged.
- History imports remain recorded tx0 actions, with position/genesis-bound
  sourceHash, delayed PREVRANDAO, both anchors and bounded multipart replay.
  No L1/L2 RPC is configured in the signer.
- The optional indexer recognizes 0x45 receipts and reports collected base fees
  (`baseFeeWei`, accounting version 2); native receipts remain zero-priced.

Obsolete rollup-format fixtures and their dedicated compatibility tests were
removed. Ordinary Ethereum type-0 transactions and historical L1 header layouts
are still part of the accepted execution/ancestry contract.

## Focused validation so far

- `cargo +1.95.0 test --locked -p eez-evm -p eez-evm-inspector -p eez-protocol -p eez-composer --lib`:
  Composer 407 passed / 1 ignored; EVM 35, inspector 29 and protocol 206 passed.
  Log: `native-v1-core-unit-4.log`.
- The new EEZL2 selector regression first failed because block admission allowed
  unknown native selectors. The shared admission check now admits only the two
  current EEZL2 entry points; exact ABI/value/source/effect checks still follow.
  Logs: `native-v1-selector-before.log`, `native-v1-core-unit-4.log`.
- `forge test --root contracts`: 130 passed, none skipped.
  Log: `native-v1-contracts-full.log`.
- Pinned core tests with the AGENTS exclusions: 778 passed, 3 skipped. The skipped
  cases are `testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`, and
  `testForkRejectsWrongSigningKey`; no fork-RPC environment was supplied.
  `script/*`, `test_Doc_OriginalValue_SeedInSetup`, and
  `test_PostCost_Incremental` are exclusions, not coverage.
  Log: `native-v1-core-contracts.log`.
- `cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml --test l2_fees`:
  6 passed. Mock RPC receipts test gas, collected fees, resumable accounting,
  malformed evidence and overflow; they do not validate a live indexer database.
- Deployment preflight: 12 tests passed, including nine actual renderer rejection
  cases and mocked broadcast prevention; Chiado configuration: 19 passed.
  These do not establish a deployment broadcast.

The history replay's fixed gas expectation was independently remeasured against
the current generated runtime using disposable Anvil 1.7.1 / Osaka. Both empty
imports at cutoffs 104 and 116 consume **41,092 gas**. The runtime digest and
results are in [the oracle](history-gas-oracle.json). The prior runtime's 41,077
expectation failed real replay before this correction. Native nonce preservation
is covered separately; the Anvil oracle executes ordinary zero-price calls.

The 10 `CrossChainProxyPolicyTest` cases close C09's code-context gap on both
chains: unauthorized contexts reject, manager-preserving delegation can forward
locally, CALLCODE does not preserve that caller, and its self-probe can succeed
without dispatch. The inspector regression confirms that both opcodes execute
through the EVM. There is no universal opcode rejection or protocol bytecode change.

The first combined signer run had 260 passing and 62 failing tests: the failures
used recordings from the previous profile. All three owned source-based native
capture scenarios now pass with network-denied signers, and their exact fresh
0x45/ProfileV3 requests, genesis and responses replace those recordings. The
regenerated 29-case corpus passes through real TCP/Stateless validation, including
24 successful proofs and the explicit rejection cases. The final workspace run
passes all 320 signer tests, with two ignored artifact utilities (fixture inventory
and vector generation). A `--lib` signer invocation selected no target because
this package is a binary; it is not coverage. The separate expanded process
matrix and its corrections are recorded below.

- `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings`:
  passed after removing redundant clones/references/casts reported by the first
  two runs. Logs: `native-v1-clippy-workspace-{1,2,3}.log`.
- `BASE_PORT=26800 BASE_CHAIN_ID=43337 bash scripts/ci/run-protocol-anvil.sh`:
  **64/64 passed**, no failures/timeouts/unrun scenarios. These are pinned-core
  semantic Anvil scenarios, separate from RollupX follower/prover process tests.
  Log: `native-v1-protocol-anvil-2.log`; per-scenario diagnostics under
  `eez-core-protocol/tmp/e2e-ci-pNgH8S1P` are local-only.
  The initial run with default `/tmp` failed 13 dry runs with empty Forge output
  before being stopped. A focused scenario passed with worktree-local `TMPDIR`
  and reproduced the empty failure with default `/tmp`. The complete rerun used
  the corrected temporary directory, unchanged sources and checked-disjoint
  ports. Subsequent tooling diagnostics established user-quota exhaustion on
  the host's `/tmp` tmpfs; the initial Forge output alone did not identify that
  cause. Initial logs are retained as
  `native-v1-protocol-anvil.log` and `native-v1-protocol-anvil-diagnostic*.log`.
- `bash scripts/update-eezl2-genesis.sh --check`: passed, generated state root
  `0xbf6ab976da61c036e82b55520ae881251921d7f1c280e5e665031f15f87042b1`.
  An actual render with an explicit mixed-case collector normalized that address
  into both output genesis/profile without modifying the base file. No deployment
  broadcast was made. Logs: `native-v1-genesis-check-2.log` and
  `native-v1-render-positive-2.log`.

The first full tooling run selected 397 tests and ended with 41 failure reports,
13 errors and 8 skips. The diagnostics expose host `/tmp` user-quota exhaustion
(`tmpfs` with `usrquota`), including scripts with absolute `/tmp` paths. This is
not successful tooling coverage. The public replay case reproduced the failure
and passed unchanged in a private namespace backed by the worktree's temporary
directory. The unchanged full matrix then passed in that namespace: **389 passed, 8 skipped**
(397 selected). No assertion was weakened:

```bash
export TMPDIR="$PWD/artifacts/tmp"
bwrap --bind / / --dev /dev --bind "$TMPDIR" /tmp --chdir "$PWD" \
  python3 -m unittest discover -s scripts/ci/tests -v
```

This bind exists only in the validation subprocess; host temporary data and
mounts are unchanged. Logs: `native-v1-tooling-full.log`,
`native-v1-public-replay-diagnostic.log`,
`native-v1-public-replay-private-tmp-2.log` and
`native-v1-tooling-private-tmp.log`. The first namespace probe omitted standard
`/dev` devices and failed before the test; adding `--dev /dev` fixed that setup.

The eight tooling exclusions are two real Kurtosis resource probes, the real
Docker data-only payload case, native archive relocation, two Docker runtime
build/context cases and two Gitleaks cases (tool unavailable). These require
separate opt-in environments or CI; the mocked passes do not establish them.
The seven SDK fee tests also pass (`node sdk/l2-fees.test.mjs`). The pinned
`forge build --root eez-core-protocol` succeeds; its existing lint warnings do not
change the clean protocol checkout.

The first workspace test pass reached `eez-node-common` after the codec/KZG,
Composer, RPC, deriver, driver, EVM, inspector, follower, L1 and node tests passed.
Three pending-RPC tests rejected their own incomplete genesis because their
replacement `rollupX` object omitted the now-mandatory collector. The fixtures
now supply it. Runtime configuration validation is unchanged. The rerun retains
all target failures with `--no-fail-fast`; initial log:
`native-v1-workspace-tests-1.log`. The complete second run reaches every non-E2E
workspace target and leaves one failing pending-RPC test; all other targets pass,
including the independent KZG/public-input/rolling-hash oracles. Log:
`native-v1-workspace-tests-2.log`.

That last RPC test claimed an import had executed while its parent fixture still
carried the bootstrap randomness. It also expected the L1 cache alone to change
an executed parent's environment. The corrected fixture explicitly represents
advance, repeat and a rebuilt parent: new or invalidated cached evidence cannot
rewrite an executed choice. The scheduled-import rejection and startup catch-up
gates remain checked. No production gate was removed; the affected package's
final repeat passes all **29** `eez-node-common` tests with `l1-startup` enabled:
`cargo +1.95.0 test --locked -p eez-node-common --features l1-startup --lib`.
Log: `native-v1-node-common-final.log`. This repeat changes only test fixtures;
it does not require rerunning the already-passing unrelated workspace packages.

The non-E2E workspace result, combined with the corrected package repeat, is
**1,580 passed, zero remaining failures, three ignored diagnostic/artifact utilities**.
This is the complete `--workspace --exclude eez-node-e2e --no-fail-fast` run plus
that targeted repeat, not a claim that the failed invocation itself exited zero.
The signer contributor checks also pass:

```bash
cargo +1.95.0 check --locked -p eez-proof-signer --all-targets
cargo +1.95.0 clippy --locked -p eez-proof-signer -p eez-node-common --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked -p eez-proof-signer --no-deps
```

Logs: `native-v1-signer-check-final.log`, `native-v1-signer-rpc-clippy-final.log`
and `native-v1-signer-doc-final.log`.

An initial independent read-only review found no additional correctness gap in
the paths it inspected; the later process-driven review below found two integration
regressions outside that initial conclusion. It confirmed source-position retention, exact gas reconstruction,
real nonce absence, collector/self-destruct ordering, ProfileV3/specification-3
versus capability-2 separation, and rejection tests reaching their intended gates
without signing. It identified two retired-option tests and an obsolete node
comment, now removed. A second focused review confirmed the pending-RPC fixture
correction: the helper inherits the supplied executed parent; the deriver still
invalidates and rebuilds an orphaned suffix, comparing both history hash and
randomness. The RPC unit test does not establish that process-level recovery;
the imported-history reorg scenario owns that obligation. This is static evidence,
separate from suite results.

Only disposable local test contracts/chains are deployed by validation. No public
deployment, service restart, funding or operator-state operation is authorized or
claimed by this record.


## Process-driven corrections

The expanded native-profile process matrix reproduced two integration regressions:

1. Both nested fee-quote scenarios rejected the current manager deployment. The
   estimator still recognized the prior OfflineRollup executable, so its static
   fixture test was internally consistent but stale. The fingerprint and fixture
   now describe only the current ProfileV3 bytecode; no alternate recognizer was
   retained. A new Foundry check compares the estimator fixture against the actual
   compiler artifact. It failed with the stale fixture before replacement and
   passes afterwards. The final local contract suite passes **131 tests**, none
   skipped. Logs: `native-v1-fingerprint-before.log`,
   `native-v1-contracts-final.log`; compact identity evidence:
   [manager fingerprint](manager-fingerprint.json).
2. The four-node case's first payable inbound call failed in simulation. Disabling
   transaction fee charging also bypasses pinned REVM's balance-check relaxation;
   a synthetic caller proxy therefore had no value to forward. The direct-call
   simulator now uses explicit transaction-local credit, shared with the existing
   native credit mechanism. It credits **existing balance + delivered value**,
   preserves legitimate returned value on success, removes credit on failure and
   finalizes the journal on error. It does not modify the canonical state cache
   before execution. Only the three synthetic mutable target paths opt in;
   physical L1 manager escrow, static frames and ordinary transactions retain
   their own execution. Nonce restoration and source-frame checkpoints remain.

The affected package repeat passes **408 Composer tests / one ignored diagnostic,
37 EVM tests / none ignored**, including the new unfunded/funded proxy, returned
value, revert/halt, zero-fee, invalid-context, overflow, error-recovery, repeated
successful delivery and source-rollback cases. Log:
`native-v1-simulation-final.log`. The passing payable and fee-quote process
repeats, and the separate collector-fixture correction, are recorded below.

The same review found an independent snapshot inconsistency in fee pricing:
its gas probe froze history, but each price iteration rebuilt a draft from the
mutable history source. Pricing now calls the same `l2_draft(snapshot)` as gas
estimation, preserving the recorded prefix, PREVRANDAO and state across new L1
arrivals. A focused history regression fixes the sample boundary, publishes more
evidence and verifies that both retained and newly reconstructed sample snapshots
remain unchanged. Its passing execution is recorded below. First process failures and their
node logs/datadirs are retained under `artifacts/native-v1-e2e` and
`artifacts/validation/native-v1-integration-1.log`.


## Compatibility and handoff boundaries

This is a fresh-genesis profile. The transaction/receipt bytes, reserved sender,
state transitions and required genesis collector are network-defining. Deploy
matching node, follower, signer, manager and generated genesis artifacts together.
There is no existing-database migration, old-profile fallback, mixed-version
execution or live-network activation mechanism in this change. Rolling back a
started network to different consensus bytes requires discarding that network's
state and deploying a new network; replacing a binary does not migrate it.

The proving service remains `prove.v2.Prover.ProveStream`; `prove.proto` is
unchanged. Its advertised validation identity changes to schema 6 / specification
3 / ProfileV3, while the streaming capability profile remains 2. A foreign prover
implements that same current contract and runs the checked-in conformance corpus.
No Nethermind adapter or C# executable is supplied or claimed tested; that team's
implementation is outside this repository. Signer requests contain the complete
L1 evidence required for conditional proving, authenticated against the context
checked on-chain; the signer has no configured L1 or L2 RPC source.

`feeCollector` is mandatory in genesis and the manager's profile commitment.
`EEZ_L2_FEE_COLLECTOR` is the deployment renderer input; runtime components read
its authenticated result rather than independent overrides. Generated fixtures
use public test configuration. Production base genesis still allocates no ETH.
The optional indexer emits accounting version 2 and `baseFeeWei`. Core, Reth,
Stateless, REVM and Alloy pins and both Cargo manifests/lockfiles remain unchanged.

## Reviewable file groups

- `eez-primitives`, `eez-protocol`, payload/blob consumers: canonical type/source
  identities, checked gas plans and shared reconstruction with original positions.
- `eez-evm`, Composer, driver, deriver and node/common: native execution, ordinary
  fee credit, recorded history, prefix simulation and follower/RPC integration.
- `OfflineRollup.sol`, local history contract, generated runtime and genesis:
  collector-bound profile, fixed authorization and deterministic predeploy state.
- Signer, control RPC and prover client: current-profile independent validation,
  portable identity, protocol documentation and real v2 recordings/transcripts.
- Deployment/configuration scripts, operator docs and the optional indexer:
  required collector propagation, fresh-network setup and receipt fee accounting.
- Unit, Solidity, process and tooling tests: current-profile acceptance/rejection,
  rollback, replay, liveness, original failure regressions and generated-byte drift.

No dependency upgrade, core-contract change, public deployment or PR publication
is part of this work. The final CI release-artifact/Kurtosis matrix and external
C# implementation remain separate validation boundaries; local source-based
results do not establish those environments.


## Combined process matrix and final repeats

The first expanded invocation selected **15 process tests**: **12 passed and
three failed**, with no ignored tests. Log: `native-v1-integration-1.log`. The
failures were the two nested fee quotes and the four-node payable deposit
identified above; they are not accepted as passing coverage.

The passing scenarios cover expired-source multipart catch-up with both anchors,
unused-header bridges, cold network-denied signers and public follower replay;
late original inclusion after bounded replacements; all five partial-consumption
cases including restart; imported-history reorg recovery with twenty ordinary
transactions; multiple heavy inbound deliveries within position-derived gas;
shared offline signer/two-composer recovery from a native L1 reorg; and both
nested transaction-reference directions with inline callbacks. The composer and
follower in this first invocation were built before the process-driven fixes.
The later signer subprocesses can rebuild their source dependency; therefore this
first pass is baseline evidence, not an identity claim for the final runtime.

After the fixes, `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-l1 -p eez-composer`
passes **928 tests** (408 Composer, 200 L1, 320 signer), with the same three ignored
utilities. `fee_context_history_stays_at_its_sampled_boundary_after_new_evidence`
directly covers retained and reconstructed history at the sampled quote boundary;
it does not claim to schedule a concurrent pricing process. The 29 v2 corpus
cases also run successfully inside the signer suite. Log:
`native-v1-reviewed-packages.log`. Together with the 37-test EVM repeat and prior
unaffected workspace targets, the current non-E2E total is **1,584 passed**.

Explicit vector regeneration selected exactly one utility and passed; the
manifest then verified all 29 cases. Logs:
`native-v1-vectors-final-regeneration.log`,
`native-v1-vector-manifest-final.log`. Expected responses are produced by the
request-side/captured-data oracle, never learned from the server under test.


The final independent static review re-read the corrected recognizer, temporary
credit helper and all three Composer callers, plus the shared pricing snapshot.
It found no further actionable issue in those paths. The direct quote-boundary
regression was added afterwards and passes; it addresses the reviewer's identified
coverage limitation without claiming a process-level concurrency test. Final
workspace clippy (`--all-targets -- -D warnings`), Rust/Foundry formatting, shell
syntax, `git diff --check`, generated genesis and **491 local documentation paths**
also pass. Logs: `native-v1-clippy-final.log`, `native-v1-genesis-final.log`.


The three fresh source-capture scenarios were selected separately on the same
`node_e2e` target, with prebuilt overrides cleared, the test capture destination
set, `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1` and `--test-threads=1 --nocapture`:

```text
cross_chain::minimal_bidirectional_cross_chain_smoke
cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes
cross_chain::outbound_changing_static_reads_and_write_settle_and_replay
```

Each selected one test and passed. Logs: `native-v1-capture-bidirectional-1.log`,
`native-v1-capture-lookup-1.log`, `native-v1-capture-static-1.log`. The checked-in
requests and responses, not those local logs, provide the portable cold-replay
inputs used by the final signer corpus.

The final shipping selection also builds successfully with Rust 1.95.0 and
`--locked`, using the same package/target/feature selection as `Dockerfile.runtime`
(the local debug profile, not a published release image):

```bash
cargo +1.95.0 build --locked \
  -p eez-node -p eez-follower -p eez-proof-signer -p eez-protocol -p eez-l1 \
  --bin eez-composer --bin eez-follower --bin eez-proof-signer \
  --example genesis_state_root --example genesis_block_hash \
  --example e2e-causal-verify --example verify_blob_evidence
```

Log: `native-v1-shipping-final.log`. The six-case final process selection runs
with prebuilt overrides cleared, `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`, and separate
retained logs/datadirs under `artifacts/native-v1-final`:

```text
cross_chain::inbound_value_and_calldata_apply_atomically
fee_quotes::exact_quote_with_caught_nested_revert_is_admitted_and_replayed
fee_quotes::unsigned_nested_quote_matches_receipt_and_replays
native_system::two_composers_two_keyless_followers
settlement_policy::ethereum_ordinary_burst_is_budgeted_across_sync_intervals
settlement_policy::gnosis_ordinary_burst_is_budgeted_across_sync_intervals
```

The existing `node_e2e` target is filtered to those names with
`--test-threads=1 --nocapture`; log `native-v1-integration-final.log`. This
invocation finished with **five passed, one failed, none ignored**. The two fee
quotes and atomic payable inbound pass with the corrected production paths.
Both ordinary-burst cases pass: Ethereum and Gnosis continue producing and
including transactions while L1 RPC acquisition fails, then settle and replay
after recovery. The already-passing nested
transaction-reference tests carry zero ETH; their path is unchanged by the
new positive-value credit, so they are not repeated solely to increase counts.

The remaining four-node failure reached the collector-profile assertion after
successfully completing its deposit, withdrawal and follower replay. The test
fixture's `bind_l2_genesis_reference` overwrote the explicitly chosen collector
with a default when rebinding its L1 checkpoint. The helper now reads the required
existing derivation profile and preserves its checked collector. A regression
rebinds two different checkpoints and verifies the collector, hash, timestamp and
randomness. Production deployment uses the explicit genesis renderer and does
not call this private helper. An independent read-only review checked both
fixture callers and the production renderer and found no further gap. The
focused `cargo +1.95.0 test --locked -p eez-testkit --lib` passes **20 tests**,
none ignored; together with the prior unaffected packages, the final non-E2E
workspace coverage is **1,585 passed**. Focused clippy also passes with
`--all-targets -- -D warnings`, and the exact four-node process repeat passes
**one test, none failed or ignored**. Their logs are `native-v1-testkit-final.log`,
`native-v1-testkit-clippy-final.log` and
`native-v1-collector-final.log`. The failed process logs/datadirs remain under
`artifacts/native-v1-final`; the repeat has separate diagnostics under
`artifacts/native-v1-collector-final`.

The final four-node test independently checks every block through its accepted
endpoint across two Composers and two keyless followers, including a follower
started after settlement. Its checks cover exact block/receipt equality, history
tx0 identity and delay, all imported L1 hashes, header PREVRANDAO, both cross-chain
directions, zero native nonce/fees, position-derived gas and the accumulated
ordinary fees of a distinct, initially absent collector. The run observed 21
scheduled history updates, 43 blocks using advanced L1 history and live Sync
anchors at heights 90 and 105. Exact identities are retained for the
[first matrix](native-process-executables.json),
[six-case corrected-production matrix](final-process-executables.json) and
[passing four-node repeat](collector-process-executables.json).

The selected process coverage comprises **18 distinct scenarios with resolved
results**, plus the three successful capture scenarios. This combines the first
12 passes, five later passes and the final four-node correction; it does not
reinterpret either failed invocation as a pass or claim all 178 process tests.
The controlled driver regressions
`ordinary_clock_crosses_missing_slots_and_speculative_cap_without_l1_context`
and `closed_source_and_stalled_preparation_do_not_stop_local_ticks_and_worker_is_owned`
also pass in the workspace run. They establish clock progress without settlement
calls and while its owned worker is stalled, including cancellation; the two
RPC-outage process cases provide separate chain-execution/replay evidence.

After the final fixture and documentation changes, Rust/Foundry formatting,
shell syntax and `git diff --check` pass again. The final documentation check
resolves **494 local links across 29 edited/new Markdown files**. The v2 manifest
verifies all 29 cases and all 166 recorded source-file digests still match.
Logs: `native-v1-fmt-closure.log`, `native-v1-forge-fmt-closure.log` and
`native-v1-static-closure.json`. The initialized core remains clean at the same
gitlink; Cargo manifests/lockfiles and protobuf schemas are unchanged.

## Remaining validation boundaries

- The complete release-artifact, Docker and Kurtosis matrix belongs to CI and
  was not executed locally. Shipping target selection was built in debug mode;
  no release image digest, live public deployment or performance claim follows.
- The local process selection covers the affected mechanisms, not every one of
  the 178 available scenarios. Earlier passes apply to their documented sources;
  the changed positive-value and quote paths were repeated after correction.
- Three core fork-RPC tests, two explicitly excluded core tests and eight tooling
  opt-in/tool-dependent cases remain excluded as listed above. Three workspace
  diagnostic/artifact utilities are normally ignored; vector generation was
  explicitly executed once.
- Nethermind/C# integration remains that team's work. This repository supplies
  the current language-neutral contract and executable v2 corpus; no C# runtime
  conformance is claimed.
- The catch-up test uses expired sources separated by more than 256 L1 blocks,
  complete unused-header bridges and multiple parts. It validates the required
  expiration mechanism, not a literal four-day load or throughput benchmark.
- Logs and datadirs are retained locally under ignored `artifacts/` paths. The
  checked-in compact oracles, fixtures, corpus and executable identities remain
  available to another checkout; the bulk logs do not.

No unresolved failure remains in the selected local validation. The branch is
ready for PR review, with matching contracts/genesis/binaries required for a new
network. No compatibility path or validation dedicated to a retired rollup
profile is retained, and no commit, PR publication or deployment is claimed.
