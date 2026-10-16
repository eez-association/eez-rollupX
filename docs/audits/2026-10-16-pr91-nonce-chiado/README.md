# Omitted-nonce correction: CI qualification and Chiado handover

Status: qualified, deployed and verified on the existing Chiado rollup. Public
RPC is restored. This engineering record does not establish independent
security review. The documentation commit containing this record is a successor
to the tested runtime source; it was not rebuilt into the deployed images.

| Field | Value |
| --- | --- |
| Date | 2026-10-16 UTC |
| Scope | Qualify and deploy the L1 omitted-nonce correction from PR 91; preserve the existing rollup, fee policy and accepted work |
| PR head | `bfae82bf08bc18a59a584d941f3b846ad45c3128` |
| PR base | `2c19e36bed2720c00f89841ccfca47ec7c921e06` |
| Tested runtime source | CI merge commit `646c3f9042014939a1283ea9c45f6cacebce0c1c`; its tree equals the PR head's tree, `385f7b3a7272a3a3b460561d776dfb9343d39728` |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Previous deployed source | `aa8d8a5894c69502c67dcae35d5184bc9a79b036` |
| Tools | Rust 1.95.0, Foundry 1.7.1; locked Cargo dependencies; CI release artifacts |
| Related records | [Merge-readiness review](../2026-10-16-pr91-merge-readiness/README.md), [PR 56 attribution correction](../2026-10-16-protocol-pr56/README.md), [proposal](../../proposals/cross-chain-fee-recovery.md#omitted-l1-nonce-follow-up-from-pr-56-feedback-on-2026-10-16) |

## Behavior and limits

An L1 quote without a nonce selects the first available nonce after the
Composer's contiguous owned prefix and replays those signed predecessors in
disposable native state. This supports quoting another same-account source while
earlier sources remain unmined. Explicit nonces remain exact, and quotes do not reserve nonces. The
eight-predecessor bound and final execution/funding checks remain in force.
L2 future-nonce replay and ordinary-mempool predecessor replay remain unsupported.

The [before/after regression](../2026-10-16-pr91-merge-readiness/README.md#omitted-l1-nonce-correction)
reproduced the old rejection and passed with the correction, including nested
predecessor-dependent effects, both encodings and underpayment rejection. The
candidate also includes the narrowly scoped Rustls lockfile update and CI
fixture relocation correction described in that record. The node and signer
must be upgraded together because their validation identity includes the lockfile.
No protocol, contract, genesis or fee-policy migration is required.

## Release qualification

All results below refer to [CI run 38933052555](https://github.com/eez-association/eez-rollupX/actions/runs/38933052555),
not a rebuilt approximation of its executables.

| Check | Evidence type | Result |
| --- | --- | --- |
| Lint and dependency/security checks | CI | Passed |
| Workspace debug unit tests | Unit | 1,480 passed; three explicit manual/fixture-inventory tests ignored |
| Reth shutdown regression | Unit | One passed |
| Release protocol/vector/blob checks | Release tests | 30 passed |
| Embedded native suite, four partitions | Release process | 42 passed |
| Cross-chain native suite, four partitions | Release process | 112 passed, including all 17 fee/Safe tests and both pending-predecessor regressions |
| Kurtosis | Live enclave | All 34 enabled protocol scenarios passed; causal coverage and semantic blob evidence complete |
| Aggregate CI gate | CI | Passed; all 13 jobs successful |
| Runtime payload and image provenance | Artifact verification | Passed; manifest source/protocol, archive hashes, labels and published digests verified |

Partition-filtered tests are not additional executed tests. The three ignored
unit tests are the held-pool profiling test and two signer fixture/feasibility
inventory tests. No behavioral test was retried to convert a failure into a pass.
The six protocol scenarios excluded from the default suite remain exclusions:
`staticLocalWrite`, `staticLocalWriteL2`, `nestedStaticLocalWriteL1`,
`nestedStaticLocalWriteL2`, `topLevelStaticReentrantMissingProxy` and
`topLevelStaticReentrantMissingProxyL2`. The general protocol CI uses its existing
fee-disabled profile; the release fee/Safe tests and live canaries separately
qualify fee enforcement.

## Deployment and live checks

The user authorized pushing the fix to the existing Chiado deployment. The
handover ran from 18:06:56 to 18:08:03 UTC. Public transaction ingress was paused,
both Composer queues and the ordinary ready pool were drained, and the captured
L2 watermark was checked against canonical settlement. Both services stopped
cleanly with exit code zero. Their complete stopped databases were copied to
separate directories before the matching CI node/signer pair was started;
original databases and private configuration were retained for rollback.

The original bindings, genesis, service environment and fee policy are unchanged.
Old Compose revision labels were stale; the new labels identify the actual
verified runtime source. Running Composer and signer executable hashes match
the verified CI payload. No contract deployment, genesis regeneration, new
protocol pin or mutable image-tag promotion occurred. Rollback was prepared,
not executed; an older binary must use its retained original databases and
matching configuration, never candidate-modified state.

The semantic canary used unmodified protocol PR 56 at
`bcd77d8b0d23ed9086f12ac8a0c2e65c6a2c2688`. Its fee client omits the nonce, so
it exercises the Composer correction without the separate explicit-nonce client
workaround. Contracts and compiler settings match the deployed protocol; its
isolated Foundry build and read-only readiness checks passed. The workload was
`counter-multi-tx nestedCounter nestedCounterL2`: three jobs and five source
transactions, with semantic and transaction-link checks enabled. The wrapper
held shared wallet locks across all relevant checkouts throughout the run.

| Live check | Result |
| --- | --- |
| Semantic assertions and canonical source links | Three jobs passed; all five sources committed; zero failed or unverifiable jobs |
| Omitted-nonce burst | Same sender, nonces 369, 370 and 371; all included in L1 block 23551204 after a 1.2-second send phase |
| Quote provenance | All five requests omitted nonce and used Composer quotes; both encodings returned, EIP-1559 selected; zero fallback or tariff rejection |
| Batching | The three-source burst and independent nested L1 source shared one post; the nested L2 source used a separate post |
| Native blob authentication | Both posts passed independent KZG/native semantic verification; L1 sources bound through verified settlement tables, L2 source bound by exact transaction bytes |
| State preservation | Prior canonical safe block and pre-stop watermark retained with identical hashes/state roots; canonical progress resumed and both queues empty |
| Public APIs | Four HTTP and two WebSocket routes passed; compact/debug quotes returned both formats in both directions |

The live runner completed once in 32.5 seconds; this small canary is not a
throughput benchmark. Its setup included ordinary fixture deployments/funding,
which are distinct from the five measured cross-chain sources. The canonical
safe height initially stayed unchanged while the restarted chain was idle;
pre-canary health verified preservation, and post-canary health separately
required advancement. A read-only probe raced the deliberate service stop and
received connection refused; it was not a failed runtime acceptance test.
No candidate service restart or canary retry was needed.

Public routing was restored at 18:11:52 UTC after the semantic and blob checks.
The original gateway configuration was restored byte-for-byte. The exact
post/source hashes, images, executable hashes and check summaries are recorded
in [evidence.json](evidence.json).

## Evidence and remaining checks

The [sanitized manifest](evidence.json) is checked in. The CI run and its logs are
retrievable while GitHub retains them. Private handover records, logs, image
verification output and the canary checkout are
local-only under the operator checkout's ignored
`artifacts/pr91-chiado-deploy-20261016.bou0ize2/`; they are not present in another
checkout and have no guaranteed retention. Secrets and bulk logs are not part
of this record.

All checks for this bounded deployment passed. The full Chiado stress matrix,
live Falcon workload and a fresh independent Chiado follower replay were not
repeated; earlier live results are historical, and the release process tests
provide this revision's fee/Safe and replay coverage. In-memory accepted
queues and recovery observations remain non-durable; database copies do not
provide durable queue recovery. The ready-pool drain probe does not enumerate
future, gapped transactions in Reth's separate queued subpool.
