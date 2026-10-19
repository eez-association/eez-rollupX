# Nested created-account visibility: development fix

| Field | Value |
| --- | --- |
| Date and status | 2026-10-18; fix implemented and locally validated; required CI on the updated PR remains pending |
| Scope | Preserve created account state when a nested callback returns to the suspended source EVM; PR #91 |
| Source baseline | `eez-association/eez-rollupX`, PR #91 head `0bb1396d9cb660738b17d43a6cdbf9a564177823`; PR base `2c19e36bed2720c00f89841ccfca47ec7c921e06` |
| Protocol | Initialized, unchanged `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Environment | Rust 1.95.0, Foundry 1.7.1, Solc 0.8.34; locked dependencies; new GnosisPrague cases and existing EthereumOsaka overlay controls |
| Related records | [Scope and acceptance](../../proposals/nested-created-account-visibility.md), [public report](https://github.com/eez-association/eez-rollup0/issues/208), [inspector](../../../crates/eez-evm-inspector/README.md) |

## Reproduced behavior and fix

The original L2 caller calls an L1 relay, which calls back into an L2 factory.
The factory creates a child, checks its code and constructor storage, and
returns its actual address. The resumed L2 caller then checks the child's code
and calls `add(1)`. At the baseline, both CREATE and CREATE2 reach
`MissingChildCode(actualChild)`, despite both cross-chain calls succeeding.
The existing-child controls settle and reach value 42.

The return overlay previously omitted runtime code and factory nonce changes.
Its outgoing snapshot also missed accounts and constructor storage present
only in the live journal. The fix applies the complete live account state to a
cloned cache, then propagates returned code, nonce, balance, storage and creation
state using REVM's journal operations. Enclosing reverts can therefore undo the
changes. An account or slot first loaded by the callback is compared with live
source state, rather than assumed empty or zero.

The first candidate settled both simple creation cases, but the extended
re-entry checks failed deterministically: the second callback read the child's
old value 41 instead of the source's later write 42. A retained target session
was only preloaded when first opened. It now refreshes from the new dispatch's
source snapshot before saving its rollback checkpoint; sibling callbacks within
one frame retain their cumulative cache and applied rollbacks. The first
candidate's failures and datadirs are retained, not counted as passing tests.

This reproduces and fixes a RollupX execution defect. The original historical
GPv2 transaction body remains unavailable, so this does not establish its cause.
The protocol's existing rejection of failed cross-chain calls with descendants
remains in effect. Callback-cache SELFDESTRUCT mutations and transient-storage
exchange are outside this change.

## Baseline evidence

[baseline.json](baseline.json) retains the exact source/protocol identities,
executable hashes, process counts and both RPC rejection records. The first
process run exited 101: **0 passed, 2 failed, 0 ignored**, with the specific
missing-code error in both cases. Both existing-child controls committed; the
two ordinary-EVM fixture tests passed. These were intended-success regressions,
not tests that treat rejection as a pass.

The initial fixture patch had SHA-256
`3078cf875cade0245046dc4b8926d15082fcc70243783c81a08b328c798967f6`.
The fix's process module retains the same failing path and extends it with
repeated callbacks, factory nonce continuity and follower replay. Full baseline
logs/datadirs are local-only under the operator workspace's ignored
`artifacts/issue208-nested-e2e-20261018.r5jc3f6y`; the full process log hash is
`44d80caa1f936d2fea23925a6aed3f004c382642d6784447ede70b345642d5e2`.
The original reproduction patch remains local-only in the preceding
`2026-10-18-issue208-wrapper-state` investigation. Compact observations here are
available in this checkout; the ignored artifacts are not.

## Regression coverage

| Requirement | Check | Evidence type |
| --- | --- | --- |
| R1 | New code, nonce, balance and constructor storage survive return; cold, warm-empty and prefunded creation addresses | Inspector unit tests; CREATE/CREATE2 process tests |
| R2 | Factory nonce advances across callbacks and the subsequent local CREATE uses the expected next address | Inspector unit tests; process assertions |
| R3 | Outer checkpoint restores code, nonce, balance, storage and creation flags; failed dispatch does not apply callback state | Inspector unit tests; existing overlay process control |
| R4 | A second callback can use new code and observe the original source's write; snapshots do not mutate the source cache/journal | Snapshot unit test; process assertions |
| R5 | Missing/mismatched new code and callback destruction fail explicitly; read-only cache loads do not overwrite source state | Inspector unit tests |
| Canonical execution | Source receipts, exact safe-chain block, child code/value and factory nonce | Local Composer, signer and Gnosis process tests |
| Independent replay | Fresh follower reconstructs the safe block from canonical L1/blob data and executes the second child's getter | Local follower process test |

## Commands and results

Commands run from the isolated checkout at
`/tmp/rollupx-issue208-review-20261018`. Builds use
`CARGO_TARGET_DIR=/home/edu/Development/rollupX/target`, `CARGO_BUILD_JOBS=2`,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` and that LLVM bin directory on `PATH`.
Runtime overrides `EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` are unset. Tests retain their own logs/datadirs under
the operator workspace's ignored `artifacts/issue208-fix-20261018.qpjwsswd`.
No existing network's datadir or ports are reused.

The initial 20-test component run passed with two redundant-import warnings;
the imports were removed before the clean 27-test run. Both logs are preserved.
The first workspace Clippy run failed its `if_not_else` rule; the conditional
was reordered before final validation. The first process run had 0 passed and
2 failed tests: both existing/fresh phases committed, but both extended re-entry
phases failed with `existing child has stale state`. The full tests never reached
their follower check in that run. This led to the session-refresh correction.

Final validation used unchanged runtime/test sources after that correction:

- `cargo +1.95.0 test --locked -p eez-evm-inspector -p eez-composer --lib`:
  **406 passed**, 0 failed. The existing
  `held_pool::tests::profile_blocked_backlog_discovery` manual timing profile
  remains ignored; it is not correctness coverage.
- `cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- cross_chain::nested_creation:: cross_chain::overlay_ --nocapture --test-threads=1`:
  **6 passed**, 0 failed, 0 ignored, 154 filtered out, in 401.54 seconds including
  the first proof-signer build. No unchanged-candidate retry was needed.
- `forge test --root contracts`: **89 passed**, 0 failed, 0 skipped. The fixture
  source did not change after that run.
- Locked workspace/all-target Clippy with warnings denied, Rust/Solidity
  formatting, `git diff --check`, and edited local Markdown links passed.
- The CI module-selection test passed. `nested_creation` is a child of the
  existing `cross_chain` module, so the existing CI filter includes both cases.

Both creation tests assert the existing-child control, newly created child
code and value 42 after settlement, a second callback seeing the source's local
write, a subsequent local CREATE at the expected factory nonce, and final child
values 42 and 77. A fresh follower independently reconstructs the exact safe
block from L1/blob data and executes the second child's getter. The four existing
overlay controls cover nested calls without re-entry, sibling callbacks, an
inbound round trip and the specific unsupported-failed-descendant rejection;
that rejection test is not evidence that such calls can settle.

| Evidence | Contents | Availability |
| --- | --- | --- |
| [baseline.json](baseline.json) | First missing-code failures and baseline executable hashes | Checked in |
| [first-candidate.json](first-candidate.json) | Simple-case commits, extended stale-state failures and candidate executable hashes | Checked in |
| [validation.json](validation.json) | Final source/file identities, executable hashes, observations, test counts and log hashes | Checked in |
| `artifacts/issue208-fix-20261018.qpjwsswd` | Full first/final logs and first-candidate failed datadirs | Local-only; unavailable from another checkout; passing datadirs are removed by the testkit |

## Boundaries

This change is a Composer simulation fix. It does not change the protocol pin,
production contracts, ABI, configuration, persisted schema, genesis, or signer
wire contract. Older Composers may continue rejecting this nested flow.
Historical canonical blocks still use native execution. An eventual Composer
restart has the existing in-memory queue/resubmission boundary.

The creation regressions use the requested local Gnosis profile. Full protocol
stress, Kurtosis, the complete fee/Safe process matrix, public Chiado/mainnet
execution and deployment were not repeated for this fix. Required CI on the
updated PR remains the merge gate. There is no performance or capacity claim.
