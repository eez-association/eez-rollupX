# Follow-up stabilization

Status: investigation in progress, updated 2026-10-25 UTC. The owner authorized closing
the remaining CI, dependency and timing findings after the extended validation.
This record supplements, and does not replace, the first failures and earlier
successful campaigns in [the extended audit](README.md).

## Baseline and requirements

- Implementation branch: `codex/l1-history-offline-proving`, initially clean at
  `d46f0a148a29ecdeea2eefa15e7380e72deeb277` in the existing dedicated worktree.
- Base: `a50c326d99f7cd1c21406eedbf370a344015a6b5`; pinned protocol:
  `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean.
- Failed CI: [40133261304](https://github.com/eez-association/eez-rollupX/actions/runs/40133261304),
  exact merge `d488e7b77164377fca3ee8b5c8e1f333afd1e94b`.
- Preserve the original failures and exact executables before diagnostic retries.
  A later pass does not erase an unexplained failure.
- Recover a still-usable settlement opportunity without trusting stale planning
  context, extending its deadline, weakening final validation, or blocking the
  independent ordinary L2 clock.
- Make fixture deployment deterministic without changing catch-up assertions,
  skipping ancestry, resending ambiguous transactions or merely raising timeouts.
- Resolve the cryptographic advisory through a reviewed compatible dependency
  change. Preserve signature/public-input/recovery semantics, v2 and the offline
  profile; an advisory exception is not remediation.
- Explain the original admission refusal and compare timing under matched work
  before making a performance claim. Preserve offered/accepted distinctions.

## Preserved CI failures

The run completed with passing unit, producer, viewer and Kurtosis lanes, but
two process tests and the dependency gate failed:

| Check | First observed failure |
| --- | --- |
| `settlement_policy::late_sync_settles_due_history_within_the_original_target_window` | 180-second wait for the original usable target's dispatch expired. |
| `derivable_operations::multipart::expired_sources_use_recent_anchors_with_cold_offline_signers_and_public_replay` | Fixture setup waited 60 seconds for `registerRollup`; the catch-up assertions had not started. |
| Dependency policy | `crypto-bigint 0.5.5`, `RUSTSEC-2026-0387`. |

Raw logs, workflow metadata and both failure artifact ZIPs are retained locally
under ignored `artifacts/stabilization-20261024/`. The ZIP SHA-256 values match
GitHub's published artifact digests. Their contents are not committed bulk logs.
The separate diagnostic checkout consumes the same run's immutable payloads;
it must not substitute a preceding successful runtime or rebuild a missing one.

The late-Sync trace contains a concrete stale-context rejection at
21:58:22.512224 UTC for target 30, followed by preparation of target 35. This is
the opportunity lost when a planning snapshot changes while the independent
ordinary clock advances. The event is consumed once; the former comment about
later ticks retrying the planner did not describe the separate worker.

## Exact-runtime diagnostics and deterministic regression

Both failed cases passed once when run serially using the original merge's
verified release test/runtime payloads, without rebuilding Rust. The multipart
case took 119.702 seconds and late Sync took 36.835 seconds. This selected two
tests and left 177 outside the filter. These are diagnostic reruns of flakes;
they do not turn the original CI failures into successes.

The late-Sync unit regression synchronizes the planner with a barrier, then
advances either the ordinary parent or the canonical scan generation before
releasing it. On the original implementation it fails: only parent 2 is planned,
instead of replanning against parent 3. A third case changes the L1 parent and
requires no preparation for the obsolete opportunity.

The fix returns the existing typed stale-context error from the planning fence
and refreshes within the original target window. The attempt count is bounded by
the interval's position count, and the monotonic budget is never renewed. It
retains the single owned preparation worker and the independent ordinary clock.
After the change, all 53 driver unit tests pass, including the existing blocked
worker/cancellation test. That test's expired synthetic slot was adjusted to a
valid, already-triggered grid slot; its ownership, progress and cancellation
assertions are unchanged. Two intermediate fixture failures are retained.

Local command/results are under `artifacts/extended-validation/checks/`:
The compact [check record](stabilization-checks.json) retains commands, results,
log digests, source identity and the original CI payload/artifact identities.
Raw local-only logs and datadirs are not available in another checkout.

| Directory | Result |
| --- | --- |
| `stabilization-original-cases` | Original release payload: 2/2 diagnostic passes; failures remain recorded. |
| `stabilization-planner-before` | New deterministic regression fails before the production change, as intended. |
| `stabilization-planner-after`, `stabilization-planner-final` | Intermediate fixture-coordinate failures; no passing coverage claimed. |
| `stabilization-planner-aligned` | `cargo +1.95.0 test --locked --offline -p eez-driver --lib`: 53/53 pass. |
| `stabilization-future-race-before`, `stabilization-generation-before` | Controlled suffix regressions fail before their respective corrections. |
| `stabilization-future-race-after`, `stabilization-generation-after` | 53/53 driver tests pass after each correction. |
| `stabilization-clippy` | `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings`: pass. |
| `stabilization-units` | `cargo +1.95.0 test --locked --offline -p eez-driver -p eez-composer -p eez-testkit --lib`: 502 pass (408 Composer, 74 driver, 20 testkit), one pre-existing manual timing-profile exclusion. |

The excluded `held_pool::tests::profile_blocked_backlog_discovery` records manual
component timings and makes no timing assertion. It is not claimed as executed.
The combined driver feature selection enables 21 additional derivation tests
compared with the isolated 53-test invocation. Workspace format, Python syntax
and `git diff --check` also pass. No dependency, protocol, ABI, genesis or
persistent-state format changes are included in this stabilization.

The first combined-source invocation (`stabilization-source-combined`) was
cancelled by the resource guard during compilation after 158.05 seconds, when
available host memory fell to 4,636,233,728 bytes. No process test ran. Its log
and result are retained; this is neither passing coverage nor a functional test
failure. Once 23 GiB were available again, `stabilization-source-resumed` resumed
the same command with the same one-job limit and guard.

The first source-based process regression **failed** after the initial planning
fix. One test ran, with 178 outside the filter; the resource guard did not cancel
it. At 23:54:48.519 UTC the worker accepted target 385, then the ordinary clock
committed block 383 while the worker was preparing that same block. The suffix
code converted its stale-parent rejection into success, so the owner could not
retry. No Sync invocation followed for that target. This is a second instance
of losing the opportunity before submission, not a proof-validation failure.

The corresponding `future_parent` barrier regression fails before the second
fix (`[2]` rather than plans `[2, 3]`) and passes after propagating preparatory
commit failures to the existing bounded retry. All 53 driver tests pass at this
stage. Stale **prebuilt Sync** commits still retain their original handling:
submission may already have occurred and must not be repeated by this loop.
The later process run must validate the combined runtime.

Review also reproduced a canonical-generation change during a successful Future
commit (`future_generation`). The next step must replan rather than continue a
suffix with an invalidated decision. Each suffix step now checks its parent and
canonical snapshot, including the last step before Sync composition. This also
prevents another producer's progress between iterations from extending the
planned block count beyond its intended boundary. The five controlled cases
cover parent/generation changes during planning and composition, plus a changed
L1 parent. The generation regression fails before this fence and all 53 driver
tests pass after it. Process and strict lint validation remain separate checks.

The failed process's source patch and executable hashes are retained in
`artifacts/stabilization-20261024/first-process-binaries.json`; its signer SHA-256
was `5945de0915f4ea6ef89c0bec9eb2d61f07c847e5adfe3e55dd760ef40ea30406`.
Logs and datadirs remain in that directory's `source-late-sync/` subtree.
`stabilization-source-late-sync` records exit 101 after 2,501.67 seconds,
including the first source and signer builds. Those builds also aged the test's
wall-clock genesis; subsequent source validation must build the signer before
creating the process-test fixture.

Review traced the retry through `recover_failed`, `prepare_next_slot`,
`replace_timed_out_attempt` and the held-pool ledger. Planning again does not
create a submission or charge a user's retry budget. Replacement still checks
the exact previous target/deadline/canonical generation, and the ledger admits
release only while `replacement_released` is false. The new loop neither changes
that predicate nor treats a timeout as canonical completion. The changed-L1-parent
regression separately verifies that a stale event cannot prepare for another fork.

The fixture change uses a separate history-ready checkpoint after the last real
payload import, then deploys the protocol. Startup and receipt deadlines remain
unchanged; the dense-history wait uses the existing 90-minute replay bound.
The process regression will require its first deployment block to follow the
requested historical prefix. A bounded receipt-failure diagnostic will record
head, transaction and sender nonce observations without resubmission. This
eliminates setup overlap; it does not establish why the original registration
receipt was not observed within its deadline, because that run retained no
transaction-pool snapshot. The first process test's node, follower and test
executable were fully built and hash-recorded before this fixture/telemetry
change; its signer build is independent of those changes. The first process
result covers the driver fix only. The later source run must rebuild and test
the fixture/telemetry changes together.

The admission change records the exact values used at a `pipeline_time` refusal:
remaining time, next-group work, finishing work, proof and post-proof reserve,
plus the existing candidate ID. The `Duration` saturation and comparison are
unchanged. This closes the missing numeric evidence identified in the earlier
[admission investigation](dynamic-admission.md); it does not claim that a new
estimate or the first intermittent packing failure has been fixed.

## Cryptographic dependency constraint

The registry still has no patched 0.5 release. The corrected `crypto-bigint`
generation is used by `elliptic-curve 0.14` and `k256/p256 0.14`; the pinned Alloy
and REVM packages still require the preceding family. Alloy's proposed
[migration](https://github.com/alloy-rs/core/pull/1147) is open and declares
breaking API changes. A separate `codex/crypto-bigint-remediation` worktree
evaluates the downstream adaptation, preserving the protocol/Reth/Stateless pins.
No cryptographic patch, advisory exception or dependency change has been applied
to the implementation branch. Resolving the dependency graph alone will not
establish signature compatibility or constant-time correctness.

The isolated prototype resolves to `crypto-bigint 0.7.7`, `elliptic-curve 0.14.2`
and `k256/p256 0.14.0`, retaining the exact Alloy and REVM package versions via
nine local adaptations. Type checking first fails on the renamed SEC1 point API,
then on incompatible RNG traits in EIP-7702 authorization generation. This is a
cross-package API migration, not a compatible lockfile-only security update.
The experimental branch is not part of PR #101 and is not validated for signing.

## Settlement latency decomposition

Read-only analysis of the retained, hash-bound baseline and `28049ff7` node
logs covers each sequential workload plus its following 60 seconds. It finds
**zero** `prepare=true` plans without a corresponding invocation in either run.
Thus the late-Sync race reproduced above is not an observed explanation for
this earlier campaign's p95 difference.

| Observation | Base `a50c326d` | Candidate `28049ff7` |
| --- | ---: | ---: |
| Plans / prepare=true / prepare=false | 80 / 72 / 8 | 79 / 68 / 11 |
| All transfer settlement p95 | 25.415 s | 31.599 s |
| Transfers without an intervening false plan | 3,377 | 3,357 |
| Their settlement p95 | 18.401 s | 18.438 s |
| Transfers overlapping a false plan | 425 | 533 |
| Their settlement p95 | 30.327 s | 41.541 s |

An overlap requires a final receipt's local commit to precede a false plan for
a target covering its height, before that receipt's canonical safe advancement.
Safe events are matched to the verified batch's exact post hash, L1 transaction
and L1 block; the aggregate latencies reproduce the previously recorded results.
All false plans had no queue hint and no due policy reason. They are deliberate
planning outcomes, not failed proof requests. The candidate's preparation-to-proof
p95 was 242 ms versus 270 ms in the base's original measurements.

This localizes the long tail to intervals containing no-prepare decisions; it
does not establish counterfactual inclusion or a causal performance regression.
The transaction arrival phases and histories differ. No deadline, policy,
reservation or assertion was relaxed on the basis of this correlation. The compact
[decomposition](stabilization-latency.json) retains source and log identities;
these results still do not establish performance parity or a capacity/SLA bound.
