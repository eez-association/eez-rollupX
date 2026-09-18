# Observability integration on PR #26

Status: local PR26 adaptation complete and functionally validated.
All results below apply only to the stated source and local functional scope.

The requested base is [PR #26](https://github.com/eduadiez/rollupX/pull/26),
`2be04f7c8c3b9ac7ee96979ed27336324300da1d`, based on fork main
`fc0ee0516a1dbbdf9f785a3fa3d2f1789b6d2af7`. The previous observability tip
`8d9da271a90a7f015db4cd2275b4d6b1ac76cbb6` is retained as
`backup/network-observability-before-pr26-8d9da271`. The integration merges that
exact PR head into `feat/network-observability`, preserving both histories.
The merge commit is `5f3b36b39af3ea8f56817fe1866cd49579c41806`.
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
| Native lifecycle and stage metrics retain their actual owners | Preserve capture at Composer transitions, PR26 hash-bound witnesses and cache repair | Focused all-target compilation passed; Composer: 243 / client: 31 tests passed |
| New preparation/control RPCs remain distinct from signing | W3C propagation across capabilities/preparation/retention; preparation timing never increments authorized/signing counts | Signer: 321 / client: 31 / telemetry: 44 / control RPC: 6 tests passed; two explicit signer utilities ignored |
| Long-span correlation remains bounded and complete | Add exact-transaction paged correlation with at most 1,024 members/page, immutable evidence digest and canonical checks | Observer: 84, deriver: 51 and node-common: 21 tests passed, including actual two-page fetch and rejected second pages |
| PostgreSQL accepts the new format ceiling without rewriting history | Forward migration 0022 extends settlement count/ordinal bounds; retain older migration checksums | 59 real PostgreSQL groups passed, including saved-schema21 upgrade, boundaries, rollback/reorg and downgrade refusal |
| Blob accounting preserves physical-wire meaning | Outer blob codec remains version0; tag3 is an inner operations format, not a new physical capacity version | Blob codec: 24, payload codec: 32 and protocol: 169 tests passed; real database accounting checks passed |
| Native fixtures use the supported policy and exact runtime configuration | Default/custom mandatory-policy canaries; removed flags absent; fee ceiling present in captured settings | E2E compilation and ten native trace-checker regressions passed; default-policy lifecycle, disabled-telemetry and tracing native canaries passed |
| CI retains both PR26 partitioning and observability failure drills | Four serial partitions; first cross-chain partition owns physical quota, serial fallback also runs it | Artifact contract:32 passed/1 opt-in skipped; workflow:19 passed; resource:7 passed; full tooling:249 passed/5 explicit opt-in skips; real tiny-workspace compilerless relocation passed |

Workspace/all-target compilation, strict Clippy and formatting passed. The final
unpaged correlation lookup change additionally passed all 21 node-common tests
and strict package Clippy. Driver: 56, L1: 144, node: 44, follower: 8 and testkit: 10 tests
also passed. The existing correlation response shapes are retained while their
canonicality check now uses one indexed pass for large ranges.

## Executed local validation

The source-built default-feature runtimes are frozen at merge commit `5f3b36b3`.
They are development-profile functional artifacts, not a sealed release bundle.
The [sanitized evidence manifest](pr26-integration.json) records source identities,
binary and report digests, and local evidence paths. Those ignored `target/` paths
are local-only and unavailable from another checkout; raw logs are not published.

| Check | Result |
| --- | --- |
| `cargo +1.94.0 build --locked --offline --workspace` | Passed; all four shipping-role binaries also built with default features |
| Workspace `check --all-targets`, strict Clippy, `fmt --all --check` | Passed |
| Focused Rust suites and independent KZG/public-input/rolling-hash vectors | 1,304 passed; two explicit signer inventory/measurement utilities ignored |
| `benchmark-lock-timing` feature | Affected targets compiled only; no benchmark or calibration executed |
| CI Python tooling | 249 passed, five opt-in skips; 254 selected |
| Real compilerless artifact relocation/shard/quota fixture | Passed with portable mold on PATH; shared handoff follow-up passed in 20.599 seconds |
| Real Docker compile-input and producer/consumer packaging fixtures | Both passed in 30.326 seconds, including exact packaged binary hashes and execution with compilers blocked |
| Observability Python tooling | 133 passed, one opt-in physical PostgreSQL cgroup test skipped; 134 selected |
| `smoke-database.py --previous-v21-binary ...` | 59 groups passed, including populated schema21 upgrade, all 22 checksums, 65,536-member boundaries, atomic rollback/reorg and downgrade refusal |
| `smoke-restore.py` | Six groups passed in fresh projects/volumes on the same Docker host; owned-resource cleanup verified; recovered database, dashboards, credentials, trace/log/metric navigation and heartbeat delivery |
| `lifecycle_bidirectional_cross_chain_smoke` | Passed in 68.94 seconds with native lifecycle, metrics and observer checks |
| `minimal_bidirectional_cross_chain_smoke` | Passed in 42.91 seconds with telemetry disabled |
| `restart_after_partial_keeps_accounting_and_rebuilds_intent` | Passed in 133.29 seconds: partial settlement/revert accounting, Composer restart, original unresolved admission, separate readmission, exact paid populations and follower convergence |
| `native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order` | Passed in 111.37 seconds: actual L1 replacement, orphan receipt/allocation retraction, canonical recovery in nonce order and Composer/follower convergence |
| `traced_bidirectional_cross_chain_smoke` | Passed in 48.41 seconds with the corrected success oracle; exact ancestry through control/preparation/retry/signing and matching Composer/signer success logs |

The recovery harnesses were built at `2b4056b4`; production source remains
identical to the frozen `5f3b36b3` runtimes. Both native recovery tests used actual
RPCs, original lifecycle spools and the schema22 observer, with a private
PostgreSQL socket and all migration checksums verified. Transient RPC/chain-fact
gaps during restart/reorg recovered; neither test replaced native evidence with
synthetic events. Both owned PostgreSQL containers were confirmed absent after
completion. These functional results do not measure performance or sizing.

The database and restore tests used a separate release-profile observer built
from a minimal archive of `5f3b36b3` in pinned Rust/Bookworm tooling. The host GCC15
binary needed `GLIBC_2.38` while the PostgreSQL fixture image uses Bookworm; the
first database attempt therefore failed before migrations. That failure is
retained. The compatible observer then passed both fixtures. This portability
build does not validate the entire shipping image or replace remote CI.

The first native tracing attempt failed its Composer log assertion. Retained
OTLP and JSON logs establish that both Composer and signer logged the successful
trace, but the test selected an earlier connected, failed signer request. The
fixture correction requires the matching successful preparation outcome before
selecting the trace; graph ancestry, retry, admission membership and both success
log assertions remain mandatory. The original failure remains in the evidence. The correction is commit
`70c44cb6bb6dda68827576590b4c598ed3624576`; its ten deterministic trace tests,
strict Clippy and formatting passed. Its rebuilt harness passed the native
tracing canary against the unchanged `5f3b36b3` runtime binaries.

A later opt-in Docker packaging run exposed a missing quota-handoff setup in
its tiny workspace: the consumer correctly refused to proceed without the
required image selector. The original failure is retained. The two existing
tiny-workspace tests now share one handoff fixture that invokes the archived,
ignored quota entrypoint explicitly and checks success. The Docker test also
requires its completion marker/report. Both real packaging tests and the real
compilerless relocation follow-up passed; physical ENOSPC behavior remains the
separate mandatory native gate, with no bypass added to production tooling.
These are packaging fixtures, not sealed application release artifacts.

No external Cargo dependency version, source, checksum or dependency edge changed.
Only existing workspace development dependencies were added for integration tests.
Migrations 0001–0021 remain byte-identical. Whitespace warnings in imported PR26 raw
evidence and the pre-existing migration0019 EOF are preserved to retain artifact
and migration identity; changed source/prose passes `git diff --check`.

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
