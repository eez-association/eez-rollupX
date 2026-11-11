# PR #123: redundant static rows in the Kurtosis scenarios

Status: the initial fixture accommodation is superseded by the 2026-11-10
node-side correction described below, validated locally. The original protocol
checkout and gitlink are unchanged. CI qualification of the resulting PR commit
is separate from these local results.

## Baseline and root cause

PR head `1121d4752bde629d3179512440b8beeb5508c936`, target fork
`ff7a5cdc81ce02ad30c6c267c4554de77cecb753`, protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`.
[CI run 42634765640](https://github.com/eez-association/eez-rollupX/actions/runs/42634765640)
passed all eight native E2E shards, units, lint and runtime production, but its
Kurtosis protocol suite failed `nestedStaticLocalWriteL1` and
`nestedStaticLocalWriteL2`. The CI checkout was merge commit
`1f4afd2c0d112be449fc5479230425c1f0180d00`.

Both source transactions succeeded. Both scenario self-verifiers confirmed the
reader's persisted `1 -> 2 -> 1` quote results and final state. The failure was
exact table comparison, not failed execution:

- The [L2 reader log](ci-l2-reader.log) reports
  `expectedOutgoingCalls.length: actual 3 expected 2`.
- The [L1 reader log](ci-l1-reader.log) rejects its expected entry because the
  content identity includes a two-row table, while the node posted three rows.

The replay correction in `19a86d8b` deliberately preserves one static row per
observation, including repeated byte-identical candidates. The
[signer contract](../../../crates/eez-proof-signer/SPEC.md) requires that behavior
for exact native semantic/historical binding. These two upstream E2Es still
constructed the compact candidate set `[1, 2]` for their three observations
`[1, 2, 1]`. Both forms support the core's reusable static lookup, but only the
latter matches this node's construction contract. The same obsolete helper
built the local test tables and their expectations, so local E2E success did
not expose the difference from production construction.

## Correction and limits

Both table shapes are valid core protocol inputs. This is an integration fixture
alignment with the fork, not a core protocol defect. A fork-owned adapter would
be an alternative if the upstream fixtures must retain their compact form.

The candidate changes the two scenario builders to retain all three observed rows, using the existing
`_rateAt(i)` helper for callback commitments and return bytes. Deployments,
triggers, state assertions, verifier comparisons and production sources are
unchanged. `ComputeExpected` and local execution still share one builder; there
is no network-only switch or relaxed comparison.

The existing minimal two-row reuse regressions in `EEZStaticLookup.t.sol` and
`EEZL2Coverage.t.sol` remain unchanged. New `E2EStaticObservationRows.t.sol`
regressions compare the three-observation layout through the real verifier
helpers and reject omitted rows, altered return values, altered callback targets,
and altered success (L1) or position key (L2). L1's content comparator intentionally
ignores root-dependent hashes; this test does not claim to validate those hashes.
The first draft incorrectly expected that comparator to reject a changed position
key; that test assertion was corrected, without changing the verifier.

The new positive regressions fail on the old fixtures and pass on the corrected
fixtures. This is a fixture/verifier reproduction, not a claim that the old node
executed an invalid transition. The candidate changes only protocol test fixtures, regression tests and their guide;
it changes no deployed protocol contract, ABI, Rust encoding, genesis,
production compiler setting or dependency version.

## Validation

- Focused fixture/verifier regressions: 4 passed after correction.
- Existing minimal nested-static reuse checks: 3 passed.
- Core suite: 782 passed, 3 skipped live-fork upgrade tests. Explicit exclusions:
  `test_Doc_OriginalValue_SeedInSetup` and `test_PostCost_Incremental`; script paths
  excluded with `--no-match-path 'script/*'`.
- Integration contracts: 153 passed.
- Genesis consistency: passed with unchanged state root
  `0xb8287bd821dd8954d3a5fde186e1b7ba5258f0e217d302dd6e3b9714df8d6285`.
- Isolated Kurtosis: both selected scenarios passed (138.68 seconds), no skips;
  exact table checks, causal coverage and authenticated semantic blobs all passed.
  This was a two-scenario run, not another full 64-scenario Kurtosis qualification.
- Full local Anvil scenarios: 64/64 passed; none failed, timed out or unrun.

The diagnostic network used the failing CI's immutable runtime image digests in
its own enclave `eez-pr123-static-20261109`. It isolated the fixture
change while keeping the node, signer and deployed contracts identical. The
initial `kurtosis enclave ls` automatically started a missing engine; existing
stopped enclaves and operator services were not restarted or removed. The owned
diagnostic enclave was removed after retaining its evidence.

## Publication

The local protocol commit is `a24126890dec791d82845f754f9bc55d8373fde2`
on `codex/pr123-static-observation-fixtures-20261109`. HTTPS publishing returned
403 for the configured account; SSH returned `Permission denied (publickey)`.
Nothing was pushed. At the user's request, the original protocol checkout and
gitlink `84a5b5207880f192a6a5896cbfe0bac5249db47f` are restored. The candidate
remains on its local branch and in the local-only patch
`artifacts/pr123-kurtosis-static/protocol-fixture-fix.patch` for handoff; no pin
update is included in this PR.

## Reproduction and retained evidence

[Original L1 reader failure](ci-l1-reader.log),
[original L2 reader failure](ci-l2-reader.log),
[corrected L1 reader run](fixed-nestedStaticLocalWriteL1.log),
[corrected L2 reader run](fixed-nestedStaticLocalWriteL2.log), and
[compact independent-verification summary](kurtosis-summary.json) are retained.
Full local-only evidence is in `artifacts/pr123-kurtosis-static/`; the compact
summary records hashes of the detailed evidence files. Those local artifacts
are unavailable in a fresh checkout.

The immutable image identities are in [runtime-images.json](runtime-images.json).
The original CI artifact was ID `13688579772`, ZIP SHA-256
`13090c847f3f20922938e1c0f2152d23dd6d738e8821ca6628b20861d3bd1d3b`.

The fresh diagnostic network needed a zero-value self-transfer from a public
fixture account to start interval settlement; the standard readiness check then
passed without a timeout change. Tests retained the same pinned scenarios and
all their field/state checks. With an owned prepared enclave, select the checks:

```bash
export KURTOSIS_ENCLAVE=eez-pr123-static-20261109
export EEZ_PROTOCOL_SCENARIOS='nestedStaticLocalWriteL1 nestedStaticLocalWriteL2'
export EEZ_PROTOCOL_MAX_PARALLEL=2
bash testing/kurtosis/scripts/run-protocol-e2e.sh
```

The diagnostic used source-built `eez-protocol/e2e-causal-verify` and
`eez-l1/verify_blob_evidence` (`chain-spec` enabled), Rust 1.95.0 with `--locked`,
LLVM 22 and Foundry 1.7.1. Runtime Rust sources were unchanged.


## 2026-11-10: reassessment after protocol-engineer feedback

The fixture patch above is a validated accommodation, not a required core fix.
The engineer correctly identified the final row as redundant for these scenarios.
Do not treat the earlier candidate patch as the preferred resolution without
addressing the fork's construction policy. No pin or production code was changed.

At the same parent/protocol revisions, temporary diagnostic probes established:

- The current Rust nested-table verifier accepts compact byte-identical static
  candidates for `[1,2,1]`, `[1,1,2,1]`, and `[1,2,2,1]` in both directions
  (six observed `Ok` results in one selected unit test).
- In the historical repeated-read regression, removing the identical second
  static row still passes `verify_outbound_source_group`, but
  `ReconstructionActions::bind_settlement` rejects the compact table with
  `Settlement("mutable table mismatch")`. The original generated and frozen
  historical tables still pass the same test.
- The pinned core's `test_StaticLookup_NestedRetriesAfterLocalWrites` passes in
  all three selected Solidity suites, using reusable candidates.

The local limitation is in construction and exact semantic reconstruction, not
in the core's static lookup rule. `execute_static` in `entries/materialize.rs`
allocates per observation; `SemanticTransaction::materialize` uses that builder;
`bind_entries` in `blob_semantics/reconstruction.rs` compares complete ABI bytes.
The signer also materializes and compares settlement/sidecar bytes in
`settlement/semantics.rs`. The deriver calls `bind_observed_settlement` when
reconciling posted batches. Replacing the shared builder alone would therefore
change historical reconstruction as well as new production.

This is an inefficient and restrictive fork policy; these results do not show
invalid contract execution or unauthorized signing. The complete observation
stream must remain independently validated even if published candidates are
compacted. A production fix needs cursor/position-aware reuse of complete static
programs and exact historical replay compatibility. Arbitrary ABI deduplication
or relaxed equality is not established safe by these probes.

Reproduction: apply [temporary diagnostic patch](compact-diagnostic.patch), then
run `cargo +1.95.0 test --locked -p eez-protocol --lib
static_local_write_preserves_rows_and_resolves_earlier_candidates -- --nocapture`
and `cargo +1.95.0 test --locked -p eez-protocol --test call_patterns
repeated_static_replay_compatibility -- --nocapture`. These probes print observed
results; they are not production regression assertions. Both selected tests
passed. The patch was removed after inspection. Retained output:
[local-write probes](compact-local-writes.log), [reconstruction probe](compact-replay.log),
[core reuse tests](core-reuse.log). No new full Kurtosis run, production compactor,
full signer integration test, or gas benchmark was performed in this reassessment.


## 2026-11-10: fork correction and local validation

The user authorized correcting the fork after the reassessment. The earlier
protocol-fixture patch is superseded; the gitlink remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`.

The candidate reuses only completed static programs whose position, complete
callback array, callback-result commitment, success and return bytes match and
whose row is reachable from the mutable cursor. Each observation still contributes
to the semantic stream, execution trace and independent validation. It does not
change lookup resolution or relax exact ABI comparisons.

A wire discriminator is required for deterministic historical replay: the new
`l2_only_static_compaction_requires_the_authenticated_version` regression has
identical L1 tables but distinct L2 native transaction bytes for old/new layouts.
Shape-based selection or fallback after a binding failure cannot identify the
correct L2 bytes. Direct operations V1 uses the same columns as V0 and selects
reusable candidates; V0 retains the old constructor policy. The standardized EEZ
blob stream is unchanged. All native blob readers bind the policy from the
operations payload. RPC validation identity advances to 9 and advertised
operations tag to 1; the on-chain signing domain and capability profile remain
unchanged. Readers must be upgraded before V1 production.

Source identity: parent `1121d4752bde629d3179512440b8beeb5508c936`, unchanged
protocol above, plus the implementation diff excluding this audit directory.
Its `git diff --binary --full-index --no-ext-diff` SHA-256 is
`5a43e9e8cd41dab06c5c7907aae3134b537427907d57293d8b7e0085946d01b5`.
The [source-built runtime hashes](local-runtime.sha256) were checked again after
the second process suite's build; all three remained identical. These are local
debug-profile builds with the repository's configured optimization settings,
not CI release artifacts. Rust 1.95.0, LLVM 22 and Foundry 1.7.1 were used.

The [retained validation summary](local-validation.txt) records selected output;
raw logs in `/tmp/pr123-compact-*.log` and process logs/datadirs under
`artifacts/compact-static-e2e/` are local-only.

| Check | Result |
| --- | --- |
| Composer, RPC, driver | 425, 7 and 74 passed; one manual Composer profiling utility ignored |
| L1, signer | 200 and 340 passed; five explicit signer fixture/inventory utilities ignored |
| Payload codec, blob codec | 17 + 2 and 21 + 1 passed; includes the independent Ethereum KZG oracle |
| Protocol | 232 unit, 31 call-pattern, 11 causal-evidence, 3 public-input and 2 rolling-hash tests passed |
| Prover client | 34 passed |
| Four static-local-write process cases | 4 passed, including settlement and clean-follower replay in both directions and top-level controls |
| Additional nested-call/history process cases | 7 passed, 1 environment-specific Kurtosis capture replay ignored |
| History fixture regeneration | 1 explicitly selected utility passed; subsequent exact-byte comparison passed |
| Strengthened unknown-version rejection | 1 passed; a complete otherwise-valid payload is changed only at its version byte |
| Workspace formatting and all-target Clippy | Passed, Clippy with `-D warnings` |
| Signer all-target/all-feature Clippy, all-target check, strict rustdoc | Passed |
| Dependency checks | `cargo deny check advisories bans sources` and `cargo audit` passed; audit reported 20 allowed warnings; lockfile unchanged |
| Integration contract build | Passed; no contract source changes |

The additional process run includes the cold, network-denied signer test:
six requests, four specific `observation_replay` rejections without signatures,
and two signatures over independently fixed expected digests. The ignored
`kurtosis_combined_history_observations_replay_offline` needs an owned offset-5
Kurtosis capture and retained genesis; it is not counted as executed coverage.

Commands, from the worktree root with `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` and
that installation's `bin` on `PATH`:

```bash
cargo +1.95.0 test --locked -p eez-protocol -p eez-payload-codec -p eez-blob-codec -p eez-l1 -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -p eez-driver -p eez-composer
cargo +1.95.0 test --locked --no-fail-fast -p eez-l1 -p eez-proof-signer -p eez-protocol -p eez-payload-codec -p eez-prover-client
cargo +1.95.0 test --locked --no-fail-fast -p eez-proof-signer -p eez-protocol -p eez-l1
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/compact-static-e2e/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/compact-static-e2e/datadirs"
forge build --root contracts
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- static_local_writes_reuse_candidates_and_replay --test-threads=1
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- cross_chain::history_observations:: cross_chain::call_patterns:: cross_chain::combined_history:: --skip static_local_writes_reuse_candidates_and_replay --test-threads=1
```

The first two package commands exposed outdated test expectations and fixtures;
the third is the final complete L1/signer/protocol rerun. Initially, tests expected
the V0 prefix or redundant row counts; synthetic ASCII operations lacked a valid
version, and current-profile history fixtures still contained V0/validation-8
bytes. These were corrected using the existing fixture generator while retaining
the frozen historical V0 vectors. The first process attempt could not bind local
sockets in the default sandbox; the recorded passing runs had local networking
enabled. A concurrent-build rustdoc artifact error was resolved by a sequential
rerun. These initial failures are not counted as passing validation.

Limits: no new full Kurtosis run or release-artifact matrix was performed for
this node correction. The earlier fixture-only Kurtosis result does not validate
these binaries. No gas or throughput benchmark was run. Reuse adds a scan of the
reachable host rows per static observation and exact callback comparisons for
matching candidates; this record makes no quantified performance claim. Upgrade
readers before V1 production and use matching Composer/signer validation-9
artifacts as documented in the deployment runbook.
