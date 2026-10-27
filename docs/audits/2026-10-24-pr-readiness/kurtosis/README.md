# Validation of the native-v1 profile on a fresh Kurtosis network

Status: the verified CI candidate from run `40104687331` remains installed in the preserved owned node and signer containers. Its image/archive comparisons, nine genesis/profile checks, RPC probe, ordinary settlement and cold replay passed. All 34 original protocol executions now have independent evidence: 33 passed initially; `revertCounter` passed a read-only continuation after correcting the standalone prefix helper. The initial full campaign remains recorded as FAIL. The final follower checked 206 complete batches, 3,558 selected block heights and 814 receipts at endpoint 9816. Earlier R03 campaigns retain their original identities and results.

| Field | Value |
| --- | --- |
| Authorization | User requested a background Kurtosis network and extensive protocol, cross-chain and fee validation. |
| Worktree | Dedicated `codex/l1-history-offline-proving` worktree. |
| Source baseline | `3e23955dc477c9e675364776f118b1cecda75774` plus the reviewed uncommitted native-v1/offline patch; source manifest SHA-256 `857921af067dc927f262da0ca41bd80fb635c0dcda9bd97860e36f6820996b6a`. |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, clean pinned checkout. |
| Owned enclave | `eez-native-v1-review-20261024`; name unused at preflight. |
| Environment | Linux, Docker, Kurtosis 1.20.0, Foundry 1.7.1; private fixture chain IDs 7331/696990. |
| Resource boundaries | Preserve existing enclaves, containers, wallets and databases. No shared cache pruning. New network uses only public fixture funds. |
| Raw evidence | `artifacts/pr-readiness/kurtosis/` in the worktree, local only; sanitized results and hashes will be retained here. |

## Validation plan

1. Build node, signer, genesis tools and semantic verifiers from the exact candidate, recording source/configuration and executable/image hashes. Warm the pinned protocol and integration contracts before live PBS testing.
2. Start a separately named fee-enabled network. Verify live EEZL2/proxy/genesis/manager bindings and actual canonical settlement with the independent signer. Optional UI and explorers are omitted to preserve resources; demo bridge contracts remain enabled for receipt-cost calibration.
3. Run the complete default eligible pinned protocol suite, retaining exclusions as exclusions. Check both directions, grouped/nested/static/reverted behavior, canonical receipts, exact execution tables, native blob KZG evidence and block-hash commitments.
4. Run the repository cross-chain waves and state-chaining assertions, including mixed ordinary traffic and exact destination state. Do not substitute successful receipts for semantics.
5. Run real L1 builder-refund and L2 quote/receipt/fee/balance exercises, underpayment rejection and bridge fee calibration. Independently inspect native base-fee credits to FeeVault and recipient rotation/withdrawal with the fixture owner.
6. Check history/PREVRANDAO agreement and continuous ordinary L2 production when L1 observation or proving is delayed. Restrict fault injection to services owned by this enclave and preserve first failure diagnostics.
7. Independently replay settled ranges with a fresh follower from the deployed genesis; compare canonical hashes, roots and receipts. Exercise partial consumption only with its required pinned builder behavior, recording any topology mismatch explicitly.

## Preflight observations

The machine contains active public-network services and several existing Kurtosis enclave records. They were inspected without stopping or changing them. `kurtosis enclave ls` automatically started the missing global engine; that CLI side effect did not restart those services. The selected new enclave name was unused.

Approximately 23 GiB memory and 2.1 TiB disk were available, with swap full. Heavy Cargo and Docker builds are coordinated with the parent task and bounded; no competing cold build should run while live slot-sensitive checks execute. The sandbox failed before command execution because creation of `/tmp/.git` exceeded quota; approved commands use narrow escalated execution and task-owned temporary directories.

## Preparation findings

The partial-consumption canary waited for Composer invocation before enqueueing
its first users. An idle cutoff now intentionally skips that invocation, so the
barrier could wait indefinitely. The canary now waits for a fresh sequencer
`slot.decision` with `prepare=false` and `skip_post=true`; this fixes the current
slot as ordinary and opens collection for the following cutoff. Active, uncertain,
incomplete and malformed decisions do not open the barrier. The focused regression
failed before the change and passes afterwards (7 tests). Its proof-dispatch,
nonce-replacement, partial-commitment and reconciliation assertions are unchanged.
The before/after logs are retained under the local evidence directory.

The live profile fixes the signer's request quota at 24 blocks to exercise catch-up
in several accepted parts without a literal multi-day run. Planned evidence must
show at least two accepted parts over the outage backlog, the bound per part,
continuous canonical endpoint hashes/state roots, and recovery of the safe head.
The quota will not be raised to hide failures.

## Build provenance

The first local node-image build was intentionally cancelled after 2,529 seconds:
its separate Composer/follower Cargo invocations selected different dependency
feature unions, and the regenerated history-contract metadata made that snapshot
obsolete before deployment. This was a cancelled build, not a compiler/test failure;
its log and cancellation record remain in the local evidence directory.

The baseline audit Dockerfile uses the exact packages, binaries and examples selected
by `Dockerfile.runtime` in one `cargo build --locked --timings --profile release`
invocation, with four jobs and no test targets or `--all-features`. It reuses existing
Docker caches without pruning them. Cargo completed in 989.7 seconds (1,008 seconds
including image creation). A second invocation incorporated the final Cargo comment
and recaptured test fixtures, taking 1.69 seconds with no crate compilation. Runtime
images package these seven frozen binaries; packaging does not invoke Cargo. All seven
byte hashes matched across the final source recheck. Executables extracted from
the node, signer and deploy images match that bundle. The retained
[release identities](release-identities.json) list hashes and image IDs.

The history NatSpec correction changed Solidity metadata and generated genesis
commitments; executable history instructions remained identical, as recorded by
`artifacts/pr-readiness/nonce-doc-regeneration.json`. The validated deployment uses
the regenerated runtime, not the earlier metadata snapshot. The source-process
`node_e2e-final` executable is a separate debug test artifact; deployed node, signer,
helpers and independent follower are from the one release build above.

## First live failure and packaging correction

The first enclave startup failed before deployment broadcast: `--render` validates
`crates/eez-protocol/src/l1_history/runtime.bin`, but the deploy image did not copy
that generated file. The failed task had no broadcast directory or deployment
bindings, and the fixture deployer's canonical L1 nonce remained zero. Its log, image and pre-broadcast identity evidence were retained. The minimal correction copies the generated
binary to its consumed path; runtime bytes and contracts are unchanged.

The packaging regression failed specifically on the absent runtime before the
COPY and the complete deployment-preflight suite passed afterwards (14 tests).
The first test-draft parser error and a dry-run failure caused by the host `/tmp`
quota are also retained; the corrected test and owned temporary mount resolved
those harness/environment issues. The corrected image's runtime bytes and both
genesis helpers were extracted and compared against the final source/release
bundle. All seven Rust executables retain their prior hashes.

The corrected deployment context manifest SHA-256 is `c13d4019bfd12e5d75c719c0183a258509f9f4537c7a7459af7441d0730b1a7c`; it is distinct
from the preceding Rust build source manifest. Live startup is being retried on
this same owned enclave after a successful dry-run. Kurtosis recreated only its
owned services with a new L1 genesis; no user transactions existed before the retry,
and the two genesis identities are kept separate in the logs.

## Effective deploy context and public genesis permissions

Independent review found that the production deploy Dockerfile's specific ignore
file excluded `crates/`, even after adding the COPY. The initial audit image used
its own filter and correctly contained the bytes, but did not validate this
production context. A build with the actual Dockerfile and ignore reproduced
`CopyIgnoredFile`; the minimal exception admits only the generated runtime. The
corrected production build succeeds. A real BuildKit regression exports identical
runtime bytes using that exact ignore (1 selected test passed); the existing full
runtime Docker fixture now supplies and checks the same asset. The complete heavy
runtime-artifact fixture was not rerun in this live lane.

The next startup successfully broadcast the six deployment transactions but the
non-root signer could not read `l2-genesis.json`: the timestamped temporary base
had mode 0600, which the renderer preserved. `deploy.sh` now preserves the original
base's permissions on that temporary file. Tests reproduce the old failure and
verify both the consumed base and rendered output for 0644, 0664, 0640 and 0600;
all 15 deployment-preflight tests pass. Explicitly restrictive base permissions
remain restrictive. Neither signer privileges nor validation were weakened.

Before recreating the owned disposable network, attempt 2's 48 evidence files were
archived: deployment bindings/broadcasts, both genesis files, service/image IDs,
logs and SHA-256 manifest. This is an evidence archive, not a complete datadir
snapshot: the L1 datadir was not retained, so the full failed-attempt chain cannot
be independently reconstructed from it. No L2 datadir had been created. Exported
file modes were measured (genesis 0600, bindings/profile 0644); original live file
ownership was not measured, and local exported ownership is not a substitute.
Its L1 genesis was
`0x066c54e0d84aadf31cab4c2cc58249ee199eb1529c18d16d70064a6b91f0c6fb`
and the deployer nonce was 6. No contracts or chain state from that attempt are
reused in the subsequent fresh chain. The first two attempts are failures fixed
in the candidate, not successful coverage or unexplained flaky retries.

The final deploy image uses the **production Dockerfile and its specific ignore**
against a clean copy of the exact source, omitting generated local logs/databases.
The source manifest is
`aae52f8506680ee89c6c977ebe88a5470b30564734a9e27f4ab63afee949d0be`.
Extracted runtime, genesis helpers and deployment script match their intended
source/release bytes. All seven Rust binary hashes remain unchanged. The initial rerun reused Kurtosis's cached deployment instruction because the
image tag was unchanged; that interrupted attempt is retained as `start-r2.log`.
After verifying the exact owned enclave UUID and name, it was removed and a new
enclave was created from scratch with the corrected image. No unrelated enclave
was stopped or changed. This fresh startup completed successfully.

## Fresh network and pending-RPC finding

The final enclave UUID is `78c4b5eee3a844b99f4864b33c0d0643`. Its L1 genesis is
`0x087cc8bdd45d6502bc054cec60480797d5f998554e9504f76d75a3b83fe96dc8`
and its L2 genesis is
`0xdd2a6c2ad7f382305c6a0e13e38e9dc4d952bd3a3bc78daa672859354a0ee41f`.
Live Composer/signer binary hashes match the release bundle. The signer runs as
UID/GID 65532 and reads the generated genesis with mode 0664. Inspecting its
effective process environment found no RPC URL variables. This establishes the
deployed configuration, not network isolation; source-process tests separately
deny outbound connections with seccomp while retaining inbound gRPC.

The `prove.v2` capabilities check passed with profile ID 2 and the configured
24-block request quota. Deployment checks passed for the actual EEZL2 proxy,
implementation, ownership, system address and manager bindings. At captured L1
block 65 (`0x6d0d16ae82f329be0bca39ff0ac76d2493c478637a5e5dd2346e2de6d39bb345`),
the Registry commitment was
`0x78ce57fc9f88db936efd1fa1a7eb11da35102e69efb3b8c721ba4ac87f80ebba`:
exactly the canonical L2 block 378 and the observed safe head. The L1 call was
pinned to that block hash, rather than comparing only heights.

The first blob smoke attempt waited for live PBS registrations, then failed
before broadcasting its Counter deployment. `forge create` received
`L1 derivation source unavailable: pending L1 history import has not executed`
from gas estimation. The original log is retained as
`live/phases/blob-baseline.log` (SHA-256
`3f9a9ef561ea9a0fe76aa95a7a89de9f6f0a56e049d32fffaa6a7ed40f7ef8e9`).
This is a failed live test, not blob/contract coverage.

A read-only control sampled the identical Counter initcode 40 times, recording
canonical header hashes before and after each group. Default and explicit
`latest` estimates returned 106328 gas throughout. Explicit `pending` rejected
four samples at stable L2 head 374 with the same error; it succeeded outside that
boundary. The pending environment deliberately rejects a synthetic next block
whose mandatory history import has not executed. Removing that guard would mix
future block context with old state. Compatibility with ordinary tools that
explicitly estimate at `pending` is unresolved; an explicit `latest` control can
exercise unrelated execution paths but cannot establish that compatibility.
Raw evidence is in `live/pending-counter-estimate-regression.json` and
`live/canonical-ready-baseline.json`, under the local-only artifact directory.

## Baseline live semantic results

The unchanged `verify-l2-fees.mjs` passed in 72 seconds. An underpaid source was
rejected before queue ownership, nonce consumption or target mutation. Two nested
L2-to-L1 sources executed exactly once and set their independent targets to 123
and 124. Both were canonically consumed in L2 block 438 / L1 block 74. Each used
116406 gas; pooled L2 tips of 1185136936216812 wei exceeded the receipt-backed
post/blob cost plus configured 20% profit, 1020548161914015 wei.

The unchanged `verify-l1-fees.mjs` passed in 261 seconds. It checked legacy and
EIP-1559 sources, nested L1-to-L2 execution, Safe 1.4.1 creation/owner/threshold,
underpayment rejection, actual builder payments and recipient balance changes.
Two four-source waves each shared one native blob post and refunded the largest
measured payer according to the configured 90% rule. These multi-source waves
**did not recover the full posting cost**: shortfalls were 657867378918387 and
107394271866149 wei. The first shortfall is explicitly required by the test.
This validates exact distribution and continued subsidized service under the
[accepted batching policy](../../../proposals/cross-chain-fee-recovery.md#l1-batching-and-observation-only-recovery-authorized-on-2026-10-16),
not profitability of every batch. The final assertion establishes quote
availability after a shortfall; it does not assert automatic price adjustment.

A separate Counter/blob **control** passed in 41 seconds using explicit `latest`
gas estimates for deployment and both calls. All three estimates are recorded;
no fixed gas workaround or retry was used. The original state, canonical receipt,
Registry and bidirectional correlation assertions remain intact. One authenticated
blob batch contained the exercised transactions and 24 L2 blocks, ending at safe
block 612 with Counter equal to 42. KZG/DA/commitments were verified independently;
the blob verifier reports `execution_verified=false`, so independent follower
execution is still required. Blobscan was intentionally omitted with the optional
explorer services. The control does not resolve the original Forge/pending failure.

The unchanged bridge fee suite passed in 76 seconds. It first funded actual L1
escrow through an inbound transfer, then verified first-use EIP-1559 and repeated
legacy withdrawals. Quoted and observed L1 gas matched exactly (510938 and
201060), each paid the measured posting cost plus 20%, and principal/gas changes
in user balances matched canonical receipts.

The history/FeeVault control passed in 87 seconds with explicit `latest`
estimates. Three application transactions observed the selected L1 hash and
matching PREVRANDAO. All 122 stored L1 hashes, from 0 through 121, matched L1.
Across 31 inspected L2 blocks, five scheduled imports occupied transaction zero
with zero effective gas price and no system-account nonce increment; configured
delay/interval were 6/12 seconds. FeeVault's balance increase matched exactly
438780615 wei of ordinary base fees. An unauthorized recipient change reverted;
the owner rotated the recipient, any-user withdrawal paid 1605614286866719 wei,
and its 25477475-wei remainder matched the withdrawal's own post-execution fee
credit. The original recipient was restored and all observations/receipts were
rechecked against canonical safe block 744.

The first delayed-observation fault attempt ended after eight seconds because
the audit harness requested a 1-gwei priority fee while `cast` chose a lower
automatic max fee. No transfer was accepted; the `finally` handler restored
the owned Beacon observer, independently checked running and unpaused. The
explicit diagnostic repetition derives `maxFeePerGas = 2 * baseFee + tip`, as
the existing fee suites do. The initial failed log/JSON and before/after harness
hashes are retained; this first attempt is not continuity or recovery coverage.

The next full 72-second observation window produced 36 L2 blocks while canonical
L1 advanced from 135 to 141 and the embedded L1/history origin remained 134.
It nevertheless failed a harness assertion requiring a strictly higher head at
every six-second sample. Prepared Sync blocks can already have timestamps in the
future: observed heads 816, 822 and 828 covered the following sampling interval
without increasing immediately. Maximum sampled lag was below 1.54 seconds.
The service was restored before the assertion fired. This failed invocation is
retained as `delayed-l1-observation-r1`, rather than relabeled as a pass.

A further explicit repetition samples every second and derives a four-second
lag limit from twice the configured two-second L2 interval (one interval for
block phase, one for bounded observation/scheduling delay). It also limits gaps
between samples, checks contiguous timestamps and actual transfer inclusion,
and snapshots the unsafe branch before restoration so recovery cannot race its
comparison. This exercises temporal progress; timestamp continuity alone would
not establish liveness.

## Controlled interruption results

All three corrected fault exercises passed on the identified baseline binaries.
Each used only this enclave's named container, restored it with `finally`, sent
three ordinary transfers during the pause and checked their later canonical
receipts/balance delta. Every observed unsafe block was retained through recovery;
no unsafe replacement occurred in these windows. This is evidence for these
controlled conditions, not a performance comparison or arbitrary-failure guarantee.

| Fault | Observed L2 progress in about 72 seconds | Maximum sampled wall lag / sample gap | Recovery |
| --- | --- | --- | --- |
| Pause embedded-L1 Beacon observer | 35 blocks; canonical L1 advanced while embedded L1 and selected origin stayed 151; six explicit repeated imports | 1.842 s / 1.021 s | Safe 954; actual outage production 907–941 covered by two accepted 24-block parts |
| Pause validator client, causing missed L1 slots | 37 blocks; canonical L1 number/hash stayed exactly 168, with unchanged selected origin and PREVRANDAO; six repeated imports | 2.003 s / 1.021 s | Safe 1068; backlog covered by parts of 24, 24 and 18 blocks |
| Pause proof signer | 36 blocks; L1 and selected origins continued advancing while safe stayed 1092 | 1.350 s / 1.023 s | Safe 1164; backlog covered by three 24-block parts |

The largest observed gaps between head changes were 3.03, 7.03 and 6.03 seconds;
prepared heads were ahead of the wall clock by up to 0.52, 4.96 and 3.91 seconds,
respectively. The one-second samples check the actual temporal coverage of those
heads, including the later part of each interval. The tests also require
consecutive two-second block timestamps and a parent-linked captured branch.

Each recovery collector verified actual type-3 receipts, canonical L1 inclusion,
Beacon blobs/KZG, the 24-block bound, exact pre/post block hashes and state-root
continuity, and the Registry commitment after each posting block. Pure-L2 parts
already in flight are distinguished from those covering new outage production.
These DA/commitment checks still require the separately planned independent
follower execution replay.

The delayed-observation recovery exercised both anchor modes in one continuous
sequence. Actual `getCustomData` STATICCALL return data was checked from successful
Registry transaction traces: inclusion-parent mode returned 416 bytes including
the exact inclusion timestamp; historical mode returned 384 bytes with no
inclusion timestamp. Domain, chain IDs, manager/Registry, profile/rule identities,
accepted pre-hash and canonical anchor hash all matched. The historical anchor
was block 158 in inclusion block 159; this does not claim a live test of a source
older than 256 blocks. See [the portable fault evidence](live-faults-summary.json)
for hashes, parts and trace identities.

A later read-only check reached L1 block 264. A temporary `eth_call` code
override executing `BLOCKHASH` returned the canonical parent for block 263 and
zero for expired block 1. At canonical safe L2 block 1602, the history contract
still returned block 1's exact L1 hash; all 261 stored hashes from 0 through 260
matched the parent-linked L1 chain. No contract or state was changed by this
probe. It validates retained history after the L1 opcode window expires, not a
new proof request with a historical anchor at that expired height.

Small portable facts and artifact hashes are retained in
[the baseline summary](live-baseline-summary.json). Complete quotes, receipts,
logs and blob data remain under the local-only evidence directory. These results
identify the baseline release bytes; an in-progress pending-RPC correction will
have a separate runtime identity and follow-up validation record.

## Pending-RPC shipping rebuild resource incident

The first rebuild of the frozen RPC correction started with two Cargo jobs while
the focused source-process build also used two. Available memory fell to about
3.6 GiB with swap full. The kernel subsequently confirmed a global host OOM at
11:35:28 UTC, killing the concurrent host `eez-node` compiler. The audit Docker
frontend was explicitly cancelled with SIGINT and returned 130; that cancelled
log is retained separately from the host OOM failure. No candidate image was
applied and no live service was stopped. The next shipping attempt must wait for
the host build to release resources, use one job, require at least 12 GiB
available to start and cancel only itself if availability falls below 5 GiB.

## Results

Capabilities, deployment identity, canonical settlement, the fee and bridge
suites, history/FeeVault, the explicit-latest blob control and three controlled
interruptions/recoveries have passed. The first eligible pinned protocol suite attempt failed during funding: its
bridge prerequisite passed with canonical table, causal graph and blob-semantic
checks, then L2 faucet gas estimation hit the same pending-history RPC error.
The remaining 33 scenarios never started. The runner reports them as missing
results; they are unexecuted, not 33 semantic assertion failures. The exact log
SHA-256 is `d474902cd6c529b292d9b49851c1f0cd5a54604d1647798fa3c1df60ef4c25c3`.
The 30 scenarios excluded by the pinned default suite remain explicit exclusions;
[their per-scenario reasons and profile analysis](protocol-exclusions.md) identify
26 patterns outside explicit profile restrictions, two staged-runner negative
tests and two L1-origin live-coverage gaps. This classification is static analysis,
not live rejection coverage.
Broader cross-chain waves, partial consumption and independent replay remain pending. The initial blob smoke
failed as recorded above. No overall pass or PR-readiness claim is made by this
record.

## R03 node maintenance and preserved state

The owned node was stopped deliberately at 2026-10-24T11:54:59.949020+00:00 after recording L1 block 373 and canonical L2 safe/head 2274. Shutdown completed in 0.505 seconds with exit code zero and no container OOM. Its own Composer PID 468004 had 2,429,964 KiB RSS; the earlier 6.7 GiB estimate was not an attribution to this enclave and was not used to select a process. Only the verified node container was stopped; canonical L1 and signer services continue. This deliberate maintenance is separate from the successful L1/prover fault liveness exercises.

All stopped node state is retained: L2/witness databases, the embedded L1 database, bundle captures and mounted genesis. Four TAR archives preserve file metadata and cover 9,750,156,073 logical bytes, compressed to 47,374,041 bytes; gzip CRC and TAR inventories were verified. Original binaries were copied and matched to the baseline release. [The maintenance record](node-maintenance.json) contains hashes and local-only artifact paths. An initial capture asserted the wrong Registry ABI length and aborted before any stop; the corrected capture uses the pinned three-word ABI and preserves the full identity and canonicality checks. No archive has been restored, no container was recreated, and no R03 binary has been applied yet.

The second R03 shipping attempt used one Cargo job after host Cargo had finished. Its memory guard cancelled only that build after 298.5 seconds: available memory fell from 14.31 GiB to 3.01 GiB over six seconds, below the configured 5 GiB floor. BuildKit confirmed cancellation (exit 130); this was not a Rust compilation error or a deployed candidate. The log SHA-256 is `6cb5e0dc64e643d6f98b09430e0f34fba08d12337cf2a2ce9c0b8387434d1d67`. The captured kernel interval contains no further OOM event. The failed attempt lacks a per-process sample of the peak, so it does not establish that the linker caused it.

Read-only inspection of the cached build-tool layer found GCC 12.2, mold 1.10.1, Rust 1.95.0/LLVM 22.1.2 and 22 available CPUs. The shipping flags remain `-C link-arg=-fuse-ld=mold`, thin LTO and 16 codegen units. This mold exposes thread-count flags but lacks the later `MOLD_JOBS` environment control. Source files, dependency versions, profile and optimization settings have not been changed to bypass the resource limit. The focused process regression for R03 passed after correcting its test-only probe of an internal storage slot; applying the candidate still requires a completed, verified shipping build.


## R03 applied and live RPC regression

The third shipping-selection build completed successfully in 2,482.8 seconds with one Cargo job and CPU affinity restricted to one CPU. Dependency versions, release optimization flags, thin LTO and codegen-unit settings were unchanged. Available memory stayed above 8.85 GiB; the main guard owned and waited for the Docker process throughout. The first auxiliary PID monitor ended with SIGTERM for an unestablished reason, and a separate resumed monitor preserved subsequent samples.

The exported bundle and packaged node bytes were verified. Composer, follower and both genesis helpers changed hashes; signer, causal verifier and blob verifier were byte-identical to the baseline. Nine deterministic helper comparisons over production, development, E2E and deployed genesis/profile inputs matched. An initial audit invocation paired production genesis with the private L1 profile and was correctly rejected before reaching the candidate helper; both old/new helpers subsequently rejected that same invalid pair. No genesis or contract was changed.

At 12:59:01 UTC the same stopped container was started with its four verified new executables. Its image metadata still names the original image: this is an installation of verified bytes into the preserved container, not a deployment of an immutable final image. Configuration and mounted state were preserved. Docker reassigned the dynamically requested host ports to 32869–32872, and Kurtosis immediately discovered those new endpoints. The first audit readiness probe incorrectly kept using port 32864 and timed out; no second restart was performed. Verification through the discovered L2 RPC confirmed the original genesis and safe block 2274 hash/root. Block 2275 extends it with exactly a two-second timestamp increment. L2 execution caught up to wall time while canonical settlement continued through bounded parts.

The original Counter input now passes sixty live samples: default/latest/pending estimates are all 106328, pending call returns exact runtime bytes, access-list construction succeeds, and a plain transfer estimates 21000. Ten stable samples cover five distinct parents immediately before history imports. This is the same input that failed at the baseline pending boundary, without fixed-gas workarounds or hidden retries. Complete identities and portable results are in [the RPC correction record](runtime-rpc-correction.json).

CI subsequently reported an archive dependency-feature mismatch, with a suspected Tokio `test-util` development feature in the combined shipping selection. That investigation is separate from the live semantic regression and must be resolved before declaring clean shipping-artifact compliance. The network remains on the precisely identified R03 bytes while subsequent semantic checks continue.

## R03 blob smoke, large evidence and ordinary EIP-7702

The original blob smoke now deployed Counter through Forge's normal pending
estimation, executed both calls and recovered all three transactions from
authenticated blobs. After verifying safe Counter=42 and both correlation RPC
directions, its final jq summary failed with E2BIG: the 37 retained batches
formed a 151752-byte command argument. This was a real script defect exposed
by the maintenance backlog, not a node/RPC failure. The original failed
430-second run and its empty summary remain intact.

The script now passes batch and correlation evidence through files without
truncating it. The targeted regression failed before the fix with exit 126 and
the complete existing evidence-selection suite passed afterwards (10 tests).
A separate read-only continuation used the same three transactions and all
37 original batch objects, repeated the safe-state and correlation assertions,
and wrote the complete summary at canonical safe block 4950. It broadcast
nothing. Blobscan/UI remains deliberately omitted.

An ordinary EIP-7702 live exercise passed in 54 seconds using fixture accounts:
a sponsored type 4 transaction installed a delegation and wrote 77; an ordinary
type 2 call used the persistent delegation and wrote 78; a second authorization
revoked it while preserving storage. Authority nonces advanced 0→1→2 and code
changed from empty to the exact delegation designation and back to empty.
Each receipt's actual sponsor debit and the FeeVault's block-wide base-fee
credit matched, and all three transactions were canonically safe by block 4902.
This covers ordinary delegation, not deferred cross-chain 7702 triggers. Fresh
follower replay remains a separate pending check. The small
[result record](rpc-blob-7702.json) identifies transactions and raw-evidence hashes.

## Maintenance catch-up in bounded parts

Read-only auditing completed after the original application workload was safe:
112 canonical posts covered L2 blocks 2275–4950 with no endpoint or state-root
gaps and no request exceeding the unchanged quota of 24 blocks. Each post's
transaction/receipt, Beacon blob KZG, decoded payload and Registry commitment
were verified. The old commitment at 2274 remained canonical. All 447 retained
L1 headers between the old and recovered origins matched the actual parent
chain, and the terminal PREVRANDAO matched its selected L1 origin. L1 had
advanced 451 blocks since the node was stopped; older history was authenticated
through the header chain while the actual getCustomData anchors remained within
the contract's permitted window. This does not claim acceptance of an expired
BLOCKHASH anchor.

Three hundred samples over 599 seconds observed safe progress of about 2 blocks
per second against ongoing head progress of about 0.5, draining backlog from
1159 to 259 blocks during that sampled interval. These describe this recovery,
not comparative performance. Deliberate installation downtime remains distinct
from the earlier uninterrupted L2 fault-injection tests. The
[bounded recovery record](maintenance-recovery.json) retains every accepted part
and its hash/root endpoints; fresh follower execution is still a separate check.

## Complete default protocol suite on R03

The pinned default network suite passed all 34 selected scenarios with zero
failures or skips in 672 seconds. Its first bridge prerequisite and all 33
remaining scenarios ran against the real signer and both chain fronts. Strict
result accounting, evidence completeness, causal coverage and authenticated
semantic blobs all passed. The independent receipt/table/blob verification
phase took 27.384 seconds; it can reject an upstream PASS. This exercises
inbound and outbound calls, grouped/nested calls, static calls, reentrancy,
reverts, flash loans and bridges with their pinned semantic oracles.

Three scenarios intentionally have no destination execution to pair:
revertCounter, topLevelStaticCounter and topLevelStaticReentrantCounter. The
runner records their reason and still authenticates settlement semantics.
The [scenario record](protocol-standard.json) preserves all names, claims,
transaction identities and evidence hashes. The 30 default exclusions remain
explicit in the [coverage inventory](protocol-exclusions.md); separately
selected additional cases will be recorded below. Cold follower replay is
pending and is not implied by this suite's successful receipt/table checks.

## Explicit additional protocol cases: one pass and three unresolved oracles

The explicit L1 staticLocalWrite/staticThenNestedWrites run completed both
upstream scenarios successfully. The independent runner passed
staticThenNestedWrites and rejected staticLocalWrite: that scenario needs a
lookup-only oracle, but the collector selected the mutable-table path and
required a marker it does not emit. The result remains one PASS/one FAIL,
not two passes. The retained trace shows three real results 1→2→1 and callback
reads 1, 2, 2, 1 as the second lookup rejects the first candidate. Its posted static
rows and semantic blob are retained; proving their full relationship requires
the specific independent oracle and regressions.

The two MissingProxy scenarios used the original staged negative runner with
verification enabled. Funding and deployment passed, but fee quotation rejected
both before any trigger was sent: RPC -32004, quote_unavailable / gas estimation.
This does not prove the intended terminal SimulationFailed ownership state,
which the runner never reached. No quote bypass or retry was used. These are
two failed send attempts with a concrete fee-enabled topology limitation.
The [additional results](protocol-extra-results.json) preserve all outcomes;
the 26 remaining profile exclusions were not run.

The corrected staticLocalWrite oracle subsequently passed against the same
original trigger and post, using read-only evidence collection. It authenticates
exact static rows A/B/A, the six scenario blob actions, candidate lookup order,
canonical root and absence of L2 delivery through block 5970. Its 30 focused
positive/negative regressions passed without skips. This continuation does not
turn the original suite invocation into a pass or claim that all blob triggers
are byte-bound: that broader flag remains false. The detailed
[oracle review](../static-local-write-oracle.md) records its scope and hashes.


## Workload configuration reader regression

The first broad waves run stopped before admission of its first cross-chain
trigger. Its generated YAML correctly represented enabled fees as `'true'`,
but the shared workload reader only removed double quotes. It therefore chose
the unquoted-fee branch, and the live admission guard correctly rejected the
insufficient payment. No trigger was accepted and later modes were not run.

The reader now handles paired single or double quotes for its simple boolean
and integer consumers and preserves malformed values. Comment removal requires
YAML whitespace separation, so `true#suffix` cannot become an enabled setting.
The new regressions failed against the original reader; the complete existing
quote/signing suite then passed all 13 tests, including both directions, absent
values, invalid quotes and unchanged quote/retry behavior. Shell syntax passed.
No runtime setting, tariff or quote validation was changed. All four wave modes
subsequently passed; their separate fifth state-chaining stage failed as recorded
below. The [reader regression record](yaml-workload-regression.json) preserves
the first rejection and both test results.

## Broad waves and state-chaining barrier

All four wave modes passed: four inbound operations, four outbound, eight mixed,
and 24 across three mixed-pure rounds. The assertions checked exact destination
values/balances, expected invalid-call rejection, canonical source receipts,
L1/L2 commitment agreement and signer/node attestation correlation. Read-only
collection independently authenticated 36 complete posting batches and all
60 linked L2 receipts for those 40 cross-chain triggers. A separate read-only
check also found all six intended ordinary fillers in those authenticated
payloads, with consecutive fixture nonces, successful canonical receipts and
safe inclusion. The
[wave record](cross-chain-waves.json) preserves per-mode durations and identities.

The fifth stage failed before its first source was accepted. Its old barrier
waited for Sync production: at that point safe was 6270 and the newly produced
pure Sync was 6294. The 24 ordinary history blocks exceeded the admission
allowance of 23 plus one application block. The source received `quote_limit`
one second before that post was included, and canonical safe advanced two
seconds after rejection. This is retained as a failed live stage, not a passing
retry or a protocol execution failure.

The barrier now observes a fresh accepted idle cutoff (`prepare=false`,
`skip_post=true`), opening collection for the next cutoff. It no longer depends
on an empty Sync production policy. Its new positive test failed against the
old function; all four focused tests then passed, including active/incomplete
decision rejection and canonical-receipt handling. Same-Sync and ordered-return
assertions are unchanged. A distinct live repetition passed all nine scenarios
in 489 seconds: six directional groups, poison recovery, the fee-enabled mixed
case in FIFO Syncs 6612 then 6618, and repeated identical inbound calls in one
source transaction at Sync 6660/L1 block 1104. The original wrapper remains a
failed invocation; the later result covers the corrected fifth stage. The
[barrier record](state-chaining-regression.json) separates this evidence from the
original admission failure.

## Real partial consumption and bounded replacement bid

The first fee-enabled canary failed in 239 seconds: all three users committed,
so no partial-consumption assertion was reached. Its audit copy had raised source
priority fees from the original profile’s 1 gwei to 100 gwei but retained the
500 gwei replacement. Canonical receipts show 0.02754093 ETH of bundle tips;
the pinned builder selected an `mp-ordering` block worth
0.019473239999853 ETH after refund and transfer costs. The replacement offered
only 0.0105 ETH. Its RPC acceptance was asserted, but the first copy did not
record the exact arrival time; INFO logs alone cannot reconstruct that detail.

A second audit copy changed only the replacement bid and added public progress
timestamps. At 10,000 gwei priority, 10,001 gwei max fee and 21,000 gas, its maximum
cost is 0.210021 fixture ETH. Together with the later transaction’s maximum
0.6 ETH reserve, this remains below the funded sender’s 1 ETH balance. The
repository canary’s original partial-consumption profile and runtime fee rules
were unchanged.

That attempt passed in 105 seconds. The replacement was observed pending in the
builder 0.544 seconds after proof dispatch, before canonical inclusion. In L1
block 1183 it preceded the post, first successful user and final reverted user;
the middle source had no receipt. The exact dispositions were `Committed`,
`SourceNonceSpent`, and `SourceReverted`, with attempt counts `[0,0,0]`. L2 7134
adopted the consumed prefix, distinct from the full proposed block hash. A later
nonce-1 source from the replaced user committed, and safe height 7146 retained
the earlier prefix. [The sanitized record](partial-consumption.json) preserves
both outcomes, budgets, hashes and timing limitations.

The partial post’s original KZG-authenticated claim is retained separately; its
full proposed endpoint is never rewritten to the applied prefix. Cold replay
selects complete canonical later posts, so reaching their endpoint requires
executing the intervening partial history. Explicit canary receipt/order/state
checks remain live evidence; the existing follower harness checks selected
receipt statuses and canonical block membership, not their logs or gas.

## First cold replay and native receipt decoding gap

The diagnostic follower used the retained R03 release binary and existing
`node_e2e` harness, with a new database and canonical L1/Beacon data only. Its
input selected 183 unique complete batches, 3,384 unique block heights and 719
receipts. Assembly checked the exact 112 maintenance parts and all their
endpoint/state-root continuations; the follower harness does not enforce that
selection itself. The first input-assembly attempt was rejected before launch
because the static oracles store raw L1 transaction objects without a normalized
`chain` label. The correction requires their explicit chain ID 7331; no L2
receipt or canonical claim was dropped.

The process **failed** after 22.69 seconds. It had passed the final hash at L2
7194 and the complete loop over 183 batch payloads and 3,384 blocks; its startup
log reports replay from genesis through 7290, including the real partial
consumption. It then failed at the first selected native receipt: Alloy’s
standard Ethereum provider could not decode transaction type `0x45`. This is
a test-consumer gap, not a passing receipt suite or a timeout. The unchanged
719-receipt selection, first log and failing datadir are retained in the
[diagnostic record](cold-replay-diagnostic.json). That attempt did not validate receipt comparison or the final clean-artifact
run; native receipts were retained when correcting its consumer.

The native-receipt consumer correction then passed **1/1** in 24.30 seconds
(28 seconds including the wrapper), using the exact same input SHA-256
`a1a120fb46d0b52e61be3eb6360b38f1aafc52e817a193f10840d6d4f25c68f6`
and the same R03 follower. Only the response decoder changed to
`AnyTransactionReceipt`; block handling and all assertions stayed intact. The
output independently confirms 3,384 distinct selected block heights, 3,384
derived-block checks and all 719 receipt status/canonical-membership checks at
endpoint 7194. These counts match the assembly inventory, including all 112
maintenance parts and the three ordinary EIP-7702 operations. This validates
R03 replay. The final clean-feature CI runtime was validated separately in
the closing live smoke, eligible protocol campaign and cold replay below.


## Verified final CI runtime

The producer for `319ebc2d` passed as CI run `40104687331`; its synthetic checkout
`08e16333` has the same source tree. All seven runtime binaries were verified
against its archive and compared with R03. Every binary present in the node,
signer and deploy images matched the archive. The deploy image also carried the
expected history runtime and both genesis files. All nine genesis/profile helper
outputs were unchanged. The [portable final-runtime record](final-runtime-validation.json)
binds these checks to the payload and image digests.

Only this enclave's node and signer were stopped, both with exit zero. Complete
snapshots of `/data`, the embedded L1, bundle captures, L1 genesis and deployment
output passed compressed readback and hash checks. Five installed role/helper
binaries were replaced; both standalone verifiers initially ran from the verified CI
archive. The later prefix continuation uses the separately identified local correction. Container identities, configuration, mounts and genesis were retained.
The recorded image metadata remains the original metadata, so this is a verified
byte installation rather than a claim that immutable final images were deployed.

The first start order was too eager: node contacted signer while signer was still
initializing KZG, then exited with `UNAVAILABLE`. The audit's readiness wait timed
out after 180 seconds. Both failures are preserved. The explicit continuation
checked a successful Capabilities response, byte-for-byte identical to baseline,
before restarting only node. The previous safe commitment at 8946 kept its hash
and state root; none of the 18 captured unsafe blocks changed. The first RPC head
8965 was already persisted during shutdown, so actual recovery is established by
the subsequent advancing head and safe observations. This deliberate maintenance
interval is separate from the earlier L1-loss liveness tests.

The final RPC probe passed 60 samples, including ten stable import-boundary samples
on six distinct parents. One ordinary transaction then passed exact nonce,
transfer-value, gas and FeeVault accounting, canonical settlement and KZG checks.
Its receipt is retained for the closing replay. The final campaign executed all
34 eligible scenarios successfully; independent verification passed 33 and failed
`revertCounter`. The missing history configuration is in the standalone prefix
verification helper, and the exact input fails identically with both the R03 and
CI helper. No validation guard is being removed. The first suite result remains
FAIL; a canonical DA-only record of the affected full batch is available for
replay, while its intermediate lookup-root check was still pending at that stage. The same-run cold replay then passed **1/1** in 29.94 seconds (33 seconds
including the wrapper). It independently derived the chain from genesis and
checked 206 complete batches, 3,558 distinct selected block heights and all 814
selected receipts at canonical endpoint 9816. The exact 112 maintenance parts
and three ordinary EIP-7702 receipts remain selected, together with the final
ordinary transfer. The [final replay record](cold-replay-final.json) preserves
input, executable and result hashes. Its coverage through the real partial
consumption does not assert a direct comparison of that canary receipt, nor
does it replace the intermediate lookup-root oracle verified separately below.


A final read-only health check kept endpoint 9816 canonical, observed safe 10272
and latest 10273, and rehashed all five installed node/signer executable files
against the CI package. Both owned containers remain running. The enclave is
left active; no further workloads or restarts are planned. Current local URLs
are L1 RPC `http://127.0.0.1:32835`, L2 RPC `http://127.0.0.1:32879`,
L1 front `http://127.0.0.1:32881`, L2 front `http://127.0.0.1:32880`, and signer
gRPC `127.0.0.1:32873`.


## Read-only closure of the intermediate lookup-root check

The corrected standalone helper passed the exact expanded input that still fails
with the original CI helper. All original fields were retained; only the L1
chain configuration, bounded L1 header bytes and deployed profile commitment
were added. It reconstructed the original on-chain lookup root at the two-
transaction prefix, then the complete existing collector passed on the same
trigger and post without broadcasting. That check also revalidated canonical
receipts, KZG/native semantics, the exact reverted call trace and absence of an
L2 delivery over blocks 9463–10567.

The [continuation record](protocol-prefix-continuation.json) combines the original
33 independently verified cases with this corrected case. The first full campaign
is preserved as FAIL; this is an explicit read-only correction, not a fresh full
suite pass. The collector's refreshed witness changed only the ordering of the
same code, state and key elements, while the standalone A/B used identical bytes.
The corrected helper has its own local build identity; node, signer and final
replay follower remain the verified CI `319ebc2d` binaries. No additional workload
or service restart was performed.
