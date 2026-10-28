# Original Core Rollup integration

This records PR #116's replacement of the custom manager with the unchanged
pinned Core `Rollup`, following the [accepted design](../../proposals/rollupx-manager-simplification.md).
It supersedes the earlier compatible-manager refactor; earlier deployment or
validation records do not validate this change.

| Identity | Value |
| --- | --- |
| Worktree | Dedicated `artifacts/simplify-rollupx-contract-20261028` worktree, branch `codex/simplify-rollupx-contract` |
| Base | Main `7437487449ca07ce93c01bd6c99ec6bf5b9f6170`, merged into PR branch as `bce32e399610be43f29630aa40e3a223fb17197a` before implementation |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean |
| Reviewed patch | SHA-256 `8b24273bf0c321c1c92e45d24680bca375d288d474eb48ea5e1a197201316a77` of `git diff --cached --binary` from integration commit bce32e399610be43f29630aa40e3a223fb17197a, excluding this audit directory |
| Toolchain | Rust 1.95.0, locked dependencies, LLVM 22; Foundry 1.7.1, Solidity 0.8.34, via IR, optimizer runs 1 |
| Deployment | None; no operator data, wallets or running networks changed |

## Implementation and review

- Deploy the original Core proxy/initializer; delete the custom manager, profile
  helper, compatibility fingerprints and tests of the removed policy.
- Bind the network profile, registry, proof system and attester through Core's
  registered vkey. Deployment and Composer derive it; signer startup checks it.
  The signer remains offline. Validation identity advances to version 7; the
  v2 protobuf and L2 execution/history encodings remain unchanged.
- Use exactly Core's 64-byte timestamp/hash context. Preserve exact headers,
  L1 anchor termination, target binding, witnesses, execution, entry-root chains,
  observations, native DA and KZG checks.
- Permit proof reuse after root advancement and historical proofs across known
  compatible L1 forks. Core still checks unsigned root assertions and signed
  entry roots when consuming entries. Incompatible upgrades must revoke or
  replace the old on-chain key/proof system; restarting signers is insufficient.

A separate agent reviewed the combined Rust/key/context and partial-consumption
changes and found no correctness blocker. This was a source review, not an
independent external security audit. The review followed production callers,
including deployment, Composer, signer, follower, fee estimation and lookup
verification. Core contracts, protocol pin, Cargo dependency versions, genesis
layout and L2 execution code did not change relative to the base.

## Validation

Commands ran from the isolated worktree with `--locked`. Source-based process
checks clear `EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN`; they use local test processes, not deployed networks.

| Check | Result |
| --- | --- |
| `forge test --root contracts --threads 1` | 153 passed; Core anchor availability, actual-parent timestamp/hash binding, historical proof reuse, key rotation, incorrect unsigned root rejection, deployment and gas-reserve checks |
| `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client` | 331 signer, 7 RPC and 34 client tests passed; four signer fixture/inventory utilities intentionally ignored in the ordinary suite |
| `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec -p eez-composer -p eez-deriver -p eez-l1 --lib` | 210 protocol, 21 blob codec, 16 payload codec, 419 Composer, 46 deriver and 200 L1 tests passed; one manual Composer timing utility ignored |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_deploy_preflight.py' -v` | 16 passed, including profile/key propagation and failure before manager deployment |
| Portable conformance vectors | 29 vectors passed; frozen native commitment changes also checked with Core's actual Solidity verifier |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_protocol_lookup_evidence.py' -v` | 33 passed |
| `python3 -m unittest discover -s scripts/ci/tests -v` | 419 selected: 409 passed, 9 skipped and 1 outer-sandbox socket failure; the affected two-test network-isolation module then passed outside that sandbox |
| `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec -p eez-prover-client` | 299 passed, including independent public-input, rolling-hash and Ethereum KZG oracles; no failures or ignored tests |
| `cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --example verify_blob_evidence` | 20 passed, including authenticated inherited/imported history and malformed evidence rejection |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| Rust/Solidity formatting, shell syntax, new documentation links, diff check | Passed |
| Source-based `cross_chain::history_observations::` | 1 passed: two accepted and four rejected observation cases through the standalone signer with outbound network denied |
| Source-based `partial_consumption::valid_blob_batch_resumes_at_middle_group_in_same_l1_block`; genesis regeneration check | Still running when the source update was published; final results are recorded in PR #116 validation notes |

The checked-in native requests, blocks, witnesses, genesis files and blobs were
preserved. Only new key/context commitments, responses, identities and dependent
streaming expectations changed. The independently computed Solidity verification
checks are distinct from regeneration using the signer under test.

Initial local signer and network-isolation runs could not bind TCP under the
outer filesystem/network sandbox. Rerunning with local sockets permitted passed;
this did not change production network restrictions. The nine tooling skips
were opt-in Docker payload/runtime probes (four), Kurtosis resource probes (two),
a native archive relocation probe (one), and Gitleaks tests (two; tool absent). The standalone isolation
regression separately verified that outbound connections remain denied.

The initial lookup-tool run also caught one stale `protocol_config_hash`
expectation; it was updated to the registered-key contract and the complete
33-test suite passed. A separate deployment regression reproduced shell command
substitution masking an ABI-encoding failure; splitting encoding and hashing
into checked assignments now stops before manager deployment.

Initial adaptation failures identified test factories still using the old vkey,
a streaming vector that expected an unsigned root value to be signed, and an
assertion counting a prior successful signing call. These were corrected to the
accepted contract: malformed root-array shape and invalid signed entry roots are
still rejected, while changing a nonzero unsigned root assertion leaves the
digest unchanged. No execution or malformed-event rejection test was weakened.

## Remaining validation boundary

This is a point-in-time source-publication record. Follow [PR #116](https://github.com/eez-association/eez-rollupX/pull/116)
for subsequent local process/genesis results and CI status.

The full release-artifact, protocol-Anvil and Kurtosis matrix belongs to CI on
the final PR head. Local unit/contract/process checks do not establish deployment,
public-network behavior, throughput or a live upgrade rehearsal. This release
intentionally provides no old-manager compatibility or migration path.

## Independent native-fixture oracle

The retained [Solidity oracle](oracle/FixtureOracle.t.sol) calls the pinned
`EEZ._verifyProofSystemBatch` with the actual Core manager and ECDSA verifier.
It accepts all three regenerated native signatures and rejects changed keys or
target timestamps. It does not reimplement the Solidity public-input formula.
These are manual evidence checks, separate from the regular contract suite.

Reproduce from the repository root after initializing the pinned submodule:

```bash
cargo +1.95.0 test --locked -p eez-proof-signer --bin eez-proof-signer \
  regenerate_native_fixture_commitments -- --ignored --nocapture
mkdir -p artifacts/rollup-core-fixture-oracle-test/test
cp docs/audits/2026-10-28-core-rollup/oracle/foundry.toml \
  artifacts/rollup-core-fixture-oracle-test/foundry.toml
cp docs/audits/2026-10-28-core-rollup/oracle/FixtureOracle.t.sol \
  artifacts/rollup-core-fixture-oracle-test/test/FixtureOracle.t.sol
forge test --root artifacts/rollup-core-fixture-oracle-test --threads 1 -vv
```

The generation command deliberately rewrites frozen expectations; review the
resulting diff. It also writes the three local oracle inputs. Exact block,
witness, genesis and blob inputs stay pinned by the conformance manifest.
The oracle completed with 3 passing cases. Initial local harness setup errors
(import paths and fixture paths) were corrected before this result.

The successful history process run used standalone signer SHA-256
`6b76e74649cbc9daedaeb1816f100790e2dad76bc041315ee1e7de58edf06a23` and
test executable `1528f5129b8372a9f52f1ddcd8a7d61fa1f77c2cd36408646de34cb10e2fe919`.
Raw local logs under `/tmp/rollup-*` and process state under this worktree's
ignored `artifacts/native-e2e/` are local-only and may be removed. The checked-in
inputs and oracle allow reproduction; these paths are not published artifacts.
