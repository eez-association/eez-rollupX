# Witness-feed receiver closure assessment (#141)

## Scope and provenance

Investigation for [#141](https://github.com/eez-association/eez-rollupX/issues/141),
not a recovery implementation. Its completion criterion remains:

> Document receiver-close behavior and a supported diagnostic recommendation, with any functional defect tracked separately.

The requested evidence is receiver closure in produced/derived paths, accepted-work
ownership, proving recovery, and existing task lifecycle reporting. The saved N7
review establishes discarded send failures, not a permanent settlement stall.
The original review's input provenance was **not independently checked**. N7
remains source-inspection-only evidence: no fresh runtime tests, vulnerability
scan or performance measurements were part of that review.

| Evidence | Repository revision | Protocol gitlink |
| --- | --- | --- |
| Frozen N7 source | rollup0 `1b66d9bda675e6f8e83f514153a97136909acc06` | `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Frozen N7 target | rollupX `31e148f50dd79730b17d113ac78795eb377eb5c5` | `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Issue's later code check | rollupX `716a8a3bad4415ade7e9a6bc3af4a1b35419b232` | same target pin |
| Fresh investigation / branch base | rollupX `20243f70839ec69df9312731e960cd311e060050` | same target pin |

The dedicated worktree initially contained clean `bad8d7eef7e16ac872f354d31af257f8646b2fac`
with an uninitialized submodule. It was switched to the requested main revision;
the exact protocol and nested submodules were initialized without edits. The
candidate adds tests and this record only. No wire, storage, validation, queue,
production code, dependency or protocol change is proposed by this PR.

Fresh observations below are separate from N7. The two frozen committer files
were retrieved at their exact revisions to recheck the narrow finding: target
[derived send](https://github.com/eez-association/eez-rollupX/blob/31e148f50dd79730b17d113ac78795eb377eb5c5/crates/eez-driver/src/block_committer.rs#L811)
and [sequenced send](https://github.com/eez-association/eez-rollupX/blob/31e148f50dd79730b17d113ac78795eb377eb5c5/crates/eez-driver/src/block_committer.rs#L892). This does
not revalidate the original review inputs or the full #133 inventory.

## Findings and recommendation

**Disposition: partially covered.** Exact witness lookup, on-demand capture,
parent persistence, and panic reporting already exist. The committer still lacks
a direct diagnostic for a closed witness receiver. A permanent settlement stall
is not established by the send failures or by this investigation.

Recommend a separate, diagnostic-only change: report
`eez.committer.witness_feed.send_failed` at ERROR with the exact block number/hash
and which commit path attempted delivery. Suggested message:

> Witness-feed receiver closed; block is already canonical, background capture was not notified.

Only log an attempted send failure; `witness_sender=None` or
`feed_witness=false` is intentional suppression. Keep the successful commit
result. At this point returning an error would report
a failure after canonicalization, without undoing the block. Do not claim that
proof generation or settlement has stopped. Do not mark users complete, charge a
retry, evict requests, silently reconnect, or restart the node from this event.
Normal shutdown can also close the receiver; correlate with lifecycle logs.
Rate limiting repeated events can be reviewed in that implementation if needed;
this assessment adds no diagnostic framework or configuration.

The [source implementation](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-driver/src/block_committer.rs#L551)
already logs a failed send, but its “settlement will stall” wording is stronger
than this fork's evidence supports. Do not copy that wording or its hash-only
channel type into rollupX.

## Current production flow

References in this section describe the fresh base above; test additions do not
change the cited production functions.

1. [`Actor::process_sequence`](../../../crates/eez-driver/src/block_committer.rs)
   resolves the payload, receives valid newPayload and head forkchoice responses,
   updates both local head mirrors, then sends the exact `L2Endpoint`. The send
   result is discarded and the caller receives `Ok(CommitOutcome)`.
2. `Actor::process_derive` likewise sends only after successful canonicalization
   and head updates. It sends only when `feed_witness=true`: produced prebuilt
   Sync blocks, Composer replacement paths, and the Deriver's
   `discard_stale_history_suffix` (a newly executed empty sibling) use this.
   The Deriver's `replay_block` uses false for follower/L1 replay. A false flag
   is intentional suppression, not failed delivery.
   Both paths leave safe/finalized unchanged; a later command still executes.
3. [`run_capture`](../../../crates/eez-node/src/witness_source.rs) owns the sole
   receiver. Capturing/persisting a witness runs on the blocking pool. A real
   capture failure records `capture_error`, logs
   `eez.node.witness_capture.retry`, and retries that exact endpoint. Even a
   blocking capture worker panic is converted from its join error into this
   retry path; it does not itself drop the receiver. A dropped
   or explicitly closed receiver is a different condition: no new endpoint
   reaches this loop and the send itself never updates store health. A previously
   recorded capture error also is not cleared by receiver closure.
4. [`NodeWitnessSource::block_witness`](../../../crates/eez-node/src/witness_source.rs)
   first reads the exact number/hash from the durable store. A miss re-executes
   from the local provider, validates identity, and persists the result. Corrupt
   records are errors, not misses. Recovery requires retained block, parent
   state and ancestry; channel closure cannot manufacture missing data.
5. [`Composer::block_attributes`](../../../crates/eez-composer/src/composer.rs)
   persists the exact non-genesis parent before advancing ordinary production.
   The proof construction path also collects exact witnesses on a blocking
   worker; historical terminal blocks use the same source. Thus background
   notification loss alone is insufficient evidence of a permanent proving
   stall. If required local data is gone, the source returns an error and no
   valid proof can be inferred.
6. [`run_preparation_worker`](../../../crates/eez-node/src/lib.rs) consumes optional
   bounded preparation notifications. This optimization can be lost with capture;
   complete witness backfill remains the proving contract. Startup explicitly
   queues the current non-genesis head and checks that send. It does not establish
   an automatic running-task restart or replay of every lost notification.

The applicable contracts are the accepted
[offline production and ownership profile](../../proposals/l1-origin-and-offline-proving.md#clock-settlement-and-ownership),
[streaming recovery contract](../../../crates/eez-control-rpc/STREAMING_SPEC.md),
[RPC failure and ownership rules](../../../crates/eez-control-rpc/SPEC.md), and
[signer request lifecycle](../../../crates/eez-proof-signer/docs/request-lifecycle.md).
Signer sessions are volatile; Begin/backfill and full validation remain necessary.
No signer chain RPC, validation bypass or durable pending-request queue is implied.

### Accepted-work ownership

The witness channel carries block identities, not held user requests or their
ownership tokens. Neither send mutates the held pool. Commit success does not
mean canonical L1 consumption, proof success or request completion.

Source tracing in `Composer::compose_sync_slot` shows a store-health error defers
new Sync work before draining it. In the preparation path, witness-source errors
propagate as construction failures: `eez.composer.phase2.prepare_failed` requeues
survivors without charging a settlement attempt and returns an empty-slot block.
Prover failures separately retain the complete range and source ownership.
Submitted attempts continue to require canonical reconciliation, including partial
consumption. This ownership result is a **source trace**, not an injected live
queue/settlement experiment.

On process restart, held requests and retry counters remain in memory only and
can be lost; clients may need to resubmit. Persistent witnesses do not provide
durable queue recovery. A panic-driven shutdown cannot be presented as transparent
accepted-work recovery.

### Lifecycle reporting and existing logs

Node wiring uses `spawn_critical_task("eez-witness-capture", ...)`. The pinned
[Reth runtime](https://github.com/paradigmxyz/reth/blob/3d592ece6de8c4559987416a544fc215fd6d6921/crates/tasks/src/runtime.rs)
wraps critical futures in `catch_unwind`: a panic logs `Critical task
\`eez-witness-capture\` panicked`, sends `TaskEvent::Panic`, and reaches the task
manager. A normal return or cancellation is not a panic report. The finished
critical-task counter also covers completion/drop; it does not identify the lost
block or independently prove capture health. The receiver is not respawned here.

The following log inventory is source inspection, not an assertion that a
tracing sink emitted each event in these tests. The lifecycle test observes the
public task-manager error. Existing signals are useful but not substitutes for
the missing send diagnostic:

| Condition | Existing signal |
| --- | --- |
| Block/state capture error while task runs | `eez.node.witness_capture.retry`, store health error |
| Unwound queued block | `eez.node.witness_capture.unwound` |
| Store health prevents Sync work | `eez.composer.witness_resources.unavailable` |
| Optional preparation deferred | `eez.node.witness_preparation.deferred` |
| PostBatch construction fails | `eez.composer.phase2.prepare_failed` |
| Backlog exceeds configured threshold | `eez.composer.settlement.backlog`; not a cause attribution |
| Capture future panics | Named Reth critical-task error and task-manager event |
| Send to closed receiver after canonicalization | No committer event or commit error |

Normal capture return requires channel termination (or a capture error with a
closed channel). Production does not expose `Receiver::close()` or retain a
capture JoinHandle for a routine abort operation. Tests inject these conditions
to characterize them, not to claim they commonly arise during normal service.

## Investigation tests and limits

The added tests characterize unchanged production behavior; they are not
regressions for a new recovery implementation. The existing `test_committer`
replaces command handling and never reaches the real send sites, so these tests
mock only the external Engine/payload-builder responses instead.

| Check | Evidence and boundary |
| --- | --- |
| `produced_commit_survives_closed_witness_receiver` | Real `process_sequence`, mocked Engine and payload builder; open receiver control must receive exact number/hash. Closed receiver still returns success, updates head, leaves safe/finalized intact, and permits a subsequent refresh. |
| `derived_commit_survives_closed_witness_receiver` | Real `process_derive`; open/closed receiver crossed with true/false feed flag, same head and subsequent-command assertions. No execution engine or L1 network. |
| `closed_witness_receiver_lifecycle_and_retained_store` | Real `run_capture`, Reth critical-task wrapper and temporary MDBX witness store. Explicit receiver close/normal return, task abort, and panic injected at the capture floor callback. Tests named panic reporting versus no panic event for normal return/abort; closure alone leaves store readiness unchanged. Exact retained witness remains readable, missing provider data returns an error, retained data is not deleted. |

The lifecycle test's stored witness is an existing synthetic unit-test fixture.
It does **not** claim independent execution validity or generate a signed proof.
The mock provider intentionally has no missing block/body/state; the error is
observed, not repaired by fabricating a witness. The on-demand success path with
real retained state is source-traced, not freshly proven by this test. The task
abort case tests cancellation/drop even if cancellation precedes its first poll;
it is not a claim about cancelling an already-running blocking worker. After
normal exit/abort, a named sentinel panic must be the task manager's next error;
this verifies monitor behavior without a sleep or an unprocessed-event snapshot.
Dropping a capture future also does not force-stop a blocking capture already in
progress; that worker can still finish using its cloned store/provider. No
mid-execution worker cancellation or shutdown-drain guarantee is tested here.

[#86](https://github.com/eez-association/eez-rollupX/pull/86) is related historical
evidence, not receiver-closure coverage. Its merge is
`4f58341cef307b95dbf6ac46ffc33220c8789b0c`. It fixed witness ancestors selected
from the wrong canonical branch, with three regressions in
[`witness.rs`](../../../crates/eez-driver/src/witness.rs):
`witness_headers_follow_the_parent_hash_with_competing_branches`,
`witness_headers_reject_missing_or_inconsistent_parents`, and
`witness_headers_preserve_the_complete_bounded_blockhash_window`.
Its PR records a 92-pass local cross-chain run at `b8848a0` with three manual
history exclusions, after an earlier 91-pass/1-fail run. Those are historical
claims and local-only artifacts; they are not a fresh pass at this revision and
do not cover receiver closure.

## Follow-up boundary

No new functional defect is established here. A dead background receiver with
otherwise usable state is observable degradation, not proof of failed settlement.
If receiver loss is later reproduced leaving an unsettled exact block without
reconstructible state, track it separately from #141: preserve the first missing
number/hash, capture/store errors, retained ancestry, canonical L1 consumption,
held-pool ownership, and executable identity. Reproduce with real state and
prove whether parent persistence and on-demand collection can recover it.

Any automatic capture restart, shutdown-on-close, admission health change,
notification replay, or durable queue recovery would change behavior and requires
separate scope approval and acceptance tests. This PR implements none of those.
It also does not modify or retry the parent chat's combined CI run `43188514265`,
operate deployments/wallets, or use another investigation's resources.

## Reproduction and executed checks

Commands run from the dedicated repository root, using source-built Rust 1.95.0
(`59807616e1fa2540724bfbac14d7976d7e4a3860`), LLVM 22.1.2, protoc 3.21.12,
and the locked dependencies. `/tmp/issue141-target` is an isolated target directory
on tmpfs; the repository filesystem was low on space. No shared target directory,
service, test port, wallet or datadir was reused.

```bash
export CARGO_TARGET_DIR=/tmp/issue141-target
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
cargo +1.95.0 test --locked --offline -p eez-driver --lib block_committer::
cargo +1.95.0 test --locked --offline -p eez-driver -p eez-node --lib
cargo +1.95.0 fmt --all --check
CARGO_INCREMENTAL=0 cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings
```

The first committer run executed both new send-site tests against unchanged
production code: **5 passed, 0 failed, 0 ignored, 50 filtered out**. This is a
characterization baseline, not a before/after recovery fix. There is no production
behavior change to compare. The initial formatting check failed on the newly
added test formatting; `cargo fmt --all` corrected it and the repeated check
passed. The first full-package attempt stopped at compile error E0616: the new
lifecycle test accessed Reth's private `PanickedTaskError::task_name`. It was
corrected to assert the named prefix of the public `Display` output. No test
results were produced by that failed attempt, and no production code
or expected behavior was changed for either correction.

The tests contain semantic assertions about commit results, exact block identity,
head state, lifecycle reporting, and witness-store access. They do not execute
independent EVM replay or an end-to-end signed proof. No process/Anvil/Kurtosis
suite, live receiver-failure injection, vulnerability scan, throughput benchmark,
production deployment or wallet operation was run. Protocol/ABI/genesis and signer
validation are unchanged, so their full integration matrices are outside this
investigation. In particular, historical #86 passes are not substituted for any
of these unrun checks.

Local raw build logs and initial failed formatting output are retained under
`/tmp/issue141-evidence/` for this session only; they are not durable/shared CI
artifacts. The committed validation summary records selected tests, statuses,
source and executable identities. The [Ponytail review](ponytail-review.md) records
scope, corrections and remaining limitations.

### Unit results (2026-11-12)

| Run | Driver | Node | Outcome |
| --- | --- | --- | --- |
| Initial full-package compile | Not executed | Not executed | E0616 in the new lifecycle test; corrected as described above |
| Full packages, restricted sandbox | 76 passed | 88 passed, 16 failed | All three new tests passed; 16 existing HTTP tests could not bind local sockets (`EPERM`) |
| Same command, loopback permitted | 76 passed | 104 passed | Zero failed, ignored or filtered; exact same executable hashes as the restricted run |

The loopback rerun is not represented as a clean first-run pass: those 16 tests
passed only after the environment restriction was removed. Their first failures
are retained, and the two runs' executable hashes match. No intermittent
application failure was inferred from this controlled environment difference.
The full driver run freshly passed all three #86 ancestry unit regressions; the
old cross-chain process suite was **not** rerun.

### Final static checks

Workspace Clippy (`--locked --offline --workspace --all-targets -- -D warnings`)
passed with `CARGO_INCREMENTAL=0`. Its first attempt failed on the `/tmp`
per-user disk quota before completing checks. Only this task's disposable
incremental compiler cache was removed (about 1.1 GiB); test binaries and logs
were retained. No other checkout or task's files were removed. Formatting,
`git diff --check`, local documentation link resolution, and `bash -n` on the
reproduction block passed.

[validation.json](validation.json) contains the test-source and executable
SHA-256 hashes, log digests, statuses and the 16 initial socket-denied test names.
Investigation status: complete within the recorded unit/static evidence scope.
The diagnostic implementation is a recommendation, not an implemented fix;
real-state proving recovery and live ownership observations remain unverified.
