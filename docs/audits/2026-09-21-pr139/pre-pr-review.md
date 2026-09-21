# Pre-PR review of funding deferral and diagnostics

## Scope and provenance

Follow-up to `459a6ba4` on `backport/pr-139-funding-deferral`, based on RollupX
main `4f4e73e6e6542eb9ba17097693c240a8cd2ea41d`. Protocol remains
`3c0cdf4955941a5c23639346932b55b33babebbd`. Rust 1.94.0; Foundry 1.7.1.
No contract, configuration, admission policy, retry-budget, or deployment change.
The existing source-wait policy remains: retain blocked requests without charging
settlement attempts and allow independent eligible work to progress.

## Review dispositions

| Item | Evidence / disposition |
| --- | --- |
| Process failure during a hanging poll | Reproduced with an owned real child process exiting after the first failure check and a permanently pending poll. Baseline exceeded the independent two-second test deadline. Check failure every 500 ms while retaining the same poll future; regression now passes. |
| Terminal status hidden by hanging receipt RPC | Reproduced against an owned local HTTP server while the child remained alive. Baseline exceeded the independent three-second deadline. Bound each receipt/status RPC to one second so terminal status can still be queried; retry ordinary errors. Regression now passes. |
| Pre-submission eviction becomes unknown | Baseline unit regression returned no status for an evicted composing root and queued suffix. Add metadata-only history for the latest 1,024 local removals/replacements. Current ownership wins; valid readmission clears the observation; rejected admission and delayed callbacks cannot reinstate work. Authoritative ownership retires stale observations before ledger pruning. |
| Vacuous zero-attempt assertion | Require at least one dispatch and nonempty per-dispatch attempt counts before asserting all counts are zero. All three updated native funding-recovery cases pass; previous revision's successful runs are not substituted for them. |
| Unbounded retained work | Confirmed aggregate live-pool capacity is not bounded. HTTP body collection already has a 10 MiB bound, plus existing transaction validation; neither bounds cumulative retained work. No arbitrary capacity or expiry default introduced here. |
| Backlog scan overhead | Measured the held-pool component at 1,000, 4,000 and 10,000 queued envelopes, with 64 independent tail identities. At this scale cumulative admission dominates snapshot and claim time; no speculative scan rewrite. See limitations below. |
| Full CI | Must run the existing manually dispatched CI on the task branch before claiming release/native/Kurtosis validation. Local lint/unit checks alone do not satisfy this gate. No PR is required to dispatch it. |

The terminal-observation cache is diagnostic, not a second ownership journal.
It keeps no envelopes and cannot authorize execution or recovery. Its added RPC
fields and count/restart retention semantics are documented in the
[queue contract](../../proposals/cross-chain-queue-liveness.md#local-terminal-observations).
Null status remains unknown, not proof of removal. Bounded history means sufficiently
old removals can still time out; this is intentional, not durable transaction tracking.

## Local validation

Commands run from the task worktree, with offline locked dependencies:

- `cargo +1.94.0 test --offline --locked -p eez-composer --lib --target-dir
  /home/edu/Development/eez-rollup0-fork/target/backport-rust-1.94 -j2`:
  **207 passed, 0 failed, 1 ignored**. The ignored test is the explicitly run
  component profile, not a correctness test.
- `cargo +1.94.0 test --offline --locked -p eez-testkit --lib --target-dir
  /home/edu/Development/eez-rollup0-fork/target/backport-rust-1.94 -j2 --
  --test-threads=1`: **15 passed, 0 failed**. Includes real child exit, hanging
  HTTP response, healthy slow polling without restarting, transient errors,
  terminal status classification, and existing testkit units.
- `cargo +1.94.0 clippy --offline --locked --workspace --all-targets --target-dir
  /home/edu/Development/eez-rollup0-fork/target/pr139-check -j2 -- -D warnings`:
  **passed**.
- `cargo +1.94.0 fmt --all --check` and `git diff --check`: **passed**.

Initial regression failures were preserved as evidence, not retried away:
process exit failed at two seconds; missing eviction status failed on `None`;
hanging receipt RPC failed at three seconds after the process-monitor fix.
All pass with the targeted fixes. Previous native executables recorded in the
initial audit predate this follow-up and must not validate its runtime changes.

### Updated native artifacts

Built the shipping runtime from `4fbd6b5c40a7eb2272440c06744225f65166c6bc` using
the initial audit's locked build command and explicit role optimization overrides,
then froze it in `artifacts/pr139-review/bin` before compiling native test targets.
`forge build --root contracts` passed (cached, existing dependency/lint warnings).
No protocol or Solidity changes. These are local dev-profile checks, not release CI.

| Executable | SHA-256 |
| --- | --- |
| Composer | `494ea2a8f49066ec9b3ef94723e0de07a3b70752706463f2aeb3399c046c72d5` |
| Follower | `7676f64c26c7393fc6fca0aa82697442e9b390731f30bb3db257a61a0d06cfed` |
| Proof signer | `c0ac682a08a43962b477c94ec0861c5187e3a41a01bc2fafd3f28ace58d41c62` |
| `queue_liveness-60a3a82223dfc94f` | `1e5c1939e694aad3cfc2e455395f0d341e216862229502c0b0ae1bd87fb81911` |
| `cross_chain-425a8c5f53b10d35` | `fa1358a8faf42d45568803b4a37443000ecd841d4f0b08d33b3853c938951ea6` |
| `partial_consumption-12664f2a64006788` | `d18fefd13f0a8c18a678c46aef47fdb6fe6019c047183d8cd3f587c58fe0b5b0` |
| `queue_reorg-c8a92f321947d57a` | `9330fcce1e4dc7a9ae0a9615cb4e08e1386548bc0207743e60c6075e5f8cc53d` |

Native build: `cargo +1.94.0 test --offline --locked -p eez-node-e2e
--test queue_liveness --test cross_chain --test partial_consumption --test queue_reorg
--no-run --target-dir /home/edu/Development/eez-rollup0-fork/target/backport-rust-1.94
--config profile.dev.package.eez-node.opt-level=0
--config profile.dev.package.eez-follower.opt-level=0
--config profile.dev.package.eez-node-e2e.opt-level=0 -j1`.

Direct test executables run serially with `--test-threads=1 --nocapture`,
`EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` pointing to the frozen directory,
and `EEZ_TEST_PROOF_SIGNER_BIN` to its signer. Logs and failure datadirs are retained
under `artifacts/pr139-review/{logs,datadirs}` (local-only, not in another checkout).

| Native check | Result |
| --- | --- |
| Complete `queue_liveness` target | **3 passed**, no failures, ignores or retries; 225.70 seconds. Both Ethereum profiles and Gnosis automatic admission verify four pinned funding evaluations, independent progress, later recovery, exact balance/nonce, nonempty zero-attempt dispatch evidence and follower convergence. |
| `cross_chain`: `both_directions_return_value_and_wrapper_success_repeated_waves --exact` | **1 passed**, 21 filtered out, no retries; 71.21 seconds. |
| Complete `partial_consumption` target, first run | **4 passed, 1 failed**, no ignores; 261.13 seconds. `partial_prefix_keeps_unconsumed_users_and_receipt_costs` failed during fixture startup, before settlement assertions. First/last source omission, restart after partial consumption, and same-block middle-group resume passed. |
| Complete `queue_reorg` target | **1 passed**, no failures, ignores or retries; 56.91 seconds. Real native L1 reorg recovers confirmed and unresolved generations in nonce order. |
| Diagnostic retry: `partial_prefix_keeps_unconsumed_users_and_receipt_costs --exact` | **1 passed**, 4 filtered out; 60.96 seconds. Same source/runtime/test binaries, no changes between attempts. This is a flaky result, not a clean full-suite pass. |

The partial-prefix fixture failed because embedded L1 HTTP port 35557 implied
WebSocket port 35558, which was already in use. Read-only `ss -tanp` inspection
found `127.0.0.1:35558` in `TIME_WAIT` with a loopback peer. The unchanged
`ports::PortLease::http_pair` reserves both ports but must release them before
child spawn; its documentation explicitly acknowledges the non-atomic handoff.
This is evidence of a fixture bind race, not a settlement assertion failure;
the connection's originating process was not established. No process was killed,
no host port settings changed, and no fixture retry loop added to mask failures.

Retained first failure: `artifacts/pr139-review/logs/eez-node-node-755737-10.log`
and `artifacts/pr139-review/datadirs/eez-cross-chain-755737-GHZdgA`. A diagnostic
retry is separate evidence and cannot turn the first full-suite failure green.
Both the port allocator and embedded L1 port derivation are unchanged from the
fork base. Robust subprocess socket handoff remains test-infrastructure follow-up;
this branch neither solves that race nor claims an entirely green native suite.

## Component profile

Run the composer unit executable with
`profile_blocked_backlog_discovery --ignored --test-threads=1 --nocapture`.
Three consecutive candidate samples, normal optimized dev/test package settings,
same shared host and no concurrent compiler jobs:

| Queued envelopes (1 KiB each) | Cumulative admission, ms | Snapshot, ms | Claim 64 independent tail identities, ms |
| --- | --- | --- | --- |
| 1,000 | 34.735–40.111 | 0.070–0.075 | 0.151–0.154 |
| 4,000 | 489.175–550.806 | 0.327–0.397 | 0.510–0.553 |
| 10,000 | 3,247.120–3,356.084 | 0.515–0.917 | 1.264–1.348 |

A single baseline sample before local-removal telemetry changes recorded
36.388/496.404/3,233.941 ms admission, 0.109/0.364/0.833 ms snapshot and
0.159/0.508/1.331 ms claims respectively. This is a scale check, not a statistically
controlled performance comparison or evidence of improved throughput.

The fixture models a nonce-chain backlog and independent tail identities, not
EVM execution, cryptographic ingress, network contention, or complete blocked
selection. Snapshots shallow-clone envelope buffers. Admission includes adding
the 64 independent identities. Envelope bytes exclude metadata/allocator overhead;
no resident-memory claim is made. Repeated scans in admission are a plausible
optimization target based on code inspection and steep cumulative growth, but
require profiling under realistic concurrent ingress before redesign.

## Capacity follow-up, intentionally separate

Before a public/untrusted production ingress is considered bounded, define a
global count/byte budget and optionally per-sender fairness. A per-sender limit
alone is not Sybil resistant. Account for queued, composing, in-flight and
unresolved accepted ownership; reserve already accepted work. Specify atomic
replacement byte deltas, duplicate semantics, recovery behavior and deterministic
overload responses. Test that limits cannot discard accepted generations or
break nonce-prefix recovery. Choose defaults from an explicit workload/memory
budget, not the diagnostic cache size. Expiry is a separate policy decision.

## CI gate

The existing `ci.yml` supports `workflow_dispatch` with the GitHub-hosted
`ubuntu-24.04` runner. Its release producer, unit/vector checks, four embedded
shards, four cross-chain shards, Kurtosis suite and final `CI Result` are the
required full validation. Branch runs publish run-specific candidate artifacts;
production image promotion is guarded by `refs/heads/main`. No workflow changes
or bypasses are part of this review. Record the exact run URL and source commit
when reporting the result; do not treat a queued or partial run as a pass.

The attempted branch push was blocked by the tool safety reviewer before execution:
uploading repository code/history to GitHub requires explicit user authorization.
Nothing was pushed and no CI workflow or PR was created. Full CI remains unverified
pending approval to push this branch to `eduadiez/rollupX` and dispatch `ci.yml`.
Local native checks above do not substitute for the release/Kurtosis matrix.
