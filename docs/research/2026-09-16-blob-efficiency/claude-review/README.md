# Claude review and independent disposition

Completed 2026-09-16. The user authorized sharing the proposal and a bounded
85-file source snapshot with Claude. Claude Code 2.1.273 reported the model
`claude-opus-5[1m]`. The first round used only Read/Glob/Grep on the snapshot;
the second had no tools and challenged specific findings using additional code
evidence. Claude could not edit the working repository. No runtime was changed.

The revised normative documents are the
[implementation contract](../../../proposals/derivable-chain-operations.md) and
[coverage ledger](../../../proposals/derivable-chain-operations-coverage.md).
The ledger had 48 rows at review completion; the subsequent user requirement
for signer soundness added five further gates (CO-49 through CO-53). Those later
additions were not part of the Claude review. All implementation rows remain planned. This review improves the design; it does
not certify unimplemented code or establish measured production performance.

## Evidence and process

- [Frozen input manifest](input-manifest.json): SHA-256 of the original proposal,
  ledger and provided source files. Base repository was
  `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`.
- [Initial request](prompt.txt), [Claude's first review](round-1.md), and
  [raw read-only review stream](round-1-stream.jsonl).
- [Specific challenges](round-2-prompt.txt), [Claude's response](round-2.md), and
  [raw second result](round-2.json).
- [Independent local findings](local-findings.md), recorded before Claude's
  completed review. No deployed exploit is claimed.
- [Review metadata](status.json) records the initial automatic-approval rejection
  and subsequent user authorization/completed runs.

Claude returned 16 findings. Seven recommendations were accepted, six accepted in
narrowed/corrected form, and three proposed simplifications declined. Several
valuable observations were paired with incorrect repairs or claims stronger than
their evidence.

## Finding-by-finding disposition

| Finding | Disposition | Evidence and resulting decision |
|---|---|---|
| F1: cached facts omit outbound-event evidence | **Accept; important schema gap.** | `verify_validated_intermediate_blocks` rejects observed outbound events independently of system-sender flags. Cache complete `SettlementBlockEvidence` and decoded transaction count; run the same gate before preparing ordinary facts and over the final intermediate set. This is a proposed-path omission, not proof of a deployed bypass. CO-35. |
| F2: final proving may exceed the slot budget | **Accept, narrow claims.** | Deadlines were already release gates, but no explicit cost model or calibration decision was specified. Add whole-request costing, measured release timing and an early cold-path checkpoint. The 4-second/200-ms values cited by Claude are examples/test profiles, not verified live settings. Preserve timing validation and document Future/Live changes. CO-37/44. |
| F3: remove signer-side assembler equality | **Decline the removal.** | Stateless validates many fields, but Claude had not inspected the pinned dependency and its proposed comparison list omitted unsupported optional-field absence checks. An honest producer using the same builder is not a validation premise. Retain shared assembly using already validated outputs; this is not a second EVM execution. Require exhaustive pinned Header field classification before a later field-only optimization. No demonstrated dependency cycle was found. CO-07/48. |
| F4: remove caller environment and cache generation from preparation | **Accept.** | Prepare actual validated facts and their supported input descriptor; compare final environments against those facts. References bind hashes and validation identity. Separate retention lease IDs control only resource lifetime. CO-16/40. |
| F5: replace sparse gaps with flat block counts | **Decline as the chosen format; retain as a baseline.** | Flat counts are a viable simpler alternative, but consume about 65.5 KiB at the maximum span: 8.6%/25.8%/51.6% of six/two/one useful blob capacities. The user's objective is capacity-driven settlement, so today's one-blob cost floor is not the reason to dismiss byte savings. Keep sparse records and exact size/property tests. Claude conceded this is a tradeoff, not a defect. |
| F6: environment changes force all later blocks into full exceptions | **Accept.** | Add kind-2 records installing an environment for their own block and subsequent gaps. They consume one ordinary position and may contain an empty transaction list. The old environment applies to the preceding gap; full exceptions do not change it. Summaries carry environment state, and terminal promotion cannot delay a change. No new Sync boundary is created. CO-36. |
| F7: volatile restart can cause long cross-chain unavailability | **Accept risk, narrow remedy.** | Durable facts are a possible solution, not a demonstrated necessity. Add a 600-second baseline recovery gate on named release hardware, tightened by stricter deployment requirements; require improvement if volatile re-preparation fails. This does not relax steady-state posting deadlines. Reporting a long outage alone cannot pass. CO-38. |
| F8: LRU can evict the pending proof window | **Accept risk; reject signed-root eviction rule.** | Signing or relay acceptance is not canonical settlement. Use bounded window leases, overlap accounting, expiry and request pins; LRU only unreserved facts. Refuse preparation at capacity rather than repeatedly evicting its prefix. Resource hints confer no canonical authority. Durable witnesses remain retained through finalization. CO-40. |
| F9: one RPC chunk per cached block adds avoidable overhead | **Accept.** | Use cached range references. Charge logical counts before traversing; verify local hash links, numbers, endpoints, adjacent chunks and validation identity. Compact references do not bypass evidence checks. CO-39. |
| F10: UNAVAILABLE can retry unchanged cache misses | **Accept, correct integration.** | A status change alone is insufficient: FAILED_PRECONDITION currently maps to cross-chain Actionable or Backend. Add a distinct CacheMiss detail/error and intercept it before either path. Preserve existing outbound/inbound variants; reject unrelated repair hints. Never evict a source for cache absence. CO-41. |
| F11: retention/speculative-depth consequences need explicit treatment | **Accept consequences, reject “depth zero required.”** | Existing configuration permits finite depth >= N+K-1. Add actual anchor-state/history availability tests with pruning/restart/reorg, and document safe-head lag and rollback exposure. No live pruning misconfiguration was demonstrated. CO-42. |
| F12: remove reader activation checks | **Decline as a necessary change.** | Writer-only enablement is an alternative coordination policy, not a correctness repair. Keep the shared timestamp acceptance rule and mixed-version rollout tests. Claude withdrew the assertion that it protects nothing. CO-29. |
| F13: resumed v3 must have zero ordinary blocks | **Accept need for clarity; reject zero-count rule.** | Resumption is an L1-applied outcome discovered after signing. The original payload may contain ordinary history; changing it would change signed DA and reject competing-prefix outcomes. Specify proof anchor versus applied prefix root, canonical H-1 parent and retained Sync replay. Claude withdrew the zero-count requirement. CO-43. |
| F14: conservative admission can hide byte gains | **Accept.** | Known supported inputs use derived/environment-record bounds; unknown exceptions and terminal data use full bounds. Do not reserve fictitious full headers per empty block. Measure the real admission path and stopping reason. CO-24/25/33. |
| F15: identity digest and connection behavior are unspecified | **Accept.** | Define a canonical, domain-separated identity with golden vectors. Reuse tonic channels and refresh capabilities on connection/reconnection or identity/limit changes, not every proof. Requests still check their expected identity. CO-45. |
| F16: follower replay is long and can leave partial progress | **Accept.** | Add early/middle/late crash injection at the largest legal span, idempotent resumption and measured recovery bounds. Keep environment/ancestry-aware reuse and safe-head checks. This is a rollout obligation, not a reason to publish redundant headers. CO-46. |

## Additional local corrections

**Legal benchmark geometry.** A 65,536-block wire ceiling does not imply an exactly
65,536-block post between same-residue Sync boundaries. At K=6 the maximum is
65,532; at K=5 it is 65,535. Exact codec/ingress limits and legal native posting
maxima now have separate tests. Genesis offsets and resumed placement use the
existing selector. CO-47.

**Witness identity versus availability.** Height-keyed UPSERT and a canonical-hash
check prevent using a stale witness, but can lose an earlier branch's only durable
witness after replacement and pruning. Require hash-keyed retention, a finalized
height index, transactional migration, bounded garbage collection and an
A -> B -> pruned restart -> A test. This is a guarantee to establish, not a
reproduced live outage. Do not delete required unfinalized branches to satisfy an
arbitrary branch-count heuristic. CO-22.

**Full fallback is not universal execution support.** The current follower sets
`slot_number` to None and its builder does not reproduce arbitrary ommers. Full
records are permitted only when both signer and follower reproduce the exact
block. Merely validating the supplied header is insufficient. CO-48.

**A hard cap needs a due trigger.** A node could stop sequencing at the signer cap
while elective interval/count rules never request a post. Require a hard-resource
due reason at the last feasible legal boundary. It does not change cross-chain
target selection or discard history. Test idle history, a lower signer cap and a
non-divisible Sync grid. CO-26.

## What was challenged successfully

In round two Claude withdrew or corrected its honest-producer argument for dropping
reassembly, its zero-count rule for resumed payloads, signed-root-based retention,
and the assertion that long spans require depth zero. It agreed that the activation
alternative and flat-count encoding are preferences.

Not every follow-up recommendation was accepted either. Promoting an environment
change to terminal must not make it effective one block late: the terminal retains
its actual full header and the next batch supplies its own initial environment.
Profile 1 can carry varying randomness with kind-2 records; deriving it from L1
instead is a future optimization, not an unresolved requirement here. Signed,
applied and finalized roots retain distinct roles.

## Remaining empirical work

Replay equivalence and cold proving measurements now precede the larger RPC work.
If cold proving meets every long-span gate, cache work can be removed through an
evidence-backed specification amendment; otherwise the specified fast path remains
necessary. A cold resource rejection is useful evidence, not a reason to bypass
quotas.

The 65,536 default and near-maximum native spans remain targets. There are no new
EVM replay, throughput or memory results from this review. The 600-second recovery
SLO is a proposed release requirement, not a result or a hidden change to normal
cross-chain deadlines. Source/logic checks, document consistency, coverage mapping
and arithmetic were verified here. Production acceptance still requires the
implementation and recorded gates.

## Principal independently checked code

- [Intermediate evidence gate](../../../../crates/eez-proof-signer/src/settlement/blocks.rs),
  `verify_validated_intermediate_blocks`.
- [Stateless integration](../../../../crates/eez-proof-signer/src/validate/stateless.rs),
  execution outputs, prefix hashes and authenticated ancestors.
- [Follower](../../../../crates/eez-deriver/src/deriver.rs), `batch_l2_range`, resumed
  reconciliation, environment construction and `check_claimed_state`.
- [Proof root chain](../../../../crates/eez-proof-signer/src/settlement/state_chain.rs),
  `verify_root_update_chain`.
- [Witness store](../../../../crates/eez-node/src/witness_source.rs), height-keyed
  storage, canonical-hash checks, reconstruction and purge.
- [Timing/grid](../../../../crates/eez-driver/src/timing.rs) and
  [policy configuration](../../../../crates/eez-composer/src/settlement_policy/config.rs).
- [Remote error mapping](../../../../crates/eez-prover-client/src/lib.rs), status
  classification and channel setup.
- Pinned Stateless `4fc3806...`, `validate_block_consensus` and
  `validate_execution_output`; pinned Reth `fd59fd2...`, `EthBeaconConsensus` and
  `EthBlockAssembler`. These dependencies were checked locally after Claude noted
  their absence from its snapshot.
