# feature/events protocol port

Implemented and locally validated, including the final shipping-feature build. Not deployed.
The existing Chiado network and its databases were not changed by this port.
No database copies were taken.

| Identity | Revision |
| --- | --- |
| Fork baseline | `1832f970884ca9460646a4cde87cca70eaffb3e1`, with pre-existing local settlement/timing work preserved |
| Protocol baseline | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Protocol candidate | `820e50f749f63165be6a5797d4c5fe52cbbbbe34` (`feature/events`) |
| Tools | Rust 1.94.0, Foundry 1.7.1; locked dependencies |

The branch advanced from the initially fetched `cbd94430b8858da92fdcd19703a33474bf977a6f`
to the candidate above while the port was in progress. The final update adds four
read/write/read E2E scenarios without further contract changes. This audit applies
to the working-tree port, not the unmodified fork baseline. Exact test/runtime
executable hashes, Cargo.lock hash, results and local evidence digests are in
[validation.json](validation.json).

## Implemented contract

[Requirements and compatibility](../../proposals/l2-static-execution-position.md)
map the change through Composer, signer, DA and follower reconstruction. The L2
static entry retains `expectedEntryIndex`; the signer independently verifies its
position against ordered L1 execution. Identical reads can change their result
after a successful mutable cross-chain call. Different results at the same hash
and position remain rejected. Existing restrictions on terminal static calls,
flat successful mutable roots, revert spans and explicit call gas remain in force.
Exact ABI comparisons and independent public-input recomputation are retained.

The port also updates event consumers, proxy construction, registration,
validation identity and generated genesis. A process test caught an overlooked
settlement-event payload length: the scanner now decodes the new 64-byte event
strictly, and malformed matching logs produce observation errors rather than
being interpreted as absent settlement.

Upstream prose still partly describes hash-only selection. The pinned interface,
implementation and cursor tests define the position-aware behavior adopted here;
this discrepancy is recorded in the proposal. The fork retains canonical ordered
static rows, even though upstream permits more general static table layouts.

## Baseline and end-to-end evidence

At the original pin, `TopLevelStaticEpochTest` reproduces the bug: an identical
read returns stale 10 after a write when the predicted result is 9. The baseline
check was repeated with pinned Foundry 1.7.1 using extracted source files under
`/tmp/eez-events-baseline`. At the new pin, the corresponding test returns 9.
A baseline test passing its stale-value assertion establishes reproduction, not
correct support for changing reads.

The source-built process regression
`outbound_changing_static_reads_and_write_settle_and_replay` checks two reads of
5, a successful write to 79, then two reads of 79. It asserts canonical settlement,
the L2 sum 168, one completed write, and matching independently reconstructed
follower block hash and state. It passed in 47.86 seconds, then passed again in
47.83 seconds with the corrected test-only fixture exporter.

The portable [changing-static signer fixture](../../../crates/eez-proof-signer/tests/fixtures/derivable-native-changing-static/README.md)
retains the original warm request, original attestation and exact 42-block cold
expansion (436,388 bytes). Signer tests fully re-execute the cold request, verify
the original signature and deterministic retry, and reject eight mutations before
signing. Mutations include missing/extra/reordered/modified sidecars, missing or
modified groups, and wrong/maximum execution positions. Cold validation imports
no trusted cache facts. The fixture uses bounded test-specific 64-block limits;
production admission limits are unchanged. Old immutable mixed-request bytes are
preserved and explicitly rejected at the old-selector boundary.

## Completed validation

| Check | Result |
| --- | --- |
| Core Foundry tests | 463 passed |
| Protocol Anvil scenarios | All 32 passed, including all four new read/write/read cases |
| Integration contracts | 40 passed; all 42 gas-matrix rungs retained across seven tests |
| Affected Rust packages | 1,021 passed, zero failed, three explicitly ignored utilities/profiles |
| Signer subset of that Rust run | 329 passed; cold fixture replay, cursor authorization and tamper rejection included |
| Native `cross_chain` | All 24 passed, including changed-result reads and independent replay |
| Native `chained_interstate` | All five passed |
| Native `partial_consumption` | All five passed, including omissions, follower reconstruction and restart |
| Corrected capture exporter | Repeated changing-read process passed; exact-ancestry/missing-file/bounds unit checks passed |
| Monitoring indexer | 21 ordinary tests plus one explicit PostgreSQL recovery test passed; Clippy passed |
| Python tooling | 242 passed, four optional infrastructure checks skipped |
| Deployment/configuration | Two preflight and ten Chiado configuration tests passed; settlement configuration checker passed |
| Genesis | Regeneration and check passed with this revision's helper |
| Shipping-feature build | All runtime binaries and four shipping examples compiled; optimized debug profile, matching Dockerfile.runtime features |
| Rust quality checks | Workspace formatting and all-target Clippy, signer all-target check, all-feature Clippy and strict rustdoc passed |
| Edited shell/Solidity/docs | Shell syntax, changed Solidity formatting and diff whitespace checks passed |

Core exclusions were `script/*`, `test_Doc_OriginalValue_SeedInSetup` and
`test_PostCost_Incremental`; they are not coverage. Rust ignored checks are the
Composer manual backlog profile, signer fixture inventory utility and cold-span
feasibility measurement. Python skips require optional runtime Docker, Kurtosis,
native archive or Docker payload infrastructure. Empty doctest selections are
not counted as coverage.

Principal commands, from the fork checkout with pinned tools:

```bash
(cd eez-core-protocol && forge test --no-match-path 'script/*' \
  --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental')
BASE_PORT=25545 bash scripts/ci/run-protocol-anvil.sh
forge test --root contracts
bash scripts/update-eezl2-genesis.sh --check
cargo +1.94.0 test --locked -p eez-proof-signer -p eez-control-rpc \
  -p eez-prover-client -p eez-protocol -p eez-blob-codec \
  -p eez-payload-codec -p eez-composer -p eez-l1 -p eez-deriver -p eez-testkit
cargo +1.94.0 fmt --all --check
cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 check --locked -p eez-proof-signer --all-targets
cargo +1.94.0 clippy --locked -p eez-proof-signer --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo +1.94.0 doc --locked -p eez-proof-signer --no-deps
python3 -m unittest discover -s scripts/ci/tests -v
```

Native tests were first built with `cargo +1.94.0 test --locked -p eez-node-e2e
--test cross_chain --test chained_interstate --test partial_consumption --no-run
--message-format=json`, followed by the standalone signer build. The exact
emitted test executables were run serially, with `EEZ_TEST_PREBUILT`,
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` unset. This avoided
compilation aging a freshly created genesis. No executable from a different
revision was substituted. Commands and executable identities are recorded in
`validation.json`. All 34 unique process tests passed; none was ignored.

The corrected exporter was compiled and independently exercised after the other
native suites. Subsequent Composer changes only corrected handwritten test-event
fixtures; production behavior was unchanged. Final affected Rust tests include
those corrections.

New generated state root:
`0x8b19c93d207113a489efb0eb449b14aee1d30ad94cdbe125cf1e0127bec12bcf`.
Canonical proxy init-code hash:
`0xd8e30937dfd0ac69d2334cc4ac78ef3942cec288a46bfa2521ee04dcae0f84f7`.
Vector proxy address: `0xb142ffeb406be0ae2db544f05b55b9a6bda5e179`.
The no-argument proxy constructor binds its manager to the deployer; genesis
checks that immutable instead of patching constructor arguments.

## Preserved first failures and corrections

These are distinct corrected failures, not hidden retry-only passes:

- The host Foundry had changed to 1.8.3. The initial core run had 19 failures
  (`core-tests-foundry183.log`); the initial Anvil run was stopped. Both complete
  suites subsequently passed with required Foundry 1.7.1. Those earlier results
  do not establish a protocol regression under the required toolchain.
- The first integration gas-matrix test exceeded aggregate per-test transaction
  gas. Splitting it by return size preserved every original rung and assertion.
- Whole-project Solidity formatting reports two pre-existing differences in
  `contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`.
  Their bytecode-affecting source was preserved; changed files pass formatting.
- The first Rust run rejected two old-protocol immutable mixed captures at the
  old selector, as expected for the compatibility break. They remain unchanged
  and now have explicit rejection assertions; new-format positive evidence is
  provided separately.
- The first process test signed and included its batch but failed settlement
  recognition because the scanner still required a 32-byte event. Evidence is
  retained in `process-event-decoder-failure/` and its sibling log. Fresh test
  datadirs were moved, not copied. Typed 64-byte decoding and malformed-event
  regressions fix this actual port defect.
- The next process attempt exposed an incorrect new test expectation: setup
  initializes value 5, so the expected read sum is 168, not 158. Evidence remains
  in `process-state-expectation-failure.log` and associated artifacts.
- The first successful capture contained cached-range references but no saved
  preparation witnesses. Fixture replay hit the default 16-block test quota
  and cold-assembler rejection. `incomplete-changing-fixture/` and
  `signer-tests-incomplete-capture.log` retain this incomplete evidence. The
  test-only exporter now records exact successful preparation witnesses and
  expands ranges with checked ancestry and bounds; a fresh complete capture
  passed actual cold signer validation.
- A capture-helper build was stopped before executing its test to release Cargo's
  lock while native tests ran. `capture-unit.log` is not coverage; the helper
  later passed in the complete 16-test Testkit suite.
- The initial final Rust run found five Composer tests still emitting the old
  32-byte mock event. `rust-stale-event-fixture.log` preserves that failure. The
  mock now emits both words, and a truncated-event rejection was added. The
  final 1,021-test run passed.
- The first disposable PostgreSQL recovery invocation rejected an incorrect
  test database name before touching data. Repeating with required
  `eez_indexer_test` passed. The owned tmpfs container was removed; no live
  database was used or copied.

## Evidence availability and remaining limits

Raw logs and fresh test datadirs are local-only under ignored
`artifacts/events-protocol/`; upstream scenario evidence also resides in
`eez-core-protocol/tmp/e2e-ci-PEVnu4lv/`. They have no retention guarantee and are
not available in another checkout. The small digest/result record and portable
proving fixture above are the durable evidence included with this change.

The shipping-feature build passed in 10m 12s; its exact command and seven artifact
hashes are in `validation.json`. These are local optimized-debug artifacts, not
published release images.

Full release-artifact promotion, the live Kurtosis/PBS matrix and old-history
migration/replay were not run. Additional queue/reorg and settlement-policy
process targets outside the selected suites were not rerun for this ABI port;
the changed settlement-event path is exercised by cross-chain, chained and
partial-consumption tests. Unit checks do not replace unrun topology or live
broadcast validation. No performance claim is made for this candidate.

This revision changes L2 selectors/static sidecars, contract runtime/proxy
initcode, events and genesis. It cannot safely replace the existing network's
binaries alone. Matching contracts and genesis, or a separately designed and
validated migration, are required. Mixed-version operation and old-history
replay are unsupported; downgrade after new-format blocks is unsupported.
See the [deployment compatibility guidance](../../runbooks/chiado-deployment.md).

The separate [per-attempt Chiado investigation](../2026-09-23-posting-failures-repeat/README.md)
reviews all 24 non-included events from the old deployed protocol. It establishes
actual skipped slots and several relay/builder timing causes, while preserving
two unresolved composition questions requiring unpublished builder traces.
Those performance results do not validate this new protocol port.

## Subsequent deployment

After this validation, the operator authorized a fresh network with new L1
contracts. See the [September 23 network replacement](../2026-09-23-events-network/README.md)
for backup, deployed identities and live semantic checks. This does not change
the historical validation scope above or make the old contracts compatible.
