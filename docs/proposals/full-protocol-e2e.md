# Complete pinned protocol E2E support

| Field | Value |
| --- | --- |
| Status | Scope authorized by the owner on 2026-10-30; implemented and locally qualified on 2026-10-31. All 64 pinned scenarios, independent signer regressions and cold follower replay pass; see the audit for exact source and validation limits. |
| Fork baseline | `c702f90da19cc149890d0a17a4ef6c200dcb6a21` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Related issues | #85, #81, #13 |

## Scope and acceptance

RollupX must execute all 64 E2E scenarios discovered at this protocol pin through
Composer, independent signing, native blob settlement and follower replay. The
existing runner selects only 34 by default. The upstream exclusion comments
describe pending integration, not permission to omit those cases from this work.
Keep their assertions and the protocol pin unchanged. Contract-only Anvil passes
and expected production rejections cannot replace intended-success network cases.

| ID | Requirement | Acceptance |
| --- | --- | --- |
| R1 | Complete, unique scenario accounting | All 64 pinned scenario paths and source hashes retained; every scenario executes exactly once per qualification run, no exclusions/skips/missing results |
| R2 | Faithful nested/static/failed-call behavior | Exact return/revert bytes, callback observations, rollback state, table cursors and canonical effects match scenario assertions |
| R3 | Independent signing authorization | Witness-backed outcomes, canonical table/data binding and public inputs independently checked; malicious mutations reject at the intended gate before signing |
| R4 | Canonical replay | Independently reconstruct native blobs and reproduce safe block hashes/state through follower replay, preserving partial consumption and reorg behavior |
| R5 | Regression and integration qualification | Applicable existing CI lanes, all protocol Anvil scenarios and full Kurtosis semantic suite pass on identified final source/binaries |

## Design direction

Extend the existing shared entry materializer and independent verifier to mirror
the pinned contracts' frame and cursor rules. Preserve complete load/user groups,
bounded parsing/work, independent L2 observation execution, exact commitments,
and the separation between L2 truth and L1 execution-time checks. Reuse the
existing semantic blob and execution machinery instead of adding a second
implementation selected by mode. Remove restrictions only alongside the checks
that validate the newly admitted behavior. Replay rollback locally to each frame;
never flatten nested groups or infer success from receipt presence alone.

Static alternatives require their entire callback/result commitment, not just
lookup-key deduplication. Mutable retry limitations documented by the core remain
limitations of the pin; supported E2E cases must not be confused with those
unrepresentable patterns. Investigate any specification/scenario disagreement
before changing behavior.

### Nested rollback requirements

The recorded action stream must retain every properly nested source
`Snapshot`/`Revert` scope, including scopes sharing their first action. One
optional span per action loses that information. Source blob encoding must
round-trip those boundaries; destination call-array markers follow the pinned
`TableGenerator._closeRollback` rules, which close inner scopes first and omit
an already reverted prefix when assigning an outer marker.

Session checkpoints must remain usable by an enclosing scope after an inner
rollback while their owning session remains live. Resuming an outer session
retires checkpoints belonging to temporary same-chain reentrant sessions; their
writes are already owned by the suspended EVM journal. Retain the outer session
anchor for later enclosing rollback. Concrete snapshot type equality does not
establish ownership. When several scopes queue restoration before the next dispatch, the
earliest applicable checkpoint for each destination must win. Current queue
insertion order and consuming checkpoints need regressions for both inner-then-
outer restoration and continuation between the two. Restoring source rolling
hashes, source cursors and destination state are separate obligations.

The inverse verifier must bind failed-call descendants before accepting their
terminal revert and must enforce nested rollback boundaries without treating
reverted execution as absent execution. Independent observations must reproduce
transient state within the enclosing call or rollback group; replaying each
child at the canonical prefix is insufficient.

Lookup-only failed calls with descendants replay the checked execution table
through the authenticated L2 manager in discarded scratch state. This uses the
protocol's callback and rollback execution. Replay must separately
observe the intended target frame's outcome: a manager validation error must
never count as the target's claimed revert merely because both transactions
failed. Group scheduling and exact table coverage must derive from bound native
semantics, with no extra Composer-controlled authorization path. The audit records local regressions and complete network qualification.

For a failed inbound root, preserve its full L1 callback table even though the
L2 execution is noncanonical. The shared materializer's destination entry already
separates the root target outcome from delivery completion: its rolling hash
binds the actual failed target frame, while the delivery header permits the
manager's validation to complete successfully. Scratch replay can use that
existing representation and separately capture the real root target frame.
Require both successful manager validation and the exact captured target
success/output; a manager revert must never stand in for a target revert.
Derive the scratch table and funding through shared protocol builders from
bound native actions. No Composer-supplied replay program may authorize signing.
Outer source rollback groups use scratch-state checkpoints across root
observations; individual canonical-prefix calls cannot reproduce those transient
writes. Derive observation span counts from the bound native action ranges,
keep all scopes within one canonical prefix, and restore nested snapshots in
reverse order. A separate disposable State overlay reads through the canonical
state for witness collection and prevents temporary commits from contaminating
canonical execution. The final audit records passing full network qualification.


### Cursor and parser requirements

The pinned `EXECUTION_ENTRY_SPEC.md` explicitly leaves the mutable reentrant
cursor unchanged for static reads. The materializer must
not prevent a failed mutable row from being selected again after an intervening
static read. Cover that sequence with both an identical retry and a conflicting
cached outcome; table traversal order is not authority to skip the first
matching mutable candidate.

Static candidate reuse must also preserve the pinned local-write sequence
1 → 2 → 1 in both directions. Equal position keys are not enough to merge rows:
callbacks, callback-result commitment and cached outcome must all match. The
inverse verifier resolves candidates against fixed read-only host contexts and
memoizes only within one pending read; cyclic graphs reject and shared subgraphs
must not expand exponentially. Coverage of a row does not advance the static
cursor. Positive permutations and forged outcomes, callback commitments and
identities must reach the independent verification checks before authorization.

The nested rollback decoder collects inner-to-outer spans and reverses once at
the transaction boundary, preserving canonical order without repeatedly shifting
vectors for snapshots sharing a first action. Keep exact
Snapshot/Return context validation and round-trip evidence.

### Partitioning simplification

Complete source groups already establish partition boundaries when their count
equals the settlement-host count. Validate every host's shape first, then use
those boundaries directly; computing discarded call-hash cursor counts adds work
without strengthening this decision. Unequal counts retain cursor-based
partitioning. Stream group metadata through the shared helper instead of building
temporary vectors in Composer and reconstruction. Successful partitions already
cover the supplied source groups, so callers need not repeat that sum check.
Successful nested calls do not need snapshots used only after failure. The signer's fallible load
decoding and independent execution binding remain mandatory.

This cleanup preserves accepted inputs, error precedence, wire formats and
signing identity. Qualification compares exact results against the previous
implementation and measures ordinary, static, rollback and split-host workloads
with the same release profile. See the [performance record](../audits/2026-10-30-full-protocol-e2e/performance.md).

## Compatibility and recovery

This extends the accepted proof/execution profile without authorizing a protocol
pin, Solidity ABI or native transaction format change. The validation identity
must change when validation semantics change. Old signers may reject newly
supported tables and must not be mixed with producers publishing those tables.
Upgrade follower/reconstruction and signer support before enabling producers.
Retain decoding/replay of previously supported data under unchanged execution
rules; if this cannot be maintained, document the concrete fresh-network or
activation boundary before implementing it. No generic legacy mode is planned.
Once newly admitted data is canonical, a downgrade to a follower lacking its
support cannot safely replay it. Restarts retain the existing volatile-queue
limits; clients may need to resubmit unsettled work.

RPC validation version 8 identifies the extended runtime acceptance rules. It
remains separate from the deployed signing domain, whose encoded version stays
7. Compatible protocol-support extensions do not revoke existing signatures:
the registered verification key, attester, contracts and genesis stay unchanged.
Network, registry and proof-system bindings remain mandatory. Only an explicitly
incompatible rule change or a need to revoke old attestations warrants on-chain
key replacement; this PR establishes neither.

Upgrade Composer, signer and followers together, drain/discard old proving
sessions and rebuild requests with complete witnesses. The caught-lookup replay
now reads authenticated manager state absent from older request witnesses. Old
followers cannot replay newly admitted shapes, so downgrade after their canonical
inclusion is unsupported. This is a compatibility policy, not a rehearsed live
migration of an arbitrary older network. Existing pre-baseline manager/genesis
boundaries still apply.

## Evidence

The [audit](../audits/2026-10-30-full-protocol-e2e/README.md) records baseline,
implementation, independent soundness review and validation separately. The final candidate section records completion evidence and explicit limits.
