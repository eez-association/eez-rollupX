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

The final representation cleanup starts from `44ed95b`. It keeps semantic forests
local to manifest construction, retaining only signed user identity, effect
ranges, endpoint hashes, outbound group identity and producing inbound call
coordinates. Static and reverted lookups are still validated, and non-producing
mutable entries still count toward source queue positions. Checkpoint ordering,
coverage and whole outbound-group validation remain unchanged. Stored attempts
and pending snapshots share one record type with an already-sealed parent header;
snapshots remain detached clones and ownership updates remain transactional.

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

Verification results and executable hashes are recorded in
[default-validation-evidence.json](default-validation-evidence.json). Its
`representation_cleanup` entry records this follow-up; earlier entries retain the
journal-removal results separately.

| Check on the representation cleanup | Result |
| --- | --- |
| Workspace libraries, proof signer and protocol vectors | 806 passed across 19 suites |
| Focused manifest and Composer tests | 7 and 69 passed |
| Native partial consumption, restart, reorg and resumption | 10 passed |
| Workspace Clippy, all targets, Rust 1.94.0 | Passed with warnings denied |
| Formatting and diff whitespace | Passed |

The focused regressions cover lookup-only users in held-pool order, mutable queue
positions after static and reverting lookups, complete multiroot outbound-group
endpoints, inconsistent checkpoint positions, malformed semantic forests, and
independent pending snapshots with sealed parents. Existing cases cover stale
observers, retry limits, nonce gaps, atomic rollback, reorg invalidation and
finality cleanup.

The malformed-resumption native test initially failed its error assertion during
its deliberate L1 reorg. The node correctly rejected the sidecar-free suffix and
recovered, but a concurrent rescan read the removed block before the fixture
stopped serving its canonical blob sidecar. The assertion already allowed the
same race before block-body retrieval. It now also allows the missing-blob error
for exactly the injected transaction, block and known blob hashes, only during
reorg recovery. Rejection before the reorg and healthy settlement afterward are
still required, and unrelated errors still fail. The initial failure is retained
in the evidence. The remaining cases used the same verified runtime binaries;
the test fix changed no runtime source.

The preceding journal-removal revision passed the following checks; they were
not rerun for the representation cleanup:

| Earlier check | Result |
| --- | --- |
| Python CI and canary tests | 178 passed, 4 existing skips |
| Integration contracts | 19 passed |
| Chiado blob configuration | Passed |
| Dependency audit and deny checks | Passed against cached advisories; 14 allowed audit warnings |
| Kurtosis with the pinned real builder | Restart and nonce-replacement partial-consumption canary passed |

That Kurtosis restart preserved the prior canonical partial prefix and safe-head
hash. A second canary after restart produced `Committed`, `SourceNonceSpent` and
`SourceReverted` dispositions, preserved the partial prefix, then settled a fresh
healthy transaction. All three runtime binaries installed in the containers were
matched by SHA-256 to that native build. These were locally built optimized
development binaries in an Ubuntu 24.04 validation container. The temporary
validation enclave was stopped after its evidence was saved. The unchanged full
protocol Forge and broader state-chaining suites were not rerun for these
simplifications; their earlier results remain in the historical report.

The native restart regression crashes the Composer with a retry prepared but
unposted, reopens the same chain databases, resubmits the omitted user, and checks
prefix preservation, successful settlement, fresh retry accounting and
clean-follower convergence. The embedded L1 persists the settled prefix before
the crash so the test isolates loss of local attempt metadata. This regression
originally exposed the speculative Sync-block problem addressed by the boot
rewind above.

A separate final review checked this follow-up against `44ed95b` and the complete
PR scope against the original `main`. It rechecked the protocol consumption
coordinates, materializer/FIFO validation, endpoint coverage, snapshot isolation
and transactional ownership updates. The test assertion race above was the only
finding requiring a change. No concrete issue remains in the reviewed cleanup.
The source digest and tested binary hashes are recorded in the evidence.
Protocol gitlinks and dependency versions are unchanged.

The existing source-RPC latency limit remains: serial source reads must complete
within a stable canonical generation. Large batches against a consistently slow
RPC can leave reconciliation pending. This change adds no throughput claim or
restart guarantee for unsettled transactions.
