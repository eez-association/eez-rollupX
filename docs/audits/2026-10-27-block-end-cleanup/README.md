# Block-finalization protocol balance cleanup

| Field | Value |
| --- | --- |
| Date/status | 2026-10-27; implementation, final source review and focused local validation complete; full CI pending |
| Source baseline | `eez-association/eez-rollupX` main `167373172d216586b0e03b56f36c08484b1a51b3` |
| Isolation | `codex/block-end-balance-cleanup`; initially clean worktree `artifacts/block-end-balance-cleanup-20261027` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Contract | [Accepted execution rules](../../proposals/native-system-transactions.md#complete-group-balance-cleanup), [signer specification](../../../crates/eez-proof-signer/SPEC.md) |
| Environment | Rust 1.95.0, Foundry 1.7.1, Solidity 0.8.34; locked dependencies |

The requested timing is literal block end: ordinary donations remain visible to
later transactions until the block closes, unless a complete cross-chain group
clears them first. Per-ordinary-transaction cleanup was explicitly rejected.

## Implementation boundaries

- `eez-evm` removes block-start cleanup and finalizes residual balances after
  validating the complete-group cursor. Existing group cleanup, fees, receipts,
  nonce, code and storage semantics remain intact.
- Composer already snapshots raw prefix state before finalizing a separate copy.
  Its new regression checks that hashing a donation prefix does not burn the
  donation in continuing execution.
- The shared optional `eez-evm::stateless` adapter projects closing cleanup only
  onto disposable checkpoint trie inputs. It does not change the running EVM.
  It then rechecks the actual terminal bundle with the ordinary trie; missing
  finalization cannot be concealed by the checkpoint projection. This adds one
  terminal trie reconstruction, not another EVM replay or RPC dependency.
- FIFO observations use the exact in-block transaction boundary derived by the
  signer. The canonical prefix hash binds body and parent execution, but its
  final state root already includes cleanup. Therefore closed-state RPC reads
  and in-block observations can differ for donated protocol balances. RX-PROVER-01
  history/ordinary prefix selection and its earlier-state rejection are retained.
- Same-height resumed settlement replays from H-1 with the retained body and
  appended groups, replacing the closed prefix at H. It never executes a suffix
  on the closed H state. No changes to core contracts or the protocol pin.
- One validation contract, version 5, distinguishes the new semantics in the
  manager commitment and portable identity. Proving remains v2, offline, with
  unchanged transaction encodings. Fresh networks only; matching artifacts are
  required and rollback means a fresh network, not an existing-database downgrade.

## Executed evidence

The baseline regression
`cargo +1.95.0 test --locked -p eez-evm --lib completed_blocks_clear_protocol_residue_from_standalone_transactions`
failed on main with EEZL2 balance 29 instead of zero (one selected failure).
The candidate additionally checks a later ordinary call still sees that donation.

| Requirement | Regression and observed result |
| --- | --- |
| Ordinary donations survive later ordinary transactions, then disappear at block close | Executor test above: SYSTEM 17, EEZL2 29, subsequent ordinary call still stores 29; completed balances zero; storage and user debits retained; three successful receipts, unchanged gas and no cleanup logs |
| Empty blocks clean inherited inventory; missing/zero accounts stay untouched | Executor finalization tests, including nonce/code/storage preservation and unchanged L1 simulation |
| Group atomicity remains intact | Existing load/user, inbound, revert/halt, callback, discard and incomplete-group executor regressions retained |
| Prefix hashing does not clean the continuing draft | Composer `prefix_hashing_does_not_burn_donations_in_the_continuing_draft`: independent prefix builds match, later transaction sees the donation, no repeated transaction execution |
| Projected checkpoint roots cannot conceal invalid execution | Three adapter tests: independent executor cleanup equivalence, untouched witness inventory, and rejection of an uncleaned terminal bundle even when projection matches the claimed root |
| Signer authenticates the correct in-block observation | Strengthened four scheduled-history static/lookup regressions, with and without native suffix: ordinary SYSTEM donation is visible during observation and zero in closed state; earlier/closed-state answers cannot sign |
| Every effect commitment matches independent closed-prefix construction | `ordinary_prefix_then_two_native_actions_preserve_every_execution_checkpoint`, including malformed-root rejection; 19 prefix-oracle tests against fresh process captures |
| Fresh producer/follower execution agrees | Four-node process: 50 identical blocks, 12 successful native transactions (one inbound, one load), identical receipts, fee state and L1 history; ordinary SYSTEM and forced EEZL2 donations zero in their own completed blocks on all nodes |
| Partial settlement and restart preserve canonical reconstruction | Two existing partial-consumption process scenarios passed, including a same-height middle-group resume and restart after partial settlement |

Executed commands and results:

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-evm --features stateless --lib` | 45 passed |
| Rebuilt `eez-composer` unit target, `--test-threads=2` | 419 passed; one existing manual timing profile ignored |
| Rebuilt `eez-proof-signer` unit target, `--test-threads=2` | 329 passed; three explicit fixture/inventory utilities ignored, not behavioral coverage |
| `cargo +1.95.0 test --locked -p eez-protocol -p eez-prover-client` | Protocol 211 unit + 15 integration tests; prover client 34 passed; includes independent public-input and rolling-hash vectors |
| `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec -p eez-control-rpc -p eez-prover-client -p eez-driver -p eez-deriver -- --test-threads=2` | Blob codec 21 + one independent KZG test; payload codec 16 + two integration tests; RPC 7; driver 65; deriver 46 passed. Initially stopped at the profile-vector assertion; corrected protocol/client results are in the preceding row |
| `cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --example verify_blob_evidence -- prefixes::tests` | 19 passed, one unrelated example test filtered |
| Explicit history-observation and streaming fixture generators, followed by full signer replay | Both utilities passed; all 29 frozen streaming cases replayed by the signer suite |
| `python3 crates/eez-control-rpc/conformance/check_vectors.py --write-manifest` and verification | 29 vectors, input digests and schemas verified; this structural check is separate from executable replay |
| `forge fmt --check --root contracts`; `forge test --root contracts` | Passed; 163 contract tests, zero failed/skipped; existing remapping/lint warnings |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed on final source with warnings denied |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | Passed; edited relative documentation paths also resolve |
| `RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked -p eez-proof-signer --no-deps` | Passed |
| `bash scripts/update-eezl2-genesis.sh --check` | Passed using a freshly built `eez-node` genesis helper; generated EEZL2/history artifacts unchanged, state root `0x537c279ba86cd761f16945a2cde24a55c6c2cc549918b1bd9eaae11e5db93dc3` |
| `cargo deny --offline check advisories bans sources`; `cargo audit --no-fetch` | Passed with existing allowed warnings, using the local advisory database; no new dependency versions or advisory exceptions |

The direct unit executions used the test binaries produced by
`cargo +1.95.0 test --locked --no-run -p eez-node-e2e -p eez-proof-signer -p eez-composer`.
The local build overrides `profile.dev.package.eez-node` and
`profile.dev.package.eez-follower` to `opt-level=0`, `debug=0`; other packages retain
the workspace profile. These overrides do not change shipping configuration.

Source-built process checks each selected exactly one test, with serial execution:

| `node_e2e` test | Result |
| --- | --- |
| `cross_chain::history_observations::scheduled_history_observations_are_bound_before_offline_signing` | Passed again after final fixture regeneration; two correct outcomes sign and four earlier-state outcomes do not |
| `cross_chain::minimal_bidirectional_cross_chain_smoke` | Passed; fresh accepted on-chain capture, both directions and follower replay |
| `cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes` | Passed; fresh caught-revert capture and replay |
| `cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` | Passed; fresh changing-static capture and replay |
| `native_system::two_composers_two_keyless_followers` | Passed, including a late follower and ordinary donations |
| `partial_consumption::valid_blob_batch_resumes_at_middle_group_in_same_l1_block` | Passed |
| `partial_consumption::restart_after_partial_keeps_accounting_and_rebuilds_intent` | Passed |

Inherited prebuilt overrides were cleared before source builds. Process runs then
explicitly selected frozen copies of those source-built executables, checked by
SHA-256, with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. This tests offline signing,
not a signer backed by either chain's RPC. The protocol fixture accounts are
public test material; no operator keys or public network were used.

| Executable | SHA-256 |
| --- | --- |
| Composer | `99ea1d1c4dfbae3b2b0fb2654101e6d7585a4439af2c0a1ddbb3306ab82d204d` |
| Follower | `6742201780017531efb9858ea6482536b75f59199e4426884455fec19d125554` |
| Signer | `d2042447a7c45f37055b6055d77056e674ade28ffe14239d54e06d5edd43ee1f` |

These are local validation executables, not published release artifacts. The
[streaming manifest](../../../crates/eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
records source and capture digests, including uncommitted changes during
generation. The three committed native captures and six scheduled-history cases
retain replayable evidence; local logs/datadirs in ignored
`artifacts/validation/` are unavailable from another checkout.

## Initial failures and corrections

- Two new trie tests omitted the empty-trie node (`0x80`) from their synthetic
  witness. Adding the required node fixed the fixtures; the final 45-test executor
  run includes a further untouched-inventory case. The first combined trie log
  was overwritten; the failure is retained here as a limitation of local evidence.
- The first complete Composer run rejected the old `OfflineRollup` bytecode
  fingerprint after the validation-version bump. The sole accepted fingerprint
  and test bytecode were regenerated from this revision's Foundry artifact;
  no old-bytecode fallback was added. Full Composer tests then passed.
- The RPC dependency-review guard detected four new dependency edges in
  `eez-evm`. All point to already-pinned packages; versions, sources and checksums
  are unchanged. The semantics change is explicitly bound by version 5, and the
  reviewed lockfile digest was updated. No dependency update was used to fix a build.
- An independent ABI-word test still expected version 4. It now independently
  expects 5, matching the Solidity test and portable Foundry-derived vectors.
- The first dependency check could not take the advisory database lock under the
  sandbox. It was rerun with access to that lock and passed. This was an execution
  permission failure, not a suppressed dependency finding.

These are corrected code/fixture or environment failures, not unchanged tests
that passed only on retry. First baseline, Composer and protocol failure logs
are retained locally.

## Review and limits

The final source review traced finalization, unchanged group commits and discard
paths, Composer's saved raw snapshots, the pinned Stateless checkpoint extension,
the adapter's independent terminal-root guard, FIFO observation placement, and
same-height follower reconstruction from H-1. The core gitlink is unchanged and
clean. No legacy decoder, compatibility mode, extra RPC, transaction or cleanup
log was introduced. FeeVault is excluded from cleanup.

The adapter adds one terminal trie reconstruction and a retained witness copy;
this change has no benchmark or production-latency claim. The partial-settlement
process checks exercise existing scenarios, not an additional donation-specific
partial-settlement workload. The donation timing and checkpoint cases are covered
by focused executor, Composer and signer regressions and the four-node process.

CI still owns the complete workspace/release-artifact/Kurtosis matrix. Core-only
Foundry/Anvil suites, a new load campaign, external Nethermind execution and public
deployment were not run for this fork-only change. No public network was changed
or restarted. Validation version 5 changes the manager profile and identity;
this is for fresh networks and is not an in-place binary upgrade or database
downgrade path for an existing network.
