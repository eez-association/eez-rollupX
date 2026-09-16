# Native DA gas-accounting correction

The incremental composer gas estimate previously charged DA entry copies and
raw source transactions as L1 calldata. Native posting carries those bytes in
blobs and leaves `batch.callData` empty. `projected_tx_l1_gas` now charges only
the entries actually present in the ABI and the probed inline execution work.
`TxL1Gas::total` supplies the same incremental cost to admission and rejection
diagnostics; the latter previously included another whole-post base charge.

This changes neither fee configuration nor transaction encoding. Physical blob
limits, real ABI calldata, the active calldata floor, companion gas, configured
margins, the 600,000 framing reserve and final whole-bundle execution checks
remain authoritative. The final candidate is still simulated at the affordable
network gas budget, then simulated again with its final signed gas and fees.
Failed transactions, skipped effects and an incorrect final root prevent posting.

## Coverage

| Requirement | Implementation | Automated coverage |
| --- | --- | --- |
| Do not price native payload bytes as execution calldata | `composer.rs::projected_tx_l1_gas` and its inbound/outbound callers | `native_da_changes_only_blob_references_in_postbatch_calldata` checks equal ABI bytes within a blob and one additional index word across a blob boundary |
| Preserve actual calldata and fork-floor charges | Final full-ABI projection and posting validation remain unchanged | `real_entry_calldata_and_its_floor_remain_part_of_final_gas` uses a large real call input whose floor exceeds the ordinary projection |
| Report the increment admission actually rejected | `TxL1Gas::total` is shared by admission and diagnostics | Updated `drain_budget_cuts_the_accepted_prefix_below_the_cap` checks charged terms and preservation of the accepted prefix |
| Keep execution effects authoritative after changing signed gas | Existing `PostingRuntime::finalize` and `run_bundle` | Existing `outer_success_without_complete_effects_is_rejected` and `final_gas_and_fee_changes_require_complete_effect_revalidation` pass |
| Investigate reserves without assuming every entry costs the same | No runtime reserve reduction | Six additional tests in `contracts/test/PostBatchGasPins.t.sol`, retaining its original ladder, cover anchors, new/reused proxies, multiple calls, reverted creation, deferred storage and insufficient gas |

Validation passed: 183 composer library tests, all-targets composer Clippy with
warnings denied, Rust formatting and whitespace checks, and seven contract gas
tests with Forge formatting. Rust used 1.94.0 with `--offline --locked -j2`;
contract tests used solc 0.8.34, via IR, optimizer runs 1 and Osaka. Full commands,
source hashes, patches and failed exploratory measurements are retained under
`artifacts/chiado-performance-next-20260915/gas/`.

## Reserve reductions deferred

The tested single no-call anchor fits the existing 160,000 base reserve. However,
one entry can create multiple proxies, and a reverted frame can roll back a
creation that must be repeated. The measured two-new-proxy entry consumed
657,714 caller-frame gas; recreating the same proxy after a reverted call used
666,150. These figures exclude transaction intrinsic/calldata gas and are not
public-network receipts. The existing 370,000 per-entry pin therefore does not
describe every call shape by itself. The additional anchor charge is retained
until a conservative model covers those shapes.

The insufficient-gas fixture rejects the whole post at 530,000 forwarded gas
and applies its exact intended root at 750,000. The lower number is only the
bare base plus one entry pin, not the composer's complete declaration. The
fixture does not demonstrate invalid inclusion, automatic recovery or a safe
universal reduction. Ten mocked blob references test ABI overhead, not native
KZG validity or a network's allowed blob count.

This correction is separate from the frozen preparation-v2 executable used for
the earlier small-wave Chiado comparison. The subsequent full staged comparison
is recorded in [gas-staged.md](gas-staged.md); neither the unit tests nor the
public-network pair establishes a general latency, packing or cost improvement.

## Requested staged comparison

The gas correction has now been built in the same optimized release profile as
the deployed preparation-v2 baseline. Its composer SHA-256 is
`3694f4ec562a2511d9c763745719497921084f5926ae65ec00f76666bd0e940a`.
Among the 193 recorded Rust/Cargo inputs, only `composer.rs` differs from the
deployed `98d33366...` executable; the root Cargo manifest also matches. The
unchanged signer and follower retain their exact executable hashes.

Three existing native regressions passed against this new binary: 128 mixed
sources on Gnosis with independent replay, expensive new-proxy execution with
complete-group gas splitting, and 30 mixed sources in one Ethereum Sync. These
are regression checks, separate from public Chiado performance measurements.

The requested `PREPARE_PARALLEL=200` staged comparison completed on both
versions using the existing network: 770 jobs, 870 sources, and the exact 14 scenario
counts, with `--no-verify` placed before targets as required by the unchanged
upstream parser. The collector records source identities, ownership transitions,
canonical receipts, nonces, layer placement, timing, gas and blob costs. Setup
overlap is retained rather than removed from the requested workload.

Both runs passed their canonical source, receipt, layer, ownership and nonce
checks with no evidence gaps. The gas candidate was deployed at 21:14:52 UTC
on 2026-09-15 and remains active. Configuration and existing databases were
preserved. The post-run queue is empty and both layers are advancing.

Authorized funding transferred 5.61 test xDAI from the dedicated operator to
the existing test wallet; 2.2 of that amount was bridged to its L2 address. The
runner's 4.35-token upfront faucet reserve per layer includes unused value
returned to the faucet and must not be reported as expenditure. Canonical
settlement charges are reported separately in the linked comparison.

Build commands, frozen sources, binary identities, native test logs, the exact
workload, funding calculations and launch commands are retained under
`artifacts/chiado-gas-staged-20260915/`.
