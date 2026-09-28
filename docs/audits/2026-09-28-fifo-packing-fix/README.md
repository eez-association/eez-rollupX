# FIFO prefix packing and cold signer initialization

| Field | Value |
| --- | --- |
| Date / status | September 28, 2026; fixes implemented, candidate process/live/CI validation pending |
| Scope | Release a partially consumed FIFO repair boundary; prevent lazy KZG initialization from inflating first-proof admission samples |
| Baseline / PR | Fork `eduadiez/rollupX`, PR #42; head `802dfb1a827025eaa68cee895f46d8729ad66ffc`, base `fd77a2ebc147850333171045dbd263ec74156268` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Candidate | Implementation commit containing this record; subsequent validation must identify its exact executable |
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
