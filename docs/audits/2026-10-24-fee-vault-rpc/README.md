# L2 fee treasury and single prover service

Status: implementation, local tests, fresh conformance replay and final review complete. Release/Kurtosis CI and external Nethermind execution remain outside local evidence.
This record extends the [native implementation audit](../2026-10-24-native-v1/README.md).
Earlier green results do not validate the changes recorded here.

## Source and scope

- Worktree: `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`.
- Branch: `codex/l1-history-offline-proving`.
- Base HEAD: `3e23955dc477c9e675364776f118b1cecda75774`, plus the existing native/history work.
- Core gitlink and checkout: `84a5b5207880f192a6a5896cbfe0bac5249db47f` (`feature/e2e_edge_cases`), unchanged and clean.
- Fresh networks only. No deployment, publication, branch reset or original-checkout edit.
- Accepted requirements: [treasury](../../proposals/native-system-transactions.md#one-authenticated-configuration-source),
  [RPC](../../../crates/eez-control-rpc/SPEC.md),
  [external integrations](../../../crates/eez-control-rpc/conformance/IMPLEMENTING.md).

## Changes and guarantees

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Fixed fee collector with a mutable final recipient | `contracts/src/L2/FeeVault.sol`, fixed predeploy `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078` | 11 contract cases cover initial state, owner authorization, rotation, payment destination, invalid recipients, empty vault, receiver failure/recovery, reentrancy, two-step ownership and ordinary transfers |
| Authenticated genesis and explicit deployment inputs | Constructor execution in `GenerateEEZL2Genesis.s.sol`; renderer validates every account/storage write; deployment requires treasury recipient, defaults absent owner to deployment OWNER | Reproducible generation check; actual custom owner/recipient render; preflight/Compose tests |
| Ordinary state transitions across all execution roles | Shared `eez-protocol::fee_vault` ABI; ordinary contract transactions, no privileged execution branch | New `native_system::treasury` process test; four-node test uses actual vault instead of a test-only collector override |
| One execution profile | Existing checked `Profile` and ProfileV3 commitment domain; no previous profile implementations or negotiation | Existing independent native/profile vectors and signer checks retained |
| One RPC package/service | `prove.proto` owns Capabilities, ProveStream and every message; single server registration in signer, proxy and stubs | 22 message definitions identical to the original two schemas after package normalization; client tests and generated C# types |
| Offline proof validation | Existing authenticated witness/execution/settlement pipeline unchanged | Five source process cases with network-denied signers passed; 320 signer tests include all 29 fresh conformance transcripts |

The withdrawal destination is L2-only. Permissionless callers cannot select it.
Recipient rotation changes the destination of subsequent withdrawals, including
accumulated funds. Fees from the withdrawal transaction itself are credited after
execution and can remain in the vault. The owner may transfer ownership with
acceptance; lost or renounced ownership prevents future administration.

## Executed checks

Commands run at the worktree root with Rust 1.95.0, `--locked`, Foundry 1.7.1,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` and worktree-local `TMPDIR`.
Logs under `artifacts/treasury-validation` are local-only ignored artifacts.
A private `bwrap` mount supplies that directory at `/tmp` for tests whose temporary
paths are fixed, because the shared host `/tmp` user quota is exhausted.

- `forge test --root contracts`: **142 passed**, no skips. `forge fmt --check --root contracts` passed.
- `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings`: passed after correcting single-service registrations in three test servers.
- `cargo +1.95.0 test --locked -p eez-control-rpc -p eez-prover-client -p eez-testkit -p eez-protocol`: **282 passed**, none ignored. Empty doc-test selections are not counted.
- `python3 -m unittest discover -s scripts/ci/tests -p 'test_deploy_preflight.py' -v`: **13 passed**, including 20 real renderer rejection subcases and mocked broadcast prevention/owner propagation.
- `python3 -m unittest discover -s scripts/ci/tests -p 'test_chiado_config.py' -v`: **19 passed**; Compose CLI configuration only, no daemon or deployment.
- `bash scripts/update-eezl2-genesis.sh --check`: passed, fixture state root `0xbf4ddb74585d342600860a07cebac1774c86c0fabfdde683fe696ac701d7e896`.
- Actual `--render` with owner `0x3333333333333333333333333333333333333333` and recipient `0x4444444444444444444444444444444444444444`: checked exact owner/recipient storage, code, nonce, zero funding, profile and fixed collector. The separate output root is `0x89554ec87ae9b610ab16483c3f9c038850cf29035c16a152d724d208f7c5d36d`.
- `protoc --proto_path=crates/eez-control-rpc/proto --csharp_out=artifacts/treasury-validation/csharp --descriptor_set_out=artifacts/treasury-validation/csharp/prove.descriptor.pb --include_imports prove.proto`: passed. This generates C# messages; it does not establish Nethermind execution or a compiled C# gRPC server.

Initial harness failures are retained: Foundry rejected a new test name beginning
with `testFail` before executing it; the name was corrected. The first Rust check
found remaining imported `v1` paths; the first Clippy run found test servers from
which both service registrations had accidentally been removed. The new preflight
owner test initially read the wrong profile filename. These were corrected without
weakening assertions, then the same checks were rerun successfully.

## Compatibility and remaining validation

The vault changes genesis code/storage, root and deployment identity. It is not an
upgrade to existing networks; no legacy execution, migration or activation path
is provided. ProfileV3's encoding/domain and fee accounting remain unchanged.
Treasury administration does not change that profile or require a hard fork.
Node, follower, signer and genesis/deploy artifacts must agree before network startup.

The RPC consolidation requires regenerated client bindings and the single service
route. ProveStream field definitions, limits, session controls, retry semantics,
independent public inputs and offline evidence requirements remain unchanged.
No extra signing authority or RPC trust source is added.

CI's full release-artifact/Kurtosis matrix and external Nethermind execution are
not local claims. No live deployment was performed. Process/corpus results and the fresh correctness review are recorded below.

## Process and tooling results

- `cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- native_system:: --test-threads=1 --nocapture`: **2 passed**, 177 unrelated cases filtered, no ignores. Prebuilt overrides cleared, source signer launched with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`.
  The new treasury case verifies a real reverted unauthorized setter, owner rotation,
  permissionless payment to the new recipient, its exact amount and residual base fee,
  L1 settlement and a fresh keyless follower's matching block, receipts and state.
  The four-node case checks both Composers/followers with the actual genesis vault,
  both cross-chain directions, canonical history/PREVRANDAO and independent fee sums.
- `python3 -m unittest discover -s scripts/ci/tests -v`: **390 passed, 8 skipped**
  out of 398. Skips: one opt-in real Docker payload test, two live Kurtosis-resource
  cases, one native-relocation case, two real Docker-runtime cases and two Gitleaks
  cases (tool absent). No skips are counted as coverage.

## Fresh offline captures and independent replay

The three source-capture cases passed independently, one selected test each,
with prebuilt overrides cleared, network-denied signers and serial execution:

```text
cross_chain::minimal_bidirectional_cross_chain_smoke
cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes
cross_chain::outbound_changing_static_reads_and_write_settle_and_replay
```

The existing inventory utility identified the unique complete request from L2
block 1 in each capture. [Selected content](captures.json) records the ranges and
settlement entries. The current checked-in fixtures replace the old direct-recipient
genesis captures; no extra compatibility fixtures or decoding paths were kept.
`regenerate_offline_vectors` rebuilt expectations from input-side oracles and
captured native signatures; it never asked the service under test for expected
outputs. `check_vectors.py --write-manifest` records all inputs, sources and schema.

`cargo +1.95.0 test --locked -p eez-proof-signer`: **320 passed, 2 ignored**.
The ignored cases are the explicitly invoked inventory/generation utilities,
not missing behavioral coverage. All **29 TCP/Stateless transcripts, with 24 proofs**,
passed using the new captured genesis and consolidated schema. The schema check
confirms all 22 protobuf message bodies retain identical fields, oneofs and reserved
tags after package normalization. No C# runtime implementation was executed.

[Process binary hashes](process-binaries.json) were retained after the last capture.
Earlier source runs passed, but per-run executable hashes were not saved before
subsequent Cargo builds, so their byte identity with that final run is not claimed.
The source commands, logs, captured requests/responses and current corpus remain
available as separate evidence. Local logs/datadirs are ignored and not portable;
the checked-in captures and manifests are portable.

## Fresh correctness review

Reviewed constructor-derived storage and code, fixed-address/profile binding,
owner/recipient authorization, failed payment rollback, actual fee-credit ordering,
reentrancy, RPC message definitions, effective limits, worker permit/cancellation
ownership, deployment precedence and every active schema consumer.

This found and corrected two evidence/cleanup gaps: the initial reentry test could
pass merely because the vault was empty; it now replenishes the vault before
reentering and verifies a later separate withdrawal still works. The Kurtosis
signer also inherited an unused deployment-recipient variable; that injection and
its obsolete comment were removed. The full contract suite still passes all 142
cases, and the updated preflight repeats pass all 13 tests, including dotenv/shell
precedence. Both service registrations previously had identical frame limits;
the consolidated service retains those same values and shutdown ownership.

The protocol gitlink, Cargo manifest and lockfile are unchanged. There is no new
privileged EVM branch for the vault, no RPC source for the prover, no bridge and
no competing profile implementation. The [Optimism fee-vault pattern](https://specs.optimism.io/protocol/predeploys.html#basefeevault)
is the design analogy: our smaller contract makes only local L2 payments, and is
not an implementation of Optimism's bridge/withdrawal-network behavior.

## Final checks and files

The production package/target selection from `Dockerfile.runtime` builds in the
local debug profile (not a published release image):

```bash
cargo +1.95.0 build --locked \
  -p eez-node -p eez-follower -p eez-proof-signer -p eez-protocol -p eez-l1 \
  --bin eez-composer --bin eez-follower --bin eez-proof-signer \
  --example genesis_state_root --example genesis_block_hash \
  --example e2e-causal-verify --example verify_blob_evidence
```

Final workspace Clippy with `--all-targets -- -D warnings`, Rust/Foundry formatting,
`bash -n`, `git diff --check`, all 169 conformance source digests and schema/fixture
manifest checks pass. Relative file links in the owning specifications resolve.
No pinned core or dependency files changed. The full workspace/E2E matrix, live
networks, deployment-image execution and external C# engine were not rerun or
claimed; this amendment uses the focused and process evidence above.

| Files | Purpose |
| --- | --- |
| `contracts/src/L2/FeeVault.sol`, `contracts/test/FeeVault.t.sol` | Ordinary local treasury and authorization/payment/rollback/reentry regressions |
| `scripts/solidity/GenerateEEZL2Genesis.s.sol`, `scripts/update-eezl2-genesis.sh`, three genesis files, `testing/kurtosis/l2-genesis-profile.json` | Constructor-derived fixed predeploy, exact storage/runtime checks and authenticated generated state |
| `scripts/deploy.sh`, env examples, `testing/kurtosis/main.star`, preflight/Compose tests | Explicit recipient, optional owner, deployment-only propagation and no signer override |
| `eez-protocol/src/fee_vault.rs`, `native_system/treasury.rs`, native/testkit changes | Shared ABI, real fee withdrawal and follower replay; remove test-only collector override |
| Control-RPC schema/build/include, signer service/main/worker, client and testkit proxy/stubs | One package and service; move capabilities alongside streaming, retain worker lifecycle and every message field |
| Three native captures and streaming conformance corpus | Current genesis and exact independent replay/signing expectations |
| Proposals, RPC/signer specifications, external guide, AGENTS and deployment runbooks | Describe the implemented profile, mutable treasury, single service and fresh-network compatibility boundary |

RPC naming is `prove.v2` in this implementation. Discovery moves to
`/prove.v2.Prover/Capabilities`; `/prove.v2.Prover/ProveStream` retains its path and
message fields. All generated types belong to `prove.v2`. Integrators regenerate
bindings and deploy matching clients/servers; older discovery clients receive an
unsupported-method error. There is no alternate endpoint or compatibility alias.
Naming this sole service `prove.v1` would also be technically valid, but is a
separate naming choice, not a required execution or consensus change.
