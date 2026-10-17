# Chiado batching recovery and fresh validation

Status: recovery, fixes, deployment and the requested fresh validation are
complete. The deployed timing candidate passed all 34 enabled `all` scenarios,
all 770 stress jobs / 870 sources, and all 48 Falcon Safe sources. Independent
replay and native blob checks passed; the final queue was empty. Earlier failed
attempts and remaining limitations are retained below.

| Field | Value |
| --- | --- |
| Date | 2026-10-16 UTC |
| First corrected source | `9697b317d84597f1a6617e803cb337d7765f1451` |
| Timing correction | `be8477bc7ba48d3f287d33cd6c010089458885ec`; image set in [timing-executables.json](timing-executables.json) |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Existing runtime source | `ce6574788682c54bdd3f9742870f5dbdbd9a4a9c` |
| Environment | Rust 1.95.0, Foundry 1.7.1; local process fixtures and Chiado L1 10200 / L2 906969 |
| Prior evidence | [Original failed matrix and authorized recovery](../2026-10-16-chiado-batching-stress/README.md) |
| Contract | [Authorized correction scope](../../proposals/cross-chain-fee-recovery.md#stress-recovery-and-batching-corrections-authorized-on-2026-10-16), [settlement and fee policy](../../configurable-settlement-policy.md) |
| Draft PR | [#91](https://github.com/eez-association/eez-rollupX/pull/91); base at final validation `2c19e36bed2720c00f89841ccfca47ec7c921e06`, prior published head `11da0624c37b3c670fb9ca8affb5a61f9508b3d3` |

| Final check | Result |
| --- | --- |
| Pinned protocol `all` | 34 semantic checks / 36 sources passed; six explicit exclusions |
| Requested stress matrix | 770 semantic checks / 870 sources passed, exact quoted fees, no source replacements or receipt timeouts |
| Falcon Safe | 16 creations and 32 transfers passed; 24 legacy and 24 EIP-1559 transactions |
| Native blob evidence | All 81 measured posts authenticated, binding all 954 measured sources |
| Independent L1-only replay | Final safe hash/state root, 1,056 L2 receipts and 16 controller nonce calls matched |
| Final service health | Empty queue, unchanged runtime identities, both quote formats in both directions, public HTTP and WebSocket routes working |

## Recovery and corrections

The explicitly authorized same-image recovery completed all 231 retained
sources by 07:28 UTC and restored public RPC. The earlier failed run remains
failed. Its [sanitized recovery evidence](../2026-10-16-chiado-batching-stress/recovery.json)
distinguishes 131 original envelopes from 100 explicit replacements.

The candidate aligns advisory quote history with the active posting window and
stops before unsettled native effects. L1 gas estimates include 20% reservation
headroom; this is not an additional tariff. Final native execution remains
authoritative. A native preflight failure reduces the next selection of whole
groups. Reduction does not charge users; an indivisible source can consume a
bounded retry only when native execution identifies its exact signed L1 hash.
Posting/root failures cannot establish that attribution. The conservative batch
limit persists until the queue drains.

Explicit future L1 nonces replay at most eight contiguous owned predecessors,
including real balances, nonces and nested effects on both chains. Gaps and
unsupported profiles fail explicitly. Signed successors behind owned work reach
the stateful tariff check before ordinary transaction classification. L2 future
nonces and ordinary successors behind held work remain outside this profile.
No core contract, genesis, wire schema or persistent data format changed.

## First corrected candidate checks

| Check | Evidence and result |
| --- | --- |
| Composer / L1 / node unit tests | 364 / 193 / 89 passed; one manual Composer profiling test ignored |
| Workspace lint and formatting | `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` and Rust formatting passed |
| SDK | Six tests passed |
| Integration contracts | 86 tests passed; the changed fixture passes formatting. Whole-tree formatting still reports the two unchanged baseline files `ECDSAProofSystem.sol` and `DeployRollup.s.sol` |
| Tooling | 374 selected: 367 passed, seven skipped. Six opt-in Docker/Kurtosis/archive probes and unavailable Gitleaks are not covered by this unit suite |
| Native rejection process regression | Passed with real Gnosis native execution: three dependent L1 calls settled separately after reduction, independent L2 work completed, and a follower reproduced both resulting values |
| Queued routing reproduction | Before the ingress fix, the real process and focused HTTP regression forwarded an underpaid dependent successor after its isolated trace reverted before dispatch |
| Queued routing correction | The same process case passed after the fix: three dependent L1 nonces admitted, underpayment rejected, canonical state unchanged by quotes, nonce gaps rejected |
| Broader process suite | All 24 selected fee, Safe, partial-consumption, liveness and reorg tests passed in 1,373 seconds; no ignored tests in this selection |
| Exact-image Kurtosis fee checks | Legacy/type-2 nested and Safe L1 sources, two four-user shared posts, largest-payer refunds, intentional under-recovery continuation, and two independently funded L2 nested calls passed; underpaid sources were rejected |
| Exact-image Kurtosis native blob DA | Contract deployment and two mutations authenticated in native blobs; safe state and both correlation directions passed. Blobscan was explicitly excluded |
| Exact-image Kurtosis independent replay | Shipping follower started from genesis using canonical L1 only; matched the selected 30-block range, all three smoke receipts and safe block 384 |
| Fresh Chiado protocol `all` | 34 semantic checks passed, zero failed/unverifiable; all 36 sources canonical in six posts, with maxima of 13 L1 and 14 L2 sources per post |

The two focused process diagnostics used explicitly built and hashed local role
binaries. The broader source-based run clears all prebuilt overrides. Its logs
retain the initial sandbox binding failure, an interrupted setup build, the
test-stub compile correction, and the expected before-fix routing failure.
Those attempts are not counted as passing coverage. The initial node metric
assertion expected 22 samples after a seventh submission state was added; it was
corrected to 23 and the complete node unit suite passed.

The first Kurtosis replay invocation failed before RPC startup: the existing
shell YAML reader retained single quotes around the timing value `'3'`.
The diagnostic rerun passed with explicit numeric settings from the selected
enclave profile. Source and runtime bytes were unchanged; the initial log and
failed datadir remain retained. This was a corrected test configuration, not an
unexplained retry-only pass.

## First corrected Chiado deployment

The qualified local images in [executables.json](executables.json) replaced the
node and signer at 09:31 UTC. Public HTTP/WS ingress was paused, both accepted
queues were checked empty, both writers stopped cleanly, and complete L1/L2
databases were copied before starting the candidate. Original stopped databases
remain retained. Genesis, contract bindings, fee policy and other runtime
settings were unchanged. Public RPC reopened at 09:32:01 UTC after confirming
both chain IDs, synchronization, empty queue and the exact pre-upgrade canonical
safe block hash and state root. This is a local development image deployment,
not a CI release promotion.

The separate Chiado follower initially inherited an obsolete setting through
Reth's ancestor `.env` discovery. It was restarted with a fresh checkpoint copy
and an isolated working directory; the first log and data copy remain retained.
It caught up using L1 alone and continues to follow the measured workload.

The fresh `all` run used exact quoted fees, both transaction types and the pinned
semantic verifier. Fourteen L2 sources were temporarily fee-deferred when the
dynamic posting priority price rose from 1 to 1.7538 and then 2 gwei. All settled
without replacement or extra payment when the 20-block fee-history price returned
to 1 gwei, about two minutes later. No pricing code was changed for this delay:
the cost floor deliberately allows deferral when price movement exhausts the
20% quote markup. It is a latency limitation under this workload, even though
ownership and eventual settlement succeeded.

For this small run, the 16 L2 sources paid 0.011180023261629559 test xDAI in tips
against 0.005816315038879197 of actual L1 execution/blob cost. The 20 L1 sources'
largest-payer refunds totaled 0.002814420015179696 against posting costs of
0.005391252327951561; two posts were subsidized as explicitly authorized.
These figures exclude unmeasured proving/operating costs and ordinary L2 subsidy.
The six pinned `E2E_EXCLUDE_FROM_ALL` scenarios remain exclusions, listed in the
[original record](../2026-10-16-chiado-batching-stress/README.md).

## First stress run and timing correction

The 770-job setup initially timed out with one ordinary L1 deployment still
pending; no measured source had been sent. The original setup failure is retained.
Before resuming, an independent read-only reconciliation verified all 2,850
successful deployment receipts, 2,830 predicted code addresses, unchanged source
nonces, canonical public L1 hashes and the L2 safe prefix. Only completed setup
metadata was repaired under both staged-runner wallet locks.

All 870 sources (520 L1, 350 L2) were then quoted and accepted in 5.556 seconds,
without fee multipliers, quote retries or admission errors. Quote latency was
10.53 ms median and 20.59 ms p95. These are burst intake measurements, not
sustainable settlement throughput. Settlement exposed a new blocker: a large
cohort of L2 sources became underfunded after the posting price rose. Repeated
tariff simulations were charged both to the elapsed deadline and to predicted
finishing/next-group work. Admission alternated between no progress and a
single-group probe, settling roughly one L1 source per ten seconds. The run
therefore does not qualify throughput or completion of the full matrix.

The follow-up regression models 300 rejected groups before 30 ready groups.
Before the estimate correction it stops at rejection index 275. Only a measured,
disjoint interval whose speculative effects have been removed is excluded from
future-work prediction; absolute elapsed time, proof/blob reserves, FIFO and
final funding checks remain. The corrected regression and all 366 Composer unit
tests pass (one unrelated manual profiling test remains ignored). Missing,
duplicate, mismatched, overlapping and reset timing brackets are covered.
All original 870 envelopes eventually settled by 10:13:45 UTC, without another
restart, replacement or fee increase. The first four L2 receipt waits had timed
out; their later canonical receipts are recorded separately. The measured wave
took 1,448.93 seconds (0.60 sources/second), with 1,337.21-second median and
1,413.88-second p95 settlement latency. The [first stress result](first-stress-result.json)
retains this failure. Background builds overlapped the latter part of the run,
so its timing is not a controlled performance baseline.

All 124 posts across `all` and this matrix authenticated through native blobs.
The independent follower matched a preceding safe prefix and the `all` suite's
38 receipts; that earlier replay does not cover the final complete matrix.
Across both runs, L2 tips were 0.256985924615257890 test xDAI against actual L1
execution/blob costs of 0.126248101326032717. L1 refunds were
0.054059763281559410 against costs of 0.133175491127425908. All L2 posts covered
cost; 107 L1 posts were undercovered under the authorized largest-payer policy.
Maximum shared posts contained 71 L1 or 40 L2 sources.

All 770 semantic checks eventually passed, with zero failures or unverifiable
jobs. The original verifier was stopped after 507 successful checks to raise
concurrency. A nested sourced profile initially reset the override to four;
that continuation was stopped after 602 successful checks. After correcting the
profile and confirming its effective value, the final continuation ran 16 workers
under both wallet locks with `SKIP_VERIFIED=1`: 168 passed and 602 previously
verified jobs were skipped. Completed and interrupted logs are retained; all
60-second absence checks remain. This does not change the failed latency result.
See [semantic evidence](first-stress-semantics.json). The source-based timing process run was
interrupted during its first role-binary setup, before any test completed.
The newly compiled test executable then ran against the exact hashed shipping
binaries extracted from the timing image set: all 28 dynamic-admission tests
passed in 1,568.29 seconds and all 14 fee/Safe tests passed in 618.31 seconds.
No selected tests were ignored. The roles are the production binaries; the test
executable uses its test feature selection. See [timing checks](timing-checks.json).
Exact-image Kurtosis also passed: both formats for nested/Safe sources, two
four-user largest-payer batches with actual shortfalls, two L2 fee sources, three
authenticated blob transactions and independent genesis replay through safe block
390. The fresh relay initially had no proposer registrations; it became ready
without runtime intervention. The blob wrapper selected the host default Rust
1.96.1 helper. The same native evidence was reauthenticated with the verified
Rust 1.95 helper and produced identical output. The default-toolchain replay
compilation was stopped before any test launched; the exact Rust 1.95 executable
then passed with the shipping follower. See [Kurtosis evidence](timing-kurtosis.json).
This is explicit qualification of those bytes, not a claim that the interrupted
build passed.


## Timing candidate deployment and fresh workload

The new node and signer started at 11:13 UTC; public RPC reopened at
11:13:11 UTC. Both queues were empty before the writers stopped. The handover
preserved stopped original databases and started the candidate on separate full
copies, with unchanged genesis, contract bindings and fee settings. The recorded
pre-upgrade safe block hash and state root matched after startup. All four public
HTTP routes returned their expected chain IDs. See
[deployment evidence](timing-deployment.json). The owned Kurtosis enclave was
dumped and stopped before this measurement; unrelated services/enclaves remain.

The [live harness manifest](live-harness.json) records the pinned protocol and
adapter hashes. Preparation uses 200 workers, exact quote/sign/send uses four,
and pinned semantic verification uses 16. Ordinary setup funding uses 300-wallet
chunks, independently estimated at no more than 3,134,661 gas. This reduces the
runner's unused per-chunk funding reserve without changing measured transaction
fees or workload. Source monitoring now polls the entire cohort against one
600-second deadline; no source receives an implicit fee bump or replacement.
Setup has a separately configured 600-second mining budget; the diagnostic
interruption below preceded automatic timeout. All jobs must be prepared before
any measured source is submitted.

The measured matrix is the requested pinned protocol selection:
`counter:100 counterL2:100 nestedCounter:50 nestedCounterL2:50
multi-call-nested:50 multi-call-nestedL2:50 multi-call-twice:50
multi-call-twiceL2:50 counter-multi-tx:50 reentrant:20 revertCounter:50
revertCounterL2:50 revertFromOtherChain:50 revertFromOtherChainL2:50`.
The adapter runs the staged runner with `--verify`, `PREPARE_PARALLEL=200`,
`SEND_WORKERS=4` and `VERIFY_PARALLEL=16`, holding both checkout wallet locks.
The separate `all` selection retains the six pinned exclusions:
`nestedStaticLocalWriteL2`, `staticLocalWrite`,
`topLevelStaticReentrantMissingProxy`, `nestedStaticLocalWriteL1`,
`staticLocalWriteL2` and `topLevelStaticReentrantMissingProxyL2`.
Those exclusions are not covered by the 34 passing scenarios.


The fresh matrix's setup retained 17 ordinary L1 transactions in the local pool
that were absent from the public RPC despite contiguous nonces, sufficient
balances and fee caps above the current base fee. The harness was stopped before
any measured source was sent. Under both wallet locks, the exact original bytes
were rebroadcast through public Chiado and all 17 became canonical without fee,
gas or payload changes. The resumed pinned setup monitor verified all 2,850
receipts and 2,830 predicted code addresses before source submission. The first
setup attempt and route diagnostics remain retained. This is an operational
setup-propagation workaround, not a demonstrated fix to ordinary L1 gossip.
See [setup evidence](timing-setup-propagation.json). The cross-chain source wave
is fresh, with no earlier signed fee-bearing submissions to recover.

The new `all` suite passed all 34 semantic checks with all 36 sources canonical.
The stress wave accepted and canonically completed all 870 sources, with zero
quote/admission errors, receipt timeouts, source replacements or fee increases.
Its 870 single-format quote calls took 10.72 ms median and 20.86 ms p95; source
submission took 5.603 seconds. The last canonical settlement was 492.284 seconds
(8 minutes 12 seconds) after submission began: 1.77 sources/second over this
finite mixed wave, with 87.89-second median and 456.85-second p95 latency.
The 520 L1 sources finished after 137.284 seconds. Price movement deferred the
350 L2 sources; two native preflight reductions (38 → 19 → 9 complete groups)
also retained a conservative selection cap until the queue drained. No native
rejection evicted a source. Maximum stress posts contained 70 L1 or nine L2
sources. See [stress measurements](timing-stress-result.json).

The earlier candidate took 1,448.93 seconds for the same source mix, but its
latter part overlapped builds and public-network conditions differ. This is an
observed change, not a controlled performance comparison or sustainable-capacity
claim. No compilers were running before the measured new wave. All 68 posts
across `all` and stress authenticated through native blobs. The L2 side collected
0.257300180559014171 test xDAI against 0.133396081840973591 actual execution/blob
cost; every L2-funded post covered that cost. L1 refunds totaled
0.014837922357200322 against 0.119547305566791861 cost, with 25 undercovered posts
under the explicitly authorized largest-payer policy. These are on-chain fee
figures, not profit after unmeasured proving or operating costs. See
[protocol totals](timing-protocol-result.json). All 770 stress semantic checks
passed with zero failed, skipped or unverifiable jobs. The 60-second revert
absence checks were retained. These checks completed after canonical settlement;
their wall time is not included in the settlement throughput figure.

The first Falcon setup attempt stopped before signing any measured source.
Its tenth controller deployment had an unsafe L2 receipt, and successive
`latest` reads crossed canonical reconciliation at 11:47:46 UTC. One call
returned empty data. All 20 ordinary funding/deployment transactions subsequently
became canonical, and all ten controllers had the expected configuration. The
corrected harness waits for canonical setup receipts and binds each controller
assertion to its receipt block. It reuses the ten funded gas accounts, funds the
remaining six, and generates fresh Falcon keys/controllers for the complete
48-source run. The first correction was deliberately stopped during setup,
before signing sources, to group canonical waits into funding and deployment
phases instead of waiting a separate ordinary-L2 settlement interval for each
controller. The first failure, both preparatory journals and state-transition
evidence remain retained. See [harness correction](falcon-setup-correction.json). This is a corrected harness precondition; it does not
establish that speculative `latest` reads are stable during reconciliation.

## Final Falcon, replay and health evidence

The complete corrected Falcon run passed at 11:58:28 UTC. It used real browser
Falcon signatures, 16 fresh controllers/Safes and 32 transfers: 24 legacy and 24
EIP-1559 sources, all using the exact quoted gas limits and fee fields. All 48
quotes succeeded on the first attempt. Gas used ranged from 5,443,117 to
6,001,392; quote latency was 63.19 ms median and 99.97 ms p95. Creation and both
transfer rounds checked Safe 1.4.1 code/configuration, owners, threshold, modules,
guard, fallback handler, exact execution events, nonces and recipient balances.
Replayed controller nonces, unauthorized Safe callers and changed signed
calldata were rejected by the intended checks. No wallet extension was tested.

All 13 Falcon posts authenticated through native blobs and matched independent
public L1 receipts. They contained up to five Falcon sources per post. L2 tips
totaled 0.029753798665806581 test xDAI against 0.015926106099555190 actual L1
execution/blob cost; every post covered that cost. The resulting
0.013827692566251391 surplus excludes proving, operations and ordinary setup,
so it is not a measured net profit. Quote margins and shared-batch amortization
do not imply an exact realized 20% margin. See
[Falcon measurements](timing-falcon-result.json).

The independent shipping follower replayed canonical L1 from a separate stopped
checkpoint, without the Composer's speculative overlay. It matched safe block
`758320` (`0xb9230`), its hash/state root, all 1,056 selected L2 receipts including
logs and gas/fee fields, and all 16 Falcon controller nonce calls. This covers
both protocol runs and all Falcon sources; the earlier exact-image Kurtosis run
separately tested replay from genesis. All 68 protocol posts and 13 Falcon posts
passed native blob authentication. L2 source blocks credited the configured
posting account. See [final verification](final-verification.json).

At 11:59:36 UTC, queued, ready, blocked, in-flight and unknown counts were all
zero. Node/signer container identities, image IDs and restart counts matched the
pre-workload baseline. Both Composer fronts returned both transaction formats
after load, and all four public HTTP routes plus both WebSocket routes returned
the expected chain IDs. The owned follower and observation jobs exited cleanly;
the deployed Chiado node and signer remain running.

Detailed private journals, raw signed transactions, databases and build logs
remain **local-only**, outside Git, under
`artifacts/chiado-stress-rerun-20261016.6dsi06_b` and
`artifacts/chiado-stress-fixes-20261016`, and
`artifacts/chiado-timing-rerun-20261016.2lj43r67`. Another checkout cannot retrieve these
ignored directories. `checks-so-far.json`, `candidate-inputs.json` and
`docker-source.json` record local log/source hashes and the exact recursive
submodule snapshot. Docker uses tracked files only; workload wallet files and
journals are excluded. A first build was intentionally interrupted after archive
file-mode normalization missed cached dependency layers; source bytes did not
change in that retry.

Remaining qualification limits: the six pinned exclusions were not exercised;
the ordinary L1 setup propagation issue used a same-byte public rebroadcast
workaround; price changes can defer L2 sources; and conservative native-preflight
reduction can lower throughput until the queue drains. Shared Chiado conditions
and the earlier run's overlapping builds prevent a controlled performance
comparison. The original stress used doubled fee fields and is not a comparable
baseline. Additional builders, wallet extensions, durable recovery/accounting and
the full CI release matrix are not qualified by this run. The configured L1
largest-payer policy still deliberately permits subsidies; no fee policy was
relaxed to make these checks pass.
