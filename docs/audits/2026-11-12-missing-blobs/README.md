# Missing native blob observations (#144)

This qualifies the existing native-blob observation contract and fixes three
reproduced acquisition defects. It does not establish parity with the source's
finalized missing-blob policy or change the accepted network profile.

| Field | Value |
| --- | --- |
| Date/status | 2026-11-12 UTC; component validation complete, process execution blocked by storage quota; draft PR |
| Scope | #144's unchanged policy qualification criterion; narrow supported fixes |
| Source | Baselines below; candidate and regression inputs in the accompanying PR |
| Protocol | Exact initialized gitlink below, no edits |
| Executables/environment | Identities and sanitized settings below; local-only binaries/logs |
| Related records | #144/N10, #83, #87; shared harness startup work #142 remains separate |

## Baselines and scope

- Implementation baseline: association main
  `601ebfa9c7e1a779f1afbd077bd31a3207e7a61b`.
- Protocol, initialized and unchanged:
  `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Original N10 frozen source: `eez-association/eez-rollup0`
  `1b66d9bda675e6f8e83f514153a97136909acc06`, protocol
  `143d87e62cade98bfc122d612638dcaaf1c85633`.
- Original frozen target: `31e148f50dd79730b17d113ac78795eb377eb5c5`, protocol
  `84a5b5207880f192a6a5896cbfe0bac5249db47f`. The issue's later code check was
  `716a8a3bad4415ade7e9a6bc3af4a1b35419b232`, not a replacement baseline.

[#144](https://github.com/eez-association/eez-rollupX/issues/144)'s original
criterion and frozen findings remain unchanged. The original review inputs were
**not independently checked**. Reading its linked frozen transport source here
does not independently establish that review's provenance or a complete source
policy matrix. Source calldata/timeless paths remain excluded. This work does
not restart the broader backport audit.

## Accepted behavior and flow

The pinned [blob format](../../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md)
defines encoding; it supports more transports than this fork's accepted
[mandatory native carrier](../../../crates/eez-proof-signer/SPEC.md#11-data-availability-and-sync-block-verification).
The local [L1 typed errors](../../../crates/eez-l1/src/error.rs),
[settlement ownership contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission)
and [operator retention procedure](../../runbooks/ethereum-node.md#recovery-with-a-pruned-l1)
already separate unavailable observations from invalid authenticated data.
Finality does not supply unavailable bytes.

| Observation | Existing typed handling after correction | Progress/recovery |
| --- | --- | --- |
| Required execution body missing or returned hash/number differs | `SourceIncomplete` | Retain the range; re-observe containing identity |
| Empty response or no body matching a selected transaction hash | `SourceIncomplete` | Retain the range; retry after source recovery |
| Blob HTTP 503, or new/legacy endpoints both return 404 | `SourceIncomplete` | No DA inferred from finality or endpoint status |
| Malformed beacon structure, short blob, advertised commitment mismatch | `Provider` | Retry observation; no terminal on-chain conclusion |
| Stalled beacon headers or response body | `Provider` after the 30-second total HTTP deadline | Return to the existing retry owner |
| Body matches committed KZG hash but violates native packing/encoding | `Decode` | Terminal rejection; no unchanged-input retry loop |
| Nonempty batch calldata, missing physical carrier, wrong selection/order/capacity | Rejected by native profile checks | No calldata substitute or capacity relaxation |

`scan::fetch_log_transaction` binds transaction hash, block hash/number and
position. Hydration retrieves the containing block by hash, checks hash/number,
uses its timestamp's blob allowance, recomputes KZG hashes and selects every
transaction blob in order. Only then does native decoding populate operations
and semantic transactions. The standard full-blob endpoint's 404/405 fallback
is to the legacy **beacon blob** endpoint, never to execution calldata.

The live watcher hydrates a complete scan before advancing its ring or emitting
`BatchPosted`/`ScanComplete`; a detected reorg can still retract the old tip.
The reader now also commits its in-memory chunk range only after hydration.
Deriver catch-up already aborts on the reader error and discards that chunk
iterator; no production false completion from the iterator bug was established.
Its `finish_scan` checks canonical hashes/generation and safe state before
publishing scan completion. L2 finality is bounded by validated indexed safe
state, not merely the reported finalized L1 height.

Composer reconciliation requires completed canonical scan evidence before
resolving ownership. A receipt, missing blob or failed observation is not
canonical completion and does not authorize eviction or a source retry charge.
The offline signer does not fetch beacon data: its caller must supply complete
physical blobs. Missing/invalid carriers remain rejection cases under its
existing status contract. No gRPC status or validation identity changes here.

The watcher retries nonterminal observations on later polls. Composer/follower
boot retries `SourceIncomplete` with backoff; `Provider` retains the existing
32-failure transport budget, then aborts startup with readiness closed. A retry
classification promises neither unlimited service nor eventual availability.
Permanently pruned data requires a history-serving source or supported backup.
The beacon client now has the same 30-second total request bound as canonical
L1 reads; previously a stalled body could prevent the retry owner from running.

## Reproduction and validation

Rust `1.95.0` (`59807616e`, LLVM 22.1.2), Foundry `1.7.1`
(`4072e48705af9d93e3c0f6e29e93b5e9a40caed8`), protoc `3.21.12`.
Cargo uses `--locked`, two jobs, task-owned `/tmp/issue144-target`, no incremental
compilation or debug information. No cache or service belonging to another task
was modified. All new HTTP listeners use ephemeral loopback ports; no fixture
needs a writable node datadir.

On unchanged runtime behavior plus the new tests, `cargo +1.95.0 test --locked
-p eez-l1 --lib` produced **208 passes and two failures**:

1. `blob_hydration_failure_retains_chunk_until_recovery`: missing blob returned
   `SourceIncomplete` but consumed the only chunk.
2. `unavailable_blob_observations_preserve_progress_across_finality`: malformed
   beacon `data[0]` became terminal `Decode` before any hash authentication.

The first test executable is retained locally with SHA-256
`18a6dd3c2680dad82ddb8941563619d52dd42ed80fbed1e804fe926e5af69774`.
Initial fixture compilation required explicit `Bytes`/`Block` types; the first
sandboxed run also denied socket binds (172 passed, 38 failed). Those logs are
retained and do not count as product failures or test coverage. The unsandboxed
baseline above reached both intended production failure mechanisms.

A third test, `stalled_beacon_response_reaches_retry_owner`, served successful
headers with an incomplete response body and held the socket open. On the
unchanged `Client::new()` construction it failed at the test's 35-second outer
deadline (one selected test failed, 210 filtered). This check was added after
the first two fixes; a separate whole-baseline run of this third test was not
performed. Its test executable SHA-256 is
`542fefdc02a7b5f1697c7b22e805a83be1950b8138b2e7915500e6bf7cfbead1`.
An initial short-name `--exact` invocation selected zero tests and is excluded
from evidence. The corrected fully qualified selection reached the stalled
beacon body; after the fix this must return nonterminal `Provider` at 30 seconds.

The first corrected L1 suite passed **210 tests**, none ignored or filtered. The
watcher matrix checks six negative observations both before and after a known
finalized containing block, then restores matching native bytes. It asserts
exact error categories, no head/batch/completion events or ring advancement on
failure, and exact payload delivery on recovery. Separate controls assert
missing/changed containing headers remain unavailable and authenticated invalid
native packing remains terminal. The reader repeats the same chunk after a
missing response and consumes it only on successful hydration.

These are component tests using real loopback beacon HTTP and mocked execution
RPC responses, not live consensus finality or database pruning. The finalized
case explicitly refreshes the watcher's finalized state before observing the
missing batch; it does not run a consensus client. Existing blob capacity,
mandatory-carrier, foreign-batch and scan/canonical-publication tests also run
in this L1 suite.

With the deadline regression included, the final L1 suite passes **211 tests**,
none ignored or filtered; the stalled-body test returns `Provider` at 30 seconds.
Its executable SHA-256 is
`5922b1393c41cf04f31b7a11049c48dfcf3a910548dc01e9da91ad1207215c4b`.
Workspace Clippy (`cargo +1.95.0 clippy --locked --workspace --all-targets --
-D warnings`) and `cargo +1.95.0 fmt --all --check` pass. The initial Clippy
invocation found two new test lint errors (unused read length and inverted
Boolean branch), corrected before the passing run. Integration fixture build
(`forge build --root contracts`) passes with existing compiler/lint warnings.

The selected native process regression was attempted with prebuilt overrides
cleared, the same Cargo settings above, and task-owned
`EEZ_TEST_LOG_DIR=/tmp/issue144-evidence/process-logs` and
`EEZ_TEST_DATADIR_DIR=/tmp/issue144-evidence/process-datadirs`:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  settlement_policy::missing_blobs::fresh_follower_waits_for_missing_and_malformed_blobs_then_replays \
  --exact --test-threads=1
```

It failed during dependency compilation (`rustls` metadata write: `Disk quota
exceeded`), **before executing any process test**. `/tmp` had free mount space
but its user quota was exhausted: a read-only kernel quota query reported
24.36314 GiB used against a 24.36324 GiB hard limit. The task-owned target was
about 2.7 GiB; the worktree filesystem had less than 1 GiB free. No fixtures or
process datadirs were created. The failed build log is retained; there was no
test retry or retry-only pass. After preserving all test executables, 256 MiB
of this task's generated check metadata was removed to permit evidence/PR work;
no other task's cache was changed.

The new process source passes workspace Clippy/type checking but remains
**unexecuted**. It is designed to start fresh followers behind a task-local
beacon proxy, observe repeated missing/malformed-data boot failures with no
Deriver readiness or transfer receipt, restore the authentic source, and compare
safe block hashes and exact receipts with the producing node. This planned
independent replay and its semantic assertions are not counted as passing.
The component finality matrix is the only newly executed finality evidence.

`git diff --check` and local Markdown target resolution pass. No shell scripts
or shell examples were changed in operator instructions; the reproduction
command above was issued as shown. Full release/Kurtosis matrices, live finality,
actual blob database pruning and full runtime queue ownership fault routing
remain unverified. CI owns the full matrices; the selected process regression
needs sufficient isolated storage before the PR is marked ready.

Logs and preserved baseline executables are **local only**, under
`/tmp/issue144-evidence`; they have no guaranteed retention outside this machine.
The regression inputs and sanitized result summaries are checked in here.

## Ponytail review

What this change does: preserve observation retry ownership until native blobs
are authenticated, bound stalled HTTP reads, and commit hydrated scan ranges.
The review follows [Ponytail's review instructions](https://github.com/DietrichGebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md),
reading the affected reader/watcher, Deriver completion/finality gates, Composer
ownership reconciliation, and offline signer carrier admission.

The new process test initially used structured-signal counting for follower
messages that do not emit `event_name`. Source review corrected it to observe
the actual messages and to require the post-recovery readiness message too;
no runtime logging or shared harness change was needed. The review also removed
an unnecessary mocked header read from the new component fixture.

This is a self-review, not independent security validation. No remaining
confirmed code finding was identified. Verdict: retain the PR as draft until
the selected process test executes successfully. Not checked: live operation,
full release/Kurtosis matrices, and the quota-blocked process path above.

## Prior evidence and boundaries

[#83](https://github.com/eez-association/eez-rollupX/issues/83) established
checkpoint-aware recovery from pruned deployment receipts;
[#87](https://github.com/eez-association/eez-rollupX/issues/87) added canonical
raw-header recovery and foreign-batch exclusion. Neither restores missing blob
bodies or proves this finality/error matrix. Their historical reported passes
are not counted as current passes here.

No live services, wallets, production/Kurtosis resources or main CI run
`43237662996` were touched. No protocol/ABI, dependency, configuration, storage
or queue policy changed. No shared harness/startup-retry script was changed;
#142's independent work remains separate. No performance claim, actual pruning
claim or deployment claim is made.
