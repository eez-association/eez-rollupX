# Claude review 3

Model reported by CLI: `claude-opus-5-5`. Completed successfully in read-only mode. Final design review; the requested target-rule sentence was incorporated afterward. This does not establish implementation correctness or production readiness.

I found no remaining design blockers. Plan A is coherent and can be implemented, subject to the G1–G5 gates. Design review alone doesn't show it will perform well enough or is safe for production; those questions stay with G4 and G5.

To confirm the two corrections I read two things only: `crates/eez-composer/src/prover_retry.rs` (lines 198–199 and 335–341) and `crates/eez-driver/src/timing.rs` (lines 239–305 and 340), plus the reserve sites in `composer.rs`.

## Your two corrections both hold

**1. The reserve is not counted twice.** `post_reserve()` adds `DELIVERY_RESERVE` (25 ms) to the measured finalization estimate. It never returns less than `POST_PROOF_RESERVE` (150 ms), and returns exactly 150 ms when there are no measurements. So 150 ms already covers delivery when the system starts. The replacement cutoff at `composer.rs:2035` uses the same rule: `POST_PROOF_RESERVE + submission_slack`. Review-2's `+ DELIVERY_RESERVE` was wrong, and proposal lines 169–176 now state it correctly.

**2. The startup check is consistent with the planning budget.**
- Today `preparation_time() = L1 − proof_window_open − proof_time − slack` (`timing.rs:239-244`).
- With the trigger offset at `P·L2`, that becomes `(K−P)·L2 − seed − slack`.
- Your strict check, `seed + slack + 150 < (K−P)·L2`, therefore guarantees `preparation_time() > 150 ms`, which is positive.
- The formula needs no change beyond swapping the offset. The deadline (target minus slack) and the planning budget subtract slack the same way, so it isn't counted twice.
- Running the check in Composer config/node wiring is correct. Composer already reads `emission.timing`, so the driver doesn't need to know about Composer.

## Your other changes are present and consistent

- **Decision order:** lines 69–72 run recover_failed → prepare_next_slot → authenticate → commit through cutoff → decide. The decision is keyed by target (including the L1 parent hash), generation and the cutoff block's hash. A local branch replacement or a new generation invalidates it; normal blocks built on top of the cutoff hash don't. That's correct.
- **Late head:** lines 80–85 decide once when the interval can actually run, keep the original deadline, build only the missing positions, and require "finalization/delivery reserve and a positive proof opportunity". That wording matches correction 1. Falling back to bounded structural catch-up is also correct.
- **Late arrivals:** lines 126–143 put late pure-always, fullness and cross-chain work at the next cutoff. They don't claim fullness is the only rule that can become due late, and they record the extra latency in the contract. Good.
- **EndTick:** lines 101–105 finish the boundary at wall time with the existing Empty mode, never from the pool, after rerunning all checks. An error is not treated as proof that an empty block is safe. Good.
- **Proof-start gate:** lines 164–167 swap the seed pre-gate for the exclusive-deadline gate plus shared pipeline admission, in explicit mode only. Legacy and non-exact-target paths are unchanged. Good.
- **Settings:** `future_count = K−P−1` and the conflict with `EEZ_PREPARATION_TIME_MS` are clear. The seed stays an estimator input only.

## One wording gap (not a design blocker)

Your summary says ordinary ticks run through the empty K "only under known/bounded replacement targets". The proposal doesn't say this. Lines 89–93 only say existing readiness, context, depth and outage bounds still apply, and that this is "not a promise to sequence indefinitely without L1 evidence". That's weaker than the rule you intend.

`sequencer.rs:621-627` currently stops ordinary ticks at `live_region_end`. This change replaces that stop with tick-through-K, so the rule should be explicit. Suggested sentence:

> Advance ordinary ticks to `K` only while a known target or one of the bounded (≤4) replacement targets exists; with no target, hold as today.

Without it, an implementer could reasonably build a free-running clock, which you've said is out of scope. It also gives G2 a concrete test.

## No hidden scope creep

There's no new scheduler, worker, durable state, change to the estimator, contract/DA/signer change or protocol bump. The planner is pure, and admission stays in the shared pipeline. Plan B is deferred with its rollback floor (the tolerant-reader release) and blockers listed, and the proposal doesn't claim any of them are met. Legacy mode is unchanged when the setting is absent.

## What plan A delivers

Plan A deliberately leaves every non-settling grid boundary empty. It doesn't put ordinary transactions in skipped sync positions, so one empty block per idle interval remains. Lines 26–30 and 302 say this and should stay. Whether to do plan B depends on the gap G5 measures.

## Readiness

With the one-sentence target rule added, the design is ready to implement in the planned sequence (lines 229–242). Release claims still need recorded G1–G5 evidence on the actual candidate.
