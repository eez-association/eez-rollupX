# FIFO prefix packing and cold signer initialization

| Field | Value |
| --- | --- |
| Date / status | September 28, 2026; fixes implemented, candidate process/live/CI validation pending |
| Scope | Release a partially consumed FIFO repair boundary; prevent lazy KZG initialization from inflating first-proof admission samples |
| Baseline / PR | Fork `eduadiez/rollupX`, PR #42; head `802dfb1a827025eaa68cee895f46d8729ad66ffc`, base `fd77a2ebc147850333171045dbd263ec74156268` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Candidate | `cf3407b9c05ec04e24586c5d131a65a1aa2c9dc8` |
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
