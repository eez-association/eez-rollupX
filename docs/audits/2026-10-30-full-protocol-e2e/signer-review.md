# Signer soundness review

Status: interim review; final source and network qualification remain outstanding.
This is a separate manual adversarial review of the authorization path, not an
external audit or a claim that positive E2E coverage proves soundness.

Scope: changes from fork `c702f90da19cc149890d0a17a4ef6c200dcb6a21`, with protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f` unchanged. The current runtime source
snapshot is recorded locally in `candidate/version8-process-source.json`; a final
committed revision must replace that local-only identity before this review closes.

## Authorization trace

`service/settlement_job.rs::settle_validated` checks candidate L1 context, native
DA, reconstructed semantic tables and execution effects before deriving observation
claims. It checks observation replay, cancellation and deadline again before
constructing `AttestationMaterial`. Scratch calldata and rollback spans are local
products of `fifo_observation_claims`; neither is a new RPC authorization input.
Lookup shape classification alone cannot authorize signing.

`validate/stateless.rs::replay_observations` resolves pins only against validated
canonical prefix/block commitments and requires FIFO ordering. It rebuilds a
sparse trie from the already accepted terminal witness and authenticates the
ancestor header chain before exposing `BLOCKHASH`. Missing account, storage and
code evidence remain errors; no chain RPC or zero-value fallback supplies them.

`eez-driver::observation` requires successful authenticated manager delivery for
mutable lookups, then independently compares the exact target frame's result.
The inspector matches proxy caller, target, value and calldata. A manager revert
cannot stand in for a claimed target revert. Static callbacks are matched by
caller, source-chain destination, calldata, static mode and zero value; unmade
or additional callbacks reject. Deeper static source reads have separate bound
claims. Calls inside mutable entries retain that entry's execution context.

Each scheduled observation produces one outcome; the driver rejects an unconsumed
schedule. Source rollback groups run within one canonical prefix in disposable
state. Reads reach the authenticated canonical database; scratch writes cannot
become canonical block changes. Nested rollback restores journals in reverse
order and reloads cache entries from the reverted bundle before canonical state.

## Findings and adversarial evidence

| Finding or threat | Disposition and evidence |
| --- | --- |
| Snapshot amplification: one full cache copy per nested observation scope | Replaced with REVM bundle revert positions. Shared-boundary 1,024-scope regression passes in the signer package. No full account-cache copy is retained per scope. |
| Natural failed-root retry rejected by signer accounting | Reproduced the rejection, then replaced the signer's separate partition rule with the shared complete-group helper. The positive retry and unused-slot rejection pass; all 123 signer settlement tests pass. Final runtime qualification remains due. |
| Forged target result copied consistently into native semantics and settlement | `forged_native_lookup_outcome_reaches_observation_replay_and_never_signs` reaches `observation_replay`, rejects and asserts zero signing calls. |
| Missing bytecode used only by scratch manager execution | `missing_lookup_manager_code_reaches_observation_replay_and_never_signs` removes the implementation code; canonical replay and semantic binding pass, observation replay rejects and signing remains zero. |
| Missing destination account proof | The backend regression explicitly removes exactly one destination leaf and requires `incomplete account witness`; it separately tests missing bytecode and an unknown pin. |
| Manager failure confused with target failure | Authenticated replay accepts the fixture's exact target `deadbeef` revert only after manager success; forged target success and delivery-table commitments reject. |
| Earlier-state observations accepted after history import or donation | Real EVM and service regressions reject both earlier states and require zero signing calls at the service boundary. |
| Reused mutable row changes callback execution or reverted commitment | Shared materializer and inverse-verifier regressions cover successful reuse after rollback, changed callback calldata and changed reverted trace commitment. These table regressions do not replace live execution. |
| Fixture regeneration hides an incomplete witness | The lookup witness was supplemented only after reproducing all 45 captured blocks exactly from genesis. Expected proof bytes were unchanged by supplementation; positive service replay and missing-code rejection both pass. |

The current affected-package run passes 337 signer tests. Five explicitly ignored
fixture-generation/inventory utilities are not validation coverage. The standalone
protocol run passes 222 unit tests and 15 call-pattern tests, plus the independent
public-input and rolling-hash vectors. First failures remain in the audit logs.

## Follow-up review prompted by protocol qualification

The network campaign found redundant forced rollback wrappers on naturally
reverting callbacks. The correction coalesces only a complete child range with
an already open natural failed-frame scope of the same parent, chain and end.
Partial inner ranges remain explicit: their state rollback can affect subsequent
callbacks. This changes canonical materialization, not the authority of a decoded
outcome. Exact native ABI binding, authenticated target execution and observation
result comparison remain mandatory before attestation. Both root/nested failures
and both origins are covered by the new table regression; the partial-range
regression checks that the middle callback still rolls back before the third.

The manifest's root-only cardinality assertion contradicted the shared projection
for recursive static pools. Removing that assertion does not remove forest/table
validation: `composition_fifo_actions` validates exact mutable/static coverage;
manifest user identity, physical checkpoint coverage and transaction ownership
checks remain. The new manifest test checks that recursive observations neither
advance mutable source indices nor invent producing effects.

A process regression also found checkpoints retained after their temporary
reentrant session was replaced. Checkpoint ownership matters even when concrete
snapshot types happen to match. The dispatcher now retires those descendants'
checkpoints at outer-session restoration and retains the actual outer anchor.
The focused before/after test reproduces the ownership mismatch and verifies a
later enclosing rollback still restores the outer state. Final process, signer
adversarial and complete network validation of these follow-ups is outstanding.

## Required closure

Recheck the final diff after remaining changes. Review complete-group selection
versus legacy multi-host partitioning, static candidate selection and repeated-row
rollback boundaries against the pinned resolver. Finish current-runtime process
regressions, full CI qualification and all 64 Kurtosis scenarios with independent
follower replay. Bind final findings to the committed source and exact binaries;
positive network results cannot replace the adversarial checks above.

The four follow-up pinned-actor process regressions passed, including canonical
settlement and independent fresh follower replay. Workspace library/binary checks
passed, including the forged-outcome and missing-manager-code pre-sign rejection
regressions. Full protocol qualification of a sealed updated release remains open.


## Static candidate reuse follow-up (uncommitted, validation in progress)

The complete f091f437 campaign exposed a conflict between occurrence-based
static row traversal and the pinned 1 → 2 → 1 local-write scenarios. The first
construction regression failed with three rows instead of two; after candidate
deduplication the inverse verifier still rejected valid reuse. The correction
resolves candidates against fixed host hashes and mutable cursors, independently
recomputes callback commitments, and memoizes only within a pending read.
Coverage is applied only to the successfully selected callback graph. It does
not authorize an outcome without the existing execution/observation replay.

Protocol unit validation now passes 229 tests with no exclusions. Bidirectional
1 → 2 → 1, 1 → 1 → 2 → 1 and 1 → 2 → 2 → 1 permutations pass, and forged cached
outputs, success bits, callback identities, callback commitments and mutable
callbacks reach specific independent-verifier rejections. A 40-level shared
callback graph resolves with 41 memo entries instead of exponential traversal;
a cyclic graph rejects before any coverage is established. The signer-authorizer
regression constructs its tables independently of the materializer. Its run and
the unchanged pinned-actor process regressions are still pending. Review pool
candidate scanning, repeated-row boundaries, stack bounds and final runtime
behavior before closing this finding. No full-suite qualification is claimed.

One remaining review target is `resolve_static_callbacks`: its pool-to-host path
uses `next_static_row`, which selects the first matching key, whereas pinned
`EEZ.staticCrossChainCall` scans subsequent candidates when callback hashes do
not match. The new `StaticRowResolver` handles that scan for paired mutable
hosts, but does not establish parity for the separate pool path. Determine
whether a valid pool callback can require a later host candidate and add a
focused reproducer before treating that path as fully supported. This is an
open completeness question, not evidence of an authorization bypass.

The independently assembled signer-authorizer fixture now passes its focused
run: one positive reused-candidate case and three mutations reach
`ExecutionTableMismatch` before authorization (cached return bytes, callback
commitment and callback calldata). This is an authorizer-unit check, not a new
service-level zero-signing-call test. The existing service-level adversarial
checks must still run against the final source. The composer budget boundary
check also passes for 32/64 allowed frames and rejects subsequent execution.
Logs: `candidate/revision-f091f437/static-row-reuse-signer-authorizer-final.log`
and `quote-budget-boundary-test.log` (local-only).

The follow-up stack probe also passes on a 2 MiB thread stack at depths 64, 256,
512 and 1,024 in three modes: all static, all mutable, and a mutable root with
static descendants (which exercises the new paired-row resolver). Depth 1,025
rejects at materialization in every mode. The exact current protocol library was
rechecked with Cargo; source, binary, rlib and result digests are retained in
`candidate/revision-f091f437/nesting-stack-followup-identity.json` (local-only).
This establishes the tested stack bound, not a general complexity guarantee;
the existing memoization/DAG and cycle regressions supply separate evidence.

### Follow-up findings after full package validation

**Cached-depth bound:** a new adversarial DAG test reproduced a bound bypass in
`StaticRowResolver`: it first resolved a long suffix through a short path, then
reused that suffix through a longer prefix. The old memo returned the row without
counting the suffix depth. The corrected memo retains the maximum descendant
height and checks it at every reuse. The failing-before and passing-after logs
are `static-memo-depth-before.log` and `static-memo-depth-after.log` under
`candidate/revision-f091f437/` (local-only). Cycle and shared-DAG checks also pass.
This was a verifier-bound inconsistency; no signing bypass was demonstrated.
Native semantic materialization independently enforces its own nesting bound.

**Static pool versus paired-host candidates:** the pool concern above was traced
through canonical construction and independent binding. Each top-level static
source read advances the L1 host's `CALL_BEGIN`/`CALL_END` hash, although L2's
mutable source cursor remains unchanged. Local writes between those reads
therefore do not share a host-row position. Repeated callbacks within one read
can reuse a row, but all those callbacks execute under static context. The new
`static_pool_callbacks_reuse_within_a_read_and_separate_local_write_contexts`
regression checks two callbacks per read across 1 → 2 → 1, distinct host keys,
unchanged source cursors and rejection of a row moved into another read's
position. Nested local writes inside a mutable host are the separate paired-row
case handled by candidate scanning. Native semantic reconstruction and complete
row coverage reject unobserved alternative rows; the pool binder is not a generic
interpreter for arbitrary extra contract-table rows. Existing authenticated
execution/observation replay remains mandatory before signing. No additional
pool-path extension was needed for these canonical behaviors.

The first complete protocol package run also exposed two test fixtures introduced
by this effort that used identical calldata at every recursive depth. Those
fixtures described cyclic lookup keys rather than their intended finite callback
chains. They now distinguish the remaining depth, as the unchanged pinned
`readChain(uint8)` actor does. All original table-shape and mutation assertions
remain; the explicit cycle-rejection test remains separate. The corrected run
passes 230 protocol unit tests and 16 call-pattern tests, along with codec and
independent commitment vectors. Final-source reruns after the small shared-row
lookup cleanup and exact release qualification are still required.

The signer/RPC/client package run before the cached-depth correction passed
339 signer tests (five explicit fixture utilities ignored), seven RPC tests and
34 prover-client tests. Both forged native outcome and missing-manager-bytecode
service tests reached `observation_replay` and asserted zero signing calls. This
is supporting evidence, not qualification of the later memo-depth edit.
