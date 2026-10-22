# Streaming prover backport

| Field | Value |
| --- | --- |
| Design status | Implemented within the requested #198/#205/#206 semantic backport; local validation and its limits are recorded in the audit. Upstream open PRs remain references, not accepted fork contracts |
| Date | 2026-10-19 |
| Source baseline | Fork `origin/main`, `51ef1ba08d5c506c8909bc8be2e16e8d25c0a7a8` |
| Protocol baseline | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`, unchanged |
| Related work | [#198](https://github.com/eez-association/eez-rollup0/pull/198), [#205](https://github.com/eez-association/eez-rollup0/pull/205), [#206](https://github.com/eez-association/eez-rollup0/pull/206), fork [#89](https://github.com/eez-association/eez-rollupX/pull/89) |

## Scope and dependency order

The common v2 wire contract (#198, following #168) precedes conformance tools
(#205) and the streaming implementation (#206). #206 is explicitly stacked on
#205 and supersedes #188. #198 is merged upstream; #205/#206 remain open, and
#206 has requested changes. The final inspected heads are recorded in the
[audit](../audits/2026-10-19-streaming-backport/README.md).

Fork #89 (`759ae5f5cbf87c16b52cedfecc05aae737e90a87`) already adapts #198 and
#205 to native blobs. It is unmerged and draft. The backport branch merges that
existing dependency into latest fork main, retaining its commit and saved
expectations. Its historical validation is not evidence for this implementation.

The fork has one stateless signer, independent L1 authentication, native blobs,
durable hash-keyed witnesses, ordinary preparation/cache leases, and strict
validation identity checks. It has no stateful prover crate. Preserve these
boundaries; do not import Rollup0's conditional calldata proofs, stateful Reth
branch-selection implementation, or its deployment backend selector.

## Requirements and acceptance checklist

| ID | Requirement / classification | Implementation and acceptance target |
| --- | --- | --- |
| W1 | #198 common wire schema and generated exports: supplied by fork #89 | Compile both schemas; codec round trips; preserve v1 tags |
| W2 | Baseline Begin → Block* → Finalize → Proof/EOF; optional Resume/Rewind/Cancel/subranges: adapt #206 | Real service transcripts and focused state-machine tests |
| W3 | Exact block identity, parent/state continuity, complete witnesses; duplicates idempotent; rejected gaps never advance | Validator and Block admission; malformed, conflicting and replay regressions |
| W4 | Native physical blobs and exact posting context are mandatory; never drop settlement input during fallback | Shared existing settlement pipeline; positive native vectors and negative mutations |
| W5 | Prover retrieves selected parent and complete live-slot provenance and authenticates canonical L1 independently on every Finalize | Operator-selected L2 header source plus existing canonical L1 authority; unavailable/reorg/invalid-source tests |
| W6 | Registry reads use authenticated target **parent**, per #198 review | Preserve existing `proof_context`; canonical-source regression |
| V1 | #205 wire replay, correlation, cursor, status, EOF, digest and signer assertions: adapted by #89 | Replay all 29 native cases without regenerating expectations; corrupt-oracle checks |
| V2 | Upstream calldata fixture positives are incompatible with this fork | Preserve native fixtures; document coverage and synthetic-L1 limitations |
| S1 | Best-effort prevalidation must not impede commitment or durable capture; complete proof backfills omissions | Existing bounded preparation worker and durable witness store; skipped-prefix regression |
| S2 | Reuse checked execution and settlement evidence without relaxing target, observation or signing gates | Checked validation types and one settlement implementation; cold/warm equivalence |
| S3 | Checkpoints only where settlement needs them; ordinary and empty blocks remain valid terminals | Existing native checkpoint planner, ordinary/empty/Sync fixture tests |
| S4 | Epoch and attachment fencing isolate sessions; Cancel/Rewind cannot publish stale proofs or affect peers | Controlled in-flight tests, takeover and isolation vectors |
| S5 | Bound streams, retained sessions/bytes/items, pending frames, workers, backpressure and all operation deadlines | Quota vectors, backpressure/cancellation tests; worker permits survive RPC timeout |
| S6 | Preserve closed retry list, request-bound actionable details, canonical settlement ownership and original cutoff | Existing retry tests plus v2 status/fallback tests |
| S7 | Retained quota exhaustion or pruned anchors after L1 rollback cannot permanently stall proving | One bounded fresh-Begin repair at exact requested anchor; reproduce upstream review cases |
| S8 | Unsupported optional controls fall back to baseline; absent v2 falls back to complete v1 | Mixed-version stub/service tests; no blob/context removal |
| S9 | Verify exact requested digest and canonical registered-attester signature independently | Reuse current native response verifier; wrong batch/blob/context/signature tests |
| S10 | Shared stateful backend and its Kurtosis selector: not applicable | Fork contains no such backend; retain independent stateless signer/follower architecture |
| C1 | No incidental dependency/protocol/storage change | Preserve execution pins, lockfile versions, genesis, ABI, follower replay and witness DB |
| C2 | Document configuration, upgrade order, rollback and volatile session loss | RPC/signer specs and operator docs; absent/empty/invalid/conflicting config tests |
| E1 | Combined source-based behavior and canonical recovery | Affected unit suites, native cross-chain/process tests, shared-session/reorg regressions, fresh correctness review |

## Design and alternatives

Add a v2 transport around the fork's existing execution and settlement trust
boundaries. Retained execution never constitutes parent acceptance or signing
authority. Finalize repeats target-dependent, native DA, observation and public
input checks. Do not widen the signer's accepted semantic profile.

Reuse the current bounded producer and Reth task executor. Preserve v1
preparation and lease behavior for old peers. V2 recovery is internal transport
repair under the original proof deadline, not an extra source-attempt charge.
Do not make streaming progress into a second canonical settlement cursor.

Prover-owned provenance needs an operator-configured L2 header source. Headers
are fetched by exact hash and remain claims until bound to the selected parent
and canonical L1 settlement. Missing evidence cannot authorize signing. Keep
configuration rollout explicit and preserve v1 compatibility when v2 cannot be
served. The [fork streaming profile](../../crates/eez-control-rpc/STREAMING_SPEC.md#6-rollupx-runtime-profile)
defines the implemented source checks, configuration contract, limits and
upgrade/rollback behavior.

## Compatibility and recovery

The v2 service is additive; v1 remains intact. This work changes neither native
transactions nor block hashes, blob payloads, on-chain ABI, genesis, protocol pin,
database schema or follower historical replay. Sessions and execution caches are
volatile. Reconnection can resume retained facts; restart/eviction requires
backfill from durable witnesses. Proof or ACK does not advance canonical state
or release accepted requests. Existing in-memory queue recovery limits remain.

## Validation status

The applicable requirements are implemented. Unit, conformance, protocol,
production feature-selection and selected native process checks have completed;
the audit records their exact commands and results, including the combined
process command's unexplained exit status after all 45 cases passed. It also
records source identities, the fresh correctness review, exclusions and live/CI
checks not run. No deployment or performance claim is made by this design record.
