# Independent local findings recorded before the external review

These are Codex's independent findings, not Claude output. The first external invocation
was rejected pending explicit data-transfer authorization. The user subsequently
approved the transfer, and the two-round read-only Claude review is complete; see `README.md`. Do not describe this
document as a Claude review or as external validation of the proposal.

## L1 — Native maximum-span acceptance must respect the Sync grid

Severity: medium; blocks the stated release test, not the format itself.

The plan correctly imposes a 65,536 total-block ceiling, but requires an actual
65,536-block post on both native profiles. Existing historical splitting selects
legal empty Sync boundaries using `timing.k()` and the scheduler's grid residue.
Between two same-residue Sync boundaries, the span is a multiple of K. For K=6,
65,536 is not a legal span; the largest such span below the cap is 65,532. K=5
gives 65,535. Startup/genesis offsets and resumed same-height settlement must use
the actual placement rules rather than blindly rounding every span.

Evidence: `eez-composer/src/composer.rs::grid_aligned_cap`, the historical drain's
`historical_chunk_boundary` and `empty_chunk_boundaries` selection, and
`eez-driver/src/timing.rs`.

Repair: retain exact 65,536/65,537 codec and signer boundary tests with suitable
fixtures; native posting tests select the greatest legal endpoint whose actual
proof window fits the cap and report its block count. Do not alter Sync scheduling
merely to satisfy a round-number benchmark.

## L2 — Durable witness retention needs a branch-specific audit

Severity: potentially high under long-span reorg/restart; requires a concrete
reproduction before claiming an existing exploitable failure.

The plan relies on durable witnesses after parent state is pruned and requires
reorg-safe recovery. The current witness store is keyed by height and overwrites
that entry on recapture. `NodeWitnessSource` checks the canonical hash and falls
back to re-execution on mismatch. That is safe against accepting a stale witness,
but does not guarantee availability after a different branch overwrites a witness
and an earlier branch is needed again while its parent state is pruned.

Evidence: `eez-node/src/witness_source.rs::WitnessDb::put`,
`NodeWitnessSource::block_witness`, and the finalized-height purge rule.

Repair to evaluate: hash-keyed witness storage with a height index for finalized
garbage collection, or an equally explicit retention/reconstruction guarantee.
Test a branch switch, durable recapture, restart/pruning and switch back before
closing the proposal's reorg/witness-retention coverage row. A hash check alone
proves identity, not availability.

## L3 — Separate the DA-format proof from the new proving service architecture

Severity: design/implementation sequencing improvement, not a demonstrated
correctness defect.

The request is to omit derivable ordinary data. The plan also mandates capability
negotiation, a preparation RPC, cached proof references, priority scheduling and
new resource accounting. Those may be justified for long-span Sync deadlines,
but the initial evidence measured encoding and existing limits, not a cold/warm
timing crossover. The earlier wording that there were no remaining gaps was too
strong: the performance gates still resolve material empirical questions.

Repair: require an early replay-equivalence and cold-proof performance checkpoint
before expanding the proving protocol. Preserve the desired long-span outcome;
choose the smallest proven way to meet the measured deadline. Keep cache/RPC
invariants specified if that path is selected, rather than treating cache work as
an unmeasured benefit or removing it without demonstrating a viable alternative.

## Review disposition

The grid-boundary and hash-keyed witness-retention requirements have been applied
to the normative plan. The scope concern is addressed by an early replay/cold-proof
measurement checkpoint before expanding the proving RPC. See `README.md` for the
full independent disposition, including additional fallback and resource-trigger
corrections. `status.json` records the successful authorized review rounds.
