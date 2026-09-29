# Signer L1 authorization simplification — 2026-09-29

The owner requested simpler prover authorization after reviewing the lifetime
registration/reset scan and post-replay freshness query. The accepted change is
recorded in the [proposal](../../proposals/native-system-transactions.md#signer-simplification-accepted-owner-direction-2026-09-29)
and [signer contract](../../../crates/eez-proof-signer/SPEC.md#51-header).

## Source and scope

- Dedicated branch: `backport/native-system-txs-issue-35`.
- Worktree: `/home/edu/Development/rollupX/artifacts/native-system-backport/rollupX`.
- PR #42 advanced from `0b7beed5ca48157d720d6685435ba2121c1e9c1e` to
  `cf8500e80cc10b58ab6acd59b2125dd098aa770c`. Its complete new commit changes
  recovery-test synchronization, formatting and evidence only. Both task commits
  rebased cleanly; the merge base equals the new PR head. They became `87432ab`
  (backport) and `db0b4af` (pending RPC fix).
- PR #42 merged during validation as `de5030c782e29ff07150b2728c7b564fd607f802`.
  Its tree exactly matches `cf8500e`; final rebasing onto that merged `main`
  therefore changes ancestry only, not the tested source.
- Protocol remains clean at `9d13218a355e7d543ca8a9927eea2962600b716d`.
- The original working tree's operator edits were preserved. No deployment,
  service restart, wallet funding, database recovery or upstream publication ran.

The configured SSH remote lacked a usable key. Fetching the public PR ref over
HTTPS succeeded; remote configuration was not changed.

## Behavior and correctness boundary

The signer still independently authenticates the finalized genesis reference,
accepted L2 parent, complete relevant live-slot endpoints, descendant seed and
registry state. Canonical-height reads bracket authentication so replacement of
the named L1 block invalidates the snapshot. Ordinary head extension does not.

The full registration-to-tip query and lifetime owner-reset veto are removed.
Configured genesis supplies the initial trust root. A non-genesis parent needs
an actual accepted settlement occurrence; an arbitrary assigned registry root
must still fail the execution-bound parent/prefix comparison. This change does
not implement follower recovery from manual resets.

After authentication, execution and settlement need no more L1 requests. The
`TargetRecheck` handoff is removed. The absolute deadline, cancellation, exclusive
request permit, witness execution, effect/DA checks and private attestation type
remain. A returned signature can be stale for inclusion: the pinned manager and
registry recompute the exact parent/timestamp-bound digest on-chain.

No persistent authorization cache, background scanner, new dependency, runtime
option or offline witness format was introduced. L2 execution remains stateless;
initial authorization still depends on the configured trusted L1 RPC.

## Regression evidence

- Before changing production code, `genesis_context_needs_no_registration_history_scan`
  failed on the rebased baseline with `Unavailable("rollup registration history")`.
  The same regression passes after removal of the scan.
- Canonical-source tests retain rejection of replaced targets, fabricated
  parents, wrong genesis, absent registration state, incomplete/reordered
  live-slot endpoints and source failures. Existing seed-mismatch tests remain.
- A real canonical HTTP source and tonic service test uses a synchronized
  execution stub. The source accepts only named-height reads, with finality
  already beyond the requested parent, and closes before replay is released.
  The valid request signs exactly once and releases its permit. Changing only
  the registry root rejects without signing or an actionable eviction hint and
  also releases the permit. This is lifecycle evidence, not EVM replay evidence.
- A Solidity test signs with real ECDSA and submits to the pinned registry and
  manager. The signature succeeds for its intended context, then fails with
  `InvalidProof` after changing timestamp, replacing the parent, or advancing
  to another parent. The existing independent public-input vectors remain.
- Validation identity schema 4 uses a separately calculated RLP/`cast keccak`
  vector. The independent encoder also reproduces the prior schema-3 vector.

Development failures are retained: the first service fixture used an invalid
mainnet schedule; a subsequent assertion expected an internal message that the
RPC deliberately redacts; the wrong-genesis test expected a later gate than the
existing schedule check; the old schema vector needed regeneration; Clippy found
an unused test import. These were corrected without relaxing production gates.
They are not retry-only passes of unchanged failing tests.

## Validation

- `cargo +1.95.0 test --locked --offline -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -p eez-l1`: **525 passed, zero failed, two ignored**. The ignored signer utilities inventory captures and measure cold-span feasibility; they are not counted as coverage. After strengthening the canonical service fixture to place the requested parent behind finality, its focused test passed again.
- `cargo +1.95.0 clippy --locked --offline --workspace --all-targets --all-features -- -D warnings`: passed. This also checks all signer targets/features.
- `RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked --offline -p eez-proof-signer --no-deps`: passed.
- `forge test --root contracts`: **84 passed, zero failed/skipped**, including the new real-ECDSA context regression. `forge build --root contracts` passed before the process tests.
- `cargo +1.95.0 fmt --all --check`, the edited Solidity file's format check, and `git diff --check`: passed. Full `forge fmt --check --root contracts` still fails only the two unchanged baseline files, `contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`; those were not reformatted.
- All 112 relative Markdown link paths in the changed documentation resolved at the initialized protocol pin.
- Source-based `cross_chain real_signer_rejects_ -- --test-threads=1`: **two passed, zero failed/ignored**, 22 filtered out. Both the real daemon and Composer reject tampering without advancing settlement or source nonce. The two tests took 181.50 seconds, including runtime builds.
- Source-based `cross_chain outbound_changing_static_reads_and_write_settle_and_replay -- --exact --test-threads=1`: **one passed, zero failed/ignored**, 23 filtered out, in 85.01 seconds including the signer build. It validates changing static reads around a mutable cross-chain write, actual settlement, and a fresh follower reproducing the safe block hash and resulting state.

Process commands use `cargo +1.95.0 test --locked --offline -p eez-node-e2e --test cross_chain`, with all three `EEZ_TEST_*` executable overrides cleared and no Nextest executable override present. They run serially with private fixture data and dynamically allocated ports; existing operator services/ports were inspected and left alone.

Full workspace runtime tests, release-image/CI artifacts, pinned core/Anvil and Kurtosis matrices were not repeated: this follow-up changes signer admission/lifecycle only, not core contracts, EVM execution or deployment. The affected package, on-chain signature and source-process checks above cover the changed boundaries. These local optimized-dev results do not establish release CI or live deployment.
Local-only diagnostics are under `artifacts/native-system-backport/logs/l1-simplification-*`;
process logs/datadirs are under `artifacts/native-system-backport/signer-simplification-e2e/`.
These ignored artifacts are not available in another checkout. A manifest checks
396 source/build files remained unchanged through validation. Executable SHA-256
identities for Composer, follower, signer and the process harness are retained
in `l1-simplification-executables.json`.

[Sanitized counts, source/executable identities and log digests](signer-simplification-validation.json)
are retained in this checkout; the logs themselves remain local-only.

## Compatibility

This follow-up changes no genesis, contracts, native transaction format, DA bytes,
database format, dependency pin or historical replay rule. Composer and signer
must update together because schema 4 rejects old peers and cached namespaces;
rollback also requires reverting both binaries. No migration or data reset is
needed for this follow-up. The original backport's fresh-network requirement
continues to apply. A completed proof can be unusable for inclusion after L1
changes; Composer's existing target/cutoff checks and on-chain verification retain
that responsibility.
