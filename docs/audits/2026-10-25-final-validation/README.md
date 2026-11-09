# Final validation follow-up

Status: the four requested checks have completed, with the remaining evidence
limits recorded below. Independent investigation ran concurrently; local
compilation, process tests and live latency measurements used coordinated
resource windows to avoid contaminating the comparison or disturbing other
networks. The tested runtime is `b1881cf737cdc64ee30a9767807f53d21a3daa86`;
the evidence-only follow-up does not change its implementation.

The existing implementation worktree is
`artifacts/l1-history-offline-proving`, on `codex/l1-history-offline-proving`.
It was clean at `1109dbab7f12fc735a6bf6986e7d7e7f987495d7` when this round began.
The base remains `a50c326d99f7cd1c21406eedbf370a344015a6b5`, and the initialized
protocol remains `84a5b5207880f192a6a5896cbfe0bac5249db47f`. The original checkout,
unrelated worktrees, production services and retained validation data are
preserved.

| Check | Current evidence | Remaining acceptance work |
| --- | --- | --- |
| [CI](ci.md) | The initial head passed unit, producer, eight E2E partitions and Kurtosis; a Python fixture collision failed the aggregate gate. The combined candidate passes full Lint, 410 CI tooling tests, 1,576 workspace unit tests, both targeted Reth regressions, 38 producer checks, all eight process partitions (43 embedded and 131 cross-chain tests), Kurtosis and the aggregate gate. Its immutable handoff is independently verified. | None for the tested runtime; explicit exclusions are recorded in the CI evidence. |
| [Admission width](admission.md) | Controlled regressions reproduce sparse-proof extrapolation refusing group twelve; the minimal scheduling correction preserves the measured slow-duration floor and actual deadlines. The unchanged Ethereum and Gnosis delayed mixed-wave process regressions pass on final artifacts, including `accepted > 12`. | No further admission-specific test is pending. The original incident's missing warmup remains an attribution limit. |
| [MissingProxy](missing-proxy.md) | Both strengthened process assertions and all four explicit live controls pass on final artifacts. Fee-enabled early rejection and fee-disabled terminal rejection have distinct oracles and no observed effects. | The first terminal preparation timed out before its trigger during PBS bootstrap. The later pass follows a verified readiness gate; it is not a clean first-attempt campaign. |
| [Latency](latency.md) | The complete pair passes exact receipt/accounting assertions, four mixed waves per side, 46/47 authenticated batches and cold follower replay. The candidate uses the same corrected harness and independently verified final artifacts. | Measurement is complete. Canonical-settlement p95 remains higher in this sample: 31.312 s versus 27.641 s; causation and latency equivalence are not established. |

The [earlier extended validation](../2026-10-24-extended-validation/README.md)
and its original failures remain historical evidence. Later diagnostic passes
do not erase those failures or prove their original cause. This record is not a
soundness audit and does not replace the separately discussed independent prover
review.

Independent static review of the follow-up found no actionable issue in the
predictor's production deadline consumers, the strengthened MissingProxy
assertions or the isolated CI fixture correction. Reviewers checked changes
they had not authored. A separate review of the latency identities, retained
summaries and final replay evidence confirmed the results and corrected three
presentation details: observer shutdown in the measured duration, the extra
60 seconds of plan observation, and scheduled slots versus actual sends.
No claim of latency equivalence follows from that review. Static review is
separate from the process and live evidence and does not establish a soundness
audit of the whole prover.

No protocol pin, genesis, RPC, serialization, database or dependency change is
part of this follow-up. The proof-duration predictor is an in-memory Composer
scheduling estimate; it cannot authorize execution, signing, canonical completion
or eviction. The owner-approved cryptographic advisory exception remains an
unresolved upstream dependency risk, as recorded in the linked CI evidence.
