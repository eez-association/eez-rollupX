# Result-size reserves for deferred cross-chain entries

The previous bounded model reserved the cost of a 64-byte result for every
eligible deferred entry and every stored entry awaiting cleanup. Solidity
stores results up to 31 bytes inside their length slot, 32-byte results in one
additional slot, and 33–64-byte results in two. The composer now selects a
measured reserve for that storage size instead of charging the largest case.

| Result bytes | Deferred-entry reserve | Later cleanup reserve |
| --- | ---: | ---: |
| 0–31 | 230,000 | 55,000 |
| 32 | 250,000 | 60,000 |
| 33–64 | 270,000 | 65,000 |

This changes estimates, not the work performed by the contract or the network's
gas limits. Transaction calldata, companions, target execution, the 600,000
framing allowance and the configured percentage margin remain separate.

The existing eligibility gates remain intact: successful bounded deferred
entries, at most one own-rollup update, no inline or reentrant calls and at most
64 result bytes. Immediate-call estimates do not change. Unsupported previous
queues retain the legacy model. Mixed recognized queues sum their individual
cleanup reserves from canonical storage; a consumed cursor does not erase
stored entries. The 128-total-entry measurement bound, disabled-policy path,
numeric admission cap and final complete-bundle execution checks are unchanged.
No configuration, storage layout, encoding, proof or persistent state is added.

## Evidence for choosing this change

All 43 captured post inputs from the preceding `c55872ba…` Chiado run round-trip
byte-for-byte through the contract ABI. Its 38 source-bearing posts contain 870
sources and 670 deferred entries. Of those entries, 400 satisfy the deferred
shape gates, all with 32-byte results. The previous-queue gate permits the
bounded model for 301 of them; 99 remain behind unsupported-queue fallback.

Holding the previous grouping and all other estimates fixed, the new reserves
reduce projected current-entry gas by 6,020,000 and cleanup gas by 1,190,000
across 17 source-bearing posts. Six uniquely matched included gas cuts would
allow their previously rejected complete group past the recorded inequality.
These are counterfactual admission estimates, not measured throughput gains or
reductions in paid gas. The 0–31-byte tier contributes no saving to that run:
its empty-result entries contain inline calls and remain unsupported.

All 22 recognized previous queues reconstruct the old logged cleanup exactly.
All 16 unsupported previous queues contain inline calls; none is discounted by
this change. Extending those shapes would require separate measurements.

Ordinary bundle-exclusion backoff was also reviewed and is retained. The relay
response cannot distinguish auction loss from simulation failure. Preserving
capacity for every exclusion would remove an existing safety mechanism without
adequate evidence. The existing exception for canonically proven skipped slots
and its source-ownership checks remain unchanged.

## Coverage and validation

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Select measured result-size reserves | `composer/gas.rs::Projection::entry_gas` | Unit size boundaries; Solidity cold-storage ladders on Cancun, Prague and Osaka |
| Clear actual stored results, including consumed queues | `gas::previous_cleanup` | Encodings 0–64 bytes, mixed sizes, invalid encodings and read failures; consumed/unconsumed mixed 127-entry Solidity queues |
| Admission and final sizing agree | Shared `entry_gas` through `cost` and `batch_gas` | Mixed-result complete-prefix test, exact budget boundary and atomic rejection |
| Preserve unsupported-shape fallback and measurement bound | Existing shape gates and `PostBatchGasBudget` | Existing immediate/deferred eligibility, 128-entry reserve restoration, overflow and cleanup-deferral tests |
| Larger batches apply every intended effect | Existing native fixture and independent follower | Two consecutive 45-source empty-result waves pass on each network profile, with canonical receipts, effects and order verified |
| Preserve established and expensive workloads | Existing native regressions | All retained 30/40-source, mixed 128-source and expensive/new-proxy cases pass |
| Verify current Chiado behavior with the exact public workload | Existing staged runner and canonical receipt audit | All 870 sources complete; 770 wallet nonce checks per layer, canonical placement, unchanged runtime and prior state, complete costs and zero audit gaps |

The composer suite passes 196 tests and all-target Clippy with warnings denied.
Formatting and whitespace checks pass. All 15 contract tests pass on each of
Cancun, Prague and Osaka, for 45 successful test runs. Every previous assertion
and out-of-bound control remains. The 42 uniform gas measurements on each fork
are identical to the previous fixture, while the new assertions require the
smaller reserves where applicable.

A mixed 127-entry queue containing 31-, 32- and 64-byte nonzero results measured
27,367,368 gas to post and 6,496,754 to clear. The respective summed reserves
are 31,890,000 and 7,775,000. Consumption does not change those measurements.
Measurements use cold caller-frame gas before refunds; intrinsic/calldata gas
and the operator margin are accounted for separately by the composer.

The normal Rust 1.94 release built successfully. Composer SHA-256 is
`81ac31f2a3f58d87cd039b129a796b81eb7839601c4c84b2010e32e407022765`;
the follower and signer are byte-identical to the preceding deployment. The
native harnesses were built separately and launch those frozen executables.

An identical Ethereum Osaka control against the preceding `c55872ba…` release
settled the first 45-source wave in one Sync, then split the second into 37 and
8. Its gas cut recorded a 13,381,013 planning budget, 13,176,144 projected gas,
272,168 for the next group and 2,925,000 for clearing the preceding queue. The
active transaction limit was 16,777,216. The candidate passes both 45-source
waves with the same fixtures, timings and network limits; the independent
follower verifies every call and its order in each canonical Sync.

The Gnosis Prague control already fits both 45-source waves with the old
release: that fixture has a 30,000,000 block budget and no active transaction
cap. It provides compatibility evidence, not a measured packing improvement
on that profile. Network-specific limits must not be assumed identical.

All eight native release tests pass, including both new 45-source cases. The
changed native test target also passes Clippy with warnings denied.

The existing Chiado composer was replaced at 01:30 UTC on September 16 with
the tested executable. Assigned environment, original data mounts and the prior
safe block's hash and state root are unchanged. Both heads advance, the queue
is empty and the beacon is synced with 25 peers. The existing Compose file now
references that executable; the network and its contracts were not recreated.

## Public Chiado comparison

The unchanged staged runner completed at 01:39 UTC. Both runs use the same
14-scenario workload, 770 jobs, 870 source transactions, `PREPARE_PARALLEL=200`,
28 send workers and `--no-verify`. The configuration, protocol revision and
collector identities match; only the composer executable and run label change.
All 520 inbound and 350 outbound sources have canonical placement. Both audits
record 820 `Committed` and 50 expected `SourceCompleted` outcomes, all worker
nonces match, the queues drain, and no receipt/cost evidence is missing.

| Measurement | Previous `c55872ba…` | Result-size reserves `81ac31f2…` |
| --- | ---: | ---: |
| Source-bearing posts / physical blobs | 38 / 38 | 38 / 38 |
| Largest observed source count per canonical Sync | 39 | 43 |
| Mean sources per post | 22.89 | 22.89 |
| Source-bearing post execution gas | 223,094,098 | 214,782,355 |
| Whole source-bearing bundle execution gas | 297,064,402 | 288,444,067 |
| Source-bearing bundle receipt cost, xDAI | 0.346860570013496759 | 0.219763094934703220 |
| Held-to-canonical median / p95, seconds | 146.747 / 272.286 | 135.795 / 231.150 |
| First held source to last canonical completion, seconds | 308.123 | 246.980 |
| All captured paid posts / blobs, including setup/history | 43 / 47 | 44 / 48 |
| All captured bundle execution gas | 297,630,040 | 289,122,243 |
| All captured receipt cost, xDAI | 0.348651168421418855 | 0.221751990942390605 |

This observes a 39-to-43 increase in the largest Sync and 2.90% less
source-bearing bundle gas, with no reduction in total source posts. It is one
sequential public-network pair, not a controlled causal throughput result.
Source order, accumulated state, proxy addresses and grouping can differ.
The candidate also saw five excluded attempts and two proven skipped slots,
versus twelve and nine previously. Post tips, companion prices and blob prices
were lower; the 36.64% reduction in charged source cost cannot be attributed to
the estimator change. Setup/history costs are retained, not discarded.

The 43-source post is
`0xfb82032fe4037aa8a0308b12c0c56166983286639d627b2d9355a8aafe31af55`,
at L1 block 23,077,011, transaction index zero, ending at canonical L2 Sync
185,350. Its 43 inbound sources were all the sources selected for that attempt;
43 is not a demonstrated network maximum. It used one physical blob and
104,327 encoded bytes, or 41.08% of the two-blob allowance. The complete bundle
used 8,198,674 of the block's 17,487,893 execution gas. Canonical Sync counts
are grouped by exact L2 number/hash, retaining separate posts and events within
an L1 block.

The public run intentionally skips protocol semantic assertions and does not
launch an independent live follower. It verifies canonical layers, source
ownership, nonces, timing and complete receipt costs; native tests separately
verify effects and independent follower replay. Neither relay acceptance nor
outer receipt success alone is treated as proof of correct settlement.

## Remaining constraints and stopping point

The candidate records 18 transaction-gas cuts and 11 preparation-time cuts;
these include attempts and must not be treated as unique-source counts. The
existing 500 ms preparation allocation, 20% gas margin, network limits,
unsupported-shape reserves and exclusion backoff remain unchanged.

A separate isolated study tested replacing a previously persisted queue on all
three forks: eight tests and 441 replacement combinations per fork. It confirms
that clearing and rewriting the same storage can cost substantially less than
independent cleanup plus fresh writes. Original-storage calibration measured
5,007 gas for a cold nonzero clear and 110 for the following dirty rewrite.
Those savings are not implemented or claimed as paid-gas reductions here.

A conservative proposed credit of 20,000 gas for an overlapping, originally
nonzero root-array header needs no additional storage reads. On the candidate
it would apply to 94 positions across five posts, adding 1,880,000 hypothetical
admission headroom. Only one of the 18 included gas cuts would allow its
recorded next group past that gas inequality. The other affected posts stopped
for preparation time or accepted all selected sources. There is no demonstrated
reduction in posts or overall completion time from that proposed extension.

Correct overlap accounting must count unsupported entries too: the affected
39-source post has 29 eligible entries but only 25 eligible positions overlap
its previous 33-entry queue. Using the smaller of queue length and eligible
count would over-credit four entries. Position tracking, final sizing and
rollback coverage would therefore add meaningful maintenance obligations for
one observed opportunity. This iteration retains the simpler result-size
model. Broader storage credits or support for nested/reverting shapes need
separate evidence; neither is silently included by reducing a reserve.

## Reproduction and final state

Validation commands and their recorded results:

- `cargo +1.94.0 test -p eez-composer --lib --offline --locked -j3`: 196 pass.
- `cargo +1.94.0 clippy -p eez-composer --all-targets --offline --locked -j3 -- -D warnings`: pass.
- `cargo +1.94.0 clippy -p eez-node-e2e --test dynamic_admission --offline --locked -j3 -- -D warnings`: pass against the frozen candidate source.
- `cargo +1.94.0 fmt --all -- --check` and `git diff --check`: pass.
- The existing `PostBatchGasPinsTest` runs with Cancun, Prague and Osaka:
  all 15 tests pass on every fork; commands and source identities are in
  `contracts/runs.json` in the artifact directory below.
- `build/build-candidate.py` builds the normal release and checks every
  compiler input and shipping executable identity; `native/build-native.py`
  separately builds the test harnesses. `native/check-native.py` passes all
  eight cases against the frozen release. The two prior-release controls are
  recorded separately, including the expected Ethereum split failure.
- The frozen `chiado-gas-staged-20260915/run.py` and `analyze.py` run and audit
  the public workload. `compare.py compare` requires both complete audits and
  matching workload/configuration/collector identities. Ten comparator checks
  pass, including rejection of incomplete or incompatible evidence.

Final health at 01:46:44 UTC records the tested composer, advancing fresh
heads, synced beacon with 25 peers, empty cross-chain queue and preservation of
the pre-deployment safe state. The manual runner passes
`bash scripts/run-chiado-staged-performance.sh --check`. Its funding wallet
received an authorized 0.3 testnet xDAI replenishment after the measured run;
that transfer is journaled separately from workload costs. The standard command
remains `bash scripts/run-chiado-staged-performance.sh`.

Runtime changes are confined to `composer/gas.rs`. The Solidity file adds
stricter pin assertions and mixed-result coverage; `dynamic_admission.rs` adds
the two 45-source replay regressions while retaining earlier cases. The
operator guide documents the reserve classes. There is no configuration
migration or deviation from the proposal's measured-estimate allowance in §6.3.

Raw measurements, source and executable identities, immutable workload inputs,
and the reproducible comparison are retained in
`artifacts/chiado-result-reserves-20260916/`. The previous deployment and its
measurements remain in `artifacts/chiado-shaped-admission-v2-20260915/`.
