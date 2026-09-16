# Full staged Chiado gas-accounting comparison

The gas-accounting correction was deployed on the existing Chiado network. Both
full requested workloads passed canonical placement, ownership, nonce and
receipt-cost checks for **870 cross-chain sources**, with no evidence gaps.
This pair does **not** demonstrate improved maximum packing: source-bearing
posts increased from 37 to 38, and both runs reached 34 sources in one Sync.
The later [L1 target-attribution correction](l1-gas-attribution.md) supersedes
this executable and retains this run as its comparison baseline.

The candidate was deployed at **21:14:52 UTC on 2026-09-15**. The 21:25:57 UTC
health check confirmed an empty cross-chain queue, advancing L1/L2 heads, a
synced non-optimistic beacon with 25 peers, and preservation of the earlier safe
L2 hash and state root. The normal local Compose definition was updated to this
tested executable. No chain, registry or database was recreated.

Compact measurements and evidence hashes are in [gas-staged.json](gas-staged.json).
Full local evidence is retained under
`artifacts/chiado-gas-staged-20260915/`, including each run's immutable canonical
RPC export, receipts, source/post/Sync associations, logs and timing CSVs.

## Workload and executable comparison

Both runs used the unchanged protocol checkout
`3c0cdf4955941a5c23639346932b55b33babebbd` and this command:

```sh
PREPARE_PARALLEL=200 bash script/e2e/run/network-staged.sh --no-verify \
  counter:100 counterL2:100 nestedCounter:50 nestedCounterL2:50 \
  multi-call-nested:50 multi-call-nestedL2:50 \
  multi-call-twice:50 multi-call-twiceL2:50 counter-multi-tx:50 \
  reentrant:20 revertCounter:50 revertCounterL2:50 \
  revertFromOtherChain:50 revertFromOtherChainL2:50
```

Only flag placement differs from the user's spelling: the upstream parser
requires flags before targets. Each run has 770 jobs, 2,850 setup transactions
and 870 sources: 520 inbound and 350 outbound. The existing profile retains
28 send workers, one-second receipt polling and plan-mode preparation.
No scenario-specific production change or added setup-drain barrier was used.

Baseline composer SHA-256 starts `98d33366a3f8`; candidate starts
`3694f4ec562a`. Full hashes are in the compact record. Both use the same optimized
release profile. Only `crates/eez-composer/src/composer.rs` differs among 193
recorded Rust/Cargo inputs; the root manifest also matches. The follower and
signer executables are byte-identical. The [accounting correction](gas-accounting.md)
removes the planning charge for blob bytes treated as execution calldata and
aligns rejection diagnostics with admission. Real calldata, gas reserves,
physical blob limits and final execution validation remain enforced.

The existing L1/L2 chain IDs are 10200/6290. All assigned environment values and
data mounts were preserved. Empty consolidation remains 300 L2 blocks, pure-L2
mode `always`, admission `auto`, preparation/proof/submission allowances
500/500/700 ms and the post execution-fee ceiling 500 gwei. Release builds and
native tests finished before both public runs. The upstream runner's timed
artifact warmup and preparation remained part of the requested command.

## Observed results

Timing starts when the composer accepts a source into its held pool and ends at
canonical ownership reconciliation. This is local settlement delivery, not
consensus finality. Median and p95 cover all 870 sources; p95 uses nearest rank.

| Measurement | Baseline | Gas candidate |
| --- | ---: | ---: |
| Canonically verified sources | 870 | 870 |
| Source-bearing posts / Sync blocks | 37 / 37 | 38 / 38 |
| Maximum sources in one Sync | 34 | 34 |
| Physical blobs in source-bearing posts | 37 | 38 |
| Execution gas, including all L1 companions | 303,770,899 | 286,376,697 |
| Source-bearing settlement charges, xDAI | 0.354629187 | 0.399760577 |
| Median completion, seconds | 166.943 | 157.230 |
| p95 completion, seconds | 256.912 | 247.145 |
| First source accepted to last completion, seconds | 262.973 | 253.348 |
| Gas-budget / preparation-time cuts | 27 / 6 | 26 / 6 |
| Excluded attempts / skipped target slots | 10 / 2 | 11 / 6 |

Each run ended with 820 `Committed` and 50 expected `revertCounter`
`SourceCompleted` outcomes. Every source was associated with its exact sender,
nonce, terminal ownership generation, canonical receipt, L1 post transaction
index and L2 Sync hash. All 770 workers' nonce deltas, runtime identities and
prior safe-state checks passed. Each source-bearing post occupied a distinct
L1 block in these runs; the collector supports multiple posts in the same block.

Cost includes execution and blobs for posts plus all 520 named L1 companions.
The first candidate source post also includes two ordinary setup transactions
in its settled range. These totals therefore describe the cost of source-bearing
posts, without allocating shared costs speculatively. Including all captured
setup-only/historical posts gives 44 paid posts in each run, 47/48 blobs and
0.356856327/0.403671252 xDAI respectively. Standalone setup and funding
transactions are outside both settlement-cost totals. Non-included attempts
have independently revalidated receipt absence; missing costs are never zero-filled.

## Interpretation and remaining constraints

Measured source-bearing execution gas fell 5.73%, but charges rose 12.73%.
The RPC priority estimate rose from approximately 1.0 to 1.2 gwei; the unchanged
20% margin produced post tips of approximately 1.2 and 1.44 gwei. All 75 final
source-post quotes match their exact canonical target parent, signed fees and
receipt. Candidate blob gas also briefly cost more. The underlying estimator
movement is unresolved; these measurements do not establish a fee-policy defect.

Initial setup and admission state differed. Polling bracketed 112–115 unsettled
L2 blocks at baseline ingress and 86–90 at candidate ingress. Baseline drained
four historical posts before its first source-bearing post. Candidate shared
its first source post with two remaining setup transactions. After restart,
candidate selected one calibration source from a pool of 31, then selected
the remaining 869; baseline's first selection contained all 870. Selection is
followed by gas and preparation checks, so it does not promise one final Sync.

The candidate encountered six skipped target slots versus two on baseline.
All eight are verified from consecutive canonical parent/child headers. Four
candidate skipped attempts contained sources and retained admission capacity;
the other two were historical/empty attempts. Separate backoff still applied
to other incomplete submissions. Parallel ingress order, grouping, setup and
slot differences prevent attributing the 9.626-second shorter wave or the
receipt gas decrease solely to this accounting patch. Native post calldata
was already blob-only; the change removes an obsolete estimate, not bytes
previously executed by L1.

The candidate still hit 26 gas-budget cuts and six preparation-time cuts.
These are the next constraints to examine when improving capacity. This test
does not justify lowering proxy reserves or weakening final execution checks,
and it does not establish that all 870 sources could fit one Sync.

## Validation and funding

The release build passed with Rust 1.94.0, `--release --offline --locked -j3`.
Three existing native regressions passed against the frozen candidate before
either public workload:

- `gnosis_explicit_preparation_mixed_wave_after_small_post_replays`: 128 mixed
  sources and independent replay after a small prior post.
- `gnosis_low_byte_expensive_new_proxies_split_only_complete_groups`: expensive
  execution respects complete-group gas splitting.
- `ethereum_fitting_thirty_request_wave_settles_in_one_sync`: 30 fitting sources
  settle together on the Ethereum profile.

The same source previously passed 183 composer unit tests, composer Clippy,
formatting and seven contract gas tests; see [gas-accounting.md](gas-accounting.md).
Both unchanged staged commands exited zero. Thirteen analyzer checks cover
positive and intentionally invalid receipt/ownership/canonical evidence.
Final Compose validation and whitespace checks passed.

The requested `--no-verify` disables protocol semantic assertions. No independent
live Chiado follower replay was run during these measurements; the native
replay test is separate evidence. Canonical placement and successful outer
receipts do not replace nested protocol semantic verification.

Authorized transfers supplied 5.61 test xDAI to the existing tester, of which
2.2 was bridged to that same address on L2. Funding receipts and balance checks
are retained in `initial-funding.json` and `second-funding.json`. The live poster
was not used as a funding source. The 4.35-token upfront faucet reserve per
layer includes unused value returned to the faucet and is not expenditure.
