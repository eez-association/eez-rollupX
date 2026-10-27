# Upgradeable L1 history and issue #113 validation

| Field | Value |
| --- | --- |
| Date / status | 2026-10-27; local validation, final CI remains required |
| Scope | Initially empty explicit-pair history store, upgrade authority, execution/proving/replay and fresh-network deployment assets |
| Source | `codex/upgradeable-l1-history`, based on PR #112 at `d5b6d3c9efa6ecdd8147edab532ff913f425426f`; implementation is the commit containing this record |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and unchanged, with no submodule edits |
| Isolation | `artifacts/upgradeable-l1-history-20261027`, initially clean; original checkout and operator state preserved |
| Tools | Rust 1.95.0, Foundry 1.7.1, Solidity 0.8.34, optimizer runs 1 and via IR; locked dependencies |
| Requirements | [Accepted proposal, H1–H6](../../proposals/upgradeable-l1-history.md), [issue #113](https://github.com/eez-association/eez-rollupX/issues/113), [normative history contract](../../../contracts/L1_HISTORY_SPEC.md) |

## Findings and coverage

The Solidity contract's executable tokens match the issue's proposed contract
exactly; only documentation and formatting differ. The issue was re-read after
implementation: unchanged body and no review comments. The predeploy is an
`EEZProxy` at `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077`, with its own ProxyAdmin.
The existing `EEZ_L2_UPGRADE_OWNER` initializes that admin's owner.

| Requirement | Implementation and exercised checks | Evidence |
| --- | --- | --- |
| H1: upgrade authority | Pinned EEZProxy/ProxyAdmin, unauthorized calls, transfer of ownership, compatible changed implementation and rollback | Solidity tests; real authorized/unauthorized L2 upgrade transactions |
| H2: exact store semantics | Empty/block-zero reads, positive increasing pairs, gaps/no backfill, zero hashes, uint64 maximum and atomic rollback | 13 history contract tests, including fuzzing; canonical native and ABI tests |
| H3: genesis identity | Empty app storage, proxy pointers, implementation/admin code and nonces, owner validation, generated three-account allocation | 13 malformed-genesis mutations and alternative owner; deterministic generation; deployment preflight and Docker context tests |
| H4: witnessed upgrades | Runtime validates proxy identity; EVM resolves the historical implementation from state | Real upgrade/import/settlement, cold follower DA replay; missing/replaced implementation-code witnesses reject before signing |
| H5: execution invariants | Empty public state uses the configured logical L1 origin; authenticated sparse pairs retain complete ancestry, schedule/delay, metered gas and PREVRANDAO | EVM positive-bootstrap, sparse-import, missing-evidence, late-header/omission, malformed/duplicate/position/gas tests; cross-chain and offline signing process tests |
| H6: coordinated encoding | Explicit-pair 0x46 throughout primitives, shared ABI, producer, witness, follower, pending RPC and signer; validation contract 6 | Affected unit suites, native DA/KZG/public-input oracles, regenerated frozen requests and 29 streaming cases |

The owner remains trusted with future implementation behavior. The proof
establishes execution of witnessed code, not that arbitrary replacement code
honors the intended API. Contract gaps are permanent; the producer currently
fills consecutive available records. A zero lookup is unavailable evidence and
is still rejected by L1 storage-proof verification.

No core contract, core gitlink, Cargo dependency, cross-chain 0x45 encoding or
`prove.v2` protobuf changed. This changes the history ABI, storage, native body,
genesis, profile commitment and validation identity together. It is for fresh
networks only. The runbook documents compatible future implementation upgrades,
independent ownership and rollback limits; it does not claim a live deployment.

## Executed checks

Commands ran from the isolated worktree with `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`,
that LLVM `bin` directory on PATH, `CARGO_BUILD_JOBS=2` and a worktree-local TMPDIR.
All Cargo checks used `--locked` and Rust 1.95.0.

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-blob-codec -p eez-composer -p eez-control-rpc -p eez-deriver -p eez-driver -p eez-evm -p eez-node-common -p eez-payload-codec -p eez-primitives -p eez-protocol -p eez-prover-client --lib` | 907 passed; one existing manual Composer timing profile ignored |
| `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client` | 330 signer, 7 RPC and 34 client tests passed; three explicit fixture-generation/inventory utilities ignored |
| Final EVM/signer/RPC/client run after restoring the schedule check | 48 EVM, 330 signer, 7 RPC, 34 client passed; same three signer utilities ignored |
| `cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --lib --example verify_blob_evidence` | 200 L1 units passed; all 20 example tests passed after the documented correction |
| Independent `public_inputs_hash_vectors`, `rolling_hash_vectors`, `causal_evidence`, `ethereum_kzg_compatibility`, `issue35` targets | 18 passed |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed after correcting test-only clone lints; production still has one unboxed validator variant |
| `RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked -p eez-proof-signer --no-deps` | Passed |
| `forge test --root contracts` | 163 passed, none failed or skipped |
| `bash scripts/update-eezl2-genesis.sh --check` | Passed using this worktree's freshly built genesis-state-root helper |
| `python3 -m unittest discover -s scripts/ci/tests -v` | 417 selected: 407 passed, nine skipped, one local socket test blocked by sandbox; the same two-test network-filter module passed outside sandbox |
| Focused deployment preflight tests | 15 passed |
| Added empty-history lookup-evidence test | Passed; logical bootstrap is used without a public stored record |
| Opt-in real deploy Docker context test | Passed; all three history runtime assets are included |
| `python3 crates/eez-control-rpc/conformance/check_vectors.py --write-manifest` and plain verification | 29 frozen cases verified; regenerated inputs and provenance |
| Formatting/syntax | Cargo fmt, integration forge fmt, edited shell `bash -n`, `git diff --check` passed |

The nine Python skips were three opt-in runtime Docker tests, two Kurtosis
resource tests, one Docker payload test, one native-archive test and two Gitleaks
tests (tool unavailable). The deploy context test was then explicitly enabled
and passed. Skips are not coverage. Two explicitly selected signer generators
ran separately, one test each; they are not included as signer behavior tests.
The initial L1 evidence-helper invocation omitted its required `chain-spec`
feature and selected no tests; only the corrected invocation provides coverage.
Its first actual run passed 200 L1 unit tests and failed two of 20 helper tests.
One caught an omitted early `schedule.history()` check after removing the old
bootstrap storage constructor; it was restored without changing the rejection
assertion. The other used the old combined record/ancestry cardinality error.
Its inputs now test the independent 131,072-header bound; coverage also requires
81 linked headers for two stored pairs and rejects an unused trailing header.

### Source process evidence

Shipping-role binaries were built from this implementation with the production
feature selection in a local debug profile. The node/follower package opt-level
and debug-info settings were overridden to zero for this local build; no release
performance claim is made. The E2E executable was built with the same overrides.
Inherited prebuilt overrides were cleared; the harness then explicitly selected
these fresh, frozen executables, rather than an older binary on PATH:

| Executable | SHA-256 |
| --- | --- |
| eez-composer | `74f6041e2e1ed0de74385db3a81d08dd922cf87698414c0ba4c7b3adde0996a9` |
| eez-follower | `0edd0c3274fe4dbeefab9e74fc2ca855875fec3433dc32ef6a9432f25b7e4af4` |
| eez-proof-signer | `70a1add05ba5e0b86852d7d32b04452e2012bca265155d7198cce72d6d5f11fa` |
| node_e2e | `88a95e5e41f0e621f02bc8b338b7bd3d883414d8dd57ad5518caf9c1f5aacaa3` |

Each command executed exactly one test serially via
`target/debug/deps/node_e2e-5a79f77db30356ee <filter> --test-threads=1 --nocapture`.
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` pointed at the frozen
worktree files. `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1` denied signer outbound
networking. Temporary local Anvil chains used leased ports and public fixture
keys; no deployed network or operational wallet was involved.

| Filter | Result / semantic assertions |
| --- | --- |
| `native_system::history_upgrade::history_proxy_upgrade_preserves_records_and_offline_replay` | Passed; empty genesis reads, owner, failed unauthorized upgrade, successful authorized upgrade, retained hash, subsequent import, matching PREVRANDAO, canonical settlement, cold follower block/receipt/storage equality |
| `cross_chain::minimal_bidirectional_cross_chain_smoke` | Passed; native on-chain proof/public input and safe commitment, both cross-chain directions; refreshed history prefix evidence |
| `cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes` | Passed; caught lookup revert, later observations and independent follower replay |
| `cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` | Passed; changing static reads interleaved with an outbound write and follower replay |
| `cross_chain::history_observations::scheduled_history_observations_are_bound_before_offline_signing` | Passed; six cold requests, two correct outcomes sign, four earlier-state outcomes reject with no signature |

The new upgrade test's first run imported and proved blocks but waited for
settlement of a history-only block, which does not trigger the pure-L2 transaction
policy. The corrected harness submits an ordinary transfer to trigger settlement.
The same test then passed; this was a test setup correction, not a retry-only
pass or a change to settlement policy. First failure logs/datadirs were retained.

During unit adaptation, old mock states installed only the former non-proxy
history account. They now install the generated proxy, implementation and admin
with their real nonces. All 907 affected unit tests passed after that correction.
The old estimator fixture also differed from the rebuilt OfflineRollup bytecode;
its normalized fingerprint and fixture were refreshed from the compiled artifact.
The full integration contract suite then passed. No validation was weakened.

## Reproducible evidence and limits

The checked-in [streaming manifest](../../../crates/eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
records per-file source, schema, fixture and transcript digests. Native request,
response, genesis and public deployment fixtures are checked in beside it.
Prefix witnesses are in
`crates/eez-l1/examples/verify_blob_evidence/fixtures/`. The synthetic history
observation corpus remains byte-for-byte checked by the signer unit suite.
These captures contain public local test identities, not operator credentials.

Large logs, binaries and retained failed-process datadirs are **local only** under
the isolated worktree's ignored `artifacts/validation/`; they are not available
from a clean clone. The important inputs and regression assertions are committed.
The frozen process executables precede the final test-only lint annotation and
restoration of the early genesis history-schedule check. That check rejects an
invalid reference/cutoff combination before loading genesis; all captured valid
genesis documents pass the final EVM, signer and evidence-helper checks. They are
local process evidence, not a published final-head release artifact.

Generated production state root:
`0xb8287bd821dd8954d3a5fde186e1b7ba5258f0e217d302dd6e3b9714df8d6285`.
History proxy runtime Keccak:
`0x3725e8abaa83c431d55a24c39d4327791423019e9c8a2702cc68e6201105baf7`.
Initial history implementation runtime Keccak:
`0xb68626890d206c39d46bbb14288d62514b016af441c2b021b3517d04f2036291`.

No before/after runtime comparison against the old contract was claimed: this
is an accepted fresh-network semantic change, not a claim that the former
contract was unsound. No dependency upgrades, public broadcasts, service
restarts or network replacements occurred.

The full CI release-artifact/Kurtosis matrix, core protocol Anvil suite and any
public-network upgrade rehearsal were not run locally for this change. CI must
validate the final PR head and published artifact provenance before deployment.
Actual Nethermind/C# execution and arbitrary future implementation upgrades are
not established by these tests. Sparse ancestry beyond the existing header or
session quotas remains outside the supported recovery bound.

Final source review traced proxy genesis, ordinary upgrades, producer selection,
bounded native decoding, authenticated pairs/bridge, delegated execution,
PREVRANDAO, DA reconstruction, witness capture, cold signer and follower replay.
It also checked zero rejection in storage proofs, deployment Docker inclusion,
version-6 propagation and removal of obsolete initialized-history assumptions.
The separate record and ancestry limits retain their own negative tests. No
outstanding source-level defect was found in that review; this is an engineering
self-review, not a new independent security audit.
