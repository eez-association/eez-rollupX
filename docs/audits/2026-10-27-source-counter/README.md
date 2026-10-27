# Live Sync execution-group sourceHash

Status: implemented, with local validation recorded below. Final-head release CI
and artifact provenance remain merge/deployment gates.

- PR #103 baseline/head: `9c1ab488e1b421cdc59c7e82bd95f080f7332b3e`.
- PR base: `af0a000f12b0772892058066706465c84a00fd17` (#102).
- Core pin: `84a5b5207880f192a6a5896cbfe0bac5249db47f`; unchanged.
- Branch/worktree: `codex/fee-vault-upgradeable`,
  `/home/edu/Development/rollupX/artifacts/fee-vault-upgradeable-20261026`.
- Initial Git status: clean. Latest PR refs fetched without changing branches.
  The SSH fetch stalled; the owned fetch was stopped and HTTPS fetch succeeded.
- The operator checkout and unrelated worktrees remain untouched.

The owner accepted the [execution-group ordinal](../../proposals/native-system-transactions.md#source-identity-simplification)
on 2026-10-27 after review of the clarified
[proposal](https://hackmd.io/@Yu7RfKfWRP6MwaKqIFALjw/ByGlzfA3ze).
It replaces the original-row/pre-group-prefix source identity, with no legacy
mode. Prefix execution, whole-group root checks and deployment binding remain.
The single validation version becomes 3 to distinguish changed source semantics;
prove.v2, history 0x46/version 1, core and Cargo dependencies are unchanged.
The owner additionally removed the 0x45 envelope version: its six canonical RLP
fields are chainId, sourceHash, gasLimit, to, value and input. Old seven-field
encoding must fail exact decoding; no legacy version check or parser remains.

## Requirement checklist

- Derive zero-based complete-group ordinals from full semantic execution.
- Count outbound load/user once, including included failures and static roots;
  count applying inbound deliveries; exclude nonexecuting lookups and history.
- Select consumed suffixes only after assigning original ordinals.
- Independently reconstruct/check bytes and accepted-root coverage in the signer.
- Preserve exact-target reproof, root-based follower placement and replay.
- Remove source-prefix-only hashing/helpers, retain settlement checkpoint hashing.
- Update one validation identity, manager recognition, portable vectors and docs.
- Recapture no-network execution evidence and replay the v2 corpus.
- Test malformed/reset/duplicate index, changed parent/chain, incompatible root,
  partial same-block settlement and cold follower consistency.
- Review combined diff and confirm unchanged core/protobuf/lockfile.

Local logs are retained under `artifacts/source-counter-20261027/` in this
worktree; ignored artifacts are local-only and not portable release evidence.
[Validation metadata](validation.json) records source/runtime/log digests, every
process attempt and the selected cold captures. Executed results and limitations follow.

## Implementation and requirement coverage

| Requirement | Implementation | Validation |
| --- | --- | --- |
| Exactly six `0x45` RLP fields; no version in RPC or storage encoding | `eez-primitives::SystemTransaction`, canonical envelope decoder, all native constructors | Primitive independent wire/hash/storage/receipt/RPC vectors; missing/extra fields, noncanonical integers, widths, trailing bytes and old seven-field input reject; native Composer/follower process |
| L2 chain ID, intended L1 parent and zero-based complete-group ordinal | `eez-protocol::native_source`, `OfflineContext::live_source_context`, `ReconstructionActions::source_entries` | Independent ABI vectors, distinct L1/L2/rollup IDs, first/max index, parent/chain changes, identical calls, static-only outbound, failed and nonexecuting inbound cases |
| Count the full execution before selecting a consumed suffix | Composer candidate/full rebuild, shared reconstruction, signer exact DA reconstruction, deriver consumed-range selection | Three successful postings in one L1 block, unchanged original bytes/indices, stale proof rejection, unwitnessed accepted-root rejection, cold followers |
| Preserve authorization independently of identity | Existing exact block execution, checkpoint/root coverage, semantic/DA binding, observations and public-input gates remain | Full signer suite, source/gas/outcome mutations, history observation process, commitment/KZG/prefix oracles |
| Remove obsolete source-prefix machinery | Remove identity-only `SyncPrefixHashes::prepare_transactions`, `executed_prefix_hash`, `SourcePrefix` error and original-row vectors; retain settlement prefix cache and validation | Composer capacity/speculative rollback/cache tests, driver/deriver suites, nested rollback, restart and L1 reorg processes |
| One matching fresh-network profile | Validation constant 3 in Rust and RollupX-owned `OfflineRollup`; refreshed manager recognition bytes and independent profile vectors | 165 integration-contract tests; deployment recognition and fee tests; portable identity and on-chain accepted proof captures |
| Language-neutral v2 and offline execution | Same protobuf; refreshed three cold recordings, six observation requests and 29 transport transcripts | Real TCP conformance and source processes with signer outbound networking denied |

Core contracts, ABI, public-input algorithm, gitlink, `Cargo.toml`, `Cargo.lock`
and protobuf schemas are unchanged. Manager recognition changes only because
the RollupX-owned profile commitment now identifies validation version 3. No new
environment variable or legacy execution mode was added.

## Executed validation

Commands ran in the isolated worktree with Rust 1.95.0, Foundry 1.7.1, the pinned
Solidity settings, `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` and `--locked`.
Local Cargo compilation used one job. Node process wrappers were built with:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e -p eez-proof-signer --no-run \
  --config 'profile.dev.package.eez-node.opt-level=0' \
  --config 'profile.dev.package.eez-node.debug=0'
cargo +1.95.0 build --locked -p eez-proof-signer
```

The frozen local Composer/follower wrappers call the production role entry points.
These are debug/test artifacts, not CI shipping-artifact provenance. Their SHA256s:

| Executable | SHA256 |
| --- | --- |
| Composer | `16bb8a005bb8655a94739de8ab6cd80058798348ea8e8a2925ea21e4ce492b67` |
| Follower | `7602b0a57d1174e421ce721791d20a2419f1f1465df2f0016e9ecca8f5b39035` |
| Standalone signer | `c53372833e7e6299ef835026eb10703c152def9ccf5b90379d542283f8272e30` |

Process runs used the newly built `node_e2e` executable with `--exact`,
`--test-threads=1`, the explicitly selected frozen runtimes and
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. `EEZ_TEST_PREBUILT` was unset. Logs,
failure datadirs and captures were retained in this task's artifact directory.
An owned directory was bind-mounted over `/tmp` for the process tests because
the host `/tmp` quota was exhausted. Compiler commands ran outside that mount
with the owned absolute `TMPDIR`. No shared network was restarted or deployed.

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-primitives -p eez-protocol -p eez-blob-codec -p eez-payload-codec -p eez-evm -p eez-driver -p eez-deriver -p eez-composer --lib` | 843 passed: primitives 16, protocol 211, blob 21, payload 16, EVM 41, driver 74, deriver 46, Composer 418. One pre-existing manual timing profile ignored. |
| `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client` | Signer 329, RPC 7, client 34 passed. Three explicit fixture-generation/inventory utilities excluded from behavioral coverage. Empty doctest targets are not coverage. |
| Protocol `public_inputs_hash_vectors`, `rolling_hash_vectors`, `causal_evidence` integration targets | 15 passed. Independent Solidity expectations retained. |
| `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec --test ethereum_kzg_compatibility` | 1 passed with the workspace crypto feature union. |
| `cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --example verify_blob_evidence -- prefixes::tests` | 19 passed; one unrelated CLI test filtered. Fresh RPC receipts and L1 accepted-root pins remain independent prefix oracles. |
| `forge test --root contracts` | 165 passed, zero skipped. |
| Portable encodings | Foundry ABI/calldata and Keccak plus independent Python RLP verified 18 encodings, 9 relationships, 3 validation identities and the six-field synthetic native fixture. |
| Frozen v2 corpus | All 29 cases passed inside the signer suite; 24 expected proofs. Manifest/schema checks passed. |
| Rust/Forge formatting; workspace all-target Clippy `-D warnings`; signer all-target/all-feature Clippy; signer rustdoc `-D warnings` | Passed. |
| `bash scripts/update-eezl2-genesis.sh --check` with the freshly built `eez-node` `genesis_state_root` helper | Passed; existing genesis artifacts are current, state root `0x1143e2e4dfe4f0b15fce2d7077ca6a641182d7b13b06f59e626759d08335c1bf`. |

The following 11 distinct source process scenarios passed. They assert settlement,
state/root agreement and relevant rejection behavior, not just receipts/startup:

- `cross_chain::minimal_bidirectional_cross_chain_smoke`
- `cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`
- `cross_chain::outbound_changing_static_reads_and_write_settle_and_replay`
- `partial_consumption::valid_blob_batch_resumes_at_middle_group_in_same_l1_block`
- `cross_chain::history_observations::scheduled_history_observations_are_bound_before_offline_signing`
- `partial_consumption::restart_after_partial_keeps_accounting_and_rebuilds_intent`
- `native_system::two_composers_two_keyless_followers`
- `cross_chain::outbound_multiple_proxy_calls_in_one_transaction_preserve_ordered_results`
- `cross_chain::overlay_inbound_round_trip_reenters_suspended_l1`
- `cross_chain::overlay_failed_l1_call_discards_its_callback_state`
- `queue_reorg::native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order`

The history-observation process requires two correct signatures and four
earlier-state rejections at the observation gate. The same-block process also
requires an unwitnessed-root rejection before successful reproofs, an on-chain
`InvalidProof()` for a stale proof, original native-byte equality and independent
ABI source-hash equality after partial consumption. Followers derive from L1
and cold state; sourceHash never selects replay placement.

## Retained first failures and corrections

- Initial compilation found one remaining old `SourceEntry::hash(prefix)` test
  call and unused imports/variables from removed helpers; these were corrected.
- Initial protocol and Solidity tests retained the old profile-version word 2.
  Their independent expected bytes now use 3; validation was not relaxed.
- The first same-block process oracle used L1's test chain ID 1337 instead of
  the fixture's L2 chain ID 1. It reached successful on-chain settlement but
  correctly failed byte comparison. The oracle now uses the independently read
  L2 chain ID; the full process rerun passed. A context unit assertion also
  distinguishes L1 chain 1, L2 chain 900 and registry rollup 7.
- Eleven signer tests shared a frozen seven-field native literal. Strict decoding
  rejected it with `UnexpectedLength`; the literal is now independently checked
  six-field RLP. All 329 signer tests passed after correction. The explicit
  seven-field rejection regression remains.
- Clippy found a redundant test closure; replaced it with the method reference.
- A Cargo command inside the process-test mount could not execute rustc. It ran
  successfully outside that mount. The standalone blob-codec-only KZG invocation
  lacks the workspace crypto-recovery feature; the recorded joint protocol/blob
  invocation passed without dependency or manifest changes. The prefix example
  requires its declared `chain-spec` feature, supplied in the successful command.

These are corrected test/command failures, not retry-only passes. No failed test
was ignored, validation bypassed or protocol dependency changed. Initial logs
and failed process datadirs remain local-only under the stated artifact path.

## Final review and limits

The final review traced all producers and consumers: semantic reconstruction,
incremental/final Composer assembly, Stateless authorization, conditional
accepted-root coverage, DA equality and follower consumed-prefix placement. The
only removed prefix hashing was used solely in source identity. Settlement
checkpoints, history/ordinary-prefix observation boundaries and exact root/byte
checks remain. Fields, identity constants, Solidity commitment, manager bytecode
recognition, portable vectors, current specifications and deployment guidance agree.

No additional source-level correctness defect was identified. SourceHash remains
an occurrence identifier within the configured chain/L1 target, not independent
authorization, inclusion/finality evidence or a global cross-deployment key.
Fresh matching manager/genesis/artifacts/datadirs are required. Existing databases
and older native encodings have no migration or fallback.

Remaining validation boundaries:

- Full final-head CI, release feature selection and source-to-artifact provenance
  must pass before merge/deployment. This local record does not replace them.
- The entire workspace/native/Kurtosis/Gnosis matrix was not rerun locally.
  No operator network or deployed signer was changed.
- No Nethermind implementation was supplied or executed; the v2 contract and
  portable vectors define its integration boundary, not completed C# validation.
- The isolated blob-codec crypto-feature limitation described above is unchanged;
  the workspace build and independent KZG oracle are the validated configuration.

Earlier audits remain historical evidence. The new captures and
[manifest](../../../crates/eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
identify the current inputs; old prefix-bound source captures do not validate this
amendment.
