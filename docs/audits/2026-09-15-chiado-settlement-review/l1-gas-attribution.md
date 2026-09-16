# L1 gas attribution and manual Chiado measurements

The composer now counts only L1 destination execution in the outbound
postBatch target-gas estimate. Previously it summed every recorded target,
including L2 callbacks that execute in the Sync block. This changes admission
and final gas projection without changing the transactions' execution semantics.

The mainnet rollup identifier denotes the settlement layer on both Ethereum and
Chiado; this does not hard-code an Ethereum chain ID. Every L1 frame remains
charged, including nested re-entry, failed calls and frames inside a reverted
span. The inspector returns intercepted calls' unused gas to their caller, so
nested destination frames must be charged separately. The sum saturates on
overflow. The existing entry/proxy overhead, calldata floor, framing reserve,
configured margins, companion budgets and final ordered execution checks remain.

## Coverage

| Requirement | Implementation | Coverage |
| --- | --- | --- |
| Exclude execution on L2 from the outbound L1 target estimate | `composer.rs::l1_target_gas` over the finalized action forest | `l1_admission_does_not_charge_l2_callback_execution` keeps the same L1 admission result when callback work grows, while additional L1 work reaches the budget boundary |
| Charge all L1 execution even when effects revert | Filter only destination rollup, retaining parent/re-entry/revert metadata | `reverted_l1_frames_still_consume_gas_and_overflow_cannot_wrap` |
| Use the same attribution throughout preparation | `CompositionGas::l1_target` feeds per-group admission and accepted `outbound_target_gas`, then final `projected_postbatch_gas` | Composer library suite and native/public workload validation recorded below |
| Preserve execution safety | Existing whole-bundle simulation and signed-gas revalidation | Existing final-effect, companion, calldata-floor and expensive-proxy regressions |

Validation passed 185 composer library tests, all-targets composer Clippy with
warnings denied, formatting and whitespace checks. The normal Rust 1.94 release
build passed, with only `composer.rs` changed among 193 frozen runtime inputs.
The follower and signer binaries are unchanged. Three native scenarios passed
on their first invocation with the frozen release binaries:

- `gnosis_explicit_preparation_mixed_wave_after_small_post_replays`: 128 mixed
  sources after warmup, source nonces/effects and independent follower replay.
- `gnosis_low_byte_expensive_new_proxies_split_only_complete_groups`: expensive
  target work splits at complete groups, an oversized indivisible group is
  rejected, and the follower converges.
- `ethereum_fitting_thirty_request_wave_settles_in_one_sync`: a fitting
  30-source wave settles in one Sync block.

These scenarios establish broader regression coverage; the new unit tests and
the staged nested scenarios specifically exercise L2 callback attribution.

At 22:11:47 UTC on 2026-09-15 the existing Chiado composer was replaced with
SHA-256 `1eae6cdab8deb034140a41f4ea02d211c6829542d42aceca66231e00e879bd85`.
Assigned environment values and original data mounts were identical after the
replacement. The preceding safe block retained its hash and state root, both
chains resumed advancing, and the beacon was synced with 25 connected peers.
The regular runtime Compose file also points to the validated binary. The full
public workload subsequently passed its canonical audit with no evidence gaps.

## Full Chiado workload

The unchanged 770-job / 870-source workload ran from 22:13:07 to 22:21:54 UTC.
All 520 inbound and 350 outbound sources have canonical receipt, layer,
post/Sync association and ownership evidence. The outcomes were 820 `Committed`
and 50 expected `SourceCompleted` outcomes from the revert scenarios. All 770
workers' nonce deltas passed; runtime identities, protocol inputs and the prior
safe state were preserved. The queue drained completely. Protocol semantic
assertions and an independent live follower were not run for this public wave;
the native regressions above include independent replay.

An independent log audit matched all 1,025 completed compositions and 1,895
target actions: every reported `l1_target_gas` equals the saturating sum of L1
destination gas. Across 391 outbound composition executions, 24,573,433 L1 gas
was retained, including 1,313,853 gas from reverted L1 frames, and 7,048,608 L2
gas was correctly excluded. These are repeated planning executions, not receipt
savings. Another 1,009 zero-target starts without completion are preserved
separately rather than included in the completed-composition claim.

The comparator is the preceding deployed gas-accounting candidate, SHA-256
`3694f4ec562a2511d9c763745719497921084f5926ae65ec00f76666bd0e940a`.
Both runs use the same network, configuration, optimized release profile,
protocol scenarios and runner settings.

| Measurement | Previous candidate | L1 attribution candidate |
| --- | ---: | ---: |
| Verified sources | 870 | 870 |
| Source-bearing posts / physical blobs | 38 / 38 | 37 / 37 |
| Maximum sources in one Sync | 34 | 34 |
| Execution gas including all 520 L1 companions | 286,376,697 | 275,858,229 |
| Median held-to-canonical completion, seconds | 157.230 | 126.250 |
| p95 completion, seconds | 247.145 | 256.602 |
| First source accepted to last completion, seconds | 253.348 | 262.406 |
| Gas-budget / preparation-time cuts | 26 / 6 | 23 / 8 |
| Excluded attempts / canonically skipped target slots | 11 / 6 | 2 / 8 |
| Source-bearing settlement charges, xDAI | 0.399760577 | 0.207182078 |

This is a small observed packing difference and an accounting correction, not
evidence that the full burst became faster: median latency improved, while p95
and the completion of the entire wave became slower. Receipt gas was 3.67%
lower, but this sequential public-network pair does not isolate the causal
effect of the patch. Parallel source order, batch grouping, accumulated state,
setup history, preparation decisions and available L1 slots differ.

The apparent currency saving is especially unsuitable as a patch benefit. The
post tip changed from 1.44 to 1.0 gwei under the unchanged fee policy; companion
effective prices also fell, ranging from 8 wei to about 0.5 gwei in the new run
versus about 1.2 gwei previously. Source-bearing costs include entire posts and
all canonical companions. All captured paid posts, including setup/history,
cost 0.209170974 xDAI for the new run. Neither number includes standalone setup
transactions or counts refundable worker funding as expenditure.

Fee readiness also differed. Of the 520 inbound sources, 518 were signed with
`maxFeePerGas=15` wei; the other two had caps of about 0.5 gwei. Between 22:18:58
and 22:19:43, 291 distinct sources logged a transition to blocked because their
fee cap was below the L1 base fee. Every transition matches a 15-wei signed cap
and a quoted target base fee of 16 wei. All 291 later became ready at target
base fees of 12–15 wei and completed canonically. The existing implementation classifies this
as `SourceBlocked(Fee)`, retains the source, defers its nonce chain for that
drain, continues independent senders and requeues without an attempt penalty.
The remaining zero-target starts have no explicit terminal log and are retained
without inventing a reason. This is evidence of temporarily ineligible signed
sources, not proof of how much of the slower tail each condition caused.
Potentially redundant work before fee rejection deserves separate measurement;
the validated patch does not change source fee handling or the test's fee policy.

The runner's setup mining took 182.241 seconds and is separate from the timing
table. Polling bracketed first source acceptance at 137–140 unsettled L2 blocks,
versus 86–90 previously. No setup transactions shared source-post ranges in the
new run; two did in the preceding candidate. No extra setup-drain barrier or
scenario-specific production shortcut was introduced.

At 22:24:58 UTC the network remained synced with 25 peers and an empty queue;
the pre-deployment safe block still had the same hash and state root. Compact
measurements and evidence identities are in [l1-gas-attribution.json](l1-gas-attribution.json).
The full canonical evidence, every post/Sync association, fee quotes and timing
distributions remain under `artifacts/chiado-composer-next-20260915/`.

## What the preceding workload establishes

The earlier [full staged comparison](gas-staged.md) remains immutable baseline
evidence. All 1,076 recorded composition intervals in its candidate log have
matching proxy-call counts and aggregate target-gas sums. Of 518 outbound
composition executions, 142 included L2 callback work. Across all 518 executions,
the old aggregate target-gas term was 38,962,580; L1 destination frames account
for 30,858,764 and L2 frames for 8,103,816. The L2 share is 20.8% of this estimate term,
not of total post gas or paid fees. These counts include retries and rejected
candidates and must not be presented as unique sources or direct gas savings.

The separate exclusion review found that four source-bearing attempts targeted
completely empty L1 blocks. All four passed local ordered simulation and their
relay requests were acknowledged before the slot. Successful candidate posts
used `rbuilder-chiado`-labelled blocks; those four empty blocks had a different
fee recipient and an Erigon label. This directs further investigation toward
builder/proposer payload selection, but headers do not prove remote validation
or explain why a payload was not selected. Seven other exclusions were
history-only posts, six targeting nearly full blocks. No backoff or validation
rule is weakened from this evidence.

Repeated prefix hashing remains separate work. The current state object does
not retain the canonical block builder's receipt/execution accumulators, and
`finish` consumes the builder and applies block-close changes. Reusing execution
requires explicit checkpoint equivalence tests, including rejected groups,
rebuilds and active hardfork behavior. This patch does not replace canonical
hashing with an approximate cache or lower the universal proxy reserve.

Detailed attribution, exclusion and checkpoint analysis is retained under
`artifacts/chiado-composer-next-20260915/`.

## Manual workload

From the repository root:

```sh
bash scripts/run-chiado-staged-performance.sh
```

The helper uses the existing protocol `chain.env`, test wallets and network.
It checks chain IDs, deployed contracts, funding, queue state and worker nonces,
then holds the shared worker-pool lock throughout the run. The default command
has the exact 14 requested targets, 770 jobs, 870 cross-chain sources,
`PREPARE_PARALLEL=200` and `--no-verify` in the position required by upstream.
The existing profile's configuration precedence is preserved. Use an absolute
`DEVNET_ENV` override for a different prepared Chiado profile.

Readiness alone performs no funding, build or transaction submission:

```sh
bash scripts/run-chiado-staged-performance.sh --check
```

The upstream runner prints its directory under
`eez-core-protocol/tmp/e2e-staged-net/`. `timings.csv` contains phase durations;
`job-timings.csv` contains job timings. Protocol semantic assertions are disabled;
the manual helper does not run an independent follower or the separate canonical
audit collector. Run timing comparisons without concurrent builds/native tests.

After the measured wave, an authorized 0.35-test-xDAI transfer restored readiness
for the next manual run. The source wallet funds the upstream faucet when
needed; unused per-worker funding is returned to the faucet. The final readiness
check passed on both layers. Readiness must be checked again for later runs,
and the upfront reserve must not be reported as expenditure.
