# Observability integration on PR #26

Status: integration in progress; validation results below are pending unless explicitly recorded.

The requested base is [PR #26](https://github.com/eduadiez/rollupX/pull/26),
`2be04f7c8c3b9ac7ee96979ed27336324300da1d`, based on fork main
`fc0ee0516a1dbbdf9f785a3fa3d2f1789b6d2af7`. The previous observability tip
`8d9da271a90a7f015db4cd2275b4d6b1ac76cbb6` is retained as
`backup/network-observability-before-pr26-8d9da271`. The integration merges that
exact PR head into `feat/network-observability`, preserving both histories.
The protocol pin remains `3c0cdf4955941a5c23639346932b55b33babebbd`.
PR #26 must merge first; this branch can then follow it. No GitHub merge or
publication is performed by this local integration.

## Contract and compatibility

The [derivable-operations contract](../../docs/proposals/derivable-chain-operations.md)
is the new baseline: operations tag `0x03`, profile 1 from new-network genesis, mandatory
settlement policy, 65,536 logical blocks including the terminal, independently
validated signer preparation/cache references, and the bounded priority-fee
estimator. Observability must preserve those rules and the existing optional
telemetry configuration. Disabling telemetry does not restore removed protocol
formats, disabled-policy execution, or deprecated fee settings.

| Requirement | Integration | Verification |
| --- | --- | --- |
| Native lifecycle and stage metrics retain their actual owners | Preserve capture at Composer transitions, PR26 hash-bound witnesses and cache repair | Focused all-target compilation passed; Composer:243 / client:31 tests passed |
| New preparation/control RPCs remain distinct from signing | W3C propagation across capabilities/preparation/retention; preparation timing never increments authorized/signing counts | Signer:321 / client:31 / telemetry:44 / control RPC:6 tests passed; two explicit signer utilities ignored |
| Long-span correlation remains bounded and complete | Add exact-transaction paged correlation with at most 1,024 members/page, immutable evidence digest and canonical checks | Observer84, deriver51 and node-common21 tests passed, including actual two-page fetch and rejected second pages |
| PostgreSQL accepts the new format ceiling without rewriting history | Forward migration 0022 extends settlement count/ordinal bounds; retain older migration checksums | Upgrade from schema21, boundary and downgrade-refusal checks pending |
| Blob accounting preserves physical-wire meaning | Outer blob codec remains version0; tag3 is an inner operations format, not a new physical capacity version | Blob codec24, payload codec32 and protocol169 tests passed; database accounting checks pending |
| Native fixtures use the supported policy and exact runtime configuration | Default/custom mandatory-policy canaries; removed flags absent; fee ceiling present in captured settings | E2E compilation and seven native trace-checker regressions passed; functional canaries pending |
| CI retains both PR26 partitioning and observability failure drills | Four serial partitions; first cross-chain partition owns physical quota, serial fallback also runs it | Artifact contract:32 passed/1 opt-in skipped; workflow:19 passed; resource:7 passed; full tooling:248 passed/5 explicit opt-in skips; real tiny-workspace compilerless relocation passed |

Workspace/all-target compilation, strict Clippy and formatting passed. The final
unpaged correlation lookup change additionally passed all21 node-common tests
and strict package Clippy. Driver56, L1144, node44, follower8 and testkit10 tests
also passed. The existing correlation response shapes are retained while their
canonicality check now uses one indexed pass for large ranges.

## Evidence boundaries and remaining gates

The [earlier release record](release-runtime.md) remains historical evidence for
its named binaries. Its release artifacts and old frozen-main benchmark baseline
must not be relabelled as validation of this integration. Future comparative
performance work needs a PR26-compatible uninstrumented baseline and newly built
candidate artifacts with matching workload/configuration; existing unbound plans
must be regenerated after explicit resumption.

Performance comparisons and calibration remain deferred at the user's request.
The earlier fitting40 failure is retained with its original scope, without
attributing it to the new baseline. Remote CI, publication approval, private
rollout inputs/owner, independent heartbeat, off-host recovery and seven-day
operational calibration remain separate acceptance gates. PR26's own outstanding
release-capacity/recovery measurements are not supplied by this integration.

## Existing PR26 follow-ups

Review of the inherited preparation path found two lifecycle gaps outside this
observability integration: streamed preparation ingestion and final cache
publication are not bounded by the absolute request deadline, and dropping a
preparation RPC does not signal its cancellation token to an already-running
blocking worker. The validation permit remains owned by that worker, and shutdown
waits for the preparation/validation owners; this is not a lost-worker claim.
These paths are unchanged here. The new RPC timer and worker timer report their
separate actual lifetimes, and preparation never increments signing counters.
