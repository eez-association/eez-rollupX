# Direct L1 history availability queries

Scope: return zero for unavailable L1 history lookups directly in the predeploy,
remove the reference collector, retain strict integrity failures and refresh
fresh-network commitments. No existing network is changed by this PR.

- Base: `66e79cedf4cfac3222b5de40f0fee8d1f2936000` (`main`).
- Core gitlink: `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean.
- Branch: `codex/l1-history-zero-lookup`.
- Worktree: `/home/edu/Development/rollupX/artifacts/l1-history-zero-lookup-20261027`.
- Initial status: clean; operator checkout and unrelated worktrees preserved.
- Foundry 1.7.1, Solidity 0.8.34 with pinned optimizer/metadata settings,
  Rust 1.95.0 and locked dependencies.

## Contract and coverage

The [history contract](../../../contracts/L1_HISTORY_SPEC.md) owns these accepted
rules. `getL1BlockHash` validates the endpoint through the same `current()` logic,
returns zero outside the retained range, and rejects missing covered records.
`L1StorageProof` continues to reject zero before accepting a supplied header.
Sequential applications keep their own processing cursor and need no separate
collector or duplicate hash mapping.

Validation-contract version 4 binds the changed semantics in the Rust identity
and Solidity manager. Generated runtime bytes, code identity, genesis documents,
manager-recognition bytes and portable/cold proving fixtures must agree. No
legacy getter, alternate runtime or compatibility switch is retained. V2 wire
messages, source identity, import schedule and core contracts are unchanged.

| Requirement | Implementation / regression |
| --- | --- |
| Unavailable reads return zero and later return the imported hash | Direct getter regression, pre-bootstrap reads, repeated reads through omitted imports, uint64 boundary and fuzz coverage |
| Never normalize corruption into availability | Missing covered hash, missing endpoint hash, inverted/overflowed range reject |
| Preserve atomic import rollback | Invalid/conflicting imports leave both endpoint and tentative mapping writes unchanged |
| Zero cannot authorize an L1 storage proof | `L1StorageProofTest` requires `InvalidHeader` for an unavailable block |
| Producer/follower consistency | Existing two-Composer/two-follower process now queries future hashes at one fixed L2 height on all four nodes |
| Offline proving and identity consistency | Regenerated runtime/profile/genesis, source captures, observation fixtures and v2 transcripts |

## Reproduction and checks

The new `testFutureHashReturnsZeroAndBecomesAvailableAfterImport` was run against
unchanged baseline contract code before implementing the fix. It failed with
`L1BlockNotImported(100)`. The same test passes after the fix.

Executed checks:

- Full integration contract suite: `forge test --root contracts`, 163 passed,
  zero skipped; includes 256 fuzz runs for future history numbers.
- Generated genesis check: `bash scripts/update-eezl2-genesis.sh --check`, passed.
- Rust and Foundry formatting and `git diff --check`, passed.
- Eleven portable ABI/calldata vectors independently recomputed with Foundry,
  including both version-4 profile commitments, passed.
- `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings`, passed.
- `cargo +1.95.0 test --locked -p eez-protocol -p eez-control-rpc --lib`,
  211 protocol and 7 RPC tests passed.
- The freshly rebuilt signer unit target: 329 passed, zero failed, three ignored
  fixture/inventory utilities. The observation and streaming generators were
  invoked explicitly; they are not counted as behavioral coverage.
- `python3 crates/eez-control-rpc/conformance/check_vectors.py`, 29 frozen vectors,
  input digests and schemas verified. The signer suite replays those transcripts.
- `cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --example
  verify_blob_evidence -- prefixes::tests`, 19 passed, one unrelated test filtered.

Source-built process checks, each with one explicitly selected test, all passed:

| `node_e2e` test | Evidence |
| --- | --- |
| `cross_chain::minimal_bidirectional_cross_chain_smoke` | Fresh settlement capture, both directions and follower replay |
| `cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes` | Fresh caught-revert capture and subsequent follower replay |
| `cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` | Fresh changing-static capture and follower replay |
| `cross_chain::history_observations::scheduled_history_observations_are_bound_before_offline_signing` | Correct observations sign; earlier-state observations do not |
| `native_system::two_composers_two_keyless_followers` | Identical native execution, fee state, retained imported hashes and zero future hashes on four nodes |

The process runs set `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`: the signer cannot use
L1/L2 RPC. The three source captures refresh the existing portable fixtures;
the first also supplies the two prefix-history replay oracles. After regeneration,
the complete signer suite and observation process check passed against these
exact files. All fixtures use public local-test accounts.

The source build used `cargo +1.95.0 test --locked -p eez-node-e2e
-p eez-proof-signer --no-run` and a standalone signer build. Subsequent process
runs used frozen copies of those binaries, with these SHA-256 digests:

| Executable | SHA-256 |
| --- | --- |
| Composer | `a1be7c50702e8edea442dba7eea4ed2a75efff7b69913c588c61bca29c32860f` |
| Follower | `778dac5271d6ba997c4b95700093ed088c47276ec19cf6ecdb3c41ad5c5acbde` |
| Signer | `749777c97e120a26267984ee4ba7de028ad1b36ca1e9305dd977a0ce8548b6a2` |

These are local validation artifacts, not published release digests. The streaming
manifest records source and capture digests, including uncommitted changes at
generation. Detailed logs are retained in this worktree's ignored
`artifacts/validation`; they are local-only evidence. CI owns the complete release,
workspace and Kurtosis matrix; those results are not implied by the focused local
checks. No live-network or load campaign was run for this getter change.

Re-reading [issue #107](https://github.com/eez-association/eez-rollupX/issues/107)
found its updated explicit BLOCKHASH-like availability requirement. The final
implementation returns zero on both sides of the retained range; internal
corruption remains a distinct error. This API does not impose Ethereum
BLOCKHASH's 256-block window: all imported records remain available.

An initial local Cargo build was stopped after the issue review broadened the
getter behavior to include pre-bootstrap blocks. The final source build uses
`opt-level=0` and `debug=0` for the two heavyweight role crates; this affects local
validation artifacts only. CI retains the shipping build configuration.

The first focused run exposed a test-only timestamp issue: a loop incremented
`block.timestamp` across Foundry warps. Explicit independent scheduled timestamps
replace that optimizer-sensitive expression; the production schedule is unchanged.

The first protocol-unit run found the independent ABI oracle still encoded
validation version 3. Its explicit expected word was corrected to 4; all 211
protocol tests then passed. A signer binary compiled before recapturing genesis
reported four stale-fixture failures; rebuilding after the captures resolved
them. A sandboxed suite run could not bind local RPC sockets; the same rebuilt
suite passed with loopback access. These were setup/fixture corrections, not
retry-only passes of unexplained runtime failures; the initial logs are retained.

## Final review

Re-read issue #107 and reviewed the getter through its storage-proof consumer,
genesis/runtime/profile commitments, offline signer and follower replay. The
diff removes the collector rather than keeping competing availability APIs.
No core-contract, dependency, protobuf, import scheduling or network deployment
change is included. Integrity checks and raw-storage rollback assertions remain.
Runtime hash is `0xbe389fdaa7ed5833a65c0ceffacb8e04e727127f8faf6e240611272d566b6676`;
the regenerated standard E2E fixture state root is
`0x537c279ba86cd761f16945a2cde24a55c6c2cc549918b1bd9eaae11e5db93dc3`.

## Compatibility and operations

Use fresh genesis and matching manager/node/follower/signer artifacts. Runtime,
genesis and profile commitments change. Existing deployed networks retain their
old getter behavior; this PR includes no activation/migration or in-place upgrade.
There was no live broadcast, restart, funding or deployment during this work.
