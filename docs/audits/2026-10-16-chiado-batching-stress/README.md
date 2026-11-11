# Chiado batching stress and Falcon Safe validation

Original run: Falcon passed; the full protocol matrix failed and left 231
accepted test sources pending. Read-only canonical-prefix verification completed.
The later explicitly authorized same-image recovery completed all 231 by
07:28 UTC; see the [recovery follow-up](#authorized-same-image-recovery-follow-up-07100728-utc).
The original failure and approval history below is preserved. A fresh matrix
must validate the fixes separately.
This is test evidence, not a production-readiness or security certification.

| Field | Value |
| --- | --- |
| Runtime source | `ce6574788682c54bdd3f9742870f5dbdbd9a4a9c` |
| Harness checkout | `6204796de60b338c4fdce3317ff4495ba275bcfd`, with isolated test adapters |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized, no tracked edits |
| Network | Chiado L1 10200; existing EEZ L2 906969 |
| Node image ID | `sha256:9f65a9371ae04e2c7404487527163c204e922ce5ca004a5ac8b65e75095fb785` |
| Signer image ID | `sha256:4983fe207bea7e40b2dfd092fe8ad515ffbe111b1797f4745f1f9f3bb157a681` |
| Tools | Foundry 1.7.1; Node 26.7.0; viem 2.57.5; Playwright 1.63.0 |
| Fee configuration | 20% quoted profit, zero risk/service costs, 1:1 chain conversion, 90% L1 refund |

The [preceding deployment](../2026-10-16-l1-batching/README.md) establishes the
source/image mapping. These are local development images, not CI-promoted
artifacts. Runtime image labels identify the source; preexisting Compose
container revision labels are stale and were not used as source evidence.
Services, contracts, genesis, protocol pin and fee configuration are unchanged.

## Confirmed findings

**Falcon passed all 48 intended sources:** 16 Safe creations and two waves of
16 Safe value transfers. Both legacy and EIP-1559 formats used the exact compact
quote without fee or gas changes. Real browser-generated Falcon signatures used
the application's pinned `@noble/post-quantum` implementation. Every source used
more than 5 million gas; the observed range was 5,442,219–6,003,571.

The checks validated controller authorization events, action/signature binding,
controller and Safe nonces, factory mappings, Safe runtime/implementation,
single-owner threshold, absence of modules and guards, fallback handler,
transfer events and exact recipient balances. Read-only replay, altered-payload
and unauthorized-caller probes were rejected with the expected errors. Each
whole posting's source tips covered its execution and blob costs. All 13 native
settlement blobs were authenticated; an independent public L1 RPC agreed with
the financial receipt fields. No installed wallet extension was exercised.

**Queued same-sender transactions fail admission.** The three-source
`counter-multi-tx` scenario admitted its first source, then rejected the next
nonce with `settlement tariff admission failed` / `quote_unavailable` / `gas
estimation`. The compact API also explicitly rejects quotes depending on queued
predecessors. Reusing the first quote's fee envelope did not bypass signed
admission. Serializing these transactions would change this scenario and was
not used to turn its failure into a pass.
After the predecessor settled, a separate read-only quote for the previously
rejected nonce (`0x4c`) succeeded with the same destination and calldata. It was
not broadcast. Signed admission prices an isolated source through
`quote_signed_fee`; queued predecessors are not included in that state.
The [current tariff contract](../../configurable-settlement-policy.md#opt-in-l2-tariff-admission)
already excludes future-nonce quotes from its isolated profile. This is a
compatibility limitation exposed by the broader protocol matrix, not evidence
that this batching change newly broke a previously supported fee profile.

**An isolated gas quote was insufficient for a shared static-call batch.**
`topLevelStaticReentrantCounter` was accepted with a 143,583 gas limit. Ordered
bundle validation repeatedly rejected the shared post as out of gas. The five
L1 static sources and five independent L2 sources remained queued. Only the
owned test source was replaced, preserving its payload and nonce, with a
287,166 gas limit and a 12.5% replacement fee bump. It then used 150,461 gas,
the original hash reached terminal `PoolDiscarded`, and the queue drained.
The replacement hash is
`0x577e252ac886231593af4a85bc28e009cab3bdcb86972704a8deda18e1f1f27a`.
This demonstrates both an estimation gap and a liveness consequence of retrying
the unchanged failing batch. Runtime behavior was not repaired by this test.
Separate recovery verification passed 33 pinned semantic scenarios; the
three-transaction counter remained not verifiable after its partial send.
The verification command returned failure overall, as required.

**The 770-job stress run reproduced the shared-batch gas failure.** All 770
jobs were prepared, including 2,850 setup transactions whose receipts and
2,830 predicted deployment addresses were checked. The requested matrix implies
870 source transactions because each of the 50 `counter-multi-tx` jobs has three
sources. Admission accepted 770 sources: 420 L1 and 350 L2. The second nonce was
rejected in all 50 multi-transaction jobs; their third transactions were not sent.
There were 820 source submission attempts, not 870 successful submissions.

539 sources reached canonical settlement: 289 L1 and 250 L2. These comprise
500 complete jobs plus the first source of 39 incomplete multi-transaction jobs.
The other 231 accepted sources remain pending. A candidate containing 39 L1
sources repeatedly failed ordered bundle validation at transaction index 37:
`Halted: out of gas (Gas used: 147709)`. The ordered candidate maps that index to
`revertCounter-6`, source
`0xa44ecacdf5ecba019a379dc981567c2f1b994da0f8accddefebc3d2930580ec2`.
Its isolated quote provided 147,709 gas. Final validation correctly prevented an
invalid post, but repeated unchanged attempts held up independent eligible work.
The measurement process was stopped; the Composer and its queue were preserved.
Separate read-only semantic verification selected the 539 canonical jobs and
reported **500 passed, zero failed assertions, 39 not verifiable**, exiting 1.
It did not select the 231 pending jobs and cannot establish a full matrix pass.

**The quote-history bound prevented normal recovery.** Fifty owned
`revertCounter` sources were explicitly submitted as replacements with the same
payload/nonce, twice the gas limit and a 12.5% fee bump. None was accepted:
all returned `quote_limit: quote work limit: unsettled history exceeds 256
blocks`. The original 231 requests therefore remain owned by the Composer.
This is the `MAX_HISTORY` guard in
[the quote engine](../../../crates/eez-composer/src/composer/fees/quote.rs),
which signed tariff admission also uses. The history check precedes the
advisory-parent scan that finds a reconciled execution context; after a prolonged
stall it also blocks a higher-gas replacement from reaching normal admission.
No bound was removed and no fee or final-validation check was bypassed.

## Measured scope and economics

The stress burst submitted its accepted cohort in 4.704 seconds with four
quote/send workers. All 770 requested source quotes succeeded; their latency was
10.16 ms p50, 19.14 ms p95 and 284.27 ms maximum. This is a short burst with
mostly simple protocol calls, not a sustainable capacity measurement. The 539
canonical sources span 52 posts and reached their last posting 418.227 seconds
after quote/send started. The stalled remainder prevents reporting a successful
end-to-end throughput for the complete cohort.

The largest observed stress posts contain 39 L1 sources and 37 L2 sources.
Every observed L1 payout matches 90% of the largest source's actual total
priority payment, less the refund transaction's execution cost. Multiple user
payments are not accumulated into the refund. The allowed L1 subsidy is visible
under load: 16 of the 32 L1 stress posts lost money. All 20 L2 stress posts
collected enough tips to cover their complete on-chain posting costs.

| Canonical cohort | Recovered/collected (wei) | Post execution + blobs (wei) | Surplus (wei) |
| --- | ---: | ---: | ---: |
| Stress L1, 289 sources / 32 posts | 30793202651105141 | 81234651320623387 | -50441448669518246 |
| Stress L2, 250 sources / 20 posts | 382109783147219260 | 200831804736153816 | 181277978411065444 |
| Falcon, 48 sources / 13 posts | 29950521860941479 | 15942574096278978 | 14007947764662501 |

The Falcon workload used exact quoted fees. Its aggregate surplus is about
87.87% of actual posting costs because independent quotes share cheaper posts;
20% is the quote's standalone margin, not a guarantee of exactly 20% realized
margin after batching. The protocol stress used double quoted fee fields, so
its surplus cannot validate the normal quote's margin. These totals exclude
unmeasured proving/operations costs and source-chain base fees paid by users.

An independent follower, replaying canonical L1 data from its own stopped
checkpoint without the sequencer overlay, matched the target L2 block hash and
state root, 796 L2 receipts, and the nonce state of all 16 Falcon controllers.
Those receipts include source transactions and derived L2 execution across all
completed diagnostic runs and the canonical stress prefix; they are not 796
completed stress sources. Pending sources are outside this replay result.

## Method and retained failures

The staged protocol runner predates compact fee quotes. An isolated copy kept
the pinned preparation and semantic verifiers but replaced submission and
receipt monitoring. The protocol checkout itself was not patched. The adapter
used the production SDK to bind estimates to each transaction, four concurrent
quote/send workers, alternating transaction types, and canonical ownership plus
safe receipt/block-hash checks. Preparation concurrency remained 200. The
upstream displayed send-worker count of 28 does not describe the adapter's
effective concurrency of four.
Each measured quote requested one transaction format; these latency measurements
do not describe the default response that calculates both formats.

Protocol stress envelopes doubled the quoted fee fields, while retaining the
automatically estimated gas limit. This measures batching with fee headroom;
it does **not** establish exact-quote throughput or a realized 20% margin. The
Falcon sources used unchanged quotes and are reported separately. Identical
multi-transaction payloads reused the first quote's envelope for later nonces;
those were explicit test envelopes, not successful independent quotes.

The first protocol attempt retained the old fixed 1,000,000 gas limit. Only
21 sources were accepted: upstream wallet reserves and that fixed limit caused
quote failures; future nonces were unsupported. Five accepted L2 sources were
temporarily fee-deferred. The old monitor called them disappeared, but all five
later completed without replacement or cancellation. The run remains failed.
The corrected diagnostic run accepted 34 sources, but the static-call stall and
partial multi-transaction send prevented a clean matrix pass. Its monitor and
remaining verifier processes were stopped before explicit test-source recovery;
first logs and partial semantic results are retained. Recovery verification uses
a separate copy with the single replacement mapping; it cannot erase the first
failure.

The stress setup's recovery helper also printed unidentified-consumed-nonce
warnings for transactions already mined. The runner's next ordinary receipt
poll reconciled all setup transactions. Its 65 setup fee replacements and the
original warnings are retained; setup was not a clean first pass.

The first protocol blob-verification pass authenticated 57 posts, then failed
before validation on a 31-source L2 post because `jq --argjson` exceeded Linux's
single-argument limit. The recorded receipt alone is 162,161 bytes in compact
JSON. The [helper](../../../testing/kurtosis/scripts/verify-semantic-settlement-blob.sh)
now reads transaction, receipt and trigger-list JSON through files; it preserves
every field and the existing KZG, semantic and exact L2-byte assertions. The
same failed post then passed, followed by all remaining posts. Together, all
72 protocol posts / 594 completed protocol sources and 13 Falcon posts / 48
sources are authenticated. The original failure log/exit remain; the recheck
does not turn the original helper invocation into a pass.

Four early L1 posting receipts became unavailable from the local RPC during
later accounting. The independent public Chiado RPC supplied them and agreed
with the earlier retained local receipt checks. Archival fallback was explicitly
recorded; its underlying local retention cause was not established. All 72
protocol post receipts were cross-checked through the public provider.

The pinned semantic verifier sometimes prints the first scanned L1 block beside
the correct matching post hash. Across the 533 passed diagnostic/prefix jobs,
all printed post hashes matched canonical correlation; 266 printed block
heights disagreed with their actual posting receipts. Financial calculations
and timing use the actual receipts/block timestamps, not those display values.
The protocol checkout was not changed to hide that reporting defect.

Other retained setup failures: stale operator chain-ID configuration, insufficient
L2 workload-faucet funding, and a missing pinned Chromium binary before any
Falcon broadcast. The private test profile corrected the chain ID, 4.5 existing
test-xDAI was bridged to the workload faucet, and Chromium was installed into the
private evidence directory. The operator configuration and application checkout
were preserved. Both known protocol worker-pool locks were held for submissions
and recovery; no unrelated wallets, containers or datadirs were cleared.

## Evidence and limits

Raw logs, private workload keys, signed envelopes, browser dependencies, manifests
and follower state are local-only under the operator's ignored
`artifacts/chiado-batching-stress-20261016.43nwzx0q` directory. They are not
available from a fresh checkout and must not be committed wholesale.
[result.json](result.json) contains sanitized counts, identities, limitations and
local evidence digests; [posts.json](posts.json) records the protocol posting
hashes and aggregate financial amounts. Neither file provides the private state
needed to rerun or recover this workload.

The application's source tree has no initial Git commit; exact file/lockfile
hashes identify the tested code. The browser generated Falcon keys in memory,
cleared them at completion, and never exported them. Fresh ECDSA gas-wallet test
keys are retained only in private evidence.

The six pinned `E2E_EXCLUDE_FROM_ALL` scenarios are exclusions, not coverage:
`nestedStaticLocalWriteL2`, `staticLocalWrite`,
`topLevelStaticReentrantMissingProxy`, `nestedStaticLocalWriteL1`,
`staticLocalWriteL2`, and `topLevelStaticReentrantMissingProxyL2`.

No older-runtime throughput comparison, sustainable-capacity ceiling, additional
builder qualification, durable queue recovery or full CI release-matrix result
is claimed. Ordinary L2 settlement remains subsidized by policy. Financial
surpluses below concern on-chain execution/blob costs, not unmeasured operating
or proving costs.

## Executed checks and operational boundary

The reviewed harness checkout is the working directory. Preparation used the
initialized protocol pin and its existing Foundry artifacts. The local adapter
replaced only quote/send/receipt monitoring; production SDK binding and the
pinned scenario preparation/semantic assertions were retained. Both protocol
wallet-pool locks were held. These are the selections actually exercised:

```bash
PREPARE_PARALLEL=200 DEVNET_ENV="$PRIVATE_PROFILE" \
  EEZ_STRESS_MATRIX=all bash "$EVIDENCE/harness/wrapper.sh" --verify
PREPARE_PARALLEL=200 DEVNET_ENV="$PRIVATE_PROFILE" \
  bash "$EVIDENCE/harness/wrapper.sh" --verify
```

`PRIVATE_PROFILE` and `EVIDENCE` denote the retained private configuration and
evidence directory; these are a record of execution, not a fresh recovery recipe.
The second command selects the requested `counter:100 counterL2:100
nestedCounter:50 nestedCounterL2:50 multi-call-nested:50 multi-call-nestedL2:50
multi-call-twice:50 multi-call-twiceL2:50 counter-multi-tx:50 reentrant:20
revertCounter:50 revertCounterL2:50 revertFromOtherChain:50
revertFromOtherChainL2:50` matrix. Both measurement drivers were stopped after
their stalls (exit 15). Separate verification-only invocations selected the
recovered 34-job diagnostic and the 539-job canonical stress prefix (exit 1 each).
Neither is reported as a successful original workload invocation.

The Falcon harness exited 0. Independent replay and public-receipt checks exited
0. The original protocol blob invocation exited 1; the file-input correction and
continuation exited 0. The deployed node and signer were not rebuilt, restarted
or reconfigured. The only repository implementation change is the read-only
blob helper. Its same-post live regression, shell syntax and affected tooling
checks are recorded with their results in `result.json`; no new Rust, Solidity,
Kurtosis network or CI release suite was run for this shell-only correction.

The focused blob-consumer tooling suite passed all 22 tests. The complete Python
tooling suite ran 374 tests: **366 passed, one failed, seven skipped**. The failure
is an existing CI inventory omission: `transaction_references` is declared in
the node E2E root but absent from `scripts/ci/native-tests.py`. The same single
test fails using exact baseline `6204796d` inputs, byte-identical to the unchanged
working-tree files. This run did not alter that pipeline or claim it passed.
Six skips require explicitly enabled Docker/Kurtosis/native-relocation fixtures;
the seventh requires Gitleaks, unavailable on this host. Those are exclusions,
not coverage. `git diff --check`, changed shell syntax, JSON parsing and edited
documentation links/shell examples passed.

At the final read-only status check, the original node/signer container IDs,
images, start times and restart counts were unchanged. All four public HTTP RPC
paths returned their expected chain IDs. Both quote fronts returned
`quote_limit: unsettled history exceeds 256 blocks`; the queue still contained
231 accepted test requests. Every pending request was matched to its retained
signed bytes, sender and unconsumed source nonce. The independently owned
follower and observer exited with evidence retained.

Automatic approval review rejected the prepared public RPC pause and same-image
Composer restart because the testing request did not clearly authorize that
service interruption/state recovery. Explicit approval was requested. The
rejected recovery script performed no operations: public RPC was not paused,
the Composer was not stopped, and its datadir/queue were not cleared. Do not
interpret this report as authorization to restart or as proof of durable queue
recovery. Pending signed transactions remain available for a separately
authorized recovery procedure.

## Authorized same-image recovery follow-up, 07:10–07:28 UTC

The operator explicitly approved pausing public RPC and restarting the same
Composer image. A fresh paused-ingress inventory matched all 231 outstanding
requests to the retained test submissions and unspent source nonces. Both
protocol wallet locks were held. The Composer stopped cleanly; its complete
stopped L2 database was copied before restarting the identical container/image
at 07:13:34 UTC. The signer, contracts, genesis and operator configuration did
not change.

The first replay attempt admitted zero requests: restart retained 260 valid
ordinary L2 blocks, exceeding the separate 256-block quote bound. An ordinary
background post then advanced canonical settlement, after which both quote
routes succeeded. This corrects any implication that the history error is
permanent: it is a recurrent mismatch between the quote bound and the 300-block
ordinary settlement trigger, and restarting alone does not reliably clear it.

A separate continuation replayed four sources per wave, checking each wave's
canonical source receipts and ownership before advancing. All **231** completed
by **07:28:31 UTC**, with no remaining queued, in-flight, blocked or unknown
requests. Of these, 131 retained their original signed bytes. The 100 L1
`revertCounter`/`revertFromOtherChain` requests used journalled replacements with
the same sender, nonce and payload, doubled gas limits and fresh/replacement
fees. [Sanitized identities and canonical evidence](recovery.json) retain that
distinction. Public RPC configuration was restored; all four HTTP fronts
returned the expected chain IDs. This recovers the accepted work; it does not
turn the original incomplete stress run into a pass or validate new code.

Raw journals and the stopped database remain local-only under the original
artifact directory. Source-level batching/quote corrections and a fresh complete
matrix are still required. The historical failure results above remain intact.

## Unmet criteria at the original run's completion

| Remaining item | Impact | Next concrete check |
| --- | --- | --- |
| Batch-sensitive gas budgeting | A valid isolated quote can produce an unexecutable shared candidate | Reproduce the static/reverted-call sources in a controlled shared batch; test gas headroom and candidate reduction while retaining ownership and final validation |
| Failure isolation | One repeated ordered-preflight failure stalls independent eligible work | Verify bounded retries/recomposition allow independent work without dropping whole groups or violating nonce dependencies |
| History-limit recovery | Quotes and replacement admission stop after 256 unsettled blocks | Add a bounded recovery path retaining correct state/fee validation; exercise replacements after prolonged stalls |
| Queued nonce profile | All 50 three-source counter jobs are incomplete | Extend the supported dependency-aware quote/admission profile before claiming compatibility with the complete protocol matrix |
| Pending test queue | 231 accepted sources remain unsettled | Obtain the requested recovery approval, reconcile canonical ownership again, preserve state and resubmit retained work under a bounded plan |
| CI module inventory | The preexisting `transaction_references` module is absent from the native execution suites | Classify the module in the CI runner and validate the exact artifact producer/consumer selection before release |

The previous [batching deployment checks](../2026-10-16-l1-batching/README.md)
remain evidence for their selected cases. This broader run demonstrates why they
do not qualify the full protocol stress matrix.
