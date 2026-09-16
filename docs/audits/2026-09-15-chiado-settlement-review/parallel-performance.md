# Preparation and skipped-slot performance follow-up

The final optimized candidate is now deployed on the existing public Chiado
network. The [live comparison](chiado-performance.md) records its exact identity,
matched traffic, receipt costs and a real skipped-slot retry that kept all 24
sources in one post. The native measurements below remain separate evidence.

The [large staged wave](large-staged-wave.md) settled all 870 sources in 47
posts. Its most frequent admission limit was the preparation estimate. Two
canonically skipped L1 slots also caused unnecessary capacity backoff. This
follow-up addresses those two causes without changing FIFO order, execution
rules, settlement triggers or the live Chiado configuration.

## Implementation and coverage

| Requirement | Implementation | Coverage |
| --- | --- | --- |
| Base finishing estimates on observed work | `composer/admission.rs::Sample::tail_for` uses the last clean accepted-group work and non-KZG finishing durations to predict the current finishing allowance | Injected-time tests cover small and growing candidates, slower current work, retained historical overhead and the full-pass fallback |
| Never learn a cheaper allowance from incomplete observations | `Progress::sample` requires uniquely matched starts/finishes for every final accepted group, nonzero measured work, no source deferral and no rebuild. A uniquely matched final time refusal can cancel its own start before append | Tests cover rejected-duration independence, next-attempt learning, retained blob/deadline guards, unfinished/mismatched/duplicate brackets, zero/overflowing work and rebuilt measurements; an untouched FIFO suffix contributes no work |
| Keep physical blobs and deadlines authoritative | KZG scales separately with physical blob count; the previous finishing floor retains its existing smaller-candidate scaling; exact gas, DA, proof and dispatch guards remain | Existing and new boundary tests cover another blob, unavailable/expired preparation windows, overflow and stale-estimate relearning |
| Do not infer a capacity failure from a proven absent target slot | `Submitter::canonical_target_was_skipped` verifies a strict timestamp gap between consecutive canonical headers, bound to the completed scan; composer requires matching uncharged exclusion evidence, no applied settlement/post and all sources retryable | Canonical-header tests cover 5/12-second cadence, one/multiple missed slots, stale timestamps, missing/inconsistent headers and reorgs; source-outcome tests cover partial and terminal consumption |
| Keep real exclusions conservative | `submission_backoff_required` exempts only the proven skipped-slot case; all uncertain/incomplete source outcomes retain existing backoff | Real native exclusion regression checks reduced selection, eventual canonical receipts, state and nonce order |
| Test externally observable skipped-slot behavior | The existing test-only Engine API controller omits one future slot; the native test checks actual consecutive L1 headers, four sources remaining together, exact dispatch target, no capacity backoff and independent follower convergence | `automatic_selection_survives_a_canonically_skipped_slot_without_shrinking` passed against the frozen candidate runtime |
| Preserve compatibility | Parsing/defaults and numeric caps are unchanged; automatic admission still requires enabled settlement policy | Composer/L1 library suites and read-only control-flow review; numeric mode cannot learn the new ratio, and disabled mode cannot enter these paths |

The new observation fields on `eez.composer.admission.prepared` are
`admission_ms`, `finishing_ms`, `non_blob_finishing_ms`,
`measured_group_work_ms` and `group_work_calibrated`. Calibration eligibility
does not mean the ratio was below one or a smaller reserve was used. A zero
non-KZG duration, missing clean measurement or a ratio at least one keeps the
existing full-admission-pass prediction. Prediction arithmetic fails closed.

No new configuration, transaction encoding, contract change, storage migration
or consensus parameter is introduced. This follow-up's production delta,
relative to the frozen `release/baseline-source`, consists of three files:
`composer.rs`, `composer/admission.rs` and `submitter.rs`. Earlier branch changes
are outside this audit's delta. Test support
changes are confined to `dynamic_admission.rs`, `eez-testkit/src/l1_reorg.rs`
and `scripts/native-l1-consensus.py`. The proposal and operator guide describe
the new measured allowance and narrow canonical-gap exception.

## Validation

The final candidate passed **181 composer and 127 L1 library tests**. Formatting,
Clippy with warnings denied across composer, L1, testkit and all native-test
targets, Python syntax and whitespace checks passed. The earlier v1 L1 run
had 11 local-socket permission failures inside the sandbox; the unchanged suite
passed with loopback permission. Both logs are retained. The first v2 unit run
caught an error in the new test helper: it marked a fresh attempt as already
prepared, correctly triggering the rebuild safeguard. The helper now models
the runtime's fresh attempt with a previous sample and empty current progress.
The assertions and production safeguard remain intact; failed and passing logs
are both retained.

The new skipped-slot native case passed on its first invocation. Its fixture
uses an explicit 500 ms preparation, 500 ms proof and 1,500 ms submission budget
so the existing 500 ms capture poll can arm the omission before the target.
The controller rejects requests for past or misaligned slots. Production timing
is unchanged. The event is matched to the exact dropped target, and the test
requires canonical execution and follower replay rather than synthetic receipts.

All **six native regressions passed for the final candidate on their first test invocation**: the new
skipped-slot case, real exclusion/backoff, explicit-window 128-source mixed
waves on Ethereum and Gnosis, whole-group blob cuts, and recovery of confirmed
and unresolved source generations through a real L1 reorg. These check canonical
receipts, nonce/state outcomes and independent replay. They ran while the
optimized build was finishing, so their timings are not benchmark comparisons.
No final-candidate `admission.prepared` sample exceeded its recorded preparation
window. Nine recorded composer failures occurred before the first held source
in their respective fixtures: seven ending in a prover-busy error, one proof
timeout and one canonical-evidence change. Those failures can contain multiple
underlying RPC retries, which remain in the logs. The intentional reorg case also exercised the
source-nonce reconciliation fence after ingress. All logs and eventual outcomes
are retained; a passing fixture does not imply a retry-free runtime.

The same skipped-slot harness against the frozen old binary reproduced the
original behavior: capacity fell from four to two, and the sources settled in
L2 Sync blocks 85 and 90. It failed the same-block assertion as intended. The
candidate kept all four together. The negative control is recorded separately
from passing candidate validation, and its failed log is retained.

The frozen v1 performance matrix completed **15/15 passing invocations**. It
showed no consistent packing or end-to-end latency improvement. It also exposed
one lost learning opportunity: a known time-refused next group invalidated the
otherwise clean accepted prefix. The final refinement cancels only that
uniquely matched refused measurement; it never includes rejected duration or
the rejected group's prospective blob count. The final candidate then passed
**6/6 matched performance trials**, with no failed invocation or trial rerun.

## Measured results

These are native GnosisPrague and EthereumOsaka fixtures. The table compares
the frozen baseline with the final candidate using the same compiler profile
and unchanged workload. Counts exclude the one-source warmup.

| Workload | Profile | Baseline sources per post | Final sources per post | Baseline / final total blobs |
| --- | --- | --- | --- | --- |
| Gnosis 128 | Dev | 49/49/30 | 55/55/18 | 3 / 4 |
| Gnosis 128 | Release | 57/56/15 | 57/53/18 | 4 / 4 |
| Ethereum 128 | Dev | 31/31/31/31/4 | 31/31/31/31/4 | Incomplete record / 5 |
| Ethereum 128 | Release | 31/31/31/31/4 | 31/31/31/31/4 | 5 / 5 |
| Expensive Gnosis | Release | 1/1 | 1/1 | 2 / 2 |
| Ordinary history | Release | One post | One post | 2 / 2 |

Every eligible source has complete ingress-held-to-canonical-safe timing
coverage. The expensive fixture's deliberately oversized third source is
rejected, and both fitting sources settle intact. Ordinary history has no
cross-chain ingress timing; it retains its full range and follower convergence.
The [companion JSON](parallel-performance.json) records timings, identities and
exact requirement-to-test mappings.

The baseline Ethereum dev fixture finished before its last post's cost telemetry
was recorded, so its total physical blob cost is left unknown. Its complete
source timing is established independently by the final transaction/target,
canonical inclusion and matching composer/follower L2 block and state root.
All final-candidate source posts have complete cost and reconciliation evidence.

The final Gnosis dev run preserves calibration on its time-limited 55-source
prefix; the release run excludes its gas-refused 57-source prefix and preserves
calibration on the following time-limited 53-source prefix. This is the directly
observed estimator change. Following selection also depends on existing FIFO
fairness: the first mixed attempt can restrict the next lookahead after
bypassing older inbound work. Different prefix counts cannot be attributed to
learning alone.

There is **no demonstrated reduction in total posts or general latency** in
these mixed waves. The Gnosis dev run used one additional physical blob, so it
does not establish a cost saving either. Final held-to-safe medians were
10.29/9.36 seconds for Gnosis dev/release and 14.32/14.43 seconds for Ethereum
dev/release. Single invocations, different warmup observations, empty-history
growth and host scheduling prevent a statistical speedup claim. These are local
ingress-to-safe measurements, not public Chiado latency or L1 finality.

No final comparison's recorded automatic preparation sample exceeded its
budget. Startup retries remain visible. The expensive control and final
ordinary-history control each missed an initial one-second proof budget,
retained their work and succeeded on retry. Historical preparation took
406/403 ms; its encoded range grew from 244,362 to 248,183 bytes during the
retry. Its one successful post used 262,144 blob gas and 130,417 execution gas.
These retries and differing ranges preclude an equal-payload speedup claim.

## Measurement method and provenance

The source deployed before this follow-up was frozen before parallel edits.
Its 193 Rust/Cargo identities match the deployed validation build. Baseline
fixtures and contract artifacts are frozen too. Profile comparisons use Rust
1.94.0 and the same linker; the normal dev profile uses optimization level 1,
while the release profile uses level 3 without debug assertions. Source-change
comparisons use the same profile and the same native harnesses.

The final candidate dev runtime was frozen before native testing:

| Executable | SHA-256 |
| --- | --- |
| Composer | `bd829735be0c3c43df442d47995f211e2eed4a1ece268f59fb3037db693e382a` |
| Follower | `ec7855cbb19f3c3961679c48509cbca99776285adfffe7ef3120fa8bf2b829cd` |
| Signer | `ac7e1a863b563f69c38ecde42b30355118a5bc05aaa9686a1746f7ee64248537` |

The final release composer is
`98d33366a3f8a6d27335ecf8f1f23b4fb32ef53a3c6b244bd7eaf233e595788b`.
Its 193 Rust/Cargo source identities match the dev build. The changed composer
library and executable were rebuilt; unchanged release follower/signer binaries
match the frozen baseline. The final six timing trials began only after both
builds and the six native regressions finished; the host check found no compiler
processes. Existing live services continued running.

`bundle.dispatched` records handoff before final pricing and can contain a
different proposed hash. Timing analysis joins the final builder transaction,
canonical L1 block and exact L2 endpoint, and distinguishes handoff-to-safe from
builder-dispatch-to-safe. Three initial-matrix teardown cases have missing final
cost telemetry: Ethereum baseline/v1 dev (four sources each) and Gnosis v1 dev
(36 sources). Their source timing uses the strictly checked inclusion plus
independent replay evidence above; missing cost values remain unknown. This
fallback is restricted to fully successful dynamic-wave fixtures.

The final candidate reuses the exact v1 RPC/native harness executables after
verifying their hashes and unchanged test/control source. Its changed private
admission implementation runs in the newly built composer executable. A release candidate build
that incorrectly reused unchanged artifacts after source copies preserved old
modification times was rejected before testing. Accepted optimized candidates
must actually rebuild changed crates and match the reviewed production source.

Full commands, logs, source and executable identities live under
`artifacts/performance-parallel-20260915/`. Compilation and timing runs are
separated. Existing native assertions remain enabled: receipt/effect/nonce
checks and follower convergence establish that additional packing is valid.
The upstream 770-job protocol verifier is not restarted.

Executed checks include `cargo +1.94.0 fmt --all --check`, composer/L1 library
tests with `--offline --locked -j2`, Clippy for composer/L1/testkit/node-e2e with
`--all-targets -- -D warnings`, Python syntax and `git diff --check`. Dev and
release builds cover node, follower and proof signer. Frozen native executables
run each named test with `--exact` and `--test-threads=1`, with runtime overrides
bound to recorded executable hashes. Exact commands and results are in the JSON
and `candidate-v2/`, `release/comparison-round-1.json` and
`release/comparison-round-2.json` artifacts.

## Interpretations and limits

- The previous finishing observation remains a floor with the existing
  proportional reduction for a smaller candidate. This is not an absolute
  unchanged reserve for every batch size.
- An untouched deferred FIFO suffix does not invalidate measured work in the
  accepted prefix. A known final time refusal can preserve that prefix by
  cancelling its own measurement before append. Rejected duration and extra
  blob cost never train the estimate. Unknown refusals, source deferrals,
  unfinished brackets and rebuilds retain the full-pass fallback.
- A skipped-slot exception retains the current capacity estimate. It does not
  guarantee the same count on the next attempt: actual available time, gas,
  blobs and source eligibility still constrain that attempt.
- Finishing cost may change independently of source simulation. The retained
  floor, fallback and final guards make the estimate conservative in defined
  cases; they do not make wall-clock predictions strict bounds.
- Incremental blob estimation covers the atomic terminal Sync. Earlier
  unsettled history can add blobs during exact final packing; existing
  historical draining remains authoritative.
- Conservative protocol gas projections and outbound-first execution order
  remain. These changes do not prove that arbitrary traffic fits one Sync or
  promise sub-five-second Chiado latency.

## Remaining performance work

The unchanged gas projection is another concrete packing constraint. In the
frozen v1 comparison, the first outbound Ethereum-fixture post admitted 31
sources at 13,323,896 projected gas and used 1,681,234 receipt gas; the Gnosis
fixture admitted 57 at 24,036,596 projected gas and used 2,771,860 receipt gas.
These are native fixture measurements, not public-network capacity estimates.
Later inbound work is materially more expensive: one 31-source Ethereum-fixture
post used 10,259,040 gas across the post and its companion transactions.

The model conservatively charges every entry for a possible new source proxy,
even when calls reuse one. It also charges native blob-only DA as calldata and
appears to count the leading anchor in both the measured base and entry reserve.
A separate gas-model change should distinguish entry types and possible proxy
creations, cover nested calls, rollback and cold access, and align admission
with final sizing. Unknown state must retain conservative fallback; historical
receipt gas alone is not an admission bound. The full review and exact canonical
evidence are in `artifacts/performance-parallel-20260915/gas-follow-up.md`.

Signer startup retries also predate this change. Cancellation is cooperative;
the exclusive worker permit remains held until the running worker exits, so a
retry can receive a busy response. Measure initialization phases and the time
from cancellation to worker exit before changing that lifecycle. Lazy KZG
initialization is a possible contributor, not a demonstrated cause in these
logs. The baseline comparison and source identities are recorded in
`artifacts/performance-parallel-20260915/prover-follow-up.md`.

At this stage the live Chiado deployment remains unchanged. Local measurements
must not be presented as a live deployment or a repeat of the 870-source wave.
