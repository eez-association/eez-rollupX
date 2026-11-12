# Missing native blob observations (#144)

This qualifies the existing native-blob observation contract and fixes three
reproduced acquisition defects. It does not establish parity with the source's
finalized missing-blob policy or change the accepted network profile.

| Field | Value |
| --- | --- |
| Date/status | 2026-11-12 UTC; component and focused process validation pass; full CI/live validation remains outstanding |
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

At initial draft creation, the new process source passed workspace Clippy/type
checking but remained **unexecuted**. It is designed to start fresh followers behind a task-local
beacon proxy, observe repeated missing/malformed-data boot failures with no
Deriver readiness or transfer receipt, restore the authentic source, and compare
safe block hashes and exact receipts with the producing node. This planned
independent replay and its semantic assertions are not counted as passing.
The component finality matrix is the only newly executed finality evidence.
The resumed execution below supersedes the original process-execution limit.

`git diff --check` and local Markdown target resolution pass. No shell scripts
or shell examples were changed in operator instructions; the reproduction
command above was issued as shown. Full release/Kurtosis matrices, live finality,
actual blob database pruning and full runtime queue ownership fault routing
remain unverified. CI owns the full matrices. The storage blocker and new test
observation defects were resolved in the resumed validation below.

Logs and preserved baseline executables are **local only**, under
`/tmp/issue144-evidence`; they have no guaranteed retention outside this machine.
The regression inputs and sanitized result summaries are checked in here.

## Resumed process validation after storage recovery

The operator freed space on the worktree filesystem. The task-owned target was
moved to ignored `artifacts/issue144-target`, with `/tmp/issue144-target` retained
as a symlink so existing cache paths stayed usable. `TMPDIR`, process logs and
fixture datadirs now use task-owned directories under `artifacts/`; no other
task's cache, services or configuration changed.

The same exact test selection was resumed with the previous Cargo settings,
prebuilt overrides cleared, `TMPDIR="$PWD/artifacts/issue144-tmp"`,
`EEZ_TEST_LOG_DIR="$PWD/artifacts/issue144-validation/process-logs"`, and
`EEZ_TEST_DATADIR_DIR="$PWD/artifacts/issue144-validation/process-datadirs"`.
The first runtime execution failed: **0 passed, 1 failed, 211 filtered**, none
ignored. The preceding quota failure had executed no test.

This failure was in the new test's post-recovery readiness assertion, not in
fixture startup or the production blob path. The missing-data follower logged
two typed source-incomplete boot observations, stayed alive without the transfer
receipt, and recovered matching safe block hashes and
an exact transfer receipt after the authentic source returned. The test then
incorrectly assumed the `spawning eez deriver` message was already observable.
The observed message count was zero rather than one. The malformed-data case
was not reached.

Source inspection established that safe blocks can be published during catch-up
before boot completes. A second execution added an explicit bounded readiness
wait but still failed (**0 passed, 1 failed, 211 filtered**): the default fixture
filter suppresses the follower's informational startup message. The follower
continued polling normally while the test waited for an unobservable message.
This exposed a second test-observation defect, not a production recovery error.

The final test explicitly enables `eez_follower=info` in its own follower
environment and waits for readiness after restoring data, checking that the
process remains alive. Its positive control observes that same marker, making
the pre-recovery zero-marker assertion meaningful. It retains the pre-recovery
missing-receipt assertion and post-recovery exact receipt/safe-hash comparisons.
No runtime or shared startup/retry code changed; these corrections are separate
from #142.

The first runtime sources were `a1a83108f3b2463b34bd256e40c3f21e6ec2baa6`,
protocol unchanged. Executable SHA-256 values:

| Executable | SHA-256 |
| --- | --- |
| Composer | `0618aa08839ce955811732af14ed1de3950cbb7bdd676001aae412ac44d6eadd` |
| Follower | `fe2eed5090cb3555935aaf63248c62a698a8ec89148ad7161dbbf290e343d27f` |
| Signer | `8c53f25bb8efa8a68a6afb5d3097130e4ccde218f61c0178747cba3c2a84cc9e` |
| Initial process test | `f1633a43c53c99693df68c909c0e85a4d7208588e2147ac893b018dcaea5e9e1` |

The first runtime output is local-only at
`artifacts/issue144-validation/process-resumed.log`; logs and failing datadirs
are retained alongside it. These ignored artifacts are unavailable in another
checkout and have no external retention guarantee. The corrected run uses
separate `process-corrected.log`, `process-corrected-logs` and
`process-corrected-datadirs` paths, preserving that first failure.

The final self-contained test passes: **1 passed, 0 failed, 0 ignored,
211 filtered**. The missing-data follower logged exactly two missing-blob boot
failures and one subsequent readiness message. The malformed-data follower
logged exactly two malformed-beacon boot failures and one subsequent readiness
message. Both checks asserted no readiness or transfer receipt during the fault,
then observed readiness in the same process and independently reproduced the
producer's safe block hashes and exact transfer receipt after restoring the
authentic beacon source. Semantic assertions and independent replay executed.
This is recovery coverage for fresh followers, not full queue fault routing or
live consensus finality; the component matrix remains the finality evidence.

The passing test includes both the explicit readiness wait and local follower
log filter, without an external `EEZ_TEST_LOG` override. It is a corrected test
version, not an unchanged-test retry-only pass. A separate logging-override
diagnostic was stopped during redundant runtime compilation before fault
checks; no result is claimed for it. All earlier failures remain recorded.

Production runtime Rust sources are unchanged from `a1a83108`; the final
Composer, follower and signer SHA-256 values match the table above. The final
process test SHA-256 is
`e699599ac248806c34239a72595735076a27e855fc86f30621d780da4d0c619e`.
The final output is local-only at
`artifacts/issue144-validation/process-final.log`, with runtime logs under
`process-final-logs` and identities in `final-executable-sha256.txt`. The source
changes accompanying this follow-up are confined to the test's observation
setup/synchronization and this evidence record. The original frozen baselines,
policy criterion and input-provenance limitation remain unchanged.

After the final test corrections, the complete workspace Clippy command above
passes again with `-D warnings`. Rust formatting, diff whitespace, local
Markdown target resolution and Bash syntax checks also pass. The existing 211
passing L1 tests still cover the unchanged production Rust implementation;
they were not rebuilt or rerun solely for this test/documentation follow-up.

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

Process execution subsequently exposed the unsynchronized readiness assertion
and the fixture's suppressed follower INFO messages. The final test waits for
an observable positive readiness control under its own explicit follower filter;
the production and shared harness code remain unchanged.

This is a self-review, not independent security validation. No remaining
confirmed code finding was identified. Verdict: Ship. Not checked: live operation,
full release/Kurtosis matrices, actual blob database pruning, and full runtime
queue ownership fault routing.

## Prior evidence and boundaries

[#83](https://github.com/eez-association/eez-rollupX/issues/83) established
checkpoint-aware recovery from pruned deployment receipts;
[#87](https://github.com/eez-association/eez-rollupX/issues/87) added canonical
raw-header recovery and foreign-batch exclusion. Neither restores missing blob
bodies or proves this finality/error matrix. Their historical reported passes
are not counted as current passes here.

No live services, wallets, production/Kurtosis resources or main CI run
`43237662996` were touched. No protocol/ABI, dependency, runtime configuration, storage
or queue policy changed. No shared harness/startup-retry script was changed;
#142's independent work remains separate. No performance claim, actual pruning
claim or deployment claim is made.

## Secret-scan CI follow-up

PR head `8695d94ac64de36bd33bfc1ab33e7a1d0649559e` failed
[Lint job 131397952350](https://github.com/eez-association/eez-rollupX/actions/runs/43279864064/job/131397952350)
at Gitleaks, before the remaining lint steps. The log reports three findings
over 1,132 scanned commits but omits individual locations. The original log and
redacted local reports are retained under task-owned
`artifacts/issue144-validation/secret-scan*` paths, with local-only availability.

The exact CI scanner, Gitleaks **8.30.1**, was downloaded into task artifacts
and verified against the workflow's archive SHA-256
`551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb`.
A separate full-history clone fetched all remote branches and tags plus the
exact failed merge ref `8bc27db8488f751b11cfb932a51be7c1fb5d1001`.
Its 111 remote branch names match those logged by CI. The frozen ref manifest
(`secret-scan/refs-before.txt`) has SHA-256
`1f8e9c1c547ada381be261b9080ac1f948216d0b95a9bf5c817b23f13ff0a5e1`.
Remote tips can advance: this is not a claim that their historical SHA values
equal the failed job's complete ref set.

With the original configuration and CI's `--all -m`, `--redact=100`,
`--ignore-gitleaks-allow`, and `/dev/null` ignore-file settings, the frozen clone
reproduced exactly three `generic-api-key` findings in historical commit
`d9b974c1c3d1081afbbfde05bfad4fc352aedea0`'s
[workload record](https://github.com/eez-association/eez-rollupX/blob/d9b974c1c3d1081afbbfde05bfad4fc352aedea0/docs/audits/2026-11-12-streaming-deadlines/workloads.json).
That commit is not an ancestor of the failed #164 merge. The original-config
scan of #164's own `601ebfa9..8695d94a` range was clean (two scanned commits).

| Finding line | Exact value matches the public identity fixture |
| --- | --- |
| 37 | `native-offline-bidirectional/identity.json` (`b1bfe680…`) |
| 159 | `native-osaka-lookup/identity.json` (`808c11d3…`) |
| 311 | `native-osaka-changing-static/identity.json` (`981d974d…`) |

Each historical `vkey` was compared byte-for-byte with its checked-in fixture.
These are public Keccak commitments defined by
[`NetworkProfile::verification_key`](../../../crates/eez-protocol/src/l1_history/context.rs)
and the [validation identity specification](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md),
not signing credentials. All three values already have exact-value exceptions.

Only the scanner configuration and regression changes from
[PR #163 at e6f70e20](https://github.com/eez-association/eez-rollupX/commit/e6f70e20776529fcc8971cf746d5ed6403ddc6a5)
were reused. The mixed commit was not cherry-picked. Its ingress, startup-retry,
Kurtosis and other changes were omitted. The fix adds only the anchored
`docs/audits/2026-11-12-streaming-deadlines/workloads.json` path to the three
existing digest rules. Their exact value regexes, `condition = "AND"`, default
detection, history coverage and inline-suppression protections are unchanged.

The new scanner regressions fail against the original configuration (two tests,
one failed) and pass after the fix (**two passed**, none skipped). They pin the
three permitted rules and exercise each exact value, one-character mutations,
a new credential beside an allowed value, ignored inline suppressions, and the
same allowed value outside its reviewed paths. On the identical frozen refs,
the all-ref scan changes from **three findings to zero** over **1,136 scanned
commits / 739,323,526 bytes**. Its count differs from CI's 1,132 and is reported
as local coverage, not a substituted CI pass.

Additional negative controls cover a suffixed filename and another file in the
same audit directory. Their first fixture version failed three subcases because
Git recognized the second move of identical content as a 100% rename, with no
added bytes to scan. The corrected fixture commits deletion separately so each
negative control presents an actual addition. This verifies path-rule scope;
it makes no new claim about detection of pure-rename bodies under CI's unchanged
Git diff policy. The first failure is retained in `regressions-final.log`;
the final focused scanner run passes both tests, including all nearby-path
controls, in `regressions-nearby-corrected.log`.

The complete tooling suite passes **542 tests, seven skipped** (549 selected),
using checksum-verified task-local Compose **5.5.1**, pinned Gitleaks, the
initialized protocol, `jq` and PyYAML. Skips are explicit opt-in infrastructure:
one data-only Docker image, two real Kurtosis resource probes, one real tiny
Rust artifact relocation, and three Docker runtime image probes. The suite
loaded before the final nearby-path assertions; the focused run above establishes
those final test-only additions. Python compilation, diff whitespace and edited
documentation checks pass. No Rust/native rebuild or live exercise was repeated
for these scanner configuration/test changes.

Ponytail self-review traced the actual all-ref workflow and Gitleaks-backed
regressions. The exact digest regexes and anchored new path were retained;
no blanket directory, commit or rule exemption was added. Fresh CI status is
checked after pushing and recorded in the PR, independently of the local results.

The already completed Rust, process and protocol-pin evidence above remains
applicable; this follow-up changes no runtime source or protocol state.
