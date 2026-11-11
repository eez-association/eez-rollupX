# Combined review: history, protocol balances and source identities

Historical baseline evidence and subsequent design decisions. These results do
not validate the changed runtime; see the
[current implementation audit](../2026-10-27-group-cleanup-and-history/README.md).

| Field | Value |
| --- | --- |
| Date/status | 2026-10-26; review and focused baseline validation complete; proposed protocol changes are not implemented |
| Reviewed fork | `6c3106fb482bb323eefa8898a0bd922fc7520480`, PR #103 on `codex/fee-vault-upgradeable` |
| Issues' implementation baseline | `c1dafa6113425fce74bdba8c32ae8b0d7a7ba5ae`; relevant history, native execution, source identity, system reconstruction and signer checkpoint code is unchanged at the reviewed fork |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and clean in the validation worktree |
| Draft | `2c6d346bea0b3736f7453c35174291495694d1fc`; issues #5/#6 were also read as dependencies of #7 |
| Core reference in draft issue #7 | `143d87e62cade98bfc122d612638dcaaf1c85633`, 20 commits behind the pin; `EEZL2.sol`, `EEZBase.sol`, `CORE_PROTOCOL_SPEC.md` and `EXECUTION_ENTRY_SPEC.md` are identical between these revisions |
| Environment | Rust 1.95.0, Foundry 1.7.1, Solidity 0.8.34; locked Rust dependencies |
| Evidence | [Validation results and source identities](validation.json); no network transactions, deployment or GitHub discussion edits |

## Follow-up: 2026-10-27

The owner selected complete-group cleanup as the preferred candidate for #105.
The [funding follow-up](../2026-10-27-group-funding/README.md) establishes that
canonical groups receive their complete funding without earlier residual balances;
cleanup implementation and checkpoint/replay validation remain separate work.
The recommendation below records the original 2026-10-26 review.

Later on 2026-10-27, the owner selected the
[block-wide source-index amendment](../../proposals/native-system-transactions.md#source-identity-simplification):
`H(abi.encode("SOURCE_ENTRY_V1", rollupId, parentBlockHash, entryIndex))`,
with one original assigned index per rollup across all batches in an L1 block.
An allocation/partial-resumption contract was recorded there as proposed
implementation, with explicit pending acceptance gates. This superseded this
audit's original recommendation to defer an indexing decision; it does not
retroactively validate the new design or implement the rolling-hash substitution.
The pinned core still resets its scan cursor for every batch. Existing execution
tests in this audit do not validate the proposed allocation mechanism.

The owner subsequently made the dependency boundary explicit: **never modify
`eez-core-protocol` contracts**. The core-changing allocator/ABI proposal was
withdrawn and the background implementation instructions corrected immediately.
Both the review and PR #103 worktrees retained the clean `84a5b520...` core
checkout/gitlink; no core source/ABI edits, core branch or dependency PR had been
made. The formula/index direction now requires a design using existing extension
points, with unchanged core contracts and no unauthenticated counter or silent
change to manifest-local indexing. `OfflineRollup.getCustomData` can bind
rollup-defined context through the current core public-input formula, but its
STATICCALL cannot allocate state. A RollupX-owned atomic posting integration is
an avenue to evaluate, not validated implementation evidence. #105/#108 work
continues independently.

The owner then challenged the need for allocation machinery itself. A further
source check confirmed that no production consumer requires an L1-allocated
block-wide number: source hashes select candidate inputs, followed by exact-byte
and canonical checkpoint validation. Same-context source identities are not a
global deduplication guarantee in the existing contract. Both allocator
implementation paths were stopped. A local original-stream index and an ordinal
across all separate batches are different semantics; the latter remains a pending
direction, not grounds to introduce new contract state as a correctness fix.
This reassessment is static evidence; it makes no runtime source-identity change.

The owner subsequently allowed additional sourceHash inputs if the four-field
formula cannot satisfy the requirements. The active proposal now investigates
original batch-local positions with existing deterministic context and explicitly
documents the scope change; its obsolete allocator requirements were removed.
Read-only review selected the original pre-group L2 prefix commitment as the
additional field: it excludes the current group and is already independently
replayed. The local index is the complete original mutable `PostBatch.entries`
row: anchor at 0, one row per outbound host, then all inbound mutable roots
including failed/lookup rows. L1 static rows are separate and emit no native
transaction, so appending a later candidate does not shift an earlier native
row index. This mapping was checked against `entries/materialize.rs`, Composer's
final L1 assembly, and `blob_semantics/reconstruction.rs` settlement binding.
The proposal now fixes the preimage and the original-prefix/reproof rules.
Incremental construction and resumed follower placement still require runtime
implementation and tests; this record does not claim those have run. Core,
production source identities and deployed networks remain unchanged.

The two Solidity paths in `validation.json` were also corrected to
`src/L2/EEZL2.sol` and `src/base/EEZBase.sol`. All four compared objects were
explicitly checked for existence at both revisions and compared again; they are
identical. The earlier path-only diff had not established object existence.

## Original recommendation

Keep continuous ordinary L2 production, the collector's no-progress handling,
the current beginning-of-block cleanup and current cross-chain source identities.
The dedicated optional history transaction is a reasonable separate amendment
for the next fresh network. Its main benefits are an explicit history-only
format and removing empty import transactions; it is not required to fix the
collector or provide the already implemented late-L1 production behavior.

| Issue | Disposition | Reason |
| --- | --- | --- |
| [RollupX #104](https://github.com/eez-association/eez-rollupX/issues/104) | Keep superseded; do not implement the production pause or unique-origin requirement | Closed as not planned and explicitly replaced by #108. Requiring fresh external evidence before the affected L2 block conflicts with the owner's continuous-production requirement. Preserve its still-relevant DA and anchor analysis. |
| [RollupX #105](https://github.com/eez-association/eez-rollupX/issues/105) | Retain existing cleanup policy | This proposes a different execution policy, not a demonstrated violation. Per-group cleanup is feasible if fully specified, but changes observable balances, funding lifetime and checkpoints. |
| [RollupX #107](https://github.com/eez-association/eez-rollupX/issues/107) | Adopt the tested application guard | Implemented as a reference in #103; the external deployed collector still needs adoption/redeployment. The history getter stays strict. |
| [RollupX #108](https://github.com/eez-association/eez-rollupX/issues/108) | Complete the execution specification, then implement separately if selected | Compatible with continuous production, delayed PREVRANDAO, v2 transport and offline proving. It needs a schedule and format change across all consumers. |
| [specs-draft #7](https://github.com/eez-association/specs-draft/issues/7) | Defer both the rolling-hash substitution and index change | Neither establishes a missing correctness guarantee in the current fork. Inbound and outbound operation cardinalities differ; the draft and RollupX already use different source formulas. |

Issue text is proposal/discussion evidence, not authorization to silently replace
the accepted execution contract. No new consensus defect is confirmed by this
review. That conclusion is limited to the inspected paths and tests below.

## History: optional advancement and application availability

The current profile already permits retaining the previous origin when newer
headers are unavailable. It records an empty transaction at the scheduled point.
Newer evidence received during replay cannot rewrite that decision. The late-header
regression executes the repeat, ordinary intervening blocks and a later advancing
import. This is deterministic replay of an explicit choice, without requiring a
uniquely freshest choice from independently arriving observations.

For #108, omission becomes the recorded no-advance choice. A present update must
carry a nonempty bounded consecutive range, occur at transaction zero at an
eligible timestamp, and select matching header/opcode PREVRANDAO. Specify:

1. Eligibility from the fixed genesis schedule and timestamp. The current
   [contract](../../../contracts/src/L2/L1BlockhashContract.sol) and
   [Rust schedule](../../../crates/eez-protocol/src/l1_history.rs) require
   `nextCutoff` to advance even on empty imports. Merely deleting the empty
   transaction leaves later blocks/imports invalid. Preserve the schedule's
   genesis-relative phase and checked arithmetic through skipped opportunities.
2. Explicit validation of inherited history and PREVRANDAO on an omitted update,
   including empty blocks. Current [begin](../../../crates/eez-evm/src/history.rs)
   defers this check to the mandatory history transaction at scheduled positions.
   Relaxing only the missing-transaction rejection would bypass that check.
3. The numeric type/version, canonical integer bounds, receipt format and derived
   RPC fields. Execution must preserve metering, zero fees, zero value, no system
   nonce increment, successful execution with no output/logs and bounded parsing. Public submission
   and pool paths must reject this protocol-only type.
4. Exact ordinary-prefix DA, pending simulation, observation replay and checkpoint
   behavior with both present and absent updates. Update all exhaustive type
   consumers and reject the old history form in the new fresh-network profile.

The new history type can omit `sourceHash`: authority comes from verified context,
position, parent state and ancestry, while exact typed bytes and the block bind
the occurrence. This does not remove cross-chain source identities. Omission
allows the origin/randomness to remain old; freshness is not a new consensus
guarantee. Delay alone is also not finality. Known conflicting ancestry still
needs the existing reorg/recovery treatment.

The prover can retain [v2's existing block RLP and L1-header witness](../../../crates/eez-control-rpc/proto/prove.proto).
The supplied hash list is not self-authenticating: complete contiguous header
evidence must connect it and the inherited origin to the settlement anchor.
Live cross-chain settlement retains actual-parent/time binding. Ordinary catch-up
can retain the historical anchor mode and bounded parts; histories need not be
selected as origins to be necessary links in the evidence chain.

#107 remains independent. Its selector is `L1BlockNotImported(uint64)`, including
before-bootstrap requests and missing covered records as well as future records.
Catching only that selector therefore still cannot classify every failure as
temporary. The [reference collector](../../../contracts/src/L1BlockHashCollector.sol)
checks `current()` within the transaction, returns `(false, 0)` only when ahead,
and keeps unexpected failures visible. Its constructor validates the start.
This is an immediate successful application return, not a sleep or production
pause. The same pattern is needed with #108 because imports remain delayed and
optional. Prior read-only incident evidence is in the
[polling audit](../2026-10-26-l1-history-polling/README.md); this review did not
repeat live queries or modify the external application.

## Cleanup: a policy choice with execution consequences

Pinned core explicitly pools funding across loads. RollupX funds complete tables,
retains balances within a block, removes a failed native call's temporary credit,
and clears SYSTEM/EEZL2 at the next block start. These are distinct operations.
The accounts being cleared do not include FeeVault or ordinary users.

#105 correctly observes that cleanup could be included before a complete-group
checkpoint. It is not inherently incompatible with block-hash commitments, and
the objection to invisible end-of-block changes does not prove it impossible.
However, it needs a new shared execution rule: after the outbound load/user pair,
or after a complete inbound delivery, never between load and user or inside an
inline callback. Failure/halting paths, tentative-group rollback, gas treatment,
state-root capture and partial replay must all agree. Transfers to the accounts
outside those groups would still need a stated lifetime/cleanup rule.

Retaining the current policy is the smallest coherent choice and matches the
owner's earlier cleanup decision. The core tests establish that replacement
retains funding; they do not prove that every RollupX workload needs that funding
or that a future complete-group policy is impossible. No candidate implementation
of #105 was tested here.

## Source identities: two independent proposals in specs #7

**Indexing.** RollupX already uses original `(manifestIndex, topLevelCallIndex)`
for inbound identities. [Reconstruction](../../../crates/eez-protocol/src/blob_semantics/reconstruction.rs)
numbers roots before filtering failed/static work and derives identities before
selecting a consumed suffix. The regression includes two identical successful
roots preceded by failed and static roots: their identities differ, and removing
the original bracket is correctly not equivalent to resuming its suffix.

A single original per-rollup entry ordinal could replace that tuple only after
defining exactly which entries count. Mutable rows, static rows, reverted roots,
synthetic anchors, grouped outbound hosts and physical L2 transactions are not
interchangeable units. `entryCount` is a total, not a unique occurrence index.
The complete original numbering must survive partial consumption. These indexes
are derived hash inputs, so flattening them saves no bytes in the current envelope.

**Outcome binding.** The current final L2 rolling hash includes call identity and
surviving observations, so using it for an inbound source identifier is plausible;
it must be independently verified, not trusted as a claim. It does not commit to
every state effect or every saved answer. In particular, an outgoing source entry
with no incoming callbacks can retain its seeded L2 rolling hash while its saved
`returnData` changes; `_executeEntry` checks the hash before returning that data.
The full native calldata still differs. This is a source-level observation about
the hash's coverage, not a newly reproduced proof bypass.

For inbound entries, a changed `rollingHash` already changes ABI calldata and
the full native transaction hash, even though the occurrence `sourceHash` remains
the same. Full block and public-input commitments continue to bind the candidate.
The substitution therefore needs a concrete identity requirement beyond claiming
that current results are unbound. The [follower](../../../crates/eez-deriver/src/deriver.rs)
uses source identity to locate a possible retained suffix, then reconstructs and
compares its exact bytes before treating it as already applied. The
[signer](../../../crates/eez-proof-signer/src/settlement/da.rs) independently compares
every reconstructed native transaction against validated execution. Source-hash
equality alone does not authorize different execution data.

For outbound work, one native load covers a complete signed user transaction,
potentially multiple mutable/static roots and inline callbacks. There is no
single entry-level final L2 rolling hash for that load. Preserve the current
signed-origin transaction binding; an additional outcome commitment would need a
defined ordered aggregation and rules for static, reverted and zero-event paths.
Do not substitute the L1 rolling hash or a block hash containing the transaction
being identified. Follow derivation through native bytes, execution and checkpoints
before claiming non-circularity for any adopted formula.

## Combined scope, compatibility and acceptance

The proposals should not be bundled as five bug fixes. #108 replaces #104;
#107 remains an application requirement under either history representation.
#108 leaves history outside the scope of specs #7, which then concerns only
cross-chain operations. #105 can change balance-dependent call results, while
#108 can change transaction positions/gas and origin-dependent results. If #7
is adopted too, settle those execution rules before publishing identity vectors.

For the recommended scope, leave #103's treasury/reference collector unchanged,
specify #108 as a separate fresh-network amendment, and retain current funding and
cross-chain identity rules. Update the draft's stale ABI/hash descriptions against
the pinned core instead of adapting the implementation back to version-0 prose.

Any accepted execution/identity change requires the
[validation-contract version](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md#versioning-and-operations),
genesis/profile where affected, portable vectors and Rust/C# integration contract
to agree. Keep the `prove.v2` transport and a single selected profile; fresh-network
scope does not require legacy decoding or a migration implementation. Applying
these changes to an existing network would require a separate coordinated
activation/hard-fork design. The #107 application guard alone does not.

Before adopting #108, require combined producer/follower/offline-signer checks
for omitted updates followed by later imports, bounded multipart catch-up,
malformed/oversized/duplicate/off-schedule updates, inherited/wrong PREVRANDAO,
actual-parent and historical anchors, missing ancestry, and reorg invalidation.
Exercise history plus ordinary writes and static/lookup observations, then full
outbound load/user and inbound groups with partial settlement. Every checkpoint
and final block hash must agree; reject forged earlier-state observations.

## Executed checks and limits

All tests ran against the unchanged `6c3106fb` source in the isolated PR #103
worktree. This documentation-only review has its own worktree and branch;
PR #103 and the operator checkout were not edited.

| Command | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-protocol --lib native_source::` | 2 passed, 206 filtered |
| `cargo +1.95.0 test --locked -p eez-protocol --lib blob_semantics::reconstruction::tests::` | 6 passed, 202 filtered |
| `cargo +1.95.0 test --locked -p eez-evm --lib` | 37 passed, none filtered |
| `forge test --root contracts --match-contract L1BlockHashCollectorTest -vv` | 8 passed |
| `forge test --root eez-core-protocol --match-contract EEZL2TableReuseTest --no-match-path 'script/*' --no-match-test 'test_Doc_OriginalValue_SeedInSetup\|test_PostCost_Incremental'` | 8 passed; the core script/named exclusions and other unselected suites are not coverage |

All 61 selected tests passed, with zero failures or ignored/skipped selected
tests. Core compiled 109 files with Solidity 0.8.34; existing warnings were
reported. These are unit/embedded-EVM semantic checks, not a new process, live
network, independent signer replay or validation of unimplemented proposals.
No full CI/workspace/E2E rerun was needed for this documentation-only review.
Logs remain local-only under `artifacts/combined-issues-review-20261026/` in the
validation worktree. [validation.json](validation.json) records commands, counts,
source/executable identities and digests; full logs are unavailable in a fresh
checkout.
