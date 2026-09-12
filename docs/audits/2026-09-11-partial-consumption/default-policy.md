# Partial consumption as the only submission policy

This follow-up supersedes the opt-in rollout decision in P5 of the original
[implementation report](completion.md). Partial consumption and durable recovery
are unconditional composer behavior. No migration handling was added.

Worktree: `/home/edu/Development/eez-rollup0-fork/target/worktrees/partial-consumption-default`

Branch: `fix/partial-consumption-default`

The default-policy work starts from `d09bc005ec2daa0ded91399749bb47d02ed5b9d5`.
The final simplifications start from `fc5afdd1d17bb82ff16f227a7c32cfca3fb9880e`
and stay on this same branch and worktree.

The change removes the submission-policy enum, environment parser, atomic branch,
unrecorded submission entrypoint, per-attempt relay switch, and raw-mempool
fallback. Every request has an exact timestamp pin and permits only its validated
signed inbound L1 users to revert. postBatch remains required; no dropping list
is sent. The default configuration is used by the native tests and Kurtosis profiles.

The final three simplifications remove another 123 Rust lines outside unit-test
modules:

- The observer records canonical postBatch inclusion without a second settlement
  log scan. Its diagnostic `SendOutcome::Included.state_applied` field and
  `expected_final_state` argument are removed. Canonical derivation, source
  receipts/nonces, and the settlement manifest remain the authority for user
  completion. The scanner and deriver retain their separate settlement checks.
- `HeldPool::new` requires the ledger's journal and restores queued users and
  pending nonce reservations before exposing the pool. Late attachment, optional
  journal branches, and the unused restore entrypoint are removed. Startup still
  checks journal identity; reconciliation and reorg transfers keep their identity
  checks and single pool lock.
- Submitter helpers pass the existing `PreparedTarget` instead of three separate
  target values. Exact timestamp pins, catch-up parent checks, receipt-index lag
  handling, and rejected-bundle capture remain. The unused `expectedFinalState`
  capture field is removed; replay request bytes are unchanged.

All workspace callers of the adjusted Rust interfaces are updated. Production
ledger construction requires a durable journal; in-memory ledgers remain limited
to unit tests. Composer startup opens and locks
`$EEZ_L2_DATADIR/settlement-journal.json` before launching either reth instance.
Missing/unusable storage, corruption, and lock contention prevent startup.
The journal format, reconciliation states, retry rules, proof/blob formats, and
follower derivation are unchanged by these simplifications.

Protocol and implementation sources verified:

- Core protocol remains pinned to `3c0cdf4955941a5c23639346932b55b33babebbd`, with
  OpenZeppelin at `5fd1781b1454fd1ef8e722282f86f9293cacf256`.
- [Core protocol specification](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md),
  G.1–G.5, and the pinned EEZ implementation: independent immediate execution,
  forward consumption, same-block restrictions, and replacement of deferred queues.
- [Proof-signer profile](../../../crates/eez-proof-signer/SPEC.md), sections 7 and 11:
  complete immediate prefix, `blockNumber == 0`, and authenticated native blobs.
- [Settlement ownership specification](../../../crates/eez-control-rpc/SPEC.md),
  section 8: inclusion alone cannot complete users; canonical evidence and atomic
  ownership transfers remain required.
- The original default-policy work also checked the
  [Flashbots RPC definition](https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint#eth_sendbundle)
  and pinned
  [rbuilder implementation](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/building/order_commit.rs).
  Revert permissions and invalid-transaction omission semantics are unchanged.

Final verification uses the locked dependency graph and offline cache. Executable
tests use Rust 1.96.1; Clippy uses the CI toolchain, Rust 1.94.0.

| Check | Result |
| --- | --- |
| Affected L1, composer, and node library tests | 189 passed. |
| Complete workspace library tests | 518 passed across 15 suites. |
| Workspace Clippy, all targets, warnings denied | Passed. |
| CI Python tests | 174 passed, 4 explicitly skipped. |
| Integration-contract tests | 19 passed. |
| Composer startup checks | 5 passed: missing, unusable, corrupt, locked, and fresh journal storage. |
| Native tests using freshly rebuilt runtime binaries | 17 passed: 4 partial-consumption/resumption cases, 5 chained-call cases, bidirectional smoke and repeated waves, 2 caught-revert cases, 3 L1 reorg cases, and malformed-resumption rejection/recovery. |
| Formatting and diff checks | Passed. |
| Offline dependency policy and cached advisory audit | Passed; 1,243 cached advisories, 14 allowed warnings. No online advisory refresh. |

Commands, executable hashes, and suite results are retained in
[default-validation-evidence.json](default-validation-evidence.json). Detailed
logs are in the worktree's ignored `target/simplify-*` artifacts. The sandboxed
dependency-policy check needed a retry with access to its advisory-cache lock.

New regression coverage checks successful and reverted postBatch receipts without
settlement-log RPCs, missing receipt block fields, absent/noncanonical blocks,
journal mismatch without ownership mutation, and reservation restoration before
ingress after restart. Existing tests cover duplicate observers, stale ownership,
FIFO retries, spent nonces, nonce boundaries, persistence errors, and reorgs.

The same-block resumption fixtures retain the prior corrections: wait for relay
acceptance before capturing a request, and permit only the exact retryable
missing-block-body error for a deliberately orphaned transaction during recovery.
Strict checks remain before the injected reorg and after recovery.

The final critical review checked the complete simplification diff against
`fc5afdd`, the tested-source digest, and the aggregate feature scope against
original `main` at `4013495`. It checked canonical evidence authority, earlier
observer completion, startup restoration, lock ordering, journal identity,
deadlines, target-parent propagation, capture/error paths, and all changed callers.
No concrete issues remained. No dependency versions or protocol gitlinks changed.

Kurtosis was not rerun for this follow-up; the earlier real-builder canary remains
recorded in the original implementation report. Native tests validate accounting,
replay, recovery, and follower convergence with controlled inclusion. The existing
source-RPC latency limitation remains documented in the main README.
Public-mempool submission and replacement remain outside this work's scope.
