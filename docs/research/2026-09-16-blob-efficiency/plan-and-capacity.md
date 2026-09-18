# Implementation plan, remaining gaps and capacity estimates

The authoritative implementation plan is now [Derivable ChainOperation](../../proposals/derivable-chain-operations.md), with a [coverage ledger](../../proposals/derivable-chain-operations-coverage.md). It specifies the new 65,536-block signer default and incremental validation; references to 512 below describe the inspected pre-change code.

Status: research complete enough to begin a bounded prototype; no production
format or validated long-span capacity yet. The objective is to fit more history
in a postBatch by deriving ordinary blocks, with all cross-chain data unchanged
and no general-purpose compression. See [the design](derivation-first.md).

## Plan and acceptance gates

1. **Specify a versioned ordinary-block derivation profile.** Define the anchor,
   block count, sparse transaction-to-block mapping, execution environment, fork
   activation rules, count limits and canonical encoding. Retain the terminal
   Sync block and existing entry/group encodings. A block not covered by the
   deterministic rules uses an explicit full-block fallback. Decide the profile
   independently of local operator defaults; any configurable input not otherwise
   authenticated must be carried in the operations data.

   Gate: every header/body field is classified as input, deterministic output,
   profile constant, or unchanged cross-chain data. No unclassified field can
   silently default to zero. Unknown profiles and invalid offsets fail closed.

2. **Prove replay equivalence in a prototype.** Encode existing ordinary history,
   discard original headers from the replay input, and reconstruct it from an
   authenticated anchor. Compare every reconstructed block hash with the original,
   then compare the settled Sync endpoint with canonical L1 evidence. Give the
   signer the same shared derivation rules and validate the recipe against its
   authenticated execution window.

   Gate: exact hashes for empty spans, ordinary transactions, reverts, environment
   changes, fork transitions, and full-block fallback. Preserve all cross-chain
   encodings and exercise partial/deferred settlement, same-block competing posts,
   restart and reorg recovery. Tampering with counts, transaction placement, inputs
   or terminal data must fail validation. Existing historical payload tags remain
   readable. This gate is not satisfied by the earlier lossless header-sharing
   experiment, which does not rerun the EVM.

3. **Integrate exact size accounting and bounded replay.** Update operations
   encoding/decoding, signer DA checks, follower execution-input handling, composer
   estimates, ordinary interval reservations and final size checks together.
   Preserve the cross-chain path and existing settlement triggers. Bound decoded
   counts before allocating vectors or entering replay loops; use checked range
   arithmetic and enforce block/work limits independently of byte limits.

   Gate: admission estimates never under-reserve, including sparse-map length
   boundaries, environmental exceptions, fallback blocks and terminal data. A
   candidate cannot fit the composer but violate the configured proof window.

4. **Measure and select a supported maximum span.** Exercise 512, 1,024, 4,096,
   10,000 and, only if earlier results support it, 50,000-block spans. These are
   experiment targets, not promised capacities. Include nonempty workloads as well
   as idle history. Measure witness creation/retention, streamed request size,
   peak memory, signer execution, follower catch-up and end-to-end preparation.

   Gate: selected limits fit the actual L1 target-slot proof/preparation deadline
   and safe-head latency policy, with margin. Run existing cross-chain regressions
   unchanged. Coordinate version activation across composer, signer and follower;
   retain historical decoding. This document does not change live limits.

## Concrete gaps discovered

| Gap | Why it matters / resolution |
|---|---|
| Execution-profile rules are not yet normative | Current zero randomness/empty withdrawals are implementation behavior. Specify their derivation and fork boundaries; retain variable inputs. |
| Replay equivalence is untested for the proposed format | Existing round trips preserve bytes but do not prove that omitted fields regenerate correctly. Run gate 2. |
| Signer defaults to **512 blocks/request** | `config.rs` sets this independently of blob size; `window.rs` rejects a larger declared span. At two seconds this is about 17 minutes. Actual deployments may override it; no running deployment limit was verified here. |
| Proving data remains large | The signer receives and validates full blocks/witnesses even if DA carries a compact recipe. Defaults also bound requests to 512 MiB and 1,000,000 witness-array items. DA savings do not imply witness savings. |
| Time budgets remain | Signer defaults: 120-second stream-idle timeout and 600-second total request deadline. The composer's selected L1-slot deadline may be much tighter. Increasing a timeout is not a substitute for measuring execution time. |
| Empty history still requires execution | System transitions, fees and block-hash history evolve. A compact count cannot authorize skipping that work. |
| Other policy triggers still settle | `max_unsettled_l2_blocks`, ordinary time policy and cross-chain work can make a post due before blobs fill. The design removes a DA pressure source; it does not override those triggers. |
| Existing estimates reserve full headers | Update them alongside the codec, or the old estimates can keep forcing settlement. |
| Sparse counts increase amplification | Bound block counts/allocations and range arithmetic; tiny DA must not request unbounded follower/prover work. |
| Terminal size is workload-dependent | Full Sync plus unchanged cross-chain entries/semantics still consumes capacity; reserve actual bytes rather than relying on the example below. |

Sources: [signer defaults](../../../crates/eez-proof-signer/src/config.rs),
[request block limit](../../../crates/eez-proof-signer/src/window.rs),
[settlement triggers](../../../crates/eez-composer/src/settlement_policy/mod.rs),
[witness generation](../../../crates/eez-driver/src/witness.rs).

## Capacity estimates

There is no workload-independent blocks-per-post number. Removing headers shifts
DA growth from elapsed block count toward transaction bytes and variable inputs.

Using the user's **six-blob allowance**, at current packing:

```text
total capacity = 6 × 126,976 = 761,856 bytes
ordinary-history budget = total capacity − unchanged terminal/cross-chain/framing bytes
ordinary blocks ≈ floor(ordinary-history budget / average encoded ordinary bytes per block)
```

For a transparent example, reserve **one blob-equivalent, 126,976 bytes**, for
the terminal block, cross-chain data, profile and framing. This is a byte-budget
assumption, not a dedicated physical blob, measured requirement or guaranteed
upper bound. It leaves **634,880 bytes** for ordinary history. The average below
includes signed transactions, block placement, and any per-block environment or
fallback overhead, amortized across empty blocks too.

| Average retained ordinary bytes per block | Ordinary blocks fitting the byte budget | History at 2 seconds/block |
|---:|---:|---:|
| 10 | 63,488 | 35 h 16 min |
| 100 | 6,348 | 3 h 32 min |
| 250 | 2,539 | 1 h 25 min |
| 1,000 | 634 | 21 min |

Add one retained terminal block to obtain the total block count in the modeled
postBatch. These are **arithmetic DA scenarios**, not replay benchmarks, typical
workload assertions, or operational capacity promises. Six blobs must actually
be available to this post after all applicable limits and companion usage.
The deterministic calculations and assumptions are saved in
[capacity-scenarios.json](capacity-scenarios.json).

For fully empty ordinary history under a fixed deterministic profile, the average
per-block formula is inappropriate: one span count replaces per-block records.
**10,800 empty blocks (6 hours)** and **43,200 (24 hours)** need essentially the
same span metadata, apart from count length. Either can fit the DA representation
easily if terminal data fits and no environment exceptions/fallbacks are required.
Neither is claimed to fit the existing signer/runtime limits or deadlines.

The observed median current empty-block item is 764 bytes. It consumes an
illustrative six-blob budget in about 33.2 minutes before other payload bytes.
Eliminating this per-empty-block cost directly addresses the stated problem.
For busy blocks, transaction data remains, so high transaction-byte rates can
still exhaust blobs quickly even after all derivable headers are omitted.

**Practical maximum = the minimum of DA capacity, accepted proof-window size,
witness/memory capacity, measured execution time, and applicable settlement
policy bounds.** The first operating cap should come from the long-span tests,
not the largest number in the DA table. With an unchanged default signer, a
single request remains capped at 512 blocks regardless of the new encoding.
