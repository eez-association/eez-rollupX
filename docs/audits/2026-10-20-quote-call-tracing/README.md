# Quote call tracing validation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-20 UTC; local validation complete, release CI pending |
| Scope | Optional debug quote call tracing and a local JSON-import viewer |
| Source baseline | `eez-association/eez-rollupX` `a2d7b0a3be89df3c0b6ac945acd41a830278b0a7` |
| Final runtime/test source patch | SHA-256 `dee9de9e9abf636437d68377ae89b66578bf9c9cda8931be45fcebc38efa63f3`; binary git diff from the baseline restricted to `crates/eez-composer`, `crates/eez-node`, `crates/eez-testkit`, `crates/eez-node-e2e` |
| Full-suite source patch | SHA-256 `42d5b7c3f4ea630db51ed5d5c4faed5101a5cc6ec08bdf7408b587e545c24808`; same scope, before the final ingress batch guard and its tests |
| Protocol | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`; unchanged, clean checkout |
| Environment | Rust 1.95.0, LLVM 22, Foundry 1.7.1; native local Ethereum fixtures; Node 26.7 and Playwright 1.63.0/Chromium |
| Related records | [Authorized first iteration](../../proposals/quote-call-tracing.md), [API contract](../../configurable-settlement-policy.md#compact-fee-quotes), [viewer and manual capture](../../../tools/quote-trace-viewer/README.md) |

## Scope of the evidence

This is a new debug feature. Baseline inspection found no call trace export in
`eez_debugQuoteFee`; the existing diagnostics carry costs and execution errors.
The regressions compare traced and untraced calculations and check the actual
cross-chain frame relationships. They do not diagnose the user's historical
failed transaction, whose input was not supplied.

The capture is advisory simulation evidence. It does not establish mined
inclusion, proof verification, independent follower replay, durable recovery or
production capacity. No live network deployment or restart is part of this work.

## Executed checks

| Check | Evidence type | Result |
| --- | --- | --- |
| Composer library tests | Unit/embedded | Final run: 395 passed, one existing manual component-profile test ignored |
| Node library tests | Unit/embedded | Final run: 97 passed, including trace-batch rejection before execution |
| SDK fee tests | Unit | 7 passed |
| Viewer model | Unit | 9 passed: recorded export, format validation, parent links, errors, rollback, limits, deep trees and run selection |
| Viewer import/navigation | Chromium browser | Passed using the pinned npm dependency, the recorded example and all nine exports from the final batch-guard runtime; hostile text stays text and makes no network requests |
| CI tooling | Mocked/static/process tooling | 381 passed; eight environment-dependent checks skipped |
| Formatting, documentation links and shell examples | Static | Passed |
| Nested call trace process test | Local node/Anvil process | Final run: one selected test passed in 41.69 s; both directions/formats, CREATE/CREATE2, caught and propagated reverts, unchanged gas/source state, real HTTP trace-batch rejection |
| Complete fee quote and Safe suite | Local node/Anvil process | 21 passed, none ignored, in 1047.41 s on the full-suite runtime identified below |
| Workspace Clippy, all targets, warnings denied | Static | Passed after the final runtime changes |

The tooling skips are the opt-in real Docker payload probe, two real Kurtosis
resource probes, real native archive relocation, two real Docker runtime
fixtures, and two Gitleaks checks (the scanner is installed by CI). These are
not counted as passed coverage.

The 21-test suite covers Ethereum and Gnosis fee/Safe behavior, legacy and
EIP-1559 transactions, nested calls, calldata boundaries, nonce predecessors,
underpayment, price changes, settlement and independent follower replay where
asserted by those tests. The final backend change after that suite rejects
JSON-RPC batches containing traced quotes before any member runs; it prevents
accumulating many bounded trace replies in one batch. The complete suite was
not repeated after that ingress-only guard. Final node tests, workspace Clippy,
the actual HTTP/nested tracing process test, and the browser import test passed
after the guard. The tracing process test uses the Ethereum profile.

The full Kurtosis and release-artifact CI matrices were not run locally.
Successful local tests do not establish release promotion or deployment.

## Reproduction

Commands ran in the isolated feature checkout
`artifacts/issue93-review-20261020/source`, with the pinned protocol initialized.
Existing operator state and the unrelated mainnet exclusion investigation were
left untouched. The shared Cargo cache is outside that checkout.

```bash
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH="/usr/lib/llvm-22/bin:$PATH"
export CARGO_TARGET_DIR=/home/edu/Development/rollupX/target
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN

forge build --root contracts
cargo +1.95.0 build --locked --offline \
  -p eez-node -p eez-follower -p eez-proof-signer --bins
export EEZ_TEST_LOG_DIR="$PWD/artifacts/quote-trace-validation/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/quote-trace-validation/datadirs"
cargo +1.95.0 test --locked --offline -p eez-node-e2e \
  --test node_e2e --no-run --message-format=json
```

The first process run selected an older Composer from the shared target
directory and rejected `trace` as an unknown option. Inspection confirmed that
executable lacked the new trace marker. That run does **not** validate this
feature. Subsequent runs use copies of the freshly built role executables in
`artifacts/quote-trace-validation/final-runtime/` for the full suite and
`artifacts/quote-trace-validation/batch-guard-runtime/` for the final targeted
run, with their SHA-256 identities recorded below. The test executable is
selected from Cargo's compiler-artifact JSON and copied there too.
`EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` then point explicitly to those isolated files.
This prevents another checkout from changing the executable used by later
fixture processes. It is local source validation, not CI release provenance.

| Executable | SHA-256 |
| --- | --- |
| Full-suite Composer | `cc1378765533dd14952905e716b43bc9a17d0a346e2daf026a4e23af7e465820` |
| Final batch-guard Composer | `c8b848fc9a63cde6de6e8a16ff0cbd866e2f34feae256a3866e2616222fef223` |
| Follower | `183daea16a94788133903767b8ade01baf3d44c005bdb2dc6807349f4d1a1df0` |
| Proof signer | `11c083d9304ad084cf087ba75a50be501038e5e606bcc14b2e624cb6cb75897e` |
| Full-suite test executable | `d719a4e0996ffa2c8335248c58d88de8d42a2147a996dda4c925ba9d64e7e22d` |
| Final targeted test executable | `c8fc6e851927c8e6c34a728efdc94ef05d6ffaa24d9871adbb2887979424fdb8` |

Follower and signer hashes were unchanged between those builds. With the
corresponding runtime overrides set, the copied test executable ran the
following nonempty selections serially:

```bash
"$EEZ_TEST_RUNTIME_BIN_DIR/node-e2e" fee_quotes:: --test-threads=1
# Final targeted run with the batch-guard runtime:
"$EEZ_TEST_RUNTIME_BIN_DIR/node-e2e" fee_quotes::tracing:: --test-threads=1
```

The viewer's [test instructions](../../../tools/quote-trace-viewer/README.md#tests)
run its pinned dependencies. Passing the same `EEZ_TEST_LOG_DIR` to the browser
test imports every actual process-test response in `quote-traces/`.

## Retained diagnostics and corrections

Local-only build/check logs are under `/tmp/eez-quote-trace-*.log`; process logs,
datadirs and exported responses are under the ignored
`artifacts/quote-trace-validation/` in the feature checkout. These paths are not
available in another checkout and are not a published artifact service.
The small [nested-revert export](../../../tools/quote-trace-viewer/examples/nested-revert.json)
is checked in and used by the viewer tests. It contains only public fixture
addresses and synthetic local state.

Early validation caught and corrected the following implementation/test issues:

- REVM 43's `Either` inspector adapter does not forward frame hooks. The trace
  wrapper now directly forwards all hooks; real EVM tests verify nonempty
  complete traces and unchanged results.
- The generic local-client fixture predates Byzantium. The revert-equivalence
  regression explicitly enables Byzantium so it checks REVERT bytes.
- A browser assertion raced asynchronous file loading; it now waits for import
  completion.
- The new CI lane required updating the workflow contract test and release gate.

Compilation errors, lint findings and these first failures are retained in the
local logs. A pass following a code/test correction is not evidence that an
unchanged flaky test passed reliably.
