# Partial consumption as the default behavior

This is the current scope for PR #16. It supersedes the opt-in policy and the
restart journal in the historical [plan](README.md) and
[implementation report](completion.md). All changes remain on
`fix/partial-consumption-default` in
`/home/edu/Development/eez-rollup0-fork/target/worktrees/partial-consumption-default`.
The complete feature is based on `main` at `4013495`; the removal of persistence
starts from `2bb83eb`.

## Behavior and scope

Every Composer permits the actual signed inbound L1 users after the required
postBatch to revert. Timestamp pins and native blob sidecars remain required.
No policy flag, migration handling, dropping allowlist, public-mempool fallback,
or transaction replacement mode is added.

The Composer retains a manifest and signed users in an in-memory attempt ledger.
Canonical derivation determines the applied prefix; canonical source receipts and
nonces determine which users completed, spent their source nonce, or remain
retryable. Inclusion or height advancement alone never releases all users.
Eligible omissions return to the held pool once, in FIFO order, under the existing
bounded retry policy. Ownership, reservations and reorg history remain coordinated
under the held-pool lock. A failed validation leaves the ledger unchanged.

The held pool shares the existing `OptimisticallyIncluded` directly. The separate
journal module, disk format, checksum, fsyncs, file lock, persisted target for
restart scanning, and restart restoration plumbing are removed. Composer startup
no longer requires a writable `EEZ_L2_DATADIR` for settlement storage. The existing
optional checkpoint/correlation cache and chain databases retain their purpose.
No dependency versions change; the Composer no longer directly needs serde or
serde_json, and L1 no longer directly needs serde. Serialization derives and
JSON-format compatibility assertions used only by the journal are removed;
the manifest's behavioral tests remain.

On restart, the node derives canonical state from L1 before accepting cross-chain
submissions. Boot catch-up discards any remaining speculative local suffix using
the existing reconciliation lock and forkchoice repair. Otherwise, an unposted
Sync block could outlive its lost in-memory attempt and permanently block future
emission. Live catch-up continues to retain active proposals. Pending users and
retry counters are not persisted; clients may need to resubmit unsettled signed
transactions. Expired postBatch envelopes are not replayed.

Followers still derive state exclusively from authenticated canonical L1/blob
inputs. There are no contract, proof, blob-format, or protocol-pin changes.
Dispatch and reconciliation events now provide the integration tests and real
builder canary with attempt identity and user accounting, without inspecting
private disk state or adding a debug RPC.

## Authoritative definitions

The core protocol remains pinned to
`3c0cdf4955941a5c23639346932b55b33babebbd`, with OpenZeppelin at
`5fd1781b1454fd1ef8e722282f86f9293cacf256`.

- [Core specification](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md),
  G.1–G.5: independent immediate execution, forward consumption, same-block mutable
  execution gates, and queue replacement on every verification.
- The same specification, H.9: discarding unconsumed proven entries is a liveness
  choice; state safety is enforced at consumption by the current-root check.
- [EEZ implementation](../../../eez-core-protocol/src/EEZ.sol),
  `postAndVerifyBatch`, `_findMatchingEntry`, `_entryMatches`, and
  `L2ExecutionPerformed(uint64 indexed rollupId, bytes32 newRoot)`.
- [Composer specification](../../../crates/eez-control-rpc/SPEC.md), section 8:
  canonical evidence and atomic ownership transitions govern reconciliation.
- [Proof-signer specification](../../../crates/eez-proof-signer/SPEC.md), sections
  7 and 11: complete immediate prefixes and authenticated native blob data.

The historical report records the separate Flashbots/rbuilder allowlist and
invalid-nonce omission checks. The builder pin remains unchanged.

## Verification

Final verification results and executable hashes are recorded in
[default-validation-evidence.json](default-validation-evidence.json).
The final implementation passed:

| Check | Result |
| --- | --- |
| Workspace libraries, proof signer and protocol vectors | 804 passed across 19 suites |
| Native partial consumption, restart, reorg and resumption | 10 passed |
| Python CI and canary tests | 178 passed, 4 existing skips |
| Integration contracts | 19 passed |
| Workspace Clippy, all targets, Rust 1.94.0 | Passed with warnings denied |
| Formatting, diff whitespace and Chiado blob configuration | Passed |
| Dependency audit and deny checks | Passed against cached advisories; 14 allowed audit warnings |
| Kurtosis with the pinned real builder | Restart and nonce-replacement partial-consumption canary passed |

The Kurtosis restart preserved the prior canonical partial prefix and safe-head
hash. A second canary after restart produced `Committed`, `SourceNonceSpent` and
`SourceReverted` dispositions, preserved the partial prefix, then settled a fresh
healthy transaction. All three runtime binaries installed in the running
containers were matched by SHA-256 to the native build. Follower convergence was
tested by the native cases. These were locally built
optimized development binaries in an Ubuntu 24.04 validation container. An
ordinary four-operation, two-post wave also passed before the final removal of
unused serialization traits. The temporary validation enclave was stopped after
its evidence was saved.

The new regression crashes the Composer with a retry prepared but unposted,
reopens the same chain databases, resubmits the omitted user, and checks prefix
preservation, successful settlement, fresh retry accounting and clean-follower
convergence. The embedded L1 is allowed to persist the settled prefix before the
crash so the test isolates loss of local attempt metadata.

The first restart run exposed an unposted speculative Sync block that blocked
emission after the attempt ledger was lost. The boot rewind above addresses that
failure. Other review checks cover lock ordering, rollback after late validation
errors, stale observers, nonce gaps, newer-attempt ownership, same-height resumes,
reorg invalidation and finality cleanup.

A separate final review inspected the removal diff and the complete feature diff
against the original `main`. It found no remaining concrete defect or unrelated
implementation change. The native-tested source digest and binary hashes are
recorded in the evidence. The only source edits afterward corrected two stale
durability doc comments and a mock callback error string; all 89 L1 unit tests
and workspace Clippy passed again. Protocol gitlinks and dependency versions were
unchanged. The unchanged full
protocol Forge and broader state-chaining suites were not rerun for this removal;
their earlier results remain in the historical report.

The existing source-RPC latency limit remains: serial source reads must complete
within a stable canonical generation. Large batches against a consistently slow
RPC can leave reconciliation pending. This change adds no throughput claim or
restart guarantee for unsettled transactions.
