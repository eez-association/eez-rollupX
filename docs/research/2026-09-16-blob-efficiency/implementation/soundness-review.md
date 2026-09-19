# Derivable ChainOperation signer soundness review

Date: 2026-09-17. Scope: the signer, preparation/cache path, producer integration,
and follower reconstruction changes on branch `feat/derivable-chain-operations`
relative to fork `origin/main` at `fc0ee0516a1dbbdf9f785a3fa3d2f1789b6d2af7`. This is the focused
change-correctness review required by CO-53. It is not a general security review
of the repository, dependencies, protocol contracts, key management, host, or
deployment.

The reviewed implementation files are enumerated by
[`implementation-source.sha256`](implementation-source.sha256); that manifest's
SHA-256 is `d5d983f626294111ae4a4d7f724c015f02502b5a64f84be3ec814d2de707fa74`.
Any later source edit invalidates this checkpoint until the manifest and focused
review are repeated.

## Conclusion

No unresolved signer soundness finding remains in the reviewed change. The new
path does not add a signing authority: it changes how ordinary-block execution
facts are obtained and retained, while the final request still runs every
request-specific settlement, DA, semantic, observation, deadline, and digest
gate before the only attestable value can be constructed.

The conclusion is conditional on the trusted inputs below and the pinned source
and dependency identities. It does not authorize a deployment. Operational
capacity, pruning policy, chain configuration, and production rollout remain
separate decisions.

## Trusted inputs and untrusted inputs

Trusted inputs are the startup-loaded L2 chain rules and canonical validation
identity, rollup ID, L1 posting rules, proof-system identity, system address, and
distinct attestation key. The signer process and pinned Stateless/Reth execution
implementation are part of the trusted computing base.

All RPC fields, block RLP, witnesses, cached-range declarations, v3 recipes,
PostBatch calldata, blobs, sidecars, semantic claims, lease IDs, retry history,
and producer cache hints are untrusted. Cache membership, a preparation response,
a follower result, or a producer readiness flag is never authorization to sign.

Stateless replay authenticates execution against the supplied pre-state and the
validated parent chain. Canonical fork choice is committed through the existing
settlement root-update chain and posting context; Stateless alone does not select
canonical history. Physical blob commitments bind the exact published v3 bytes
into the independently recomputed public-input digest.

## Authorization argument

For every ordinary block, cold validation exact-decodes canonical block RLP,
checks the declared number, parent and block hash, recovers senders, validates
consensus and execution with the supplied witness, verifies receipts, gas,
requests and the post-state root, and checks state-root continuity. Reconstruction
then authenticates the parent header and replay pre-state, assembles a complete
block from explicit execution inputs plus actual replay outputs, forbids hidden
state-provider reads, and requires byte-identical canonical RLP and hash equality
with the validated block.

Preparation runs that same execution and reconstruction boundary and then runs
the complete intermediate-block gate. Only then can it create an immutable
`PreparedOrdinaryBlock`. That fact retains the validation identity, full block
identity and ancestry, pre/post roots, exact reconstruction data, recovered
senders, receipt results, state checkpoints, and settlement evidence needed by
later gates. Preparation has no PostBatch input, cannot construct
`AttestablePublicInputsHash`, and has no path to `Attester::sign`.

Range resolution is bounded by the final format count before traversal. It
selects facts by validation identity and full block hash, then rechecks number,
declared endpoints, parent links and state-root telescoping over the complete
range. Lease IDs affect eviction ownership only. Expiry, cache loss, eviction,
repair and retry can cause an explicit miss or additional validation; they cannot
create a fact or widen acceptance.

The final request always supplies and freshly validates the terminal block and
its observation witness. It combines cold and prepared ordinary facts only after
checking full-window ancestry and roots, then reruns the full intermediate gate,
terminal framing, root-update chain, effect authorization, v3 DA position and
environment binding, unchanged sidecar/group rules, native semantics,
witness-backed observation replay, cancellation and deadline checks. Only after
those checks does `validate_and_settle` privately construct
`AttestablePublicInputsHash`; `Attester::sign` accepts only that type.

Therefore cached validation is a refinement of complete authenticated execution:
it can reuse an immutable block-local fact in the same validation namespace, but
it cannot skip a request-specific authorization gate. For the same v3 request,
cold, warm and mixed paths must produce the same digest and recovered attester.

## Change invariants reviewed

| Invariant | Enforcement | Discriminating evidence |
|---|---|---|
| Only `ChainOperation.operations` changes; cross-chain bytes/order and terminal/sidecars stay exact | v3 payload owns only ordinary representation; existing terminal, entry and group encodings feed the unchanged gates | captured mixed inbound/outbound/caught-revert fixture, sidecar/group mutations, full cross-chain process suite |
| Every omitted ordinary output is independently reproduced | Stateless output plus authenticated parent feeds the shared assembler; complete RLP/hash equality is mandatory | `reconstruction_fact_is_built_only_from_exact_replay_outputs`, independent typed/reverted/7702 fixtures, clean-follower maximum runs |
| A syntax-valid recipe or stub backend cannot fabricate reconstruction authority | reconstruction constructor is private to the Stateless path; final DA requires a fact for every position | `successful_backend_without_reconstruction_cannot_authorize_v3`, environment/count/transaction/terminal mutations with zero sign calls |
| Cached facts cannot authorize a different request or chain | identity/full-hash keys, endpoint/count/parent/root checks, final DA/environment and settlement gates | cold/warm/mixed digest equivalence, changed cached environment rejection, cache-miss identity/membership tests |
| Terminal execution and observations are never cached | cached terminal is rejected structurally; final terminal carries a full witness; observation replay precedes attestable construction | `cache_miss_is_structured_and_cached_terminal_is_never_accepted`, forged lookup outcome rejection with zero sign calls |
| Resource ownership cannot become signing authority | leases only increment bounded cache reservation counts; all facts were already validated; proof pins outlive lease expiry | overlap/expiry/exact-replacement tests and cache-miss repair tests |
| Cache and lease transitions are atomic | all refcount changes are prevalidated; failed release restores the lease | `exact_lease_replacement_preserves_overlapping_reservations`, `failed_refcount_transitions_leave_leases_and_entries_unchanged` |
| Preparation cannot starve or weaken final proving | one preparation stream, one final request, shared fair worker; preparation releases it after each block; final holds it through all gates | process suites with active background preparation, timeout/cancellation worker-lifetime tests |
| Missing speculative preparation data cannot deadlock reconciliation or soften final validation | exact `(number, hash)` applied floor stops catch-up; transient prewarm predecessor miss only retries; final witness/cache miss remains authoritative | partial-consumption process suite and exact-floor Node tests |
| Larger limits remain bounded and negotiated | 65,536 format/request ceiling; byte/item/message/deadline limits unchanged; preparation partitions by the intersection of all advertised quotas | 65,536/65,537 boundaries, per-quota grouping and individually oversized-block rejection |
| No rejected or cancelled request signs | private attestable type after all gates; cancellation/deadline checks before construction and signing | attester call counters across mutation, observation, cancellation, timeout and detached-worker cases |

## Findings and dispositions

1. **Attestable value existed before observation replay.** The earlier design
   checkpoint identified that the control flow rejected bad observations but the
   private type was constructed too early. `validate_and_settle` now performs
   observation replay plus cancellation/deadline checks first and constructs the
   type afterward. Captured forged-observation RPC evidence proves zero signing
   calls.
2. **Preparation grouping initially considered only the 64-block bound.** A legal
   long range could exceed byte, per-message, or witness-item quotas. The client
   now computes each prefix using the intersection of every negotiated limit and
   rejects a single block that cannot fit. Unit tests isolate every limit.
3. **A signer restart or lease expiry made release fail as if durable source data
   were bad.** Exact `FAILED_PRECONDITION` release loss now clears only local
   cache/lease hints and rebuilds from durable witnesses. Other failures retain
   their closed error behavior.
4. **Reservation changes could have partially mutated refcounts after an internal
   invariant failure.** Every transition is now completely prevalidated; failed
   release restores its removed lease. Overlap, exact replacement, and injected
   invariant-failure tests establish all-or-nothing behavior.
5. **Producer prewarming could demand a standalone witness for a synthetic,
   partially applied prefix and block reconciliation.** The worker now stops at
   the exact canonical floor. A temporarily missing predecessor is only a
   prewarming retry and refreshes the durable watermark. Corruption, invalid
   ancestry, and final proof witness misses remain hard failures.
6. **The first native harness opened a second preparation lease beside producer
   prewarming, and retained-prefix measurements did not release abandoned leases.**
   The harness now consumes producer-prepared facts for live runs and explicitly
   releases each retained prefix. This corrected measurement authority without
   changing signer authorization.
7. **A restart backlog larger than one format window could retry forever, and a
   same-floor branch replacement could extend the stale branch lease.** The
   ancestry walk now retains the oldest bounded prefix with constant maximum
   memory, records whether it joined the exact prepared endpoint, holds one
   maximum lease until the authenticated floor advances, and releases/rebuilds
   on a branch mismatch. The regression covers bounded monotonic prefixes,
   competing ancestry and a synthetic partial floor.
8. **Idle leases were not renewed and a signer restart could leave the producer
   retrying a stale lease.** The producer now renews the exact retained ranges
   every 30 seconds both while idle and while holding a full window. A missing
   signer lease clears all client hints and immediately rebuilds the last durable
   endpoint. Preparation failures trigger an immediate exact renewal probe so a
   lost lease is distinguished from real cache capacity exhaustion. Client RPC
   tests cover successful coalesced renewal and restart-lost renewal; the signer
   scheduling test proves a queued final proof receives the next block boundary.
9. **The bounded queue could consume a newer queued endpoint and then overwrite
   it with an older overflow watermark.** Once overflow exists, all later
   endpoints now replace that watermark until the worker takes it. Notification
   holds the shared watermark lock across observation, nonblocking queue send,
   and overflow publication, so cloned producers cannot reopen the ordering
   race. The exact A/B/C and deterministic concurrent-clone interleavings have
   regression tests.
10. **Restart did not notify preparation about already durable history.** Startup
    now submits the persisted canonical head to the same capture path used for
    new commits. Existing evidence is reused; a missing final capture is rebuilt
    while provider state is still available, and the durable endpoint then wakes
    preparation without waiting for another block.
11. **Publish plus lease extension used multiple expiry snapshots.** Cache
    publication and reservation now expire leases once and use the same instant
    for all checks and mutations. An internal expiration refcount failure retains
    the lease instead of silently discarding ownership.
12. **The follower expanded the accepted sparse plan into legacy parallel vectors
    and retained an unreachable local-default replay branch.** Runtime decoding
    now returns the bounded `Payload` directly and reconciliation consumes its
    allocation-free `BlockPlan` iterator. Fresh and resumed reuse both require
    complete published inputs and canonical ancestry; no local cadence or default
    header fields participate. Tags 0 through 2, `DecodedBatch` and their codec
    feature were deleted; every shipping consumer uses `Payload::decode`. Fresh
    replay reuses the terminal block already admitted by that bounded decoder.
    Replay transaction lists retain the exact admitted bytes through
    reference-counted buffers instead of copying their contents. Codec, deriver,
    clean-follower, bidirectional cross-chain and same-height resumption
    regressions pass on the pinned source.

## Residual limits

This review cannot prove absence of implementation defects in Rust, the pinned
execution engine, or cryptographic libraries. It establishes that the new code
preserves the signer authorization structure and that identified change-specific
failures are resolved with discriminating tests. Changing chain rules, validation
identity construction, the reconstruction fact fields, final gate order, cache
keying, or the private attestable boundary invalidates this review and requires a
new focused soundness check.

The retained native corpora and raw maximum-run logs are currently local artifacts;
their hashes and structured results are committed, but reproducible distribution
is an operational evidence follow-up. Production emission/deployment is not
performed by this branch.
