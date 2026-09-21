# Configurable settlement policy implementation and coverage

Source of truth: [proposal](configurable-settlement-policy.md). Implementation
baseline: `71a8f52` (includes partial-consumption reconciliation, PR #16).
Status: implemented and validated in native tests, Kurtosis, and public Chiado
runs. The original 44 requirement rows and 63 acceptance cases have recorded
evidence below. Later extensions and revision-specific results are identified
separately; results for an older executable do not validate later source edits.

The 2026-09-17 new-network revision makes settlement policy mandatory and removes
the disabled-policy compatibility path. Rows and evidence below that describe
opt-in activation or disabled behavior are historical. Current configuration is
documented in [the operator guide](../configurable-settlement-policy.md).

Subsequent live investigation found a
[cross-chain nonce-gap liveness failure](../audits/2026-09-15-chiado-nonce-gap/README.md).
The [queue-liveness correction](cross-chain-queue-liveness.md) repairs admission
after rollback, ordered recovery, explicit readmission, and independent sender
progress. Its [implementation audit](../audits/2026-09-15-cross-chain-queue-liveness/README.md)
records 89 passing native scenarios, six canonical Chiado requests, independent
follower convergence and deployment on September 15. The original incident's
initiating event remains unproven; the reproduced failures and final correction
are distinguished from that historical uncertainty.

The 2026-09-15 bridge-latency follow-up in proposal §4.2 has a separate
[implementation and validation mapping](../audits/2026-09-15-chiado-settlement-review/bridge-latency-fix.md).
It separates physical-blob preparation cost, adds an optional precise
preparation allowance, and reclassifies a caught-up, aligned future terminal
as steady only after the existing guards pass. Its current validation status
is recorded in that audit; earlier executable results are not substituted.

The later [packing refinement and coverage](../audits/2026-09-15-chiado-settlement-review/packing-improvement.md)
extends §4.2 and §6.3: effect-prefix work is measured during admission, fixed
finishing overhead is not multiplied by group count, successful FIFO prefixes
avoid redundant count backoff, and companion planning distinguishes estimated
consumption from declared headroom. Its final binary passed 216 unit tests and
46 native cases, including one-Sync fitting workloads on both profiles. The
separate live 28-scenario wave required two posts; the audit preserves its
remaining gas-estimation constraint and the timing experiment's skipped slots.

The subsequent [bounded admission reserve audit](../audits/2026-09-15-chiado-settlement-review/shaped-admission.md)
records measured simple-entry estimates, canonical prior-queue cleanup and
per-occurrence existing-proxy evidence. It preserves the disabled-policy path,
network limits and final complete-bundle checks. Validation includes 194
composer unit tests, 42 contract test runs across three forks, six native
scenarios and the exact 870-source Chiado cohort. Two consecutive 40-source
waves fit one Sync each on both native profiles. Public peak packing rose
from 34 to 39, but aggregate post count and gas increased; this is not recorded
as a general mixed-workload efficiency improvement.

The [result-size reserve refinement](../audits/2026-09-16-chiado-result-reserves/README.md)
selects measured deferred-entry and cleanup reserves for packed, one-slot and
two-slot result storage. It adds no configuration and preserves the existing
eligibility gates and final complete-bundle checks. Its release passes 196
composer unit tests, 45 contract test runs and eight native cases. Controlled
Ethereum tests now fit both consecutive 45-source waves; the old release split
the second into 37 and 8. The exact public Chiado workload completes all 870
sources with no audit gaps, a largest observed Sync of 43 versus 39, unchanged
38 source posts and 2.90% less measured bundle gas. The audit distinguishes
those observations from fee/inclusion effects and explains why further
position-dependent storage credits were left out of this iteration.

The 2026-09-14 ordinary-sequencing extension in proposal §6.5 has a separate
[implementation and test mapping](../audits/2026-09-14-sequencing-da-budget/README.md).
It replaces AC38's former accept-then-report behavior for newly submitted
ordinary data; existing incompatible history still uses explicit error paths.

The [Chiado stress audit](../audits/2026-09-14-chiado-crosschain-stress/README.md)
records 1,344 canonically settled cross-chain requests, authenticated blob data,
and independent follower parity. The [recovery audit](../audits/2026-09-14-chiado-capacity-recovery/README.md)
retains the separate unresolved ordinary-pool delay and its evidence. These
testnet results do not claim a universal packing maximum or mainnet deployment.

## Compatibility interpretation

The later implementation scope supersedes compatibility with existing networks:
this is a new network with one settlement policy from genesis. The
`EEZ_SETTLEMENT_POLICY_ENABLED` toggle and disabled-policy runtime path are
removed. Every deployment must select its pure mode, general threshold, and fee
ceilings explicitly; omitted or conflicting settings fail startup. The
proposal's illustrative 15 minutes / 150 blocks / 10% remain profile choices,
not implicit production defaults.

## Architecture at the starting baseline

* The sequencer produces Live, Future, and Sync blocks using `RollupTiming`.
  `SyncSlotComposer::compose_sync_slot` currently returns `None` to request a
  mempool fallback. Enabled policy needs a distinct end-tick outcome.
* The composer currently posts empty batches unconditionally, parses admission
  limits per tick, and uses a 300-block historical split. Its existing ECDSA
  proof, witness retention, held pool, and optimistic ledger remain authoritative.
* `L1CanonicalHead` and the deriver already publish validated per-transaction
  evidence, including multiple transactions per L1 block and partial endpoints.
  The policy additionally needs a canonical timestamped commitment anchor.
* Native encoding already supports multiple blobs. Composer, transport,
  follower, and signer still share a fixed six-blob admission bound.
* Local target execution and ordered composition already exist. Production
  posting still has smoke-test fee defaults and signs with the gas ceiling.
* Existing Rust integration tests, native test runner, and Kurtosis profiles are
  the integration infrastructure; no parallel framework is introduced.

## Normative requirement mapping

Each item covers the cited proposal sections as well as their acceptance cases.
Paths below are relative to `crates/` unless stated otherwise.

| ID / requirement | Implementation location | Automated evidence required | Config/docs | Status |
| --- | --- | --- | --- | --- |
| R01 Opt-in legacy compatibility; one authoritative enabled evaluator | `eez-composer/src/settlement_policy/`, composer; node | `absent_settings_preserve_legacy_defaults_and_old_cap`; operational default tests; five disabled-policy native partial/restart/resume regressions | activation and migration | unit/native verified |
| R02 Exactly always/interval; positive interval; supplied inactive interval validated (§3.1) | `eez-composer/src/settlement_policy/config.rs` | `modes_and_inactive_interval_are_validated`, `enabled_defaults_are_explicit_and_fullness_is_off` | [operator configuration](../configurable-settlement-policy.md) | unit verified |
| R03 Positive N unchanged by Sync rounding; optional fullness off or 1..10000 (§3.1,4) | `eez-composer/src/settlement_policy/{config.rs,mod.rs}` | `rejects_invalid_numbers_and_disabled_or_obsolete_settings`, `general_wins_on_both_grids_without_rounding_n`, fullness boundary tests | [operator configuration](../configurable-settlement-policy.md) | unit verified |
| R04 Startup-only settings, no ignored options, existing source precedence (§3) | `eez-composer/src/composer/config.rs`; `eez-node/src/lib.rs`; Compose profiles | startup settings tests; `scripts/check-settlement-policy-config.py` (both profiles, legacy/enabled, actual Compose precedence) | [operator configuration](../configurable-settlement-policy.md) | config/component verified |
| R05 Depth 0 or >= N+K-1; reject obsolete batch cap when enabled (§3.1) | `settlement_policy/config.rs`; `composer/config.rs` emission settings | `depth_bound_preserves_grid_and_uses_checked_arithmetic`; old/disabled cap tests; Compose absence checks | [operator configuration](../configurable-settlement-policy.md) | unit/config verified |
| R06 User admission limit parsed/validated once; catch-up/prover guards independent (§3.1,6.4) | composer construction | `ComposerSettings`; native `more_than_three_hundred_blocks_settle_in_one_native_batch`; prover failure test added | manual prover sizing | unit/native verified |
| R07 Canonical anchor includes L1 hash/time/transaction position and exact L2 endpoint (§3,4,8) | `eez-l1/src/l1_canonical_head.rs`; `composer/policy.rs` | canonical-head identity tests; four native policy partial/resume cases including two posts in one L1 block | [operator configuration](../configurable-settlement-policy.md) | unit/native verified |
| R08 Registration/genesis bootstrap; unavailable evidence blocks readiness (§3.2) | `eez-l1/src/l1_reader.rs`; `composer/policy.rs` | `registration_bootstrap_uses_canonical_event_identity_and_timestamp`, missing/invalid/reorged evidence tests (7 reader tests) | [operator configuration](../configurable-settlement-policy.md) | unit verified |
| R09 Source completion/receipt alone never advances policy anchor (§4,8) | canonical head/reconciliation | canonical-head no-advancement tests; native first-source omission, partial cost and same-block resumption | none | unit/native verified |
| R10 OR rules; all simultaneous reasons form one candidate (§4) | `eez-composer/src/settlement_policy/mod.rs` | `truth_table_is_deterministic_across_independent_rule_combinations`; simultaneous rules test | [operator configuration](../configurable-settlement-policy.md) | unit verified |
| R11 Included pure transactions across intervening blocks; reverted included counts; pool alone does not (§4,5) | activity index + composer | native always/interval and restart cases; reverted-activity fault test added | none | native verified (included revert retained through prover outage) |
| R12 Target L1 time vs canonical settlement time; checked units, no universal slot conversion (§4,7) | policy evaluator; `Submitter::next_policy_target` | `timer_rounds_neither_target_nor_interval_to_l1_slots`; exact interval/both-grid tests; Ethereum 12s/2s Kurtosis and native Gnosis 5s/1s | [operator configuration](../configurable-settlement-policy.md) | unit/native verified |
| R13 Finite in-memory drain, no endless extension on empty ticks; intermediate prefix retains obligation (§3.3,4) | `PolicyState` finite drain; canonical hash coverage | finite policy tests; native `blob_overflow_drains_largest_empty_prefixes_without_resetting_the_interval` | [operator configuration](../configurable-settlement-policy.md) | unit/native verified |
| R14 Hash-based coverage/reorg; same-height partial replacement not cursor-only completion (§4,8) | policy index/accounting integration | `canonical_publication_and_reorg_restore_the_original_timer_and_rebuild_pending_activity`; index/ownership reorg tests; four native partial/resume cases | none | component + partial native verified |
| R15 Restart drops intent only, rebuilds anchor/activity with current settings (§3.3,8) | index initialization, existing recovery | native current-interval restart, intermediate-prefix restart, startup cap rejection/recovery, and policy partial restart while relay response is held | no journal | unit/native verified |
| R16 No zero-progress posts except valid commitment recovery (§4) | evaluator/composer | `empty_sync_and_mempool_absence_do_not_imply_settlement`; unchanged-commitment anchor test; native same-height partial/resume | none | unit/native verified |
| R17 Fullness uses unpadded encoded whole range / all B usable blobs; checked integer arithmetic (§4.1) | payload/blob sizing; activity index; policy fullness arithmetic | `fullness_is_aggregate_integer_soft_trigger_including_empty_history`; exact native/RLP length tests | [operator configuration](../configurable-settlement-policy.md) | unit verified |
| R18 Fullness soft overshoot, empty history eligible, never padding/admission cap (§4.1,6.5) | packing/evaluator | aggregate fullness arithmetic; native `fullness_overshoot_posts_all_fitting_blobs_without_waiting_for_other_rules` | soft target docs | unit/native verified |
| R19 Incremental hash/codec-keyed activity and byte index; idle path avoids expensive work (§4.1,9) | composer index | index new-block/reorg/codec tests; native interval suppresses preparation/fees/submission; cache-pruning append/reorg assertions passed in the 402-test library sweep | metrics | unit/native verified |
| R20 Steady X composition; past/capped catch-up and late modes hold fresh X but allow due history (§4.2) | composer Sync integration; future-terminal recheck in Sequencer | driver outcome matrix; native late Sync below 300 and finite predecessor; [September 15 refinement](../audits/2026-09-15-chiado-settlement-review/bridge-latency-fix.md) | optional preparation allowance described in refinement | component/native verified; refinement also verified on Chiado |
| R21 Explicit no-emission outcome; empty Sync and failed/stale build never pool fallback (§4.2) | driver slot/sequencer/composer | `explicit_policy_outcomes_never_use_sync_pool_fallback_in_any_slot_mode` (12 outcomes); native idle suppression | none | component/native verified |
| R22 Existing one-in-flight/nonce/ownership reconciliation precedes new attempt (§3.3,5,8) | composer/optimistic/submitter | `policy_restart_after_partial_keeps_accounting_and_rebuilds_intent` crashes after send while relay response is held; canonical adoption/ownership and submitter deadline tests | no new journal | unit/native verified |
| R23 Executable/prerequisite/temporary/capacity/invalid X classification; invalid pool not perpetual empty trigger (§5) | cross-chain admission | finite predecessor ownership/cancellation tests; native underpriced/underfunded recovery and complete-group gas deferral; finite predecessor native test | diagnostics | unit/native verified |
| R24 Reuse ECDSA proof path/retries; stale target reproves as needed (§4.2,5) | composer/prover retry/submitter | native historical multi-prefix ECDSA drain; ResourceExhausted/Unavailable prover recovery; submitter missed/reorged target tests | no new worker/timeout | unit/native verified |
| R25 Effective B=min(tx,block-own companions,transport,optional cap), no target/other-user prediction (§6.1) | `eez-blob-codec/src/limits.rs`; `Composer::posting_blob_allowance` | network/transport/operator/companion minimum and fork tests | [operator configuration](../configurable-settlement-policy.md) | unit verified |
| R26 Active outgoing vs containing-block historical fork limits; cap not consensus admission (§6.1,7) | profile-aware reader/transport/signer; additive L1 target context | nine-blob transport/authentication/RPC and containing-fork tests; native eight-blob post with live follower | [operator configuration](../configurable-settlement-policy.md) | component/native verified |
| R27 Whole-range one multi-blob batch; exact blob/gas overflow only causes split (§6.2) | composer packing | native multi-blob soft overshoot with both transactions replayed from one canonical post, eight-blob post, two-prefix drain, and 315-block single post | none | native verified; soft-overshoot oracle correction tracked in [release validation](../../observability/validation/release-runtime.md) |
| R28 Largest valid prefix; empty grid Sync terminal; atomic groups; recompute metadata (§6.2) | historical packing/composition | native two-prefix drain, ordinary admission/cap-increase checks, retained-cap startup rejection; exact follower replay | explicit unpostable error | unit/native verified |
| R29 Fits/over-budget/unavailable distinct; cheap sizing before expensive artifacts (§6.2) | preparation/packing errors | index missing/inconsistent history tests; canonical witness missing-input test; native prover resource/transport and fee-cap recovery without splitting | diagnostics | unit/native verified |
| R30 Whole ordered bundle per-tx and block execution/blob/gas/balance/nonce validity (§6.3) | `composer/posting.rs::simulate` / `run_bundle`; native Ethereum/Gnosis block executors | native ordered executor tests; two inbound companion gas-stress cases, rich two-companion settlement and partial receipt accounting | [operator configuration](../configurable-settlement-policy.md) | unit/native verified on Ethereum/Gnosis |
| R31 Active gas rules and target execution/EIP150; verify effects, not outer success (§6.3) | `composer/posting.rs::EffectInspector` / `run_bundle` | `outer_success_without_complete_effects_is_rejected`; real static lookup inspector; native low-byte expensive targets/new proxies and partial applied roots | [operator configuration](../configurable-settlement-policy.md) | unit/native verified |
| R32 Group admission reduction before irreparable DA shrink (§6.3) | cross-chain composer | native `settlement_policy_gas.rs`: complete-group target/companion gas deferral and indivisible oversized group | reason metrics | native verified on Ethereum/Gnosis |
| R33 Fee-history estimator by default, validated fixed override/bounds (§6.3.1) | `PostingFeeConfig`; `PostingRuntime::quote`; early `ComposerSettings` | fee/config tests; native receipt tests assert bounded dynamic pricing | [operator configuration](../configurable-settlement-policy.md) | unit + native default verified |
| R34 Separate execution, blob, and priority price ceilings; bounded raw priority estimation; checked execution/blob/gas margins (§6.3.1) | `eez-l1/src/posting_fees.rs`; `ComposerSettings` | independent priority bounds, self-feedback regression, checked ceil/overflow, early invalid override and native bounded-priority receipt tests | [operator configuration](../configurable-settlement-policy.md) | unit/config/native verified; priority correction below |
| R35 Quotes bind network/canonical sample/target; refresh unsent on changes, no parallel repricing (§6.3.1) | existing observer; `replace_prepared_post_batch`; `send_bundle_quoted` | unsent repricing/ownership tests; `quoted_dispatch_checks_network_head_and_target_before_recording_or_sending`; canonical reorg integration | [operator configuration](../configurable-settlement-policy.md) | component verified |
| R36 Reprice only poster tx; preserve companions; insufficient caps retain due and never capacity-split (§6.3.1) | codec reprice helper; optimistic ledger; final preflight | repricing preserves authorized content in both wrappers; native underpriced/funded-companion recovery and fee-cap retry | [operator configuration](../configurable-settlement-policy.md) | unit/native verified |
| R37 Estimated gas plus margin validated for complete effects; changes revalidate/reprove bound inputs (§6.3.1) | `PostingRuntime::finalize`; `L1SlotState` posting context; `SkipTopFrame` | `final_gas_and_fee_changes_require_complete_effect_revalidation`; priced frame context/no-fee test; native gas-stress and declared-gas/receipt checks | [operator configuration](../configurable-settlement-policy.md) | unit/native verified |
| R38 Estimated vs receipt cost, full physical blob charge, separate reserve/poster/companions (§6.3.1,9) | receipt/cost telemetry | cost ledger canonical/reverted/unincluded tests; native always, partial, and bounded-priority cost receipts | native units + normalized metrics | unit/native verified |
| R39 Reorg-aware cost attribution and same-workload cost evidence both profiles (§6.3.1,12) | integration harness/cost records | `reorg_retracts_costs_and_same_height_replacement_is_accounted_once`; native equal-range cost matrix on both profiles | artifacts, no promised savings | component/native verified |
| R40 Ethereum and actual Gnosis/Chiado adapter/fork/timing support (§7,12) | native node Gnosis EVM retained in `ChainProvider`; per-target EVM environment | native Gnosis always + independent replay; Ethereum/Gnosis cost receipts; pinned chain/executor tests; public Chiado audits | [operator configuration](../configurable-settlement-policy.md) | unit/config/native and Chiado verified |
| R41 Existing witness retention and replay fallback; missing data preserves work (§8.2) | witness source/composer | native retained/315-block witness history; canonical witness replacement/missing-input tests; native empty-cache replay | no retention redesign | unit/native verified |
| R42 Existing encoding/serialization/CLI unaffected outside opt-in; reversible config (§1,7,11) | all touched paths | existing codec/golden signer fixtures; required target RPC and missing-context rejection tests; disabled defaults and native legacy partial regressions | migration without data conversion | unit/native verified |
| R43 Observability due reasons, lateness, age, bytes/blob/gas/cost, errors and applied coverage (§9,12) | tracing + harness | native assertions on due/idle reasons, timing, admission limits, actual blob count, exact estimates, applied coverage and receipt-normalized costs | operator documentation | unit/native verified |
| R44 All relevant fmt/lint/build/tests, native receipt/root/follower evidence and final source audit (§12) | existing build/test infrastructure | recorded checks below; final library/native/signing/CI checks, frozen runtime, source review, and Kurtosis receipt/DA/replay evidence | final coverage audit | verified |


## Acceptance case ledger (proposal §10)

Each case must link to executing coverage before completion.

| ID | Situation | Expected result | Implementation / test evidence | Status |
| --- | --- | --- | --- | --- |
| AC01 | Empty scheduled Sync; no pending activity; general and fullness thresholds not reached | No settlement | native `always_skips_idle_syncs_then_settles_included_activity_and_replays`; evaluator empty-Sync test | native verified |
| AC02 | At least one admissible cross-chain transaction; all timers below threshold | Settlement due at the next eligible opportunity | native `cross_chain_overrides_interval_and_validates_two_ordered_companions` (two companions) | native verified |
| AC03 | `always`; an earlier L2 block in this slot has a pure transaction; terminal Sync empty | Settlement due | native always test; `intervening_pure_activity_is_due_always_even_with_empty_terminal` | native verified |
| AC04 | `always`; submission missed; next slot is empty | Previous due activity is retried, not forgotten | `always_activity_extends_only_for_new_obligations`; native `fee_rpc_outage_then_dropped_bundle_preserves_due_activity_until_canonical_replay` | unit/native verified |
| AC05 | Idle general-rule batch stays in flight through several empty slots, then succeeds | Clear the covered intent; no follow-up batch unless the fresh general threshold or another rule is due | `failed_or_delayed_submission_does_not_extend_periodic_drain` | unit verified |
| AC06 | Pure interval batch succeeds after newer pure transactions arrive | Apply the new settlement timer to uncovered activity; do not inherit an expired old timer for already-completed work | `newer_timed_activity_uses_new_anchor_after_original_drain_completes` | unit verified |
| AC07 | `interval`; pure transactions pending; interval, general, and fullness thresholds not reached | No settlement yet | native `interval_preserves_execution_but_waits_below_all_thresholds`; interval evaluator tests | native verified |
| AC08 | `interval`; pure transactions pending; interval elapsed from latest L1 settlement | Settlement due | native interval posting from registration timestamp | native verified |
| AC09 | `interval`; interval elapsed but no transactions and general threshold not reached | No settlement from the pure rule | `interval_uses_latest_l1_settlement_and_requires_pure_activity` | unit verified |
| AC10 | All blocks empty; general block threshold reached | Consolidation due | native general empty consolidation | native verified |
| AC11 | Two-second L2 cadence; N=150; pure interval=15 minutes; fullness off; no cross-chain work | General rule becomes due around five minutes, before the pure interval | `general_wins_on_both_grids_without_rounding_n` | unit verified |
| AC12 | One-second L2 cadence; N=150; pure interval=15 minutes; fullness off; no cross-chain work | Same policy becomes due around two minutes thirty seconds | `general_wins_on_both_grids_without_rounding_n` | unit verified |
| AC13 | N is crossed between eligible Sync boundaries | Attempt at the next eligible boundary; report actual age | same timing test + native general threshold; age diagnostic asserted | unit/native verified |
| AC14 | Nonzero speculative-depth limit is below `N + K - 1` | Reject startup with the required minimum; do not add an early-settlement trigger | `depth_bound_preserves_grid_and_uses_checked_arithmetic`; startup config tests | unit verified |
| AC15 | Speculative depth is zero or satisfies the compatibility bound | Preserve the configured guard; the general trigger is reachable on the existing Sync grid | same depth/config tests; Compose profiles preserve depth 0 | unit/config verified |
| AC16 | Obsolete `EEZ_MAX_BLOCKS_PER_BATCH` remains configured | Reject startup with migration guidance; do not retain a hidden 300-block split | enabled old-cap rejection and legacy default tests; actual Compose absent-key checks | unit/config verified |
| AC17 | Pure transaction arrives after the interval already elapsed; general threshold has not fired | Due at the next eligible opportunity; do not grant a fresh full interval | interval equality/late activity evaluator test; native elapsed registration interval | unit/native verified |
| AC18 | A cross-chain or empty consolidation succeeds before the pure timer expires | Timer anchor advances to that settlement | canonical anchor tests; native empty and rich settlement anchors | unit/native verified |
| AC19 | Due data needs three blobs; target network and transaction permit three | One batch with three blobs | native one-batch two/eight-blob cases; codec exact counts | component/native verified |
| AC20 | Fixture permits more than six blobs and the fitting candidate needs them | Composer, transport, signer, and follower all accept the same permitted count | native `retained_backlog_posts_more_than_six_blobs_in_one_prague_batch_and_replays` | native verified |
| AC21 | Same data needs three blobs; effective target allowance is two | Blob-driven split at valid boundaries; drain the remainder | native `blob_overflow_drains_largest_empty_prefixes_without_resetting_the_interval` | native verified |
| AC22 | Ten blobs allowed; threshold=10%; estimated usage=126,976 bytes | Fullness makes settlement due; nominally one full usable blob is 10% of the allowance | `fullness_is_aggregate_integer_soft_trigger_including_empty_history` | unit verified |
| AC23 | Ten blobs allowed; threshold=10%; estimate jumps from 9% to 12% | Settle the fitting candidate; exceeding the soft target is allowed | same fullness boundary/overshoot tests; native soft-overshoot two-blob batch | unit/native verified |
| AC24 | Six blobs allowed; threshold=10%; estimate=76,185 versus 76,186 bytes | Below target versus due in the integer predicate; actual inclusion need not hit this byte count | aggregate fullness one-byte boundary test | unit verified |
| AC25 | Fullness becomes due with data needing several blobs but fitting both budgets | One batch with all required blobs; do not split at the trigger percentage | native `fullness_overshoot_posts_all_fitting_blobs_without_waiting_for_other_rules` | native verified |
| AC26 | Fullness disabled; other rules not due | Wait; fullness does not create an implicit posting rule | config/evaluator off tests; native interval no-post case | unit/native verified |
| AC27 | All blocks empty; fullness threshold reached before the general block threshold | Consolidation due from fullness | aggregate fullness empty-history tests | unit verified |
| AC28 | Allowance changes while work is pending in the same process | Recompute using target rules; preserve already due canonical intent | `changing_allowance_does_not_cancel_existing_drain` | unit verified |
| AC29 | Small cross-chain batch fits the blobs but exceeds the L1 gas budget | Limit admission or split only at valid boundaries; an indivisible oversized group is deferred/rejected explicitly | native `settlement_policy_gas.rs`: two low-byte expensive outbound groups; indivisible oversized group | native verified on Ethereum/Gnosis |
| AC30 | Simulation or receipt succeeds while an intended entry is skipped | Do not classify the full range/group as settled | ordered effect inspector tests; native policy partial/same-height replacement | component/native verified |
| AC31 | More than 300 blocks fit in the permitted blobs and validated resource budget | One batch; the old 300-block split does not apply | native `more_than_three_hundred_blocks_settle_in_one_native_batch` (353.89s) | native verified |
| AC32 | Capacity-driven first batch succeeds; later due data remains; process stays running | Continue the drain without waiting a new interval or fullness threshold | native blob-overflow two-prefix drain | native verified |
| AC33 | Failure or observer verdict without validated commitment advancement | Do not reset settlement age or clear activity | canonical evidence tests; native first-source omission and partial recovery | unit/native verified |
| AC34 | Restart with unsettled activity | Recover the canonical anchor and activity; evaluate current rules without restoring old policy intent | native retained-backlog and restart interval tests | native verified |
| AC35 | Restart without a newer settlement; interval already elapsed and pure activity remains | Settlement is still due; process startup does not reset the timer | native restart interval anchored to latest canonical L1 settlement | native verified |
| AC36 | Restart after an intermediate batch settles; none of the current rules is due | The old drain target is lost; wait until a normal rule becomes due | `restart_after_intermediate_prefix_discards_old_intent_and_uses_current_rules`; current-rule evaluator tests | unit/native verified |
| AC37 | Reorg makes previously settled activity pending | Restore the canonical anchor and activity; reconcile in-memory intent and reevaluate | `canonical_publication_and_reorg_restore_the_original_timer_and_rebuild_pending_activity`; canonical RPC/generation and ownership reorg tests | component verified; no injected enabled-node L1 fork claimed |
| AC38 | Ordinary data would exceed the entire hard allowance before its first valid split boundary | Defer ordinary overflow, preserve empty Sync boundaries, reject individually unpostable transactions; retain explicit errors for incompatible history | Network-derived interval sizing and ordinary burst/admission tests; retained-cap startup checks | [2026-09-14 prevention audit](../audits/2026-09-14-sequencing-da-budget/README.md) |
| AC39 | `postBatch` fits its gas budget but required inbound transactions make the bundle too large | Reduce complete admitted groups; do not dispatch the oversized bundle | native `settlement_policy_gas.rs`: measured companion execution exceeds one block, complete groups deferred | native verified on Ethereum/Gnosis |
| AC40 | Same batch runs against two profiles with different fee markets or fork gas rules | Estimate and price it from the selected network; no shared currency-price assumption | native always and receipt cost comparisons for actual Ethereum/Gnosis executors | native verified |
| AC41 | Production priority-fee override is unset | Use the active network's fee estimator; do not fall back to the smoke-test 10-gwei tip | native default quote assertion; real receipt prices in both cost runs | native verified |
| AC42 | Estimated gas is well below the permitted transaction maximum | Declare sufficient estimated gas plus validated margin; report expected usage and balance reservation separately | native always candidate gas below maximum, receipt cost/reservation checks | native verified |
| AC43 | Partially filled physical blob is posted | Charge/report its full protocol blob-gas cost; fullness percentage is not a billing discount | native receipt full physical-blob gas; both cost fixtures | native verified |
| AC44 | Fees move before an unsent candidate is dispatched | Refresh and revalidate within configured caps through existing attempt ownership | quote/unsent repricing/dispatch invalidation components; native partial/retry final poster hash | component/native verified |
| AC45 | Fee quote's sample is reorged out, or the canonical head/target changes | Invalidate or refresh the quote and final validation before dispatch | `quoted_dispatch_checks_network_head_and_target_before_recording_or_sending`; canonical head reorg/generation integration | component verified |
| AC46 | A fixed priority-fee override is invalid or outside configured bounds | Fail startup; do not silently use the default or estimator | `PostingFeeConfig` invalid override/bounds tests; `ComposerSettings` early validation | unit verified |
| AC47 | Companion user transaction is underpriced | Preserve its signed bytes and authorized fees; use existing admission/retry handling | `underpriced_companion_keeps_its_nonce_until_the_user_authorizes_replacement`; signed-byte/nonce and funding-recovery native assertions | unit/native verified |
| AC48 | Lower gas limit produces outer-call success but skips an intended entry | Fail preflight; increase gas within the allowed budget and recheck, or preserve pending work with an error | `outer_success_without_complete_effects_is_rejected`; `final_gas_and_fee_changes_require_complete_effect_revalidation`; native gas/partial tests | unit/native verified |
| AC49 | Fee or gas changes affect simulated execution or proof-bound inputs | Revalidate the ordered bundle and recompose/reprove as required through the existing path | `final_gas_and_fee_changes_require_complete_effect_revalidation`; unsent repricing and native preflight tests | unit/component verified |
| AC50 | Fee estimator is unavailable or configured ceilings are insufficient | Keep settlement due; report the fee error/block and retry without overriding caps | native fee-cap recovery and `fee_rpc_outage_then_dropped_bundle_preserves_due_activity_until_canonical_replay` | unit/native verified |
| AC51 | An included attempt reverts or applies only a prefix | Record its actual receipt cost independently of whether the entire intended range settled | native policy partial costs include successful and reverted companions; omitted tx is zero | native verified |
| AC52 | Deferred inbound effects appear in a companion transaction, outside the `postBatch` receipt | Attribute and validate the complete bundle's effects before resolving ownership | native two-companion settlement and four partial/resume policy cases | native verified |
| AC53 | Cursor reaches the proposed height but a different partial block hash is applied | Account for the applied prefix; recover the unconsumed tail without releasing it as completed | native policy partial prefix and two-post same-L1-block hash resumption | native verified |
| AC54 | Restart after fullness became due; new allowance would make it below threshold | Use the new allowance; wait unless a current rule makes settlement due | restart/current-allowance evaluator tests | unit verified |
| AC55 | Restart with a longer interval or fullness disabled | Apply current startup settings to the backlog; no durable policy revision is needed | restart/current-settings evaluator tests; native restart profile change | unit/native verified |
| AC56 | Crash after bundle send and before response persistence | Use existing attempt/nonce reconciliation before permitting another submission | `policy_restart_after_partial_keeps_accounting_and_rebuilds_intent`: crash after the retry reaches relay, before response; canonical prefix/nonce recovery and exact-user resubmission | native verified |
| AC57 | A due historical batch needs an ECDSA proof | Use the existing proof path, timing, retries, and validation | native historical two-prefix drain through real ECDSA signer | native verified |
| AC58 | Every steady trigger is late but historical work is due | Historical settlement can target a fresh L1 opportunity; it is not hidden behind the late-slot return | `late_sync_can_post_due_history_below_three_hundred_blocks_to_a_fresh_target`; paused fixture resumes after deadline with signer ready | native verified |
| AC59 | Policy skips posting while the ordinary mempool is non-empty | Build the defined empty Sync; do not enter a mempool-driven fallback | explicit driver outcome matrix across three slot modes | component verified |
| AC60 | The empty Sync build fails during a policy skip | Report the error and end the tick; no pool-driven Sync fallback | explicit driver outcome matrix including EndTick/build failure | component verified |
| AC61 | A valid cross-chain group needs earlier history, but other triggers are below threshold | Its finite predecessor inherits urgency and can settle first | finite predecessor pool ownership/hash/cancellation tests; `cross_chain_group_alone_makes_its_finite_historical_predecessor_due` | native verified |
| AC62 | A delayed batch needs older witnesses | Read retained canonical witnesses or use the existing replay fallback | native persisted restart/315-block history passed; `retained_backlog_replays_old_witnesses_when_the_capture_store_is_empty` | unit/native verified |
| AC63 | Required witness data remains unavailable | Report the error and preserve pending work; do not shrink or classify the batch as settled | `canonical_witness_hit_reorg_and_missing_input_preserve_retained_data`; missing index input tests; native prover outage retains full pending range | unit/component verified |


## Interpretations and architecture

* **Activation and migration (§3.1):** the later new-network requirement removes
  activation and migration. The policy is mandatory from genesis; required
  settings and fee ceilings are explicit, and old configurations fail startup.
* **Signer trust input (§6.1,7):** the original signer sees an L2 chain document,
  timeless batch and blobs. It cannot independently select an L1 fork. An
  explicit protobuf `L1PostingContext` supplies chain ID/timestamp;
  `EEZ_L1_CHAIN_CONFIG` independently selects a trusted L1 profile. Both are now
  required for every request, per the subsequent instruction to remove legacy
  client compatibility. Missing context is rejected at header admission; a
  missing profile prevents startup. Composer and signer must be upgraded together. The shared strict loader moved from signer to `eez-l1`, retaining
  its tests. Batch payloads, native stream bytes and signed proof inputs are
  unchanged. The proposal should name this additional startup/wire input.
* **Restart (§3.3,8):** baseline boot discarded every speculative L2 block
  because cross-chain ownership is volatile. Enabled startup now retains the
  canonical ordinary prefix above the validated anchor, stopping before an
  unrecovered system-bearing Sync. It retains included pure activity without
  creating a journal or advancing safe/finalized. Native restart tests cover it.
* **Cross-chain predecessors (§5):** a replaced proposed Sync is not committed
  historical history. The dependency uses an existing empty canonical endpoint
  and the held pool's queued/in-flight group ownership, including users not yet
  in the optimistic ledger. Temporary/indivisible failures and invalidation
  cancel affected dependencies. A non-steady opportunity can finish history
  while fresh cross-chain work waits for a steady opportunity; that can require
  a separate final post. Independent periodic intent is unaffected.
* **Gas/fees (§6.3):** the final ordered simulation uses the actual Ethereum or
  Gnosis executor. It observes real lookup calls, applied roots and skips; it
  does not substitute successful mocked calls. Target-state gas/fee changes
  invalidate prior final execution. Synthetic composition frames expose the
  target GASPRICE, poster ORIGIN, manager CALLER and value while avoiding a
  fictitious manager fee charge. Actual signed preflight retains native fees,
  balance/nonce validation and the Gnosis collector.
* **Limits and unavailable inputs (§6):** only actual blob/gas overflow can
  select a smaller prefix. Known prover resource errors stop that proof episode;
  the next scheduler attempt uses current inputs and existing retries. Manual
  signer sizing remains required; there is no automatic prover-based split,
  worker or storage policy.
* **Evidence scope (§12):** native Ethereum/Gnosis tests and Kurtosis validate
  actual transactions, roots, DA and independent replay. Reorg assertions use
  the integrated canonical publication/index/policy path, canonical RPC tests,
  ownership/cost rollback tests, and native same-height partial resumption.
  This is not an injected enabled-node native L1 fork. Subsequent public Chiado
  deployment, recovery, and stress runs have their own audit reports and binary
  identities. No Ethereum mainnet activation is claimed.
* **Fixture compatibility:** the generic Anvil harness has a separate embedded
  simulation chain. Enabled full preflight uses `CrossChainWorld` or Kurtosis,
  where canonical L1 and local state agree. No production preflight bypass was
  introduced. The private Gnosis fixture uses `EEZ_L1_CHAIN=testing-gnosis`, a
  distinct chain ID, native collector/reward/deposit settings, and
  `EEZ_L1_CHAIN_CONFIG=gnosis:<genesis path>`. These are local test inputs.

## Validation record

Validation uses Rust **1.94.0** and Foundry **1.7.1**, at protocol submodule
`3c0cdf4955941a5c23639346932b55b33babebbd`. Full commands, test inventories,
results, source fingerprints, and executable hashes are retained locally in
`artifacts/pr-preparation-20260914/`. The complete original matrix validates
the Rust source in feature commit `8980484`; the final dependency correction
changes only four packages in `Cargo.lock`. Its rebuild and revalidation records
are in the `tls-fixed-candidate/` subdirectory. Tests execute the emitted libtest
binaries serially against each frozen composer, follower, and signer;
subsequent composer unit-test compilation cannot replace those runtimes.

| Candidate check | Result |
| --- | --- |
| Rust unit and protocol-vector suites | **932 passed on each lockfile**: 789 across the non-composer libraries, signer, and vector targets; 143 composer tests, including the two composition-ownership regressions |
| Native node integration matrix | **93 passed** on the original reviewed runtime; **17 selected cases passed** again with the final lockfile. The separate Kurtosis replay case requires an enclave and is explicitly ignored in the native matrix. |
| Python CI and verification helpers | **216 unique tests covered**, including all four opt-in real infrastructure checks: Docker runtime packaging, native archive relocation, Docker payload extraction, and isolated Kurtosis resources |
| Strict workspace lint | `cargo +1.94.0 clippy --offline --locked --workspace --all-targets -j 2 -- -D warnings` passed with no lint allowances |
| Minimal feature build | Driver, node-common, and L1 with `--no-default-features` passed |
| Shipping executables and native test inventory | Both builds passed with `--offline --locked`; runtime files were frozen before test-only feature unification |
| Dependency security policy | `cargo deny check advisories bans sources` and `cargo audit` passed with the final lockfile and unchanged policies; 14 existing allowed warnings remain |
| Contract and complete protocol Anvil suites | **427 Solidity tests passed** (19 integration + 408 core protocol), and all **28 Anvil scenarios passed** |
| Configuration propagation | Enabled and disabled Chiado/Kurtosis Compose checks, Chiado blob configuration, and explorer Compose validation passed |
| Public explorer and monitor checks | Blockscout WSS, Blobscan pages/charts/currency and authenticated binary download, monitor links, canonical safe-block identity, and historical multi-blob decode passed; these read-only checks use the existing deployment |
| Explorer offline checks | Chiado profile/fork/capacity/fee checks and archive-only blob proxy checks passed. Whitespace normalization of the compatibility patch applies with zero fuzz and produces the same 1,078 source files byte for byte. |
| Formatting, syntax, and patch hygiene | Rust formatting, Python/shell/JavaScript syntax, whitespace, source inventory, and documentation-link checks passed; repeated before commit |

Final-lock native revalidation covers legacy posting, Ethereum/Gnosis `always`
behavior, the canonical interval timer, empty consolidation, posting more than
six blobs, ordered cross-chain overrides, Gnosis ordinary data budgets, automatic
mixed admission on both profiles, same-block partial settlement, restart after
partial settlement, complete companion-gas budgets, receipt costs with the
then-current network priority margin, and rejection of a tampered witness. All 17 exact case
names and commands are recorded in
`tls-fixed-candidate/native-revalidation-selection.json` and its result file.
This is targeted revalidation of the dependency update; the complete 93-case
matrix was run before that update. The existing GitHub CI runs the full native
inventory and Kurtosis for the submitted revision.

The regular Python/helper suite and both enabled/disabled deployment profiles
were also rerun against the final lockfile: 212 Python cases passed, with the
four opt-in cases already verified separately. All six original/final runtime
hashes and the reviewed Rust-source fingerprints were checked before commit.

The runtime SHA-256 identities distinguish the original full-matrix run from
the final lockfile containing the Rustls correction:

| Runtime | Executable | SHA-256 |
| --- | --- | --- |
| Original feature | `eez-composer` | `928093b745a17d585ba886348d448fafadab4fd7347e37ea88cb23fc48a6a460` |
| Original feature | `eez-follower` | `1c503cffb8d2f973cad933216694da0b8a401584df1aa10d636e3771337f3bfe` |
| Original feature | `eez-proof-signer` | `b8b7ced8eea5b3d39ae536edfb9ba53b2fb1f4cb73ca42da7db2810d4d2cc5bf` |
| Final lockfile | `eez-composer` | `eb1d43d14ae737a1d415e3405d4a62f18262185ab7060ae9a0891e34a6554332` |
| Final lockfile | `eez-follower` | `4e3ed8c0b4ead252e6d9183f70c6b5f10caa7d23bb496039ed147e8144b9818b` |
| Final lockfile | `eez-proof-signer` | `9419528e2abc023f731c5a3f832829a623d03707032e44566e0ac9a29550e392` |

The final `Cargo.lock` SHA-256 is
`857d0de8c8009004981da2928952cfbebfeeaacdee8e123d0f24516e3316ae2c`.

The existing native inventory is authoritative in
`scripts/ci/native-tests.py`. Build its non-composer targets before the composer
unit target; keep node cases serial as required by `.config/nextest.toml`.
For direct libtest execution, use `EEZ_TEST_PREBUILT=1`,
`EEZ_TEST_RUNTIME_BIN_DIR=<frozen runtime>`, and
`EEZ_TEST_PROOF_SIGNER_BIN=<frozen runtime>/eez-proof-signer`.
The recorded build manifests identify Cargo's actual artifact paths.

Other reproducible checks:

```sh
cargo +1.94.0 fmt --all -- --check
cargo deny check advisories bans sources
cargo audit
cargo +1.94.0 clippy --offline --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 check --offline --locked -p eez-driver -p eez-node-common \
  -p eez-l1 --no-default-features
python3 -m unittest discover -s scripts/ci/tests -v
python3 scripts/check-settlement-policy-config.py
python3 -m unittest discover -s scripts/ci/tests -p 'test_chiado_config.py' -v
bash scripts/ci/run-protocol-anvil.sh
git diff --check
```

The first local Anvil invocation passed 23 scenarios; five could not start
because its selected ports overlapped the host's ephemeral range. After checking
an available range below that boundary, the unchanged complete suite passed
28/28 in 179.44 seconds. Both runs remain in the evidence. No assertion, retry
policy, or protocol source was changed to make the result pass.

The opt-in helper flags are `EEZ_TEST_DOCKER_RUNTIME=1`,
`EEZ_TEST_NATIVE_ARCHIVE=1`, `EEZ_TEST_DOCKER_PAYLOAD=1`, and
`EEZ_TEST_KURTOSIS_RESOURCES=1`. Native relocation requires CI's existing `mold`
linker. Local packaging probes use disposable tiny workspaces and enclaves;
they do not replace full node integration tests.

Earlier deployment evidence remains separately versioned:

- [Original cost and Kurtosis audit](../audits/2026-09-13-settlement-policy/README.md):
  equivalent 40-block Ethereum/Gnosis workloads, native multi-blob verification,
  and independent Kurtosis replay through L2 block 288.
- [Public Chiado deployment](../audits/2026-09-13-chiado-settlement-policy/README.md)
  and [automatic-admission validation](../audits/2026-09-14-dynamic-admission/README.md):
  real relay, canonical receipts/roots/DA, and all 28 protocol scenarios.
- [Ordinary DA-budget validation](../audits/2026-09-14-sequencing-da-budget/README.md),
  [capacity recovery](../audits/2026-09-14-chiado-capacity-recovery/README.md), and
  [cross-chain stress](../audits/2026-09-14-chiado-crosschain-stress/README.md):
  retained-history verification and 1,344 settled stress requests.

Those deployed binaries precede the final composition-ownership cleanup. Their
results establish deployment behavior at the recorded revisions; they are not
presented as a deployment of the new PR candidate. The separate ordinary-pool
selection delay remains unexplained in the recovery audit.

## Meaningful files changed

| Files / area | Purpose |
| --- | --- |
| `eez-composer/src/settlement_policy/{config,mod,index,tests}.rs` | Typed startup options, authoritative deterministic OR evaluator, finite volatile intent, incremental canonical transaction/byte accounting |
| `eez-composer/src/composer/{config,policy,posting,costs,admission,sequencing}.rs`, `composer.rs` | Startup propagation, canonical anchor integration, multi-blob planning, valid prefixes, automatic group admission, ordinary interval DA budgets, native bundle preflight/fees and canonical receipt costs |
| Composer local client/provider/slot, held pool and optimistic ledger | Native Gnosis execution context, retryable source funding/fee failures, predecessor ownership and unsent poster repricing through existing ownership |
| `eez-driver/src/{slot,sequencer,block_committer,lib}.rs` | Explicit Sync outcomes prevent a mempool fallback; deterministic boundary tests preserve cadence |
| `eez-deriver/src/deriver.rs` | Opt-in boot retention of accumulated pure history while preserving existing cross-chain recovery |
| `eez-l1` canonical head/reader/watcher/submitter/scan | Canonical registration/commitment timestamps, complete scan/generation identity, historical fork rules and quote-bound future dispatch |
| `eez-l1/src/{posting_fees,chain_config}.rs`, `chain_config/tests.rs` | Checked network fee/cost operations and shared strict chain-profile loader |
| Blob/payload codecs and `eez-blob-codec/src/limits.rs` | Existing-format exact sizing, shared resolved maxima, multi-blob sidecars and authorized poster repricing |
| Prover RPC/prover-client/prover; signer config/service/settlement/validation | Required L1 target input, independently trusted signer rules, matching >6-blob admission and missing-context rejection |
| Node/follower startup and node witness-source tests | Validate/propagate profiles before readiness; preserve Gnosis adapter and canonical witness/replay behavior |
| `eez-node-common/src/{payload,pool}.rs`, `eez-payload-codec/src/block_size.rs` | Bound ordinary data before block construction, retain overflow and nonce dependencies, reject individually unpostable transactions, and preserve composer ownership during pool reinjection |
| Rust node integration tests and testkit | Native modes, exact follower replay, partial/restart, gas/blob/cost/fault matrices, isolated reversible relay controls |
| `scripts/ci/native-tests.py` and native/runtime artifact tests | Include all new test targets in existing archives and execution groups; preserve serial node execution, relocation and exact shipping-binary identity checks |
| Kurtosis blob-verification wrapper and Python tests | Explicit `EEZ_VERIFY_BLOBSCAN=false` for the profile without explorers; native DA checks always run and the default still requires successful explorer verification |
| Python builder stub/tests | Narrow temporary RPC faults and acknowledged-but-unincluded bundle fixture; existing atomic production submission is unchanged |
| Example envs, Compose files, Kurtosis profile/main/check scripts | Explicit opt-in settings and fee policy, obsolete-cap removal, existing precedence, selected signer/follower profiles |
| `testing/explorers/*`, public-gateway guide, `scripts/deploy-ui-demo.sh` | Separate Chiado Blockscout/Blobscan deployment, pinned compatibility patch, public routing, and canonical-data/browser verification; operational details are in the explorer README |
| Workspace/crate manifests and lockfile | Existing native Gnosis executor reuse, optional shared chain-spec loading, REVM synthetic fee flag and test-only dependencies; the final dependency audit also required the Rustls security update described below |
| Operator guide, this ledger, audit evidence | Configuration defaults/units/validation/migration, complete requirement mapping, actual receipt/root/DA/cost evidence and reproduction |

## Priority fee estimator feedback correction

The September 18 correction removes percentage uplift from priority pricing.
Reth's `eth_maxPriorityFeePerGas` oracle includes recent poster transactions
and samples the cheapest transactions, which is not aligned with an exact
next-block bundle. Dynamic mode now selects the p90 of 20 per-block p90 rewards
from `eth_feeHistory`, within an explicit independent priority ceiling. The
optional relay floor and exact fixed override remain available.

| Requirement | Implementation/configuration | Automated coverage |
| --- | --- | --- |
| Reproduce the observed 20% recursion exactly | Existing checked percentage helper, used only in regression setup | `observed_percentage_uplift_reproduces_the_self_feedback_loop` checks all three observed values |
| Prevent recursive self-uplift across sustained posts | Stateless p90-of-p90 selection followed by a clamp, without multiplying prior selections | `fee_history_selects_recent_p90_of_p90_without_chasing_one_spike`; `sustained_self_posting_does_not_amplify_estimator_samples` |
| Require a priority ceiling independent from the execution-price ceiling | `PostingFeeConfig::parse` rejects dynamic mode without `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE`; a fixed override safely defaults its maximum to itself | `dynamic_fee_history_requires_an_independent_priority_ceiling` and composer startup tests |
| Keep sparse blocks, spikes, relay floors and external market changes safe | Empty-block zeros and isolated spikes are absorbed by the outer percentile; the selected market value is clamped between explicit minimum and maximum | `fee_history_selects_recent_p90_of_p90_without_chasing_one_spike`; `sparse_samples_use_the_floor_and_spikes_use_the_priority_ceiling`; `external_fee_history_changes_are_followed_within_independent_bounds` |
| Remove the unsafe priority multiplier | No priority-margin field is parsed, transported, logged, or applied | Source/configuration inventory and bounded quote tests |
| Fixed override is exact and does not require an estimator | Existing override branch | `fixed_priority_override_is_exact_and_does_not_query_the_estimator` |
| Parsed bounds reach the actual signed poster | `ComposerSettings` → `PostingRuntime`; fee events expose the history estimate, bounds and selected tip | Ethereum/Gnosis `*_receipts_use_bounded_fee_history_priority` |
| Receipt costs and canonical settlement remain correct | Existing bundle preflight and cost accounting | Both new native tests compare two distinct included posters to their canonical-parent quotes, receipt charges, physical blobs, applied roots, canonical L2 hashes and recorded costs |
| Deployment transports the safe bounds | Chiado Compose environment, Kurtosis `settlement_environment`; all three environment examples and operator guide | `scripts/check-settlement-policy-config.py`, Chiado static config, and Kurtosis profile tests |

The current Kurtosis PBS minimum remains a local builder fixture. A production
profile may select a zero minimum when its relay permits it. The operator guide
distinguishes priority-fee spend from execution/blob price headroom and gas
reservation. Activation requires an updated composer binary and node restart;
contracts, signing protocol, and historical data need no migration.

The current validation matrix includes bounded fee-history priority tests on
Ethereum and Gnosis. These cases verify two distinct posts against canonical-parent quotes,
receipts, physical blob charges, applied roots, and recorded costs. Original
extension measurements and executable identities remain locally in
`artifacts/network-fee-margin-20260913/`; they are not substituted for current
candidate validation.

## Composition error ownership

The final branch review found an error path relevant to R22/R23 and proposal
§5/§8: an outer composition failure could restore the original drain after an
inner stage had already evicted, released, or requeued some candidates.
`HeldPool::requeue_composing` now restores only candidates still owned by that
composition, under the existing pool lock and ledger. All three outer error
boundaries use this check; the existing recovery mechanisms remain authoritative.

The regression tests
`compose_error_restores_only_remaining_owners_without_undoing_classification`
and `compose_error_cannot_requeue_users_awaiting_canonical_evidence` check FIFO
restoration, eviction/release preservation, newer replacements, repeated error
handling, and requests already awaiting canonical evidence. This correction is
separate from the ordinary-pool selection delay in the Chiado recovery audit;
that incident's root cause remains unestablished.

## Dependency security correction

The final dependency audit found
[RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html),
published on September 14. The inherited `rustls 0.23.40` accepts some TLS 1.3
handshake messages across encryption-level boundaries. The lockfile now selects
the patched `rustls 0.23.45`, with its required `rustls-webpki 0.103.15`,
`aws-lc-rs 1.18.1`, and `aws-lc-sys 0.45.0` dependencies. No unrelated package
versions or dependency edges were changed by this correction.

Both `cargo deny check advisories bans sources` and `cargo audit` pass with the
existing policies. The 14 already allowed warnings remain; no advisory exception
or test allowance was added. Validation distinguishes the original full-matrix
runtime from the rebuilt runtime containing this security patch. Existing live
Chiado deployment evidence predates the patch.
