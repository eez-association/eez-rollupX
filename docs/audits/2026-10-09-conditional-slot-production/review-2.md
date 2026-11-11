# Claude review 2

Model reported by CLI: `claude-opus-5-5`. Completed successfully in read-only mode. This revises review 1 after the primary reviewer's challenge. It is reviewer output, not an accepted specification; final dispositions are in [README.md](README.md).

I recommend **(A)**: ship conditional cadence and the position cutoff P first, and keep every grid boundary empty. The draft's direction holds, but it needs the timing, ordering and recovery fixes below. The broader feature (B) is small in code but not in rollout: it needs two releases with a rollback floor and adds a new invariant across six components. B should be a specified follow-on, not hand-waved. A does not deliver ordinary transactions at skipped sync positions, and the draft already says so (`conditional-slot-production.md:26-30`). Keep that wording.

## 1. "Not due at P only changes through fullness" is wrong

Agreed, review-1 B2 overstated this:
- `PURE_ALWAYS` becomes due as soon as `latest_pure_block` is set (`settlement_policy/mod.rs:220-222`).
- `CROSS_CHAIN` depends on the queue (`:233-246`).
- Both can flip after P.

**Contract to adopt:** the decision at P uses only blocks up to and including P and the queue as it stands at P. Activity or arrivals after P, in any mode including `always`, are handled at the next cutoff. Don't "always prepare" in `always` mode; that brings back the burst we're removing. Only GENERAL, WINDOW_LIMIT, PURE_INTERVAL and the predecessor are exact at P. Document this extra latency (up to one interval plus K−P blocks) in `configurable-settlement-policy.md`.

## 2. Indivisible spans: what actually gets worse

What the code does today:
- The only path that falls back to a smaller prefix is `emit_historical_chunk`. It walks empty boundaries from the highest down, and only on `OverBudget{window, blob, gas}` (`composer.rs:5625-5629`).
- Any other error returns `Err` (`:5630`): proof failure, deadline miss, request or witness quota, cache miss. The next slot then retries the *largest* prefix that fits the budget.
- So today's K-sized fallback exists only for those three budget dimensions. For proof, quota and deadline failures there is no granularity liveness guarantee at all.

Request quotas and cold cache:
- With preparation enabled, ordinary blocks are prepared in chunks bounded by blocks, bytes and witness items (`eez-prover-client/src/lib.rs:413-439`). The final `Prove` carries the terminal plus cached ranges (`:632-665`).
- A cold-cache rebuild costs time proportional to the span. The worst span is already reachable today, because a live settlement after deferrals proves `cursor..terminal`.
- Without preparation (`preparation_max_blocks = 0`), the whole span goes in one request, but that is equally true of today's largest prefix.

**What B actually makes worse:**
- (a) It removes K-granularity for the three `OverBudget` dimensions unless the segment is guaranteed to fit them by construction:
  - bytes: segment-based DA admission;
  - window: WINDOW_LIMIT evaluated exactly at P, with the signer limit intersected (`config.rs:32-41`);
  - gas: anchor-only gas depends only on blob count.
- (b) It rules out adding "shrink on proof/quota failure" later.
- (c) It breaks the documented "intervals are independently postable" property (control-rpc §3.2).

What makes B safe without proof-time estimates or witness accounting: a boundary may be nonempty only if the segment already fits those three `OverBudget` dimensions. Accept (b) explicitly. Don't claim "no new risk".

## 3. Errors are not capacity results

Agreed. Only a typed result of "limit < empty under a matching, authenticated `(target, generation, interval_start_hash)`" may map to an empty B. Errors must keep failing as today (`sequencing.rs:88-90, 135, 177, 231`): absent or stale plan, unavailable ancestor, witness source not ready, L1 head missing. Withdraw review-1's "empty always fits".

## 4. Tolerant reader and bounded scans

Agreed:
- The reader must accept an unowned nonempty boundary in history without needing a plan record. The plan governs only *producing* a new boundary.
- The DA walk resets the base at the most recent empty grid block or owned boundary. It already walks `parent → base` (`sequencing.rs:170-191`), so no new scheduler is needed.
- Bound the walk at `max_blocks + K` and return an error (not Empty) beyond that, since it means an invariant was broken.
- Cache the walk by `(base hash, parent hash)`.
- During a posting outage, plans are Empty because the policy is due. So new segments don't grow; only `validate_retained_byte_segments` at startup scans the whole backlog, and it already does.
- The tolerant reader must ship **one release before** B. The rollback floor is that release, not A.

## Proof-start gate in position mode

Your proposal is cleaner, with one condition:
- Only one seed-based start gate remains for exact targets: the sequencer's `now + proof > deadline` (`sequencer.rs:83-84`).
- The Composer's anchor-only path already uses the shared `PipelineDeadline` (`composer.rs:5853-5857`). `proof_start_cutoff_ms` applies only when no deadline exists (`:6972-6976`).
- In position mode, use the `now >= deadline` branch that cross-chain already uses.

**Condition:** keep a *static* startup check: `seed + slack + POST_PROOF_RESERVE + DELIVERY_RESERVE ≤ (K−P)·L2`. Reject the config otherwise. Without it, every attempt is a deadline-limited proof that is predictably doomed and occupies the single-flight signer. This is a config sanity check, not a runtime gate; final admission still applies. Legacy mode stays unchanged.

## Draft fixes needed even under A

1. **Decision ordering.** Order the steps as: `recover_failed`, then `prepare_next_slot` (a timed-out replacement forces preparation, `composer.rs:2005-2069`), then authenticate the parent, then commit P, then decide. Key the decision by `(target, generation, hash of P)`. The draft (lines 66-73, 89) lists these steps but doesn't put `prepare_next_slot` before the decision.
2. **Late head.** The trigger is `head.timestamp + offset` (`slot.rs:245`). If the head arrives after the cutoff:
   - decide once on arrival, as long as the remaining window still covers slack and reserves;
   - otherwise take the ordinary path;
   - prepare only positions not yet committed.
3. **Ticks past `live_region_end`.** Ordinary ticks must now run to K−1 and produce an empty K at its wall-clock time (`sequencer.rs:621-627`). Do this only under a known or replacement target (`slot.rs:259-274`, max 4). With no target, hold as today; no free-running clock.
4. **Other `proof_window_open` consumers.** Derive the replacement window (`composer.rs:2033`), `preparation_time()` (`timing.rs:239`) and `nominal_preparation_ms` (`composer.rs:7002`) from P·L2.
5. **Failed prepared terminal.** On `EndTick` (`sequencer.rs:786`), let the tick at ts(K) commit K in Empty mode, never from the pool. This keeps R6 without waiting for catch-up.
6. **Contract text.** State the latency contract from item 1. Withdraw review-1's "fullness-only" argument and "empty always fits".

## B, specified for later

Invariant: B is nonempty only under a current Ordinary plan for that exact target and branch, and only when `segment(base → B+K)` fits all three:
- native stream bytes with K empties reserved (`total_blocks` = segment);
- `max_blocks`;
- WINDOW_LIMIT and GENERAL, evaluated exactly at P.

Recovery splits only at empty or owned boundaries, which is unchanged. Blockers:
- tolerant-reader release first;
- plan record readable by the payload job (`composer.rs:1679-1686`);
- control-rpc §3.2 edit;
- signer test of a nonempty intermediate boundary through cached ranges;
- accepting the loss of future shrink-on-proof-failure.

## Recommended scope

**Ship A** with release gates:
1. **G1 timing:** legacy parity across the presets, P validation, and the static seed/slack check.
2. **G2 sequencer:** decision ordering, late head, tick-through-K, EndTick fallback, no duplicate commits.
3. **G3 planner:** pure (no `PolicyState` mutation); the item-1 latency contract tested in `always`, interval and cross-chain modes.
4. **G4 process:** recovery, reorg, the 4-replacement path, follower replay.
5. **G5 matched load:** per-position inclusion, with the empty-boundary gap measured.

Decide on B from G5's measured gap.

**Changes before final review:** draft fixes 1–6, the static gate check above, release gates G1–G5, and a short "B follow-on" section holding the invariant and blockers above. That lets the deferral be judged without overclaiming.
