# Conditional slot production: implementation draft

Date: 2026-10-09 UTC. PR: [#79](https://github.com/eduadiez/rollupX/pull/79).
Status: draft, opt-in; not an authorization or readiness finding for deployment.

## Identity and scope

- Fork base: `4c4fec08d8a713497e1c8fc23bc0cf6fe0273e0f` (`main`).
- Initial documentation head: `6957ab3b66d9d8429682b7fbe50b85712202ef98`.
- Protocol gitlink: `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized
  recursively without edits. No dependency, ABI, wire or storage change.
- Implementation uses the same PR branch, isolated from the user's dirty
  worktree. Unrelated Compose edits, operator state and local artifacts are not
  included. No deployed services were started, stopped, upgraded or funded.
- Rust 1.95.0, LLVM 22, Foundry 1.7.1, pinned Solc 0.8.34. Local Cargo target
  cache is outside the isolated worktree; all commands use its source and lockfile.

The first-release implementation follows [the proposal](../../proposals/conditional-slot-production.md):
opt-in block-position trigger, side-effect-free preparation reasons, paced idle
suffix, existing early preparation when due, and empty sync checkpoints retained.
The cutoff event and ticks share the decision path; ticks retry unavailable
context after the prefix. A semaphore bounds blocking planning even after a wait
timeout. Final admission and settlement ownership remain unchanged.

## Validation record

Commands run from the isolated PR root, with `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`,
LLVM's bin directory on PATH, and the task's Cargo target directory. Dependencies
were never upgraded to bypass failures.

| Check | Result |
| --- | --- |
| `cargo +1.95.0 check --locked -p eez-driver -p eez-composer -j 4` | Passed initial implementation build. |
| `cargo +1.95.0 test --locked -p eez-driver -p eez-composer --lib -j 4` | Initial run: 316 Composer + 69 driver tests passed; the manual `held_pool::tests::profile_blocked_backlog_discovery` timing profile was ignored. First review-correction rerun: 316 Composer + 70 driver passed, same exclusion. |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_chiado_config.py' -v` | 17 passed after correcting the newly added test fixture; see first failures below. |
| Corresponding `test_ethereum_config.py` suite | 9 passed. |
| Corresponding `test_kurtosis_ci.py` suite | 17 passed, 1 explicitly skipped real-Kurtosis probe (not enabled). |
| `python3 scripts/check-settlement-policy-config.py` | Chiado, Ethereum and Kurtosis mandatory configuration propagation passed. |
| `forge build --root contracts` | Passed; pre-existing Solidity/lint warnings, including missing optional `lib/forge-std` path warning. |

First failures: the new Chiado transport test initially treated the separate
Beacon-only `docker-compose.chiado.yml` as a node profile. Its first run failed
on absent Beacon fixture paths; adding those paths exposed the incorrect
`eez-node` lookup. The test was corrected to target the actual node profile.
These were deterministic test-fixture errors, not runtime passes on retry.

The first workspace Clippy run rejected an unchecked Duration subtraction in
the new startup check. It was changed to saturating subtraction, without an
allow-list exception; timing validation still rejects invalid positions first.
The next lint pass identified a large by-value test-helper argument; it now
borrows the policy snapshot instead.

The first focused process-test attempt built the node test binary but failed
before starting any chain: the sandbox denied binding its HTTP reservation
(`crates/eez-testkit/src/ports.rs:79`, `Operation not permitted`). This is not
runtime evidence for or against the scheduler. The rerun requests isolated
local socket access and retains test logs/datadirs under
`artifacts/conditional-slot/` (local-only, not published evidence).

## Implementation review iteration

Claude Code 2.1.295 / CLI-reported `claude-opus-5-5`, read-only scoped source,
user-authorized Anthropic access; no permission denials or writes. The first
implementation review completed successfully and found three blockers:

1. One-shot cutoff failures could stall the suffix. Both ticks and cutoff events
   now use the decision path; stale evidence retries, while planning failure
   chooses paced fallback with existing Empty maintenance.
2. Existing Empty mode can evaluate policy and post, contrary to an ordinary
   decision. Added internal Ordinary mode for successful not-due hints only;
   it builds the same empty checkpoint without initiating policy settlement.
3. Ticks waited for the next L1 head/cutoff despite an armed replacement. Position
   mode now publishes bounded replacements at their preceding boundary time,
   preserves the prior valid boundary decision, and catches up only due blocks.

Additional corrections cap the planner wait at one L2 interval, gate queue hints
on absence of a blocking owner, revalidate decision context before Steady
composition, refresh tick phase from wall time, and document admission snapshot
timing. Regressions now assert recovery from scan lag/generation changes instead
of accepting a frozen interval. Follow-up review and rerun results follow below.

The second implementation review confirmed the main fixes and identified a
newer SyncSlot clearing an unfinished older ordinary decision. The trigger now
preserves that decision through catch-up; a dedicated regression covers it.
A remaining optimistic blocking owner explicitly selects paced recovery rather
than the non-posting Ordinary path. A same-branch canonical scan generation
change intentionally invalidates the hint and permits fresh recovery policy
evaluation; strict no-post suppression is guaranteed only in the decision's
unchanged context.

The suggested additional `max_blocks <= max_speculative_depth` restriction is
not adopted: existing policy configuration already requires depth zero or
`D >= N + K - 1`, while GENERAL fires at the first boundary at/after N regardless
of the signer window M. That boundary is at most N+K-1 blocks from the settled
anchor and therefore reachable. A regression covers N, offset residues and
M=D/D+1/D+K-1. A final focused review checks this reasoning and the corrections.

The third focused implementation review completed successfully with the same
read-only Claude model. It confirmed the GENERAL/depth argument and the
blocked-owner recovery behavior; process and load scenarios remain release gates.
All three implementation reviews reported no permission denials and made no edits.

Final focused package rerun after the corrections: **317 Composer + 71 driver
tests passed**, with the same one manual component-profile exclusion. All three
configuration/tooling selections and Compose propagation passed again.

Workspace `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings`
passed after both lint corrections. The process rerun reached a live disposable
node and signer, but the original observer waited for a driver INFO event excluded
by the harness's stdout filter. Retained logs show successful Composer hints and
Ordinary checkpoints. The regression now selects the Composer's successful
not-due hint (not an error fallback) and explicitly requires the corresponding
Ordinary invocation before independent replay. This corrects test observation,
not scheduler behavior; its first failure is retained rather than counted as coverage.

That observer failure selected exactly one test and timed out waiting for the
filtered signal after 180 seconds (320.13 seconds including cold signer setup).
Local-only retained evidence (not available from another checkout):

- Node log `artifacts/conditional-slot/logs/eez-node-node-892796-2.log`, SHA-256
  `ef292232d07414f95dd10420fd269594b3192d5de6da3e2a42963b3a9df0786a`.
- Signer log `artifacts/conditional-slot/logs/eez-proof-signer-892796-0.log`, SHA-256
  `8dbe514ddfe132e57c45ba92d74a0db4eeccfa394574665509538f2ec0e1fe11`.
- Failing test executable SHA-256
  `024d4065a6963723fcc311acd4e85beca720931deebf22d8a3c7cd947433525a`;
  signer SHA-256 `566f843767f8becc774339f5cec20ad24d72b163793afb9ea2c6c3ea784eeaaa`.

The corrected process regression is rebuilt at `e1a3e0a` (all runtime and test
source committed before starting this run). Later evidence-only commits do not
change those executable inputs.

**Corrected source-based process regression: PASS**, one intended test selected,
133 filtered out, none ignored; execution including signer build/setup took
116.26 seconds. Command (with the toolchain/target cache described above):

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
EEZ_TEST_LOG_DIR="$PWD/artifacts/conditional-slot/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/conditional-slot/datadirs" \
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  settlement_policy::conditional_position_paces_late_pool_transactions_and_replays \
  --test-threads=1
```

This is a corrected-test pass, not a flaky unchanged retry. The test reaches a
successful real Composer not-due hint, submits an ordinary transfer after P,
asserts inclusion before K in the paced suffix, asserts an empty K and the
non-posting Ordinary invocation, then requires canonical safe-prefix convergence
with an independent follower and matching transaction block hash/receipt status.
It does not validate cross-chain semantics, reorg/restart or sustained latency.
The fixture uses disposable local EthereumOsaka/Anvil, a real signer and a local
builder stub; it is not production PBS/topology or live-mainnet evidence.

Passing executable SHA-256:
`bae93c98531714edbe106e35e5ffadd4b16346f541e7d240e39f5faee2c4c152`;
the signer digest is unchanged from the failed observer run. Passing local-only
node log `eez-node-node-924294-2.log` under the log directory above has SHA-256
`4ec439a8e0381dcd1124f80b55fc2b19425a49ce1326fea338f1a7dbad924009`;
follower log `eez-node-conditional-position-follower-924294-3.log` has SHA-256
`f835eed7c1014481db3a0662f984f4b8e19120eb28aae007c4ceb49e36a9880b`.
Final formatting, whitespace and added shell-example syntax checks passed.

The original empty-suffix behavior was traced in source at both design and PR
baselines; no matched baseline/candidate runtime or performance comparison has
yet been executed. Unit/mocked checks do not establish throughput, durable queue
recovery, independent process replay, or production readiness.

## CI failure investigation and repair

The first implementation CI run
([37934352557](https://github.com/eduadiez/rollupX/actions/runs/37934352557))
tested head `32279e83b3f6759d9e6d944e9727ecb6a2974722` against the base and
protocol pin above. Build, lint, debug units and Kurtosis passed. Three native
E2E shards failed; the aggregate gate correctly failed without promotion.

- Both manager-destination rejection tests queried
  `eez_getCrossChainTransaction` on the regular L2 RPC and received `-32601`.
  Their shared helper is unchanged from the PR base; it missed the earlier
  Composer-front-only migration. Use the L2 Composer front, retaining the exact
  `SimulationFailed` and manager-address assertions.
- The conditional position test timed out waiting for a setup transaction
  receipt, before its own assertions. The retained node log shows five ordinary
  transactions before reconciliation of endpoint 25, but four included in that
  settlement and no remaining activity afterward. A live acceptance changed
  the descendants' authenticated seed. `discard_stale_seed_suffix` used an
  ancestor-only `reorg_to`, which could lose ordinary transactions from the
  removed suffix instead of returning them to Reth's pool.

The repair executes an empty sibling of the first stale child using the exact
retained parent and newly authenticated seed, then commits that sibling directly.
Reth sees the removed suffix in its normal reorg notification and revalidates
its ordinary transactions. No transaction is blindly replayed under a changed
execution context. The empty replacement is unsafe; accepted endpoint, safe and
finalized advancement remain governed by canonical settlement. This uses the
same direct-fork-switch principle as existing Composer replacement recovery.
No protocol, dependency, wire, schema or queue-durability change is introduced.
Receipt-wait errors now identify the setup action and transaction hash.

Baseline local selection of all three CI failures reproduced both RPC failures;
the conditional test passed unchanged (one pass, two failures, 131 filtered).
That pass is a timing-dependent retry, not evidence that the CI failure was fixed.
Logs/datadirs remain local-only under `artifacts/ci-37934352557-baseline/`.
The CI node log `eez-node-node-1513-2.log` has SHA-256
`84f2a299602f3cf75524951acfffbdfcbf382f03119943557006f1b269f60633`;
it was downloaded from `embedded-e2e-2-37934352557-1` (three-day retention).

A new controlled process regression holds a real cross-chain bundle only long
enough to fault the canonical log reader, releases it into its target block,
and includes an ordinary transfer beyond the still-unobserved endpoint. It then
restores the reader and requires the same signed transaction to obtain a new
successful receipt and the recipient's exact balance increase, without client
resubmission. The delay targets reconciliation, not proving or L1 mining.
The final regression additionally triggers another settlement and requires an
independent follower to replay the repaired history, with a matching receipt
block hash and exact recipient balance. The original conditional test retains
its separate idle-suffix inclusion and independent replay assertions.

The controlled regression **failed on unchanged runtime code** at the intended
assertion: `stale-seed transaction was lost: timed out after 180s`. One test was
selected, 134 filtered; total elapsed time including signer build/setup was
285.66 seconds. Local-only diagnostics are in
`artifacts/ci-seed-regression-baseline/`; node log `eez-node-node-1362712-2.log`
has SHA-256 `cea6754de6bd1e17d2f526412966358be5a62574c1c1c89e9a572728b2fc13c6`.
This run used the new test with runtime sources from `32279e8`; candidate edits
were made only after its test executable had been compiled.

Focused reproduction/validation uses the source-based process procedure above,
with separate diagnostic directories and these test filters:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  conditional_settlement_requeues_ordinary_stale_seed_descendants --test-threads=1
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  conditional_ call_to_the_l1_manager_is_rejected \
  call_to_the_l2_manager_is_rejected --test-threads=1
```

Claude's scoped, read-only review (`claude-opus-5-5`, no permission denials)
found no runtime/test blockers. It checked the exact-parent construction,
reconciliation lock, safe/finalized ordering and witness delivery, and requested
the final validation record. Its suggested independent replay coverage was added
to the controlled regression. Existing profile validation bounds `extra_data`
without assigning it Sync/action meaning; copying that metadata does not copy
transactions or authorize cross-chain effects.

The initial narrower `eez-deriver`/`eez-driver` unit selection selected a different
RocksDB feature set (without the normal Composer `jemalloc` feature), starting a
native rebuild and blocking the process suite's signer build. That owned check
was interrupted, not counted as passing; the follow-up includes Composer in the
unit selection. No dependency versions or feature declarations were changed.

The first fixed process selection **passed all four tests**, none ignored,
131 filtered, in 389.89 seconds including signer build/build-lock waiting.
That run used the initial controlled regression (re-inclusion and balance),
before its follower-replay extension; all runtime changes are identical to the
committed repair `43fe13eca8e7cd55d48569e35e6be2f3ea93d51c`.
Its test executable SHA-256 was
`78c55bb39d24a2723a64d1c2b7898b0c128fa12d8d26ad7c510f646c934e6ba0`.
Local-only diagnostics are in `artifacts/ci-37934352557-fixed/`; the controlled
recovery node log `eez-node-node-1386417-12.log` has SHA-256
`b3e27e6fa89760476e2b7644ded76d1490c359a0918a81c663e53e51c363d216`.
This is a failing-baseline/passing-repair comparison of the same transaction-loss
assertion, not a passing retry of unchanged code. It is not a performance result.

The final strengthened regression at `43fe13e` also **passed**, one selected,
134 filtered, none ignored, in 115.47 seconds including signer build/setup.
It requires canonical acceptance, re-inclusion without resubmission, a second
settlement, and independent follower replay of the repaired history with the
same transaction block hash, successful receipt and exact recipient balance.
Executable SHA-256:
`fbc3fcfe4f3bbbbf471e2ac78522fe8eaeeeebda004a94440bf09146d28ecce2`.
Local-only logs under `artifacts/ci-seed-replay-fixed/logs/`:

- Node `eez-node-node-1421368-2.log`, SHA-256
  `0765e5fd94b6e3d58ca640d4b9a0d6a72956c7b3a6fd8acb09d6d1fee2af282b`.
- Follower `eez-node-stale-seed-follower-1421368-3.log`, SHA-256
  `9f2b850ca2c73d441c4744d348081ced1b47c0cff6a1ac66bb519b044f2a9891`.

Final `cargo +1.95.0 test --locked -p eez-composer -p eez-deriver -p eez-driver
--lib -j 4` passed: **317 Composer + 44 deriver + 71 driver = 432 tests**,
with only the existing manual component timing profile ignored. No tests were
filtered from this unit selection.

Final workspace `cargo +1.95.0 clippy --locked --workspace --all-targets -j 4
-- -D warnings`, `cargo +1.95.0 fmt --all --check`, `git diff --check` and syntax
checking of the added shell examples all passed. The protocol checkout remains
clean at the recorded pin. The full native partial-consumption/reorg/admission
matrix, production-feature artifact checks and Kurtosis were not repeated
locally for this repair; the fresh CI run must validate those lanes. Earlier CI
successes do not validate this edited revision. No deployment was performed.

## Remaining release gates

The full [G1–G5 acceptance list](../../proposals/conditional-slot-production.md#validation-and-release-gates)
remains authoritative. In particular retain explicit gates for process-level
deadline/admission, cross-chain and partial-consumption recovery, reorg/restart,
four skipped-slot replacements and late-original replay, and matched baseline
versus candidate reactive/load workloads. No live network benchmark or deployment
was authorized or performed. Empty-boundary compatibility is not a substitute
for running the follower/replay scenarios.
