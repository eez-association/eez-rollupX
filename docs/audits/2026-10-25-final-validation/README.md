# Final validation follow-up

Status: in progress. The owner requested parallel completion of the four
remaining checks. Independent investigation runs concurrently; local compilation,
process tests and live latency measurements share one coordinated resource
window so they do not contaminate the comparison or disturb other networks.

The existing implementation worktree is
`artifacts/l1-history-offline-proving`, on `codex/l1-history-offline-proving`.
It was clean at `1109dbab7f12fc735a6bf6986e7d7e7f987495d7` when this round began.
The base remains `a50c326d99f7cd1c21406eedbf370a344015a6b5`, and the initialized
protocol remains `84a5b5207880f192a6a5896cbfe0bac5249db47f`. The original checkout,
unrelated worktrees, production services and retained validation data are
preserved.

| Check | Current evidence | Remaining acceptance work |
| --- | --- | --- |
| [CI](ci.md) | The initial head passed unit, producer, eight E2E partitions and Kurtosis; a Python fixture collision failed the aggregate gate. Deterministic before/after reproduction and 408 local tooling passes validate its correction. | Full CI for the combined changes, including previously skipped Clippy/configuration checks. |
| [Admission width](admission.md) | Controlled regressions reproduce sparse-proof extrapolation refusing group twelve; the minimal scheduling correction preserves the measured slow-duration floor and actual deadlines. | Combined validation and the unchanged delayed mixed-wave process regression on the final executable. The original incident's missing warmup remains an attribution limit. |
| [MissingProxy](missing-proxy.md) | Both process assertions are strengthened; fee-enabled early rejection and fee-disabled terminal rejection have distinct oracles. | Execute both directions against the final runtime, including live terminal checks. |
| [Latency](latency.md) | Baseline executables and the existing analysis oracle are reverified; the paired bounded workload is prepared. | Matched final-runtime baseline/candidate run with exact fee/state/settlement assertions and recorded host conditions. |

The [earlier extended validation](../2026-10-24-extended-validation/README.md)
and its original failures remain historical evidence. Later diagnostic passes
do not erase those failures or prove their original cause. This record is not a
soundness audit and does not replace the separately discussed independent prover
review.

No protocol pin, genesis, RPC, serialization, database or dependency change is
part of this follow-up. The proof-duration predictor is an in-memory Composer
scheduling estimate; it cannot authorize execution, signing, canonical completion
or eviction. The owner-approved cryptographic advisory exception remains an
unresolved upstream dependency risk, as recorded in the linked CI evidence.
