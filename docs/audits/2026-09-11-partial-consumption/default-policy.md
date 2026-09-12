# Partial consumption as the only submission policy

This follow-up supersedes the opt-in rollout decision in P5 of the original
[implementation report](completion.md). Partial consumption and durable recovery
are now the normal, unconditional composer behavior. No migration handling was
added, as requested.

Worktree: `/home/edu/Development/eez-rollup0-fork/target/worktrees/partial-consumption-default`

Branch: `fix/partial-consumption-default`

Base: `d09bc005ec2daa0ded91399749bb47d02ed5b9d5` (`fix/partial-consumption`)

The change removes the submission-policy enum, environment parser, atomic branch,
unrecorded submission entrypoint, per-attempt relay switch, and raw-mempool
fallback. Every request has an exact timestamp pin and permits only its validated
signed inbound L1 users to revert. postBatch remains required; no dropping list
is sent. Excluding unit-test modules and integration-test files, the affected
Rust code shrinks by 152 lines.

Production ledger construction now requires opening a durable journal. Only unit
tests can construct an in-memory ledger. Composer startup opens and locks
`$EEZ_L2_DATADIR/settlement-journal.json` before launching either reth instance.
Missing/unusable storage, corruption, and lock contention prevent startup.
The existing journal format, reconciliation states, retry rules, proof format,
blob format, and follower derivation are unchanged.

The default configuration is used by the partial-consumption integration tests
and all Kurtosis profiles. The deleted fallback's test signal and assertion were
removed. A replacement transport test exercises the real submission entrypoint
and HTTP boundary, including inbound revert permissions, a lone postBatch,
unsupported relay responses, and a failed dispatch-record callback.

The valid same-block resumption fixture now waits for relay acceptance before
holding the next request. Waiting only for the composer's observer handoff could
hold the previous empty batch. The corrected fixture keeps its strict checks on
all three proven users, original proof/blob bytes, same-block L1 ordering, and
fresh-follower convergence.

Protocol and implementation sources verified:

- Core protocol remains pinned to `3c0cdf4955941a5c23639346932b55b33babebbd`, with
  OpenZeppelin at `5fd1781b1454fd1ef8e722282f86f9293cacf256`.
- [Core protocol specification](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md),
  G.1–G.5: immediate execution, forward consumption, queue replacement, and mutable
  same-block consumption. The corresponding pinned EEZ implementation was checked.
- [Proof-signer profile](../../../crates/eez-proof-signer/SPEC.md), sections 7 and 11:
  supported immediate prefix, `blockNumber == 0`, and mandatory native blobs.
- [Flashbots RPC definition](https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint#eth_sendbundle)
  defines revert permissions and timestamp bounds. The pinned
  [rbuilder implementation](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/building/order_commit.rs)
  distinguishes included EVM reverts from invalid-transaction omission.

Verification uses the locked dependency graph and offline dependency cache.
Executable tests use Rust 1.96.1, with debug symbols and incremental compilation
disabled. Clippy uses the CI toolchain, Rust 1.94.0.

| Check | Result |
| --- | --- |
| Focused L1 unit tests | 86 passed. |
| Complete workspace library tests | 516 passed across 15 suites. |
| Workspace Clippy, all targets, warnings denied | Passed. |
| CI Python tests | 174 passed, 4 explicitly skipped. |
| Contract tests | 19 passed. |
| Composer startup checks | 5 passed: missing datadir, unusable datadir, corrupt journal, locked journal, and creation of a fresh journal. |
| Partial-consumption native tests | 4 passed in 354.46 seconds: first/middle/last omissions and valid same-block resumption. The corrected resumption case also passed separately in 80.98 seconds. |
| Broader native regressions | 13 additional cases passed: 5 chained-call cases, bidirectional smoke, repeated bidirectional waves, 2 caught-revert cases, 3 L1 reorg cases, and malformed-resumption rejection/recovery. The corrected malformed-resumption case passed twice, in 79.61 and 75.48 seconds. |
| Formatting, diff checks, changed deployment-file parsing, Chiado blob configuration | Passed. |
| Offline dependency policy and cached advisory check | Passed; the advisory check uses 1,243 cached advisories and reports 14 allowed warnings: 7 unmaintained packages, 5 unsoundness notices, and 2 yanked versions. Dependencies are unchanged. |

Exact commands, rebuilt executable hashes, and per-suite results are retained in
[default-validation-evidence.json](default-validation-evidence.json). Detailed
logs are in the worktree's ignored `target/default-*` artifacts. The initial
sandboxed L1 run could not open loopback sockets; the same tests passed with
loopback access.
The first workspace run found a stale fallback signal in the test catalog; its
removal was followed by the successful full workspace rerun above.
The first native resumption run captured an empty batch and failed its fixture
precondition. The relay-acceptance barrier fixed that race; the focused test and
the complete four-case partial-consumption rerun both passed.

The broader run passed 16 cases, then the malformed-resumption test rejected a
retryable missing-block-body error for its deliberately orphaned transaction.
The node had rejected the malformed batch and restored canonical settlement.
The [blob reader](../../../crates/eez-l1/src/blob_source.rs) explicitly reports
that condition as `SourceIncomplete`, and the deriver retries on the next L1
event. The test now permits that exact transaction/block error only after its
injected reorg and before recovery. Its assertion windows overlap so no log
records are lost between them. Other fatal signals remain failures, and two
further healthy batches must settle without any fatal signal. Both reruns passed.
The production executables retained their exact hashes through this test-only
correction; the other 16 successful native results cover the final runtime code.

The final critical review checked the complete follow-up diff against `d09bc00`
and the aggregate task scope against original `main` at `4013495`. It checked
durable construction, journal-before-dispatch ordering, exact timestamp pins,
revert-allowlist membership, relay error paths, and the two corrected fixture
races. No additional concrete issues remained. The deleted signal's test
catalog entry was also removed; no public-mempool path or migration layer was
introduced. The original worktree and branch were left unchanged.

No dependency versions or protocol gitlinks changed. The existing source-RPC
latency limitation remains documented in the main README. Kurtosis has not been
rerun for this follow-up;
the prior real-builder canary is recorded separately in the original report.
The native relay controls inclusion and validates accounting/replay; it does not
retest builder allowlist semantics. Public-mempool submission and replacement
remain outside this follow-up's scope.
