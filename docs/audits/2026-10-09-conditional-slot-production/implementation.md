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

The original empty-suffix behavior was traced in source at both design and PR
baselines; no matched baseline/candidate runtime or performance comparison has
yet been executed. Unit/mocked checks do not establish throughput, durable queue
recovery, independent process replay, or production readiness.

## Remaining release gates

The full [G1–G5 acceptance list](../../proposals/conditional-slot-production.md#validation-and-release-gates)
remains authoritative. In particular retain explicit gates for process-level
deadline/admission, cross-chain and partial-consumption recovery, reorg/restart,
four skipped-slot replacements and late-original replay, and matched baseline
versus candidate reactive/load workloads. No live network benchmark or deployment
was authorized or performed. Empty-boundary compatibility is not a substitute
for running the follower/replay scenarios.
