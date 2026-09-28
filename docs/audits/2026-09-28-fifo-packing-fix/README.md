# FIFO prefix packing and cold signer initialization

| Field | Value |
| --- | --- |
| Date / status | September 28, 2026; refined release built; correctness, live and CI validation in progress |
| Scope | Release a partially consumed FIFO repair boundary; initialize signer KZG before readiness; learn proof scaling from wider measured workloads |
| Baseline / PR | Fork `eduadiez/rollupX`, PR #42; head `802dfb1a827025eaa68cee895f46d8729ad66ffc`, base `fd77a2ebc147850333171045dbd263ec74156268` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Candidates | Initial `cf3407b9c05ec04e24586c5d131a65a1aa2c9dc8`; refined `ca370742920852ad1105df4e40d70625d8749145` |
| Environment | Rust 1.95.0, locked dependencies, Foundry 1.7.1; disposable native Gnosis Prague networks |
| Contracts | [Settlement policy](../../configurable-settlement-policy.md), [signer specification](../../../crates/eez-proof-signer/SPEC.md), [proposal](../../proposals/cross-chain-admission-throughput.md) |
| Prior evidence | [Chiado trial](../2026-09-28-chiado-strict-fifo/README.md); older results do not validate this candidate |

## Findings and implementation

The live trial identified five small retained-prefix tails. The held pool used
only the retry prefix's last source as its boundary, even after canonical
consumption of earlier sources. The candidate retains both endpoint identities
and clears the boundary on canonical resolution of its first source. The next
snapshot still passes strict FIFO admission. No receipt, relay observation or
inconclusive resolution clears it. Replacement identities and reorg recovery
remain covered. Policy observation becomes `strict-fifo-6`; the estimator remains
`candidate-framing-10`. No storage or wire migration is introduced.

The latest baseline CI run [36394880028](https://github.com/eduadiez/rollupX/actions/runs/36394880028)
failed only cross-chain shard 1/4: gas/FIFO expected two posts, observed four.
Other native shards, lint, producer and Kurtosis passed. Full failure artifacts
were unavailable because GitHub reported its artifact storage quota exhausted.
The retained job tail contains alternating timing deferrals and one-group probes.

The unchanged release passed locally with ordinary CPU availability, with its
processes limited to two CPUs, and with forty additional empty history blocks.
Adding an 850 ms pause to only the isolated signer's first proof reproduced the
same four-post assertion. That proof took 2,808.8 ms; the retained 25% allowance
predicted 3,511.0 ms for subsequent proofs taking 17–26 ms. The first proof still
settled within its real deadline; conservative later admission caused the split.
This reproduces the failure mechanism, but missing CI artifacts prevent proving
the origin of all historical CI latency.

The default Alloy KZG settings lazily load and precompute tables on first use.
The signer now initializes those same settings in one awaited blocking worker
before opening its listener. A worker failure prevents readiness. This changes
startup placement of existing work, with no calibration post, trusted-setup
change, skipped validation, or relaxation of proof timing history/deadlines.
The native gas/FIFO regression also checks initialization precedes serving.

## Evidence and checks

[Baseline timing summary](evidence/baseline.json) is checked in. Full logs,
datadirs, source patches, executable identities and the controlled-delay runner
are local-only under `artifacts/fifo-packing-fix-20260928/`; other checkouts cannot
retrieve them from Git. The runner pauses only the signer child of its own test
process and always resumes it after 850 ms. Live Chiado services were untouched
during these baseline checks.

Baseline runtime source is `b2ff3b894faa509fe310b6b6d38781d335f8f5fc`, with production
sources verified identical to PR head before this patch. Composer SHA-256 is
`8ea0151ddc95f2b94331adb00383cd23706790e964ac1223ebb856c9e0196e55`, follower
`6b0cc805d10494445c70cd4d82e7243d8bec927b61b500c459df9f84d495ffc6`, signer
`a8e6d6513a05b431c7c85493f595f9fbf89761f27a05282a0ceb41bd21b32c34`.
The frozen gas harness hash is
`ce35e3e0343b324222545c7641afdff4159bf0c2f0223bca586ff084a8758257`.
The extra-history harness has its separate identity and source patch retained.
Process gas cases validate effects, nonces, FIFO receipts and independent follower
replay before checking post count; an expected packing failure is still a failure.

| Check | Result |
| --- | --- |
| New canonical-prefix unit regression against old implementation | Failed as expected: younger queue tail remained hidden |
| `cargo +1.95.0 test --locked -p eez-composer --lib` after fix | 289 passed, one manual profile ignored |
| Baseline gas case, normal / two CPUs / extra history | One selected case each, passed; 1+3 sources in two posts |
| Baseline gas case, delayed first proof | One selected case, failed; four one-source posts |

All native tests use isolated networks and explicit hash-verified release paths;
source builds use `--locked`. No result here establishes candidate deployment or
live throughput improvement. Candidate release comparison, affected signer/RPC
and native regressions, formatting/Clippy, verified Chiado workload and PR CI
remain required. The existing operator runbook edits and recovery audits are
outside this change and preserved.

## Candidate build and initial validation

Both local release images were built from committed archives at `cf3407b`, with
the same recorded Docker recipe, Rust 1.95.0, release profile and shipping feature
selection as the baseline. Image/executable identity is retained locally in
`artifacts/fifo-packing-fix-20260928/identity.json` and its `images/` build records.

| Artifact | SHA-256 |
| --- | --- |
| Node image `eez-node:cf3407b-fifo-progress-local` | `780a08f3f906e163064a2f816207cddc134ad4b5d1c982f773c441dba4b233f0` |
| Signer image `eez-proof-signer:cf3407b-fifo-progress-local` | `0abebd1bc17f7c6ff7a890625bf807f7d4b4302bec31f111896efb65b659d155` |
| Composer executable | `bf0166cfcaeefebde8c2115be435b0b9c32bae3346e6f896dbae4f2284e1907c` |
| Signer executable | `055643037223f97eb5fab602de48dbee33ae88e22312f8f09fe06a9b9627d5df` |
| Follower executable | Identical to baseline hash above |
| Updated gas/readiness harness | `ad1d79eebf9f6c3062d6e1d900b6af9b77870143cf92742e5a67ecf326743e0d` |

Formatting, workspace/all-target Clippy (`-D warnings`), signer all-target check,
signer all-feature Clippy, and signer rustdoc (`-D warnings`) passed. The signer,
control-RPC and prover-client packages passed 330, 6 and 30 tests respectively.
The signer excludes its explicit captured-fixture inventory and cold-span
measurement utilities. The first restricted invocation failed 75 signer cases
because loopback socket creation was denied; the socket-enabled invocation
passed. This is an environment correction, not a hidden retry-only pass.
`cargo deny check advisories bans sources` and `cargo audit` passed; audit reported
nine allowed warnings. The locked graph and dependency versions are unchanged.

The first candidate gas case passed its exact two-post / 1+3-source assertions,
effects, FIFO, nonce and follower checks. Startup initialized KZG in 2,035.3 ms;
the first proof took 23.6 ms and the second 25.3 ms. The initial delay injector
reacted after that fast first proof had already completed, so this case is
**not evidence for the delayed-proof regression**. Its original logs and identity
remain under `candidate-gas-delayed-proof/` despite that misleading directory
name. A synchronized diagnostic must stop the isolated signer before the first
proof, resume 850 ms after observing the request, and assert that the recorded
proof duration actually includes the delay.

The [synchronized repeat](evidence/synchronized-delay.json) subsequently reproduced
the baseline's four-post failure and passed the candidate's two-post / 1+3-source
assertions. Both processes were paused before proving and resumed 850 ms after
the request was observed; measured first-proof durations were 2,880.1 ms and
883.6 ms respectively. Baseline's remaining proofs took 17.9–22.4 ms; candidate's
second proof took 25.0 ms. The workload and deadline settings are unchanged;
candidate's harness adds only the startup-readiness assertion. Source effects,
nonce order and independent follower replay are asserted before post count.

All [12 selected native correctness cases](evidence/native-correctness.json)
passed without retries. They cover both forks' older-inbound ordering and
complete gas-group splits, the exact two-post gas case, fee/balance/escrow
liveness, partial consumption, blob splitting and late-original inclusion.
Two non-FIFO speculative candidates were discarded; neither reached proving.
[Source-check commands and exit codes](evidence/source-checks.json) are retained
separately. Paired throughput, live rollout and CI remain pending.

## First candidate performance regression and timing refinement

The fitting comparison retained 120 sources in four posts per variant. Candidate
mean pipeline time was 341.4 ms versus 293.0 ms; one Ethereum blob-preparation
sample took 322.2 ms versus baseline 168.3–173.9 ms. A separate reverse-order
Ethereum pair is retained rather than replacing those measurements.

The 128-source tight-window comparison was worse for `cf3407b`: Ethereum used
four posts (39/39/39/11), versus baseline three (39/39/50); Gnosis used four
(22/22/22/62), versus baseline two (18/110). All effects and replay assertions
passed, but these are performance regressions. **No Chiado rollout followed
these results.** Full logs and frozen identities remain under `tight/` and
`ethereum-followup/` in the local evidence directory.

These candidates did not inherit a pool retry boundary. Their timing estimates
continued multiplying the initial one-group sample despite measured 22–39-group
proofs taking 42–67 ms. Increasing the group count beyond a covering observation
restored an estimate near the configured 500 ms and repeatedly triggered FIFO
rebuilds. This mechanism amplifies sample/timing variation and does not establish
that the new boundary-clearing rule caused the observed difference.

Policy `strict-fifo-7` keeps all slower and incomparable timing observations,
but supersedes a sparse sample's scaling term when a no-cheaper observation
supplies at least as much of every requested dimension and more of one. Request
counts clip only this comparison, preserving larger witness/blob observations
when needed. The measured slow-proof floor, 25% headroom, 32-sample bound, seed
cap and actual deadline gates remain. No count assertion or cutoff was relaxed.

The recorded 1-group/17 ms and 22-group/43 ms shape reproduces a deterministic
unit failure: a 110-group/2-blob request predicted 500 ms instead of the wider
sample's 268.75 ms. After refinement, all 291 Composer unit cases pass, with the
manual profile excluded. Regressions retain expensive sparse samples, unseen
history and larger blob observations; existing overflow/deadline tests pass.
Workspace/all-target Clippy also passes. New release/performance/live/CI results
are still required for this refinement; `cf3407b` results do not validate it.

## Refined release comparison

The refined images use committed source `ca370742920852ad1105df4e40d70625d8749145`
and the same release recipe/features as both earlier variants. Node image
`eez-node:ca37074-fifo-progress-local` has SHA-256
`a37c649f6fc2943fcec2f288f6d7be4ea2e4dc9537821fab96acd18829b9d806`;
signer image `eez-proof-signer:ca37074-fifo-progress-local` has SHA-256
`efc9939694bb54d9cf08122ded58a72e45b6fcad3a8072deb483d34fd0968b66`.
Composer executable SHA-256 is
`d94b44eb528bbfa5d0ca5eed56e1678ddc5a196ea8586fae8c5072ea50d7ad7b`;
signer and follower executables are byte-identical to the initial candidate.
The frozen test harnesses are also unchanged. Local records are under
`artifacts/fifo-packing-fix-20260928/refined/`.

All four paired tight-window cases passed effects, FIFO, nonce and independent
follower assertions. Each row below excludes the intentional one-source fixture
post and measures 128 sources. No discarded non-FIFO candidate reached proving.

| Fork | Baseline posts / source counts | Refined posts / source counts |
| --- | --- | --- |
| Ethereum | 3 / 39, 39, 50 | 2 / 39, 89 |
| Gnosis | 3 / 20, 1, 107 | 2 / 19, 109 |

[Both the original regression and refined comparison](evidence/packing-comparisons.json)
are retained. Baseline Gnosis used two posts in the first pair and three here;
shared-host timing and deadline variation prevent interpreting one pair as a
guaranteed percentage gain. The refined candidate removes the observed repeated
small-batch pattern in this comparison. Broader correctness, fitting-wave
latency, the same-day Chiado pair and CI are still pending.

The final release subsequently passed all [12 correctness cases](evidence/refined-correctness.json)
without retries, including semantic assertions and independent follower replay.
Both discarded non-FIFO candidates were stopped before proving. The
[synchronized first-proof delay](evidence/refined-delay.json) also passed on this
release: the first proof took 872.459 ms with the 850 ms injected pause, the
second took 30.432 ms, and the unchanged assertions required two posts with
1+3 sources. The baseline's synchronized four-post failure remains recorded above.

The [fitting-wave pair](evidence/refined-fitting.json) passed four selected
process cases, with two 30-source waves in each case. Each variant settled 120
sources in four posts, with no calibration post or admission cut. All eight
retained receipt gas costs matched native preflight, and every signed limit used
exactly 5% over required gas. Complete admission bounds and framing matched in
every pair; small calldata/receipt differences reflect the fresh fixtures.

| Fitting-wave measurement | Baseline | Refined |
| --- | ---: | ---: |
| Mean preparation | 151.00 ms | 142.25 ms |
| Mean proof | 43.62 ms | 47.47 ms |
| Mean finalization | 66.41 ms | 84.83 ms |
| Mean delivery | 17.82 ms | 19.11 ms |
| Mean start-to-delivery | 280.38 ms | 295.59 ms |

[Per-wave observations](evidence/refined-fitting-waves.csv) retain the increases,
including a 170.55 ms candidate finalization sample. Mean pipeline time increased
5.4%; individual paired ratios range 0.887–1.227. This small shared-host sample
preserves packing but does not establish latency parity or rule out a secondary
timing regression. No local compiler or other native suite ran during the paired
measurement. The live pair is needed to assess source throughput and retained
FIFO tails under the actual workload.

## Canonical-post synchronization in the liveness fixture

[CI 36411833402](https://github.com/eduadiez/rollupX/actions/runs/36411833402)
passed the runtime producer, lint, all four cross-chain shards, three embedded
shards and Kurtosis (completed at 12:43 UTC). Embedded shard 1 failed
`gnosis_blocked_outbound_balance_allows_independent_auto_work`: it expected one
L1 post while the count was still zero. Its first source nonce had advanced in
the optimistic L2 Sync block. The assertion ran at 11:42:41.237 UTC, while the
acknowledged bundle targeted 11:42:43 UTC. Those observations did not yet prove
canonical L1 consumption.

The unchanged frozen harness reproduced the same assertion locally against the
`ca37074` release. The shared fee/balance fixture now waits for its exact expected
canonical post count together with the blocked-source and nonce conditions.
The original count and subsequent recovery/effect checks are retained. This is
a test synchronization correction; the deployed runtime code is unchanged.

All six affected cases passed with the rebuilt harness without retries.
[Failure, executable identity and fixed-case results](evidence/canonical-wait-regression.json)
are retained. The original CI failure remains a failure; the edited harness
still requires a new full CI run.

## Live baseline and transport failure

The first same-day baseline attempt (`20260928-112913`) failed ordinary L1
deployment transport for 46 jobs after three 15-second client timeouts. It sent
only 804 of the intended 870 sources. All 804 were subsequently attributed to
canonical settlements and the queue drained. The remaining semantic verification
was interrupted only then; this attempt is **not a valid performance baseline or
a semantic pass**. Its logs, partial canonical measurements and explicit
disposition are retained locally in `live-baseline/`.

The temporary ordinary-L1 adapter had waited for upstream acknowledgement before
returning local mempool acceptance. Its replacement returns the local hash and
owns forwarding of the identical signed bytes in a bounded queue (2,048 pending
requests, eight workers, five upstream request starts/second). Duplicate requests
share the same pending work; retryable transport failures have five attempts.
The staged runner still requires canonical deployment receipts before sending
the measured wave. A controlled blocked-upstream probe checked prompt local
acknowledgement, single duplicate ownership and completion accounting. This is
task-local transport tooling, not a node or protocol modification.

The fresh baseline (`20260928-115327`) used the same private workload profile,
770 jobs / 870 sources, preparation concurrency 200, send concurrency 28 and
5% gas margin. All 2,812 setup transactions mined and all 870 source submissions
succeeded. The adapter drained with zero failed broadcasts. All 870 sources
(520 inbound / 350 outbound) were canonically attributed to 33 posts in blocks
23269333–23269373, using 173,929,920 postBatch gas. Send plus receipt monitoring
took 219.723 seconds; three L1 slots were skipped between first and last post.

Repeated attribution was identical, and all 33 receipts matched the independent
L1 endpoint by hash, block, status and gas. Grafana's actual panel queries returned
all 33 canonical post rows and 870 accepted-source estimate rows (2,353 source
attempt rows in total). Semantic verification is still running; this record does
not yet claim a complete verified baseline or candidate deployment.

At the operator's request to parallelize, verification was increased from four
to sixteen workers after the timed workload and canonical checks had finished.
Only the task's verification scheduler was paused; its active checks drained
normally. All 269 completed checks were retained and no semantic check was
interrupted. The remaining checks use the pinned runner's existing
`--verify-only` mode with `SKIP_VERIFIED=1`, holding the same protocol wallet lock
on inherited FD 200 for the complete invocation. This path cannot fund, deploy,
resume or resend transactions. The original wrapper ended with signal 15 during
this controlled handoff; its exit is not reported as a passing full run.

A private copy of the workload profile changes only `VERIFY_PARALLEL=16`;
the operator's original profile remains intact. The continuation checks that
all source/deployment transaction CSVs remain byte-identical and that all 770
distinct semantic jobs finish successfully. The candidate will use sixteen
verification workers from startup, with the same measured preparation/send
settings. Verification completion is recorded separately from transaction timing.
