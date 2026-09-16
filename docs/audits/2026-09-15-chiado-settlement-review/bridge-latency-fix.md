# Bridge admission and scheduling correction

The candidate was deployed to Chiado at **13:18:05 UTC on 2026-09-15**. A repeated
24-bridge wave settled correctly and an independent follower reproduced its
final state. Observed median safe delivery improved from **24.865 s to 6.012 s**;
p95 improved from **32.212 s to 9.813 s**. This does **not** meet a usual
sub-five-second delivery target: nine of the 24 observed deliveries were under
five seconds. All **44 unique native cases** now have passing results, alongside
210 unit tests and the checks below. Compact measurements, case-by-case results
and executable identities are in [bridge-latency-fix.json](bridge-latency-fix.json).

The baseline is the [earlier 24-bridge wave](bridge-wave-latency.md). Both runs
exclude a separate historical warm-up and use the same 2.1-second spacing.

## Changes

The automatic admission model now measures sidecar/KZG preparation separately
and charges it per physical blob. Prefix execution retains its conservative
quadratic allowance. After one cold-start sample, admission considers the
finite queued snapshot and stops at measured time, gas or data capacity; it no
longer imposes a doubling ramp from the previous small post. A group that adds a
blob must pass the additional preparation estimate before entering the draft.
Existing resource-cut/exclusion backoff and source ownership are preserved.
An estimate that blocks the first group is discarded so a bounded one-group
probe can learn again. A temporary slow sample cannot freeze the queue forever.
The reset applies to estimate rejection, not source-state or transport errors.

`EEZ_PREPARATION_TIME_MS` optionally supplies a positive preparation allowance.
With it, the scheduler triggers at the exact millisecond offset reserving
preparation, proof and relay slack before the next L1 slot. The default remains
unset and preserves the old rounded trigger. Invalid settings fail startup;
the combined budget must leave at least one Live L2 block. Timestamps remain
whole seconds and the Sync grid does not change.

After rebuilding missed intervals, a terminal at the requested future height
and timestamp can use steady composition immediately if it still satisfies
the deadline and speculative guard. Past, capped, misaligned and intermediate
boundaries remain empty. Existing canonical-parent checks, proof cutoffs,
final gas/blob validation and single-attempt ownership remain in force.

The deployed live profile is preparation 500 ms, proof 500 ms and relay slack
700 ms on Chiado's existing 5-second L1 / 1-second L2 cadence. Its trigger is
T−1.7 s instead of T−3 s. The proof allowance includes headroom over the earlier
two-blob historical proof measurements of 225–248 ms, not only small bridges'
23–44 ms proofs. These are operator settings, not new global defaults.

## Coverage

| Requirement | Implementation | Automated evidence |
| --- | --- | --- |
| Separate physical-blob cost from group growth; no artificial ramp after learning | `composer/admission.rs::Sample`, `AutomaticAdmission`, `Attempt`; KZG measurement in `composer.rs` | One-blob wave, additional-blob, small-candidate-after-two-blobs and arithmetic-overflow unit cases; existing automatic mixed-wave/replay tests |
| Stop before an estimated preparation tail cannot fit | `Attempt::can_start_group`, `can_finish_group`; both composition directions | 101-ms-window and exact-boundary unit regressions; native gas/blob, exclusion and FIFO ownership tests |
| A stale slow estimate cannot permanently stop the queue | `Progress::estimate_blocked`, `AutomaticAdmission::finish` | Deferral/probe/relearning regression; existing source/transport backoff test remains intact |
| Explicit optional preparation budget, invalid values fail, default compatible | `timing.rs::RollupTiming::from_env`, validation, trigger and Future suffix | Isolated-process default/non-default/invalid parsing cases; Ethereum/Chiado budget tests; Compose propagation test |
| Scheduler retains millisecond precision | `slot.rs::PendingSyncSlot`, `spawn_l1_anchored` | Fractional trigger, boundary and delayed-loop test through the scheduler's actual target/wait calculation |
| Use a still-eligible future slot after catch-up | `sequencer.rs::advance_sync_slot` | Component test across K=5/6/12 and every genesis residue, including late, misaligned, capped and speculative-paused cases; existing stale-parent/no-pool-fallback and intermediate-empty tests |
| Both directions, real proofs and independent derivation with new timing | `dynamic_admission.rs` | New Ethereum and native Gnosis mixed-wave tests after a single small post; each sends 64 inbound plus 64 outbound requests and verifies receipts, nonces, values and follower convergence |
| Existing operational behavior remains valid | Existing native testkit and protocol tests | Targeted policy, restart, reorg, partial-consumption, gas, blob, queue and legacy-cross-chain matrix |

Configuration is exposed in both node Compose profiles and documented in
`.env.example`, `.env.chiado.example` and the settlement operations guide. The
proposal's section 4.2 records the user-authorized follow-up: this intentionally
refines catch-up classification and adds an optional operational timing setting.
It does not add automatic proof tuning or change settlement triggers.

## Validation and reproducibility

Local evidence is retained in
[`artifacts/bridge-latency-fix-20260915-final`](../../../artifacts/bridge-latency-fix-20260915-final)
and the bounded live comparison in
[`artifacts/chiado-bridge-wave-latency-fix-20260915`](../../../artifacts/chiado-bridge-wave-latency-fix-20260915).
Signed requests and deployment configuration remain private local artifacts.

- Unit tests: 166 composer and 44 driver tests passed.
- Python checks: 224 run, 220 passed, four existing opt-in skips.
- Runtime binaries built offline with Rust 1.94.0 and frozen before building
  integration tests. Source identities and binary SHA-256 values are recorded.
- Clippy passed for all targets in the driver, composer and native test packages.
- Both new native 128-request mixed waves passed, on Ethereum and Gnosis profiles.
- All 42 selected existing native regression cases have passing results against
  the same frozen binaries. The initial matrix passed 41/42; the obsolete
  admission-ramp assertion described below was corrected and its case passed
  separately. The initial failure is retained, not reported as a passing run.
- The repeated Chiado wave and its independent canonical replay passed.
- Formatting and `git diff --check` passed after the final test correction.

The first candidate's two new native waves passed before final review identified
the stale-estimate issue. Its broader run was stopped and preserved. The first
reset implementation failed the existing source/transport retry test; the
narrowed correction passes it unchanged. The complete selected native matrix
is rerun against the final binaries; earlier candidates do not count as final
runtime validation.

One existing native exclusion test assumed the removed admission ramp: after
learning from one request, it expected two submitted sources and a one-source
retry. The candidate instead submitted all four queued sources, reduced the
retry to two, and recovered all four correctly. The test now requires that
four-to-two behavior and retains its receipt, nonce and independent-follower
assertions. The original failed assertion and executable are preserved; this
test-only correction does not change the deployed binary. The corrected test
passed, and Clippy passed again for that test target. Source manifests confirm
that it was the only Rust-source change after the runtime binaries were frozen.

Validation commands included:

```sh
cargo +1.94.0 test --offline --locked -p eez-composer -p eez-driver --lib -j3
cargo +1.94.0 clippy --offline --locked -p eez-driver -p eez-composer -p eez-node-e2e --all-targets -j2 -- -D warnings
python3 -m unittest discover -s scripts/ci/tests
python3 artifacts/bridge-latency-fix-20260915-final/run-native.py smoke 2
python3 artifacts/bridge-latency-fix-20260915-final/run-native.py regression 3
cargo +1.94.0 test --offline --locked -p eez-node-e2e --test dynamic_admission --no-run --message-format=json -j2
python3 artifacts/bridge-latency-fix-20260915-final/run-native.py amended 1
cargo +1.94.0 clippy --offline --locked -p eez-node-e2e --test dynamic_admission -j2 -- -D warnings
cargo +1.94.0 fmt --all -- --check
git diff --check
python3 artifacts/bridge-latency-fix-20260915-final/build-report.py
```

The native runner executes the repository's existing test binaries with isolated
data directories and the frozen composer, follower and ECDSA signer. Its per-case
logs and runtime/executable hashes identify what was tested. `build-report.py`
checks the final 44-case result set, the retained obsolete assertion, exact
runtime/source identities, live receipts and independent replay before writing
the compact audit.

## Live deployment and comparison

The user explicitly authorized deployment while regression tests continued.
Before replacement, the final candidate had passed 210 unit tests, Clippy and
both new native mixed waves; three of the 42 existing native regression cases
had completed. The deployment changed only the composer image and its timing
settings. It preserved the database and verified the prior canonical safe hash
and state root. The beacon and signer were not restarted. The image contains
composer SHA-256
`321025bcd368c0dfc8f11d24f79e677d5ea1f98db41eae1d2629deb0ce398840`.

The bounded public-ingress workload used one warm-up, then 24 small native
bridges from the dedicated test account. The warm-up began with 902 unsettled
blocks and completed in 38.837 s, including three historical/terminal posts,
one missing Chiado slot and two preparation-deadline deferrals. It is excluded
from both comparison statistics. These deferrals show why a historical backlog
still needs separate allowance when estimating user latency.

The measured wave ran from **13:22:32.351 to 13:23:25.639 UTC**, beginning with
two unsettled L2 blocks. Verification completed at 13:23:26.434. Native regression
tests ran concurrently on the same host. The beacon had 23 connected peers in
every sampled snapshot, versus 11 during the baseline. These are different live
windows, so the comparison does not isolate the effect of each code change.

| Measurement | Baseline | Deployed candidate |
| --- | ---: | ---: |
| Observed safe delivery, median | 24.865 s | 6.012 s |
| Observed safe delivery, p95 | 32.212 s | 9.813 s |
| Server source reconciliation, median | 24.675 s | 5.866 s |
| Request to dispatch, median | 20.510 s | 3.330 s |
| Dispatch to reconciliation, median per included post | 4.649 s | 2.525 s |
| Maximum sampled outstanding sources | 15 | 6 |
| Missed L1 slots during measured wave | 1 | 0 |
| Excluded bundles during measured wave | 0 | 1 |

The first candidate bundle received a builder RPC response without an error
(`result: null`) but was absent from target block 23,069,044. That block contained
**zero transactions and zero blobs**, so
gas/blob occupancy did not cause the exclusion. The available evidence does not
identify the builder/proposer decision. The request retried successfully, with
the admission backoff intact. Its backlog then drained in one five-source post.
All ten successful wave posts landed in consecutive L1 blocks
**23,069,045–23,069,054**, with source counts `1,5,2,2,3,2,3,2,2,2` and one blob
each. No preparation deadline was exceeded during the measured wave.

The exact destination increase was **24,000,000,000,276 wei**. The L1 sender
nonce advanced from 896 to 920; the L2 sender nonce remained 812. Every source
had a successful canonical receipt, exactly one registry consumption event and
the correct bridge amount. Every included post's safe balance matched the
complete ordered source prefix, and the queue drained. The independent follower
replayed through L2 block **141,215**, matching both hash
`0x1ee39999581488ad300857fbcb99f4263947de9c5051a2a9f91ee4c7aef80a8d`
and state root
`0xc0ddf715f0c43abed7bb02912b9583134b056e5c540fbb98ac0779afa5bf6c31`.
It used follower SHA-256
`9584d300d41a35e3df0f3a9ca9f4e6f61a2b0533ec5f76a80c02502c5f18a1a5`.

Server logs independently put safe delivery at **5.470 s median**. For included
posts, the L1 payload arrived 297 ms after its slot timestamp at the median;
canonical L1 commitment took 708 ms, and safe L2 became available at 868 ms.
The median canonical-L1-to-safe-L2 interval was 157 ms. Source ownership
reconciliation followed safe L2 by another 298 ms at the median. Thus polling
alone does not explain the remaining delay: request arrival relative to the
five-second slot and the reserved preparation/proof/submission window matter.
Individual observation RPC rounds reached 1.5 s under concurrent load; both
client observations and server events are retained rather than conflated.

All percentiles use nearest rank. Transactions in the same post are correlated.
"Safe" here means canonical L1 receipt plus derived L2 delivery, not L1 consensus
finality. Raw receipts, timing stages, executable identities and replay evidence
are retained in the artifact directories above.

The final live health check at **13:36:12 UTC** confirmed advancing L1 and L2
heads, an empty cross-chain queue, 23 beacon peers, fully validating sync, and
the unchanged deployed executable. The wave's safe block hash and state root
were preserved. General consolidation remains 1,800 L2 blocks, pure L2 mode is
`always`, and the fullness trigger remains off; empty unsafe history accumulating
below that general threshold is expected.

## Limits

The later [first bridge after an idle interval](bridge-6bd0ae1b.md) took 81.899 s
to safe delivery. Its 1,300-block initial backlog required four historical posts
before the bridge, with two exclusions, a skipped L1 slot and historical target
selection repeatedly moving to the second upcoming slot. This is a remaining
latency gap beyond the warmed-wave result above.

Admission estimates are not worst-case execution bounds. Expensive groups,
host contention, missed network slots, unavailable blob data, relay exclusion,
or a historical predecessor can still require later opportunities. Absolute
validation and deadline guards continue to decide whether a candidate is safe
to submit. The target is to remove unnecessary local waiting; a live sample
cannot establish a universal sub-five-second guarantee or L1 consensus finality.
