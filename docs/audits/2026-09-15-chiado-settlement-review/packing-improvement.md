# Cross-chain packing improvement

The [staged baseline](protocol-staged-wave.md) admitted a simultaneous
30-request wave as five Sync blocks (3, 3, 10, 10, 4 requests). The measured
limiter was preparation estimation; summing signed companion gas limits was
another constraint that would have prevented the full wave from reaching final
simulation. The baseline receipts alone do not prove a merged batch fits.

## Implementation and coverage

| Requirement | Implementation | Coverage |
| --- | --- | --- |
| Charge actual repeated prefix execution before selecting more groups | `composer.rs` computes each group's effect-prefix hashes during admission, caches only accepted groups, and checks positions against the canonical rebuild before using the hashes | Native mixed-direction, multiple-call, reverting-call, poison-middle and partial-consumption cases; independent signer and follower verification |
| Separate per-blob work from finishing work without multiplying fixed overhead | `admission.rs::Sample::tail_for` reserves current measured admission work or the previous finishing sample, whichever is greater; only smaller candidates scale the prior sample down; KZG scales separately with blobs | Injected-time tests cover a 30-group one-blob wave, a small prior post's fixed overhead, slower current work, an additional physical blob, deadline equality, stale samples and overflow |
| Avoid an extra count-limited post after a successful FIFO prefix | `Selection::fifo_bypassed` identifies older eligible work bypassed by canonical phase order; `AutomaticAdmission::finish` retains backoff for that case and for incomplete preparation or failed submission | FIFO bypass/source-block tests; successful-prefix and repeated-backoff unit tests; native exclusion, blocked-source, escrow and mixed-wave regressions |
| Distinguish source gas estimates, declarations and block headroom | `LocalChainClient` returns source simulation gas without changing its public receipt-status API; `PostBatchGasBudget` tracks estimated consumption and the largest preceding-source estimate plus next declaration | Unit tests cover high declarations with cheap execution, per-transaction rules, overflow, rejection without spending budget, and postBatch growth invalidating an earlier companion's headroom |
| Preserve final execution, ownership and derivation checks | Existing final native ordered bundle simulation, canonical transaction reconstruction, ECDSA validation and source reconciliation remain authoritative | Ethereum/Gnosis one-Sync 17-inbound/13-outbound tests, actual expensive-companion cuts, nonce checks, canonical receipts and independent follower convergence |

No configuration option, transaction format, contract, protocol source or
consensus limit changes. The operator guide and proposal describe the refined
estimator and the conditions that still require FIFO backoff.

Source composition bypasses proxy consumption. Its gas is therefore a planning
estimate, combined with the existing protocol-work projections, rather than a
canonical receipt measurement. The final native simulation still checks the
whole ordered bundle, complete effects, actual cumulative gas and each next
transaction's declared gas against the remaining block budget. A prediction
never authorizes dispatch of a failing candidate.

## Validation and executable identity

The final candidate passed **216 Rust unit tests**, **46 native integration
cases** and **220 Python helper tests** (four existing opt-in cases skipped).
Strict Clippy, Rust formatting, runtime/test builds and whitespace checks passed.
The native cases used the frozen composer, follower and signer binaries; the two
new one-Sync tests ran serially, followed by 44 selected regression cases using
three independent native fixtures at a time. Every final selected case passed
on its first invocation. No assertion was weakened or retry added.

The new `ethereum_fitting_thirty_request_wave_settles_in_one_sync` and
`gnosis_fitting_thirty_request_wave_settles_in_one_sync` cases settle 17 inbound
and 13 outbound requests in one post after a small warm-up. They check exactly
one dispatch, one canonical Sync block, successful source receipts, nonce and
state outcomes, and independent follower convergence. Their preparation took
281/496 ms and 153/487 ms respectively. The inbound declarations total 34M gas,
above the fixture's 30M block budget, while actual execution fits. These are
controlled counter workloads, **not** the 28 mixed protocol scenarios below or
Chiado's smaller live block budget.

The regression inventory includes automatic and explicit-window mixed waves,
real gas/blob capacity cuts, blocked fees/balances/escrow, exclusion backoff,
partial and same-L1-block consumption, restart, reorg, static/nested/multiple
calls, caught reverts, poison-middle handling, nonce dependencies, interval and
empty settlement, fee costs, and independent replay on both network profiles.
Exact case names, commands, executable identities, source hashes, and compact
live evidence are in [packing-improvement.json](packing-improvement.json).

The final candidate is based on `f46e40bad4d49e365ca6f62296ab07f57bd9b259`
with the recorded working-tree source changes. Runtime SHA-256 identities:

| Executable | SHA-256 |
| --- | --- |
| Composer | `f6e672458cc14f3c8bd2d01513baae71cc0a8cc6dd087642780fdba8ed12cb3c` |
| Follower | `9584d300d41a35e3df0f3a9ca9f4e6f61a2b0533ec5f76a80c02502c5f18a1a5` |
| Signer | `ac7e1a863b563f69c38ecde42b30355118a5bc05aaa9686a1746f7ee64248537` |

Commands and full logs are retained in
`artifacts/chiado-packing-fix-20260915-final/`:

```sh
cargo +1.94.0 test -p eez-composer -p eez-driver --lib --offline --locked -j 3
cargo +1.94.0 clippy -p eez-composer -p eez-driver -p eez-node-e2e \
  --all-targets --offline --locked -j 3 -- -D warnings
python3 -m unittest discover -s scripts/ci/tests
cargo +1.94.0 build -p eez-node -p eez-follower -p eez-proof-signer \
  --bins --offline --locked -j 3 --message-format=json
cargo +1.94.0 test -p eez-node-e2e --no-run --offline --locked -j 3 \
  --message-format=json
python3 artifacts/chiado-packing-fix-20260915-final/run-native.py smoke 1
python3 artifacts/chiado-packing-fix-20260915-final/run-native.py regression 3
cargo +1.94.0 fmt --all -- --check
git diff --check
```

The initial candidate passed the Gnosis one-Sync test but split Ethereum into
26 and four requests. Preparation of the 26-request batch took 265 ms against
495 ms available. This exposed fixed finishing overhead still being multiplied
by group count. The refined estimator uses current measured admission work for
growth, retains the previous finishing observation as a floor, and keeps the
physical-blob allowance separate. A deterministic test reproduces that boundary
and verifies that a genuinely slower admission still stops.

## Live Chiado comparison

The validated composer was deployed at **16:11:55 UTC**. A single counter
request warmed its volatile estimator after restart. The unchanged upstream
command then ran from `eez-core-protocol`, at clean submodule revision
`3c0cdf4955941a5c23639346932b55b33babebbd`:

```sh
bash script/e2e/run/network-staged.sh --no-verify all
```

All 28 scenarios sent their 30 source transactions (17 inbound, 13 outbound).
The independent analysis checked each canonical receipt and transaction
position, source nonce, source ownership generation, L1-to-L2 settlement
correlation, every settled L2 hash, safe advancement and an empty final queue.
For inbound sources, it checked execution after their corresponding post in
the same L1 block. Correlations are arrays and posts are ordered by L1 block
**and transaction index**; the analysis does not assume one post per L1 block.
Protocol scenario-specific semantic verification was skipped as requested.

| Run | Preparation window | Successful cross-chain posts | Requests per post | Acceptance → safe median / maximum |
| --- | --- | --- | --- | --- |
| Original baseline, 14:55 | 500 ms | 5 | 3, 3, 10, 10, 4 | 33.150 / 43.043 s |
| Packing fix, 16:13 | 500 ms | 2 | 17, 13 | **4.180 / 9.088 s** |
| Window experiment, 16:25 | 1,000 ms | 2 | 3, 27 | 43.669 / 43.680 s |

These are individual live waves, not a latency distribution over repeated
network conditions. Timings begin at composer ingress acceptance and end at
locally observed canonical safe advancement, not Ethereum-style finalized
consensus. Deployment/setup time is excluded from those latency values. Setup
settlements are recorded separately in the evidence. In the 16:13 run, some
deployment history was included in the first rich post, whereas the baseline
had already settled its deployments. The batch byte counts are therefore not
an equal-payload compression or fee comparison.

The 500 ms run settled in L1 blocks **23,070,889 and 23,070,890**, corresponding
to L2 Sync blocks **151,445 and 151,450**. Each post used one blob. Preparation
took 360/496 ms for 17 requests and 265/495 ms for the remaining 13. The first
cut was the preparation estimate; the second selection considered the complete
remaining queue. Actual bundle gas was 6,478,594 and 5,132,496 respectively.

### Why the larger window was not retained

The 1,000 ms experiment prepared 26 requests with **two blobs in 562/991 ms**.
Its first cut was the existing conservative gas projection, not elapsed time:
projected post gas 13,127,052; the next candidate's additional cost exceeded the
13,381,013 planning allowance. That allowance reserves the configured 20% gas
margin and the existing fixed planning margin under the 16,777,216 per-tx cap.
Final native execution of the 26-request candidate measured 8,795,827 poster
gas. Thus the projection is conservative; it does not establish that the full
30-request candidate exceeds consensus gas capacity.

Three exact target slots then did not exist on Chiado. Canonical timestamps
jumped from 16:25:30 to 16:25:40, 16:25:45 to 16:25:55, and 16:25:55 to
16:26:05. The corresponding bundles were sent at 16:25:33.748,
16:25:48.295 and 16:25:58.249, ahead of their target deadlines. This was not
late local preparation. Existing canonical reconciliation and submission
backoff retained the sources; final successful posts contained 3 and 27
requests. No failed post was counted as a successful settlement or charged
as included. The longer observed latency cannot be attributed to the window
change from this single, differently affected network sample.

The original **500 ms preparation setting was restored at 16:29:22 UTC**,
keeping the validated packing binary. Increasing the window alone did not
achieve one post for this workload. A final counter warm-up settled, both chains
advanced and the cross-chain queue was empty. The empty consolidation threshold
remains 300 L2 blocks, pure-L2 settlement remains `always`, admission remains
`auto`, and the execution fee ceiling remains 500 gwei. Each restart preserved
the prior canonical safe hash/state root and chain-data mounts. The beacon and
proof-signer containers were not restarted.

## Remaining limit and next optimization

The measured improvement is five successful cross-chain posts reduced to two
at the original timing setting, with complete canonical settlement. It is not
proof that every mixed workload fits one Sync, nor a sub-five-second guarantee.

The next useful optimization is a more accurate **pre-proof protocol gas
estimate**, particularly distinguishing new-proxy deployment from existing
proxy bookkeeping and deferred entries. Both incremental admission and full
candidate sizing use the conservative pins, so changing only one would move
the rejection later. Any replacement needs network-aware execution evidence,
new/established proxy and nested/deferred tests, the configured final gas margin,
and unchanged exact ordered bundle validation. The current change deliberately
retains those pins and all hard execution/blob/deadline guards. The distinct
question of reducing backoff after a genuinely skipped L1 slot also needs a
canonical-outcome-based design; relay failure alone cannot justify it.
