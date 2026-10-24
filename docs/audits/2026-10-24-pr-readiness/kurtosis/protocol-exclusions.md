# Pinned protocol exclusions and remaining live coverage

At protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f`, `all` selects 34 of 64
scenarios. This is an inventory of the other 30, not a live passing result.
The runner has not been modified to remove any exclusion.

The exact marker/reason/source digest for every row is in
[the machine-readable inventory](protocol-exclusions.json). The marker is the
effective selector. Eight scenario comments still say they are included by
default, contradicting their markers; the pinned checkout is preserved.

Static comparison identifies 26 scenarios whose call patterns exceed an explicit
current signer-profile restriction, two requiring the staged runner's exact
negative-test oracle, and two L1-origin cases without an established profile
prohibition. Those last two are a live coverage gap, not proven incompatibility.
Anvil success uses a different proof setup and does not prove acceptance by the
real signer. The excluded set contains no performance or stress tests. The
missing-proxy cases intentionally test an absent contract and require a
rejection-aware runner.

| Scenario | Classification | Relevant rule / unverified behavior |
| --- | --- | --- |
| `catchNestedRevertAfterCallback` | profile limitation | A failed cross-chain frame contains a completed callback/descendant; failed calls with descendants are outside the signer profile. |
| `catchRemoteAndNestedRevertsAfterSuccessfulCallbacks` | profile limitation | A failed cross-chain frame contains a completed callback/descendant; failed calls with descendants are outside the signer profile. |
| `catchRemoteRevertAfterCallback` | profile limitation | A failed cross-chain frame contains a completed callback/descendant; failed calls with descendants are outside the signer profile. |
| `catchNestedRevertAfterCallbackL2` | profile limitation | A failed cross-chain frame contains a completed callback/descendant; failed calls with descendants are outside the signer profile. |
| `catchRemoteAndNestedRevertsAfterSuccessfulCallbacksL2` | profile limitation | A failed cross-chain frame contains a completed callback/descendant; failed calls with descendants are outside the signer profile. |
| `catchRemoteRevertAfterCallbackL2` | profile limitation | A failed cross-chain frame contains a completed callback/descendant; failed calls with descendants are outside the signer profile. |
| `nestedRollbackGroupsRestoreStateBeforeContinuationL2` | profile limitation | Inner and outer source rollback windows overlap; current canonical revert spans must be non-overlapping. |
| `nestedRollbackRestoresInnerThenOuterStateL2` | profile limitation | Inner and outer source rollback windows overlap; current canonical revert spans must be non-overlapping. |
| `rollbackGroupWithStaticReadThenWriteL2` | profile limitation | One L2 host mixes static roots with nested mutable roots, reverted mutable roots or root rollback spans; the current mixed-static host profile allows successful flat mutable roots only. |
| `deepStaticReadCallbacksUnderStaticAndMutableCalls` | profile limitation | A static callback itself makes another cross-chain read; admitted top-level static callbacks must be terminal. |
| `mutableCallMakesReadThatCallsBack` | profile limitation | A static call inside a mutable cross-chain frame has callback descendants; only terminal static calls are admitted in that position. |
| `nestedStaticLocalWriteL2` | profile limitation | A static call inside a mutable cross-chain frame has callback descendants; only terminal static calls are admitted in that position. |
| `staticLocalWrite` | live validation pending | No prohibiting rule established in the current L1-origin profile. Explicit live selection is needed; default exclusion alone does not show unsupported behavior. |
| `staticReadCallsBackAndReadsAgain` | profile limitation | A static callback itself makes another cross-chain read; admitted top-level static callbacks must be terminal. |
| `staticThenNestedWrites` | live validation pending | No prohibiting rule established in the current L1-origin profile. Explicit live selection is needed; default exclusion alone does not show unsupported behavior. |
| `topLevelStaticReentrantMissingProxy` | negative-test runner prerequisite | The staged runner expects terminal SimulationFailed with the exact missing static callback proxy error and no receipt; parallel successful-execution verification is not that oracle. |
| `deepStaticReadCallbacksUnderStaticAndMutableCallsL2` | profile limitation | A static callback itself makes another cross-chain read; admitted top-level static callbacks must be terminal. |
| `mutableCallMakesReadThatCallsBackL2` | profile limitation | A static call inside a mutable cross-chain frame has callback descendants; only terminal static calls are admitted in that position. |
| `nestedStaticLocalWriteL1` | profile limitation | A static call inside a mutable cross-chain frame has callback descendants; only terminal static calls are admitted in that position. |
| `staticLocalWriteL2` | profile limitation | Local writes change callback results at the same L2 static lookup position; the current profile rejects conflicting results without mutable cross-chain cursor advancement. |
| `staticReadCallsBackAndReadsAgainL2` | profile limitation | A static callback itself makes another cross-chain read; admitted top-level static callbacks must be terminal. |
| `staticReadCatchesCallbackRevertAndChecksUncaughtPropagationL2` | profile limitation | A static callback fails; current authenticated static callbacks must succeed and be terminal. Propagating a failed root with that descendant is also excluded. |
| `staticReadHandlesFailedCallbackL2` | profile limitation | A static callback fails; current authenticated static callbacks must succeed and be terminal. Propagating a failed root with that descendant is also excluded. |
| `staticReadPropagatesFailedCallbackL2` | profile limitation | A static callback fails; current authenticated static callbacks must succeed and be terminal. Propagating a failed root with that descendant is also excluded. |
| `staticReadThenCatchFailedWriteL2` | profile limitation | One L2 host mixes static roots with nested mutable roots, reverted mutable roots or root rollback spans; the current mixed-static host profile allows successful flat mutable roots only. |
| `staticReadThenNestedWriteCaughtRevertAndRollbackL2` | profile limitation | One L2 host mixes static roots with nested mutable roots, reverted mutable roots or root rollback spans; the current mixed-static host profile allows successful flat mutable roots only. |
| `staticReadThenRollbackWriteL2` | profile limitation | One L2 host mixes static roots with nested mutable roots, reverted mutable roots or root rollback spans; the current mixed-static host profile allows successful flat mutable roots only. |
| `staticReadThenWriteWithCallbackL2` | profile limitation | One L2 host mixes static roots with nested mutable roots, reverted mutable roots or root rollback spans; the current mixed-static host profile allows successful flat mutable roots only. |
| `staticThenNestedWritesL2` | profile limitation | One L2 host mixes static roots with nested mutable roots, reverted mutable roots or root rollback spans; the current mixed-static host profile allows successful flat mutable roots only. |
| `topLevelStaticReentrantMissingProxyL2` | negative-test runner prerequisite | The staged runner expects terminal SimulationFailed with the exact missing static callback proxy error and no receipt; parallel successful-execution verification is not that oracle. |

Profile rules: [supported scope](../../../../crates/eez-proof-signer/SPEC.md#1-purpose-and-supported-profile),
[entry classification](../../../../crates/eez-proof-signer/SPEC.md#83-entry-classification),
[outbound authorization](../../../../crates/eez-proof-signer/SPEC.md#10-outbound-authorization),
and the [pinned live runner guidance](../../../../eez-core-protocol/script/e2e/README.md).
