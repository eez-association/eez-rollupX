# Functional parity against the exact PR base

Status: independent review of `b0195b0b` against `a50c326d`, 2026-10-24 UTC. This is a supported-behavior comparison for **new networks**, not a request to restore legacy encodings or APIs. The [machine-readable matrix](parity.json) records every row, source, test, limitation and all 26 profile exclusions. No build, test, deployment or network lifecycle action was executed by this reviewer.

## Identity and evidence boundary

- PR [#101](https://github.com/eez-association/eez-rollupX/pull/101), base `main` at `a50c326d99f7cd1c21406eedbf370a344015a6b5`; candidate `codex/l1-history-offline-proving` at `b0195b0bc1ca90d71792fcfbc8613e8b00b00473`.
- Verified merge-base is exactly `a50c326d99f7cd1c21406eedbf370a344015a6b5`. Both core gitlinks and the clean initialized checkout are `84a5b5207880f192a6a5896cbfe0bac5249db47f` (`feature/e2e_edge_cases`). Upstream main was not substituted. The old RPC specification introduction still names `9a1f8153`; the actual main gitlink and pinned source, not that stale prose, govern this comparison.
- The worktree was clean when this review began. Parent-owned dependency/identity changes subsequently appeared and are outside this frozen comparison; they were preserved.
- Baseline [CI 39776847137](https://github.com/eez-association/eez-rollupX/actions/runs/39776847137) succeeded, including all eight native E2E partitions and Kurtosis. Its deliverables are OCI payloads, not GitHub downloadable artifacts.
- Existing final live roles come from verified CI `40104687331`, PR head `319ebc2d`. The delta to `b0195b0b` changes the prefix helper, its feature selection/consumers and test fixtures, not node/signer execution source. That does not turn the old executable identity into a newly built `b0195b0b` identity.
- The previous CI remains a failed run: 172 process passes and two first failures. Their corrected test-only fixtures have separate directed passes. A final all-green gate must be recorded independently.

Read contract: repository AGENTS, the pinned core execution/static/caveat/multiprover specifications, signer scope and authorization, RPC streaming/ownership, settlement policy, the accepted native proposal and L1 history contract. The core supports more call shapes than this signer.

## Result

No lost cross-chain feature was identified among the supported previous profile. The 26 excluded complex scenarios are **pre-existing signer limits**. The implementation retains ordinary Ethereum execution, supported nested/static/revert flows, quote/tariff checks, canonical queue ownership and keyless follower replay while deliberately replacing native encoding, treasury policy, history/PREVRANDAO and prover transport for new networks.

One concrete public RPC gap was reproduced during this review: `debug_traceCall` at `pending` rejects a required unexecuted history prefix. At stable L2 parent **12302**, the first read-only sample at **17:29:13.108603 UTC** returned `-32603` for pending debug, while debug `latest` and `eth_call pending` succeeded. No transaction was broadcast. The live record SHA-256 is `79b28fc523a3076dd4071d0a3fd298375a6bb718338e667c53a1ae7bb14c9278` (`artifacts/extended-validation/kurtosis/pending-debug/comparison.json`, local-only). The exact main control is still to run. A documented narrowing of the new RPC surface is not evidence of previous functionality being preserved. Root owns the correction.

The three-method pending adapter already provides the required integration point: reuse its checked prefix/state and Reth tracing, preserving trace options, historical selectors, transaction-index replay, executed pending payloads and blocking-worker permit ownership. Do not make the environment-only path accept missing history. Engine-auth `eth_call` is an internal surface and is recorded separately from public debug.

## Functional matrix

| Area | Previous contract → candidate | Evidence and remaining boundary |
| --- | --- | --- |
| F01: Ordinary EVM | Same pinned interpreter/precompiles; native nonce exception and final base-fee credit are the custom handler changes. | Plain/inspected success/revert/halt/refund/floor and SELFDESTRUCT fee controls; Safe/process/live execution. No exhaustive opcode differential or same-workload main A/B yet. |
| F02: Native transactions | `0x76` plus system nonce → accepted `0x45`/version 1, sourceHash, encoded checked gas, nonce-free reserved sender. | Intentional fresh-genesis wire/receipt/state change. Strict decoding and two Composer/two follower test; no old-network migration or closed Ethereum receipt-enum compatibility claim. |
| F03: 7702 | Ordinary L2 support retained. Cross-chain front 7702 envelopes remain rejected, as in main. | Live delegation/use/revocation plus three receipts in cold replay. Front rejection unit exists. This is not 7702 cross-chain support. |
| F04: Blobs / issuance | Native L1 DA retained; user L2 blobs and beacon withdrawals remain unsupported. | Block/Engine rejection controls, independent KZG/semantic checks and cold derivation. Posting receipts alone do not prove DA semantics. |
| F05: Nested cross-chain | Same core ABI, call tree, grouping, value/return/source attribution and one-L2 restriction. | 39 shared cross-chain process tests by name; all default 34 upstream executions have independent evidence after one helper continuation. No core-only multiprover/multi-L2 support claim. |
| F06: Reverts / rollback | Supported terminal failures and non-overlapping source spans retained. | Bidirectional caught-failure tests and revertCounter evidence; failed descendants/overlapping windows remain excluded. Proposed specific negative controls are below. |
| F07: Static calls | Same position-bound cached answers and admitted terminal callbacks. | Mixed/changing-static process tests; both extra L1 static scenarios now independently checked. Two MissingProxy early live rejections do not replace their staged terminal oracle. |
| F08: Fees / quotes | Base-fee burn → FeeVault credit intentionally; tips, tariff quotes, posting reservations, refunds and final admission retained. | Receipt-based accounting, underpayment and nonce-queue quote tests, owner rotation/withdrawal/replay. Compare formulas rather than identical wei amounts across different L1/history costs. |
| F09: History / PREVRANDAO | New checked delayed recorded import/repeat, shared seed and two anchor modes replace signer chain-provider authentication. | 122 stored L1 hashes, import ordering, delay/seed, negative evidence rules and cold catch-up controls. Delay is not finality; arbitrary pre-bootstrap history is not promised. |
| F10: Follower / prover | L1 DA/genesis-only follower retained; complete offline witnesses remove signer RPC. | 206 complete batches, 3558 selected heights, 814 receipts, 112 catch-up parts. Partial claimed 7134 stays separate; complete 7146 is selected. Receipt checks are status/canonical membership, not every log/gas field. |
| F11: Partial / restart / reorg | Canonical ownership and whole groups retained; history rewind and streaming recovery added. | Partial-prefix/restart/reorg/shared-signer process coverage. Pending queues/counters still in memory; no durable client-intent guarantee. |
| F12: Local liveness | Ordinary L2 production independent of L1 observation, proving and settlement. | Controlled roughly 72-second observer/missed-slot/signer outages with ordinary transfers and catch-up. Four calendar days and sustained mixed-load TPS are not established by that evidence. |
| F13: RPC | Public eth call/estimate/access-list pending repaired with the real prefix; debug pending remains unadapted. | Confirmed stable-parent debug failure above. HTTP evidence exists; WS/IPC registration was reviewed, without dedicated transport probes. Fix required before broad RPC parity claim. |
| F14: Prover / C# | `prove.v1` Prove/preparation/leases → single v2/offline portable streaming contract. | 29 conformance inputs and 24 proof expectations; execution/settlement negative gates and streaming recovery. Actual Nethermind server interoperability remains external/unverified. |
| F15: Rules / lifecycle | Same Ethereum/Gnosis dependency family and Reth shutdown backport; additional offline profile commitments. | Both native process profiles covered; one concrete Kurtosis test network. Later parent-owned Tokio change needs its own lifecycle/streaming check. |

Evidence owners: [exact CI inventory](../2026-10-24-pr-readiness/ci-runtime-consumption.json), [final live runtime](../2026-10-24-pr-readiness/kurtosis/final-runtime-validation.json), [cold replay](../2026-10-24-pr-readiness/kurtosis/cold-replay-final.json), [fault results](../2026-10-24-pr-readiness/kurtosis/live-faults-summary.json), [extra static results](../2026-10-24-pr-readiness/kurtosis/protocol-extra-results.json), [prefix helper correction](../2026-10-24-pr-readiness/prefix-history-verifier.json), [new MissingProxy observations](kurtosis/missing-proxy.json). Files called `live-baseline-summary` describe earlier **candidate** bytes, not main; they must not be used as the old-version A/B control.

## The 26 scenarios outside the signer profile

These classifications follow the scenario call trees and the accepted profile, not their exclusion comment text. All 30 pinned source digests match the earlier inventory. The common core gitlink proves the scenarios are identical. `entries/mod.rs` is byte-identical between base and candidate; the materializer/verifier changes only remove redundant zero-valued callback-count plumbing, not these shape checks. The same signer scope already appears in main.

| Existing limit | Scenarios | Existing check / coverage boundary |
| --- | --- | --- |
| Failed cross-chain frame has descendants | `catchNestedRevertAfterCallback`<br>`catchRemoteAndNestedRevertsAfterSuccessfulCallbacks`<br>`catchRemoteRevertAfterCallback`<br>`catchNestedRevertAfterCallbackL2`<br>`catchRemoteAndNestedRevertsAfterSuccessfulCallbacksL2`<br>`catchRemoteRevertAfterCallbackL2` | Materializer rejection has a unit control. The 26-scenario inventory is static; no live signer rejection is claimed for these six complete scenarios. |
| Nested source rollback windows overlap | `nestedRollbackGroupsRestoreStateBeforeContinuationL2`<br>`nestedRollbackRestoresInnerThenOuterStateL2` | Both overlap guards are present. No dedicated exact overlapping-window negative was identified in this scoped test review; nested retry positive tests are not that negative. |
| Static sources mixed with nested, failed or rollback mutable sources | `rollbackGroupWithStaticReadThenWriteL2`<br>`staticReadThenCatchFailedWriteL2`<br>`staticReadThenNestedWriteCaughtRevertAndRollbackL2`<br>`staticReadThenRollbackWriteL2`<br>`staticReadThenWriteWithCallbackL2`<br>`staticThenNestedWritesL2` | Successful flat mixed hosts are covered. Those controls do not individually reproduce all six excluded mixed/nested/rollback shapes. |
| Static callback itself has a cross-chain descendant | `deepStaticReadCallbacksUnderStaticAndMutableCalls`<br>`staticReadCallsBackAndReadsAgain`<br>`deepStaticReadCallbacksUnderStaticAndMutableCallsL2`<br>`staticReadCallsBackAndReadsAgainL2` | Materializer test asserts the callbacks-with-descendants error; not a full live signer scenario. |
| Static read inside a mutable cross-chain frame has callbacks | `mutableCallMakesReadThatCallsBack`<br>`nestedStaticLocalWriteL2`<br>`mutableCallMakesReadThatCallsBackL2`<br>`nestedStaticLocalWriteL1` | Materializer test asserts nested-static-with-descendants rejection; not a full live signer scenario. |
| Different answers share the same L2 position/lookup identity | `staticLocalWriteL2` | Signer outbound authorization rejects conflicting cached results; distinct-position results have a positive control. This does not claim execution of the complete excluded core scenario. |
| Static callback fails, caught or propagated | `staticReadCatchesCallbackRevertAndChecksUncaughtPropagationL2`<br>`staticReadHandlesFailedCallbackL2`<br>`staticReadPropagatesFailedCallbackL2` | Materializer test asserts failed-static-callback rejection. A propagated failed root also violates the failed-descendants rule. |

Counts: **6 + 2 + 6 + 4 + 4 + 1 + 3 = 26**. No entire excluded scenario is labeled live-passing. A core Anvil pass uses a different proving arrangement and does not establish signer authorization. Extending any of these shapes requires an explicit profile/validation change; relaxing its rejection is not parity work.

The other four excluded-by-default scenarios have a different status:

- `staticLocalWrite` and `staticThenNestedWrites` have independent L1-origin live evidence. The original staticLocalWrite collector FAIL remains; its later exact-post continuation adds an oracle, not a hidden retry of the scenario.
- `topLevelStaticReentrantMissingProxy` and its L2 counterpart have exact terminal `SimulationFailed` process regressions in `cross_chain.rs`. The fee-enabled live route now has two independently bound **early-admission** rejection results: correct missing proxy/chain, unchanged nonce/balance, no receipt or queue ownership. That does not claim the pinned staged terminal runner reached its expected branch.

The most useful additional negative coverage is one case per restriction class, paired with its nearest valid shape. For overlapping rollback and mixed static/nested windows, add an exact reason assertion that reaches the shape guard. For the other classes, carry the decoded semantic request into real authorization/signing and assert no signature/effect; an earlier decoder failure proves a different property. Existing materializer negatives and core successes must retain their narrower labels.

## Comparable baseline without a heavy build

The exact main payloads were identified from CI consumer jobs `119841498919` and `119841499207`. Their public registry manifests are still readable. Only metadata was fetched; neither layer was downloaded by this reviewer.

| Payload | Immutable reference | Compressed layer |
| --- | --- | --- |
| Runtime | `ghcr.io/eez-association/eez-rollupx-ci-runtime@sha256:97af11400987132646cde6a036425b3cf870184bcdbf7166549d38eee9f57f98` | 146,416,008 bytes |
| Tests | `ghcr.io/eez-association/eez-rollupx-ci-tests@sha256:86c735ce0f79cd3319a2fd34c82499b1ce46b028f2f3f63cbe15c83893fd43d0` | 159,330,256 bytes |

The JSON record also contains the matching node/signer/deploy image digests. Plan:

1. Use a dedicated clean checkout of exact main, its pinned protocol and its own `ci-payload.sh`/`prepare-runtime-artifacts.sh`/native consumer. Validate the root, protocol, lockfile, platform, archive and binary hashes. Do not substitute cached current executables.
2. Use exact baseline deploy contracts/genesis and fresh datadirs with owned disjoint ports/container/enclave names. Never start the old `0x76` runtime against the new `0x45` chain. The existing native runner calls `forge build`; a strictly build-free live comparison must use the matching published deploy payload. Do not substitute current contract artifacts.
3. List the archived test executables and require a nonzero intended selection. The nine shared exact test names in the JSON have been verified in both source revisions; 39 cross-chain, 15 quote, five partial-consumption and 28 dynamic-admission names are common. Name equality is only a selector check: inspect assertions when comparing outcomes.
4. Run a small common semantic set first: bidirectional smoke, wrapper returns, a caught revert, changing static reads, quote/underpayment, partial restart, reorg and a finite mixed wave. Use the matching test/runtime bundle for each revision.
5. For a load comparison, keep workload mix/amounts, network rules, container resources, build profile and concurrency fixed. Run serially without heavy builds. Report offered, accepted, included and settled work, actual state/returns, fee formulas, queue outcomes and follower evidence. Record contention and first failures.

Different genesis/native envelopes, mandatory history gas and the accepted fee-recipient change make block hashes and absolute costs unsuitable equality targets. Do not normalize commitments or claimed/applied endpoints to force agreement. Semantic parity and performance are separate claims; no main/candidate performance measurement is established yet.

## Remaining closure work

1. Correct and regression-test the reproduced public pending debug gap; compare the same input on main. Retain the failure and verify both advance and repeat imports without weakening missing/malformed-history rejection.
2. Complete the current candidate CI and exact-baseline comparison. Prior failed CI and retry-only evidence remain explicitly failed, even after targeted fixture corrections.
3. Add the small exclusion-boundary negative set where current tests stop at materialization or a different failure. Keep the supported profile fixed.
4. Record transport and actual Nethermind integration as separate limits. Neither generated clients nor Rust conformance proves an external engine executes the protocol correctly.

This audit does not assert legacy compatibility, an exhaustive EVM conformance result, durable queue recovery, arbitrary reorg immunity, complete receipt-field comparison, a four-day wall-clock stress result or sustained-throughput parity.
