# Chiado settlement stall, deep replacement reorgs and witness wedge

| Field | Value |
| --- | --- |
| Date and status | 2026-10-02 UTC. Three defects reproduced by failing process/unit regressions at the baseline; fixes implemented on `fix/settlement-stall-recovery`. Not yet deployed; the live Chiado node still runs the baseline image with the operational workaround below. |
| Scope | Why the existing Chiado L2 stopped including L2 transactions, the two deep L2 reorgs and two block-production wedges observed during recovery, and the corrections. Excluded: why the backlog first exceeded 2,048 blocks (logs before 14:24 UTC were already rotated); DA capacity under heavy pure-L2 load. |
| Source | Baseline `fa916b13254a36423cad130080dde367bb18a7d6` (`main`). Fix: branch `fix/settlement-stall-recovery`. |
| Protocol | Gitlink `855fe0602484750861b4c31502a1a91355d33ed6`, unchanged. |
| Executables | Live: `ghcr.io/eduadiez/rollupx-node@sha256:56082b972f626240d867c59d7413032bbe5e8f62464b1d68b282c64629774725`, `ghcr.io/eduadiez/rollupx-proof-signer@sha256:947ea1634644a772f4938cd77a4ba4d5e1e263ff45a970412ebd9d4b8bd0f979` (tag `fa916b1…`). Tests: local debug builds of the baseline and the branch. |
| Environment | Chiado L1 (2 blobs per transaction/block), K = 5, Rust 1.95.0, Foundry 1.7.1, local Anvil process harness. |
| Related records | [Deadline replacement design](../../proposals/skipped-slot-speculative-retry.md), [RPC replacement contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission), [historical-prefix audit](../2026-09-23-historical-target/README.md). |

## Findings

### 1. Block-cap historical emission never refreshes its anchor

Observed on Chiado: settled L2 height 78,665 with the head above 86,300. Each
node run settled exactly one historical prefix (78,610→78,640, then
78,640→78,665); every later Sync slot logged
`historical chunk emission failed … canonical anchor changed before historical
prefix sizing` (618 times in one run), and committed an empty block. Held
cross-chain requests were never included.

Cause: `historical_prefix_bytes` requires the validated canonical endpoint to
equal the policy runtime's cached anchor. Only `canonical_policy_anchor` /
`evaluate_settlement_policy` update that cache and the byte index. A Steady slot
with held cross-chain work and a backlog above `max_blocks` takes the block-cap
branch, which calls neither. After its own prefix lands, the cache is stale
forever. Slots with an empty held pool evaluate policy first, so the stall needs
pending user work — which is also why users saw no transactions processed.

Fix: `refresh_historical_index` refreshes the anchor and rebases the byte index
(prefix sums start at the index anchor) before every historical prefix; the
sizing guard also checks the index anchor.

### 2. Deadline replacement unwinds history behind an empty Sync

Observed on Chiado after raising the batch cap: a historical prefix ending at
78,810 missed its L1 slot. At the next deadline the node inserted a new block at
78,810 and switched branches, unwinding 78,811–87,675 (~8,865 blocks; Reth spent
~6.5 minutes in changeset reverts with block production frozen). A second
replacement later unwound ~440 blocks during catch-up.

Cause: `replace_timed_out_attempt` and the matching `reconcile_attempt` branch
skip the branch switch only if an empty rebuild has the original's exact hash.
Since commit `bb57fea`, minimal and historical Sync blocks carry the configured
coinbase (all 400 sampled live blocks: `0x3ed3…`), while the rebuild uses the
zero address. The hashes never match, so every replacement of an empty Sync
became a branch switch. The process harness defaults the coinbase to zero, so
the existing historical-prefix regression did not cover it.

Fix: an original Sync with no transactions has no cross-chain group to remove;
both sites now leave it canonical and only re-prove it. Nonempty Syncs keep the
specified replacement behavior.

### 3. Witness capture and preparation retry unwound blocks forever

Observed after each deep reorg: `durable witness capture failed; admission
paused, retrying exact block <hash>` for a block on the unwound branch, payload
builder `durable witness capture unavailable`, and no L2 blocks until restart.
The signer also rejected ranges from the unwound branch
(`missing required ancestor headers`).

Cause: the capture loop drops a failed block only below the finalized floor;
Reth no longer serves an unwound hash (reported as `not captured yet`). The
preparation worker retried the same rejected target without adopting newer
queued targets, also pausing admission.

Fix: capture skips a block only when Reth no longer knows its hash **and** its
height holds another block or lies above the canonical tip; canonical capture
failures still fail closed. The preparation worker retries toward the newest
queued endpoint after a signer rejection, as its missing-predecessor branch
already did. The preparation change follows from code inspection; no
standalone reproduction isolates it from the capture wedge.

## Evidence inventory

| ID | Contents / provenance | Location | Availability |
| --- | --- | --- | --- |
| E1 | Live node log excerpts quoted above (14:24–16:20 UTC) | This record | Raw logs rotated by Docker's `local` driver; only these excerpts remain |
| E2 | Baseline process-test outputs | `artifacts/native-e2e/outputs/baseline{1,2,3,4}.out` | Local only |
| E3 | Baseline node/signer logs and failing datadirs | `artifacts/native-e2e/baseline*/` | Local only |
| E4 | Post-fix process-test output and logs | `artifacts/native-e2e/fixed/`, `artifacts/native-e2e/outputs/fixed*.out` | Local only |

## Reproduction and executed checks

| Question | Check | Type | Baseline | Fix |
| --- | --- | --- | --- | --- |
| #1 | `settlement_policy::backlog_above_the_block_cap_settles_through_consecutive_historical_prefixes` (signer cap 10, six dropped bundles, one held inbound request) | Process | Fail: request never settled; 30 anchor errors (E2 `baseline4.out`) | Pass (E4) |
| #2 | `dynamic_admission::historical_prefix_replacement_keeps_the_empty_boundary_with_a_configured_coinbase` | Process | Fail: `deadline replacement rewrote the historical boundary` (E2 `baseline2.out`) | Pass (E4) |
| #2 control | `dynamic_admission::historical_blob_prefix_preserves_the_skipped_slot_replacement_target` (zero coinbase) | Process | Pass | Pass (E4) |
| #3 | `witness_source::tests::capture_skips_only_blocks_that_left_the_canonical_chain` | Unit (mock provider) | Fail: admission stays paused | Pass |

With the fix, these existing process regressions also passed: the six
`dynamic_admission` skipped-slot, late-original and admission-reorg tests
(9/9 together with the regressions above, E4 `fixed1.out`) and every test in
`settlement_policy`, `settlement_policy_gas`, `settlement_policy_cost`,
`queue_reorg`, `queue_liveness` and `partial_consumption` (51/51, none ignored,
E4 `fixed-modules.out`). Composer (296) and node (63) unit tests, workspace
Clippy with `-D warnings` and `cargo fmt --check` pass. The module run's binary
differed from the committed source only by the position of one doc comment.
Not run locally: `cross_chain`, the remaining `dynamic_admission` tests, and
the CI release-artifact and Kurtosis matrices.

## Operational changes

Within the user's authorization during the incident: the operator set
`EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS=16384` (keeps the block-cap branch
unreachable) and `EEZ_PREPARATION_TIME_MS=2500` (prefix preparation exceeded the
500 ms allowance), and the node was restarted three times. Unsettled blocks
78,811–87,675 and 87,351–~87,790 were unwound and rebuilt; transactions in them
returned to the pool or were lost and may need resubmission. The network caught
up at 16:13 UTC and settled normally afterwards.

## Open work

| Item | Impact | Next action |
| --- | --- | --- |
| Deploy the fix | Live node keeps the workaround settings | CI image, then node-only restart |
| Initial backlog growth | Under heavy pure-L2 load, two blobs per slot settle about as many blocks as are produced | Capacity review; separate from these defects |
| Eligibility is checked after the branch switch in `replace_timed_out_attempt` | A long switch can complete and then fail release | Consider validating before switching |
