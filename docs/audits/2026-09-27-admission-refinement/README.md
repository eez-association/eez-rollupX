# Admission minimum checks and measured reserve refinement

| Field | Value |
| --- | --- |
| Date and status | September 27, 2026 UTC; implemented, validation and local rollout in progress |
| Scope | Avoid repeated capacity-refused source simulation; tighten measured posting/companion reserves; expose cheap bounds and full simulations separately |
| Source baseline | `239b8224e04b049bb9303fc1bd756033a6b9f2a4`, branch `feat/cross-chain-admission-throughput`; deployed runtime `e5b2cde65c9166ceae87bd4ee723820ed63ea5f6` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Environment | Rust 1.94.0, Foundry 1.7.1; locked dependencies; local native process tests and existing Chiado deployment |
| Related records | [Accepted follow-up](../../proposals/cross-chain-admission-throughput.md#september-27-follow-up-cheaper-capacity-checks-and-tighter-reserves), [baseline and budget review](../2026-09-26-admission-packing/README.md), [admission contract](../../configurable-settlement-policy.md), [monitoring contract](../../../monitoring/indexer/README.md#admission-history) |

## Baseline and implementation

The preceding verified wave settled 870 sources / 770 semantic jobs using 28
postBatches. Seventeen bundle-limited postings consumed 43.0% of the actual
network block budget on average; their signed transaction-start envelopes needed
47.7%. The cap itself is the canonical parent block limit. Gas estimation, rather
than a percentage reduction of that cap, was the conservative component.

The baseline recorded 7,672 capacity-refused full source simulations. A replay
of the old estimator's minimum arithmetic identified 7,100 of these before
simulation. This is a retrospective arithmetic check, not a measured candidate
speedup; version 6 uses a lower inbound minimum and may reject fewer cheaply.

Policy `packing-minimum-check-3` checks current accepted budgets before source
execution. It preserves source ownership, sender/direction nonce dependencies,
whole groups and independent-source packing. Empty candidates run ordinary
classification. The bound concerns the admission model, not actual EVM gas.
Cheap misses do not train execution timing samples. No source execution result
is reused across a changed parent or accepted prefix.

Estimator `companion-storage-6` distinguishes zero, retained and fresh storage
words; fresh nonzero writes retain 23,000 gas. Measured protocol framing charges
are reduced, while matching-identity queue scans increase from 12,000 to 12,500.
Every root still reserves scans of the entire accepted queue prefix: successful
inner calls alone cannot prove that an outer source committed cursor progress.
Unsupported shapes retain their previous conservative fallback.

The new `source_capacity_check` event reports the minimum contribution, signed
requirements and active budgets. Grafana distinguishes these from full estimates
and shows full simulation and cheap rejection counts. Upgrade the strict
admission-aware collector and additive schema before the new Composer; keep
them on Composer rollback. Existing JSON history and checkpoints are preserved.

Exact whole-bundle execution currently follows proof production and prepared
attempt ownership. An extra near-cutoff execution pass would require another
bounded prove/rebuild lifecycle and a proved fallback under the same target.
This iteration retains final exact native execution and does not implement that
extra proof cycle or bypass proof validation.

## Executed local checks

Commands run from the repository root. Raw logs are local-only under
`artifacts/admission-refinement/`, unavailable in another checkout. Final compact
results and executable identities will be added after validation.

| Check | Result |
| --- | --- |
| Unchanged baseline gas fixtures, Prague | 50 passed |
| Revised gas fixtures, Cancun / Prague / Osaka | 50 passed on each fork |
| All integration contract tests | 69 passed, zero skipped |
| Composer library tests | 264 passed; one existing manual test ignored |
| Indexer locked workspace, including owned PostgreSQL recovery test | 26 passed, zero ignored |
| Native admission/queue process selection | 48 selected across seven targets; pending |
| Workspace and indexer clippy | Pending final workspace result; indexer passed |

First failures are retained. The initial Composer run exposed a stale test
expectation for a zero-to-nonzero packed flag: it now changes the base charge as
well as adding the fresh allowance. The assertion was corrected to the full
20,000 delta; 264 tests then passed. A sandboxed indexer run could not bind its
mock HTTP server; the authorized run with loopback and the owned disposable
PostgreSQL database passed all 26 tests. Initial workspace clippy rejected an
inverted `if` layout; the equivalent branches were reordered. Native compilation
was deliberately interrupted before tests to validate the corrected source.

`forge fmt --check --root contracts` reports existing formatting differences in
unchanged `ECDSAProofSystem.sol` and `DeployRollup.s.sol`; these are outside this
change. The edited gas fixture itself has no formatting difference.

## Operational work and remaining acceptance

No new runtime has been deployed for this iteration yet. Remaining: exact local
shipping images, native process gate, collector-first image rollout, verified
870-source wave, canonical reconciliation and Grafana coverage. Preserve operator
configuration, contract/genesis identity, datadirs and retained monitoring logs.
Live performance comparisons share mutable chain state and cannot establish a
causal speedup from one run. The extra proof/refinement cycle remains separate.
