# Complete pinned protocol E2E support

Status: owner-authorized scope; review corrections implemented and locally validated.
Current-head CI and full-network qualification remain pending. Prior qualification
applies only to the revisions identified in the audit. Related issues: #85,
#81, #13. Fork baseline: `c702f90da19cc149890d0a17a4ef6c200dcb6a21`;
unchanged protocol: `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
See the [audit](../audits/2026-10-30-full-protocol-e2e/README.md) for source identities,
executed checks, failures and performance limits. The [review-correction audit](../audits/2026-11-09-pr123-fixes/README.md)
records the corrected implementation and its current validation.

## Scope and acceptance

Support every one of the pin's 64 E2E scenarios through composition, independent
signing, native blob settlement and follower replay. The baseline runner selected
only 34. Keep the protocol pin and scenario assertions unchanged; contract-only
passes and expected production rejections cannot replace intended-success cases.

| Requirement | Acceptance |
| --- | --- |
| Complete scenario accounting | All 64 paths/hashes retained; no skips, exclusions, duplicate or missing results |
| Execution fidelity | Exact return/revert bytes, callback observations, rollback state, cursors and canonical effects |
| Independent authorization | Witness-backed execution, native-table binding and public inputs checked independently; adversarial inputs reject before signing |
| Canonical replay | Independent native-blob reconstruction reproduces safe block hashes/state; partial consumption and reorg behavior preserved |
| Qualification | Applicable CI/regressions, protocol Anvil checks and complete Kurtosis suite with semantic verification on identified artifacts |

## Design and review order

Extend the existing shared materializer and verifier using the pinned contracts'
frame/cursor rules. Keep complete load/user groups, bounded parsing and execution,
exact commitments, and independent observations. Do not add a parallel legacy
implementation or let a Composer-supplied replay program authorize signing.

| Review area | Responsibility |
| --- | --- |
| [Materializer](../../crates/eez-protocol/src/entries/materialize.rs) | Recorded call forest → source/delivery tables and root-bound traces |
| [Independent verifier](../../crates/eez-protocol/src/entries/verify.rs) | Recompute table traversal, commitments, coverage and surviving effects |
| [Composition](../../crates/eez-protocol/src/composition.rs), [settlement](../../crates/eez-protocol/src/settlement.rs) and [native transactions](../../crates/eez-protocol/src/system_tx.rs) | Preserve whole groups and rollback scopes through encoding and replay |
| [Observation executor](../../crates/eez-driver/src/observation.rs) and [signer contract](../../crates/eez-proof-signer/SPEC.md) | Execute lookup-only outcomes against authenticated state before authorizing effects |
| [Scenario inventory](../audits/2026-10-30-full-protocol-e2e/scenarios.md) and [signer review](../audits/2026-10-30-full-protocol-e2e/signer-review.md) | Positive coverage, adversarial regressions and evidence |

### Rollback and lookup invariants

- Preserve all nested source Snapshot/Revert scopes, including shared starts.
  Round-trip their boundaries; destination markers follow
  `TableGenerator._closeRollback`: close inner scopes first and omit already
  reverted prefixes from outer markers. Reject crossing or partial-subtree spans.
- Keep checkpoints owned by live sessions. An inner rollback must not consume an
  enclosing checkpoint; returning to an outer session retires temporary reentrant
  sessions. When restoration is queued, the earliest applicable checkpoint wins.
- Validate failed-frame descendants before accepting the terminal revert. Restore
  state, cursor/hash state and committed effects as required, but retain execution
  coverage. Checkpoint hash-prefix lengths and value totals only across properly
  nested, append-only effect accumulation.
- Lookup-only failed roots retain their full callback table. Reconstruct their
  destination execution from bound native actions and run it through authenticated
  manager state in a disposable overlay. Require successful manager validation
  separately from the exact target success/revert and output. A manager failure
  cannot stand in for a target revert.
- Replay root rollback groups in one canonical-prefix scratch overlay so callbacks
  see transient writes. Derive spans from bound actions, restore nested snapshots
  in reverse order, and never commit scratch writes to canonical state.

### Review corrections (2026-11-09)

The owner authorized fixes for the deep review of head `8b5cafd3`. Keep the
implementation in the existing materializer and verifier, without new runtime
modes or a parallel historical decoder:

- Reject successful non-applying L1 roots with callbacks at construction and
  signer classification. Failed-root callbacks remain supported because L1
  enforces their rollback. Simulation of an enclosing source revert is not
  execution authority on L1.
- Rebuild a reused root program and compare its complete table and trace; never
  concatenate callbacks from successive attempts.
- Compute settlement and escrow transfers using the existing paired traversal,
  which restores complete ancestor effects. Planning uses a root-finalized copy;
  signing still verifies actual roots and independently executed evidence.
  Admission checks these pairs on every chain. Escrow funding includes executed
  withdrawals that later revert; only surviving withdrawals debit reservations.
- Preserve baseline static table bytes and terminal-failed-attempt source slots,
  with frozen historical replay fixtures. Keep serialized allocation separate
  from the actual VM cursor; an observed retry may cover one identical terminal
  failed slot, never an unused extra slot.
  Do not relax exact native binding or accept unobserved duplicate rows.
- Align mixed-static acceptance with the same bounded failed-frame and rollback
  semantics already checked by the verifier.

Acceptance requires the original failing cases to pass, adversarial retry/table
and lookup cases to reject, package and source process regressions, and integrated
CI qualification. Prior full-network results do not qualify these corrections.

### Static reads and partitioning

Static reads leave mutable cursors unchanged. A failed mutable candidate can be
selected again after a read. Preserve one static row per observation for exact
historical reconstruction; runtime lookup can still select an earlier identical
candidate. Each observed read can cover one byte-identical row, never an unchecked
extra row. Position keys alone cannot distinguish local-write sequences such as
1 → 2 → 1. Resolve against fixed host contexts,
memoize only within one pending read, reject cycles and enforce nesting limits even
for cached suffixes. Only selected, validated callback graphs establish coverage.

Complete source groups supply partition boundaries when their count equals the
host count; validate every host's shape first. Otherwise use cursor-based
partitioning. Neither path replaces independent execution/receipt binding.
Construction borrows immutable actions/tables and keeps mutable cursors separate.
Differential tests and matched benchmarks qualify these simplifications; they do
not establish whole-node throughput.

## Compatibility and recovery

No protocol pin, deployed ABI, genesis or native wire/storage format changes are
required for baseline-compatible deployments. RPC validation version 8 identifies
the extended acceptance rules; deployed signing domain 7 and the registered vkey
remain unchanged. Network, registry and proof-system bindings stay mandatory.
This extension neither revokes existing signatures nor requires key rotation.

Upgrade Composer, signer and followers together before enabling the new behavior.
Drain/discard old proving sessions and rebuild requests with complete witnesses:
older witnesses may lack authenticated manager data needed for lookup replay.
Previously supported data retains its decoding/execution rules. Old followers
cannot replay newly admitted canonical shapes, so downgrade after inclusion is
unsupported. Existing pre-baseline contract/genesis boundaries still apply; this
is not a rehearsed migration of arbitrary older networks. Pending queues and retry
counters remain in memory; clients may need to resubmit after restart.

Passing this suite does not admit every Core configuration. The signer contract
still limits support to one L2 and one proof system, with existing gas/frame/witness
bounds; explicit cross-chain call gas, sender-bound batches and other L2 targets
remain unsupported. Pinned mutable-retry limitations also remain.

The [prior design record](https://github.com/eez-association/eez-rollupX/blob/5d74a42ae8ae9fe2a79ab02fdf7c6e5c247e698d/docs/proposals/full-protocol-e2e.md)
preserves the detailed investigation rationale; current rules belong in the linked
implementation and owning specifications.

## Structural simplification experiment

Owner-authorized on 2026-11-01 against `73458a03`, with the same protocol pin.
Prototype assigning L2 root slots during materialization and sharing trace fold/
rollback mechanics between construction and finalization. Preserve static cursor
positions, first-match retry rules, nested scope restoration, exact table bytes,
finalization checks and independent signer authorization. Keep a prototype only
if it reduces complexity and passes differential/regression checks and matched
performance measurements; previous qualification does not qualify these changes.

The experiment retains inline root-slot assignment (79 fewer production lines).
Shared trace folding was rejected: it saved only five further lines and increased
median finalization time by about 3% on two workloads. Finalization and independent
verification remain unchanged. The retained candidate passed the full local qualification, including all 64
protocol scenarios and independent cold replay; see the linked audit.
