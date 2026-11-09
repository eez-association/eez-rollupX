# Chiado fee quote and canonical settlement load test

The deployed candidate handled **100 background quotes/s for 60 seconds**, with
6,000/6,000 successful responses and **21.48 ms p95**. Each request estimated gas
internally and returned both legacy and EIP-1559 options. An additional 225 quotes
for signed submissions ran in that interval. This establishes a tested rate,
not the endpoint's maximum capacity.

With enough independent senders to offer 8 source transactions/s for 30 seconds,
all **240/240** measured transactions were accepted and canonically settled.
Settlement took approximately 84 seconds including drain: **2.86 source tx/s**,
with **54.78 seconds p95** observed settlement latency. The queue grew to 203.
This traffic rate exceeded the settlement rate observed for this workload.

| Field | Value |
| --- | --- |
| Date and status | 2026-10-15 UTC; measured traffic complete; expanded independent verification passed after the retained availability-wait correction below |
| Scope | Compact fee quotes, signed admission, canonical settlement, Safe/nested semantics and posting cost coverage on the existing Chiado deployment |
| Runtime source | `b2a82536e87d2543ce9839eb3ba15a9ceefdaefc` |
| Harness base | `eez-association/eez-rollupX` at `110874fc559b8f3fba2a7e1e6c0e18f3f2e9c6ca`, plus the harness accompanying this record |
| Protocol | Initialized, unmodified `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Node image | `sha256:712c027cd6ee482adb449d53783dc75353e5feca2b0d8ab50962875a0d5cac09` |
| Signer image | `sha256:00cdbf0a414b3bfeaaf028c38901d84c7286fbae778cffc2d60c9d6687303250` |
| Environment | Chiado 10200 / L2 906969; shared Intel Core Ultra 9 185H host, 22 logical CPUs, 60.91 GiB RAM; Node 26.7.0, viem 2.47.0 |
| Policy | Target profit 2,000 bps, risk 0, native currency conversion 1:1, configured fixed proving/operations/treasury costs 0 |
| Transport | Local Composer HTTP endpoints; the public gateway was not the load target |
| Related records | [Fee implementation/deployment](../2026-10-15-simplified-fees/README.md), [load procedure](../../runbooks/chiado-fee-load.md) |

## Workload and measurement

Equal offered proportions of four L2-origin operations:

1. L2→L1→L2 nested counters using the pinned `Counter` and `CounterAndProxy`.
2. An L2 proxy call creating one initialized Safe on L1.
3. Safe creation whose initializer delegates to official MultiSend to create
   four more Safes: five real Safe deployments per source transaction.
4. An owner-signed Safe `execTransaction` reached through an L2 proxy, moving
   one wei on L1 to a distinct recipient. Safe nonce and exact balances are checked.

These are published Safe 1.4.1 fixtures with ordinary ECDSA owners. They do not
execute the Falcon-specific cryptographic workload. Signed source transactions
alternate legacy and EIP-1559 formats. All quotes omit gas and request both
formats; submission quotes also pass through the fee SDK's binding validation.
Unique creation salts prevent quote collisions with previously deployed Safes.

Setup and one canary per category precede each run. A separate funded quote actor
serves background traffic. Fixed-rate arrivals record omissions due to a busy
actor, scheduling lag or an outstanding-request bound. A sender has only one
unsettled transaction at a time. There are no automatic quote/submission retries.
Submission-specific quotes are additional to the background quote rate.

The primary run had 64 senders; the follow-up had 256 so it could offer the full
burst without waiting for earlier transactions to settle. The same runtime,
policy, payload categories and local endpoints were used. The follow-up reused
fixtures and the primary run's unspent test funds; it was not a repeated sample
under identical chain state.

## Measured results

| Senders | Background quotes/s | Offered source tx/s | Arrival window | Successful quotes | Quote p95 ms | Accepted / settled | Omitted arrivals | Settled tx/s including drain | Settlement p95 s | Max sampled queue |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 64 | 10 | 0.5 | 60 s | 600 / 600 | 124.58 | 30 / 30 | 0 | 0.37 | 32.01 | 9 |
| 64 | 25 | 1 | 60 s | 1,500 / 1,500 | 61.02 | 60 / 60 | 0 | 0.87 | 16.55 | 19 |
| 64 | 50 | 2 | 60 s | 3,000 / 3,000 | 32.21 | 120 / 120 | 0 | 1.61 | 12.90 | 29 |
| 64 | 100 | 4 | 60 s | 6,000 / 6,000 | 21.48 | 225 / 225 | 15 | 2.81 | 21.26 | 64 |
| 64 | 25 | 8 | 30 s | 750 / 750 | 55.29 | 148 / 148 | 92 | 1.19 | 94.41 | 64 |
| 256 | 25 | 8 | 30 s | 750 / 750 | 53.03 | 240 / 240 | 0 | 2.86 | 54.78 | 203 |

All **12,600 background quotes**, and all quotes/admissions for the **831 signed
sources including eight canaries**, succeeded. The 256-sender burst had admission
p95 **7.23 ms** and settlement median **41.07 seconds**. Neither that admission
latency nor the 8 tx/s arrival rate is a settlement throughput measurement.

The primary measured stages ran from 20:32:17.847 to 20:37:53.930 UTC, with the
final drain ending at 20:39:28.991. The expanded stage began at 20:48:13.627,
stopped arrivals at 20:48:43.626 and finished drain at 20:49:38.672.
Exact timestamps, distributions and per-category results are in [results.json](results.json).

The reported cohort rate divides that stage's settled sources by the time from
its first scheduled arrival to the later of its arrival-window end and last
observed settlement. It excludes the polling delay after the last observation.
`settledInWindowPerSecond` is recorded separately. Observation polls are roughly
one second apart plus RPC processing, so settlement latency includes detection
delay. Each stage drains before the next begins.

Quote latency fell across the sequential ramp. Cache warming, changing state and
shared-host/network conditions were not controlled; higher offered load is not
established as the cause. These short cohorts do not establish a steady-state
ceiling or a pre-fee/candidate speedup. Earlier throughput audits use different
workloads and cannot supply that comparison.

## Canonical, semantic and fee checks

Across both runs, **831/831 sources** settled through **69 native blob postings**.
The workload created **1,243 Safes**, performed **208 nested round trips**, and
executed **208 Safe value transfers**, including canaries. The checks established:

- Successful source receipts in canonical L2 blocks, committed Composer ownership
  and exact settlement correlation. Receipt success alone is insufficient.
- Independent L1 receipt/block equality, the complete same-block
  `L2ExecutionPerformed` event stream and its terminal L2 hash commitment.
- KZG/native payload verification and exact signed source bytes bound to the
  settlement for every posting, using `verify-semantic-settlement-blob.sh`.
- Expected counters on both chains; every created Safe's owners, threshold and
  singleton; transfer Safe nonces and exact recipient/Safe balances. Independent
  L1 reads corroborated owners, counters and transfer effects.
- Exact L2 sender balances and nonces after gas charges. Collected test tips alone
  covered each posting's **entire** execution plus blob cost.

| Run | Sources | Postings | Collected L2 tips, xDAI | Actual posting + blob cost, xDAI |
| --- | ---: | ---: | ---: | ---: |
| Primary ramp, including canaries | 587 | 53 | 0.474230871269890515 | 0.257971915177236748 |
| Expanded burst, including canaries | 244 | 16 | 0.274510075714069322 | 0.170162063769265719 |
| Total | 831 | 69 | **0.748740946983959837** | **0.428133978946502467** |

The realized surplus over posting/blob cost is about 74.9%. Quotes target an
individual settlement estimate plus 20%; batching can reduce actual shared
cost. This does not establish total operating profit: configured fixed
proving/operations/treasury costs are zero, and setup expenses are outside
measured fee recovery.

## Settlement pressure and retained failures

The [primary burst events](burst-tail-events.json) show batches of roughly
22–25 sources reaching the **15,978,300 gas planning budget**, below the
16,777,216 per-transaction ceiling. Related successful postings used roughly
8–9.2 million receipt gas. This suggests investigating estimate/batch headroom;
it does not authorize weakening admission or final validation.

Skipped target slots and separate incomplete-inclusion observations contributed
to tail variability. The latter triggered selection backoff from 11 to 5 to 2
to 1 source around 20:38:51–20:39:06. The implementation deliberately does not
interpret a skipped slot itself as capacity feedback. The first burst's 64-sender
pool also omitted 92 arrivals. The 256-sender follow-up removed that client
constraint, but its queue still grew. L1 planning/inclusion was the observed
settlement constraint; fast quote responses did not resolve it.

Two failures were retained rather than presented as clean first-pass success:

1. Expanded **setup**, outside measurement, attempted 64 proxy creations in a
   single ordinary L2 Safe MultiSend with gas limit 16,500,000. Transaction
   `0xe6398348df8039175399eea11bd839d97b82306fe507418381b3e9f7035873a3`
   reverted after a CREATE2 ran out of gas (`GS013`). Its receipt/trace remain in
   the local evidence. Four groups of 16 succeeded. Successful funding/deployment
   steps were not duplicated and the failed hash was preserved.
2. After all expanded sources had settled, the initial independent verification
   received `null` for an L1 block lookup and exited 1. Provider heads subsequently
   agreed; the exact cause of the initial null is unproven. A bounded 60-second
   availability wait was added, keeping hash equality and every semantic check.
   **Verification only** was rerun and passed at 20:53:59.256 UTC. Original
   `run.exit=1` and `verify-failure.json` remain; `verification-retry.exit=0` is
   separate. The measured traffic was not repeated.

The primary wrapper exited 0 and verification passed at 20:44:16.076 UTC. The
standalone blob checks exited 0 for both runs. Final read-only health was healthy:
queue empty, local/independent L1 hashes equal, Beacon distance zero, runtime
identities/restart counts unchanged. A preliminary health assertion requiring
zero Beacon distance at any arbitrary instant saw distance one with all health
flags normal; that observation and the later sample are retained in
[final-health.json](final-health.json).

## Evidence and executed checks

| Evidence | Location | Availability |
| --- | --- | --- |
| Results, identities, policy, stage metrics and limits | [results.json](results.json) | Committed, sanitized |
| Settlement tail and final health | [burst-tail-events.json](burst-tail-events.json), [final-health.json](final-health.json) | Committed, sanitized |
| Reusable primary driver and metric checks | [driver](../../../scripts/chiado-fee-load.mjs), [wrapper](../../../scripts/run-chiado-fee-load.sh), [summarizer](../../../scripts/summarize-chiado-fee-load.py), [unit tests](../../../scripts/ci/tests/test_fee_load_summary.py) | Committed; pinned topology/runtime guards |
| Exact expanded workload adaptation | [prepare-expanded.mjs](prepare-expanded.mjs), [expanded-driver.patch](expanded-driver.patch) | Committed reproduction inputs; preparation spends/reuses test funds |
| Hashes and sizes of raw public metadata/log artifacts | [artifact-manifest.json](artifact-manifest.json) | Committed manifest; underlying files local-only |
| Quotes, admissions, signed envelopes, canonical receipts/events, setup failures, blob evidence and logs | Operator checkout `artifacts/chiado-fee-load-20261015/`, follow-up `expanded/` | Local-only, not available from a fresh checkout; no remote retention guarantee |
| Actor keys and owner key | Private state files in those evidence directories | Private; excluded from the manifest and repository |

Commands ran from the isolated harness checkout at the base above, using the
existing deployment checkout only for private configuration and retained
artifacts. Both checkout wallet locks were held for setup, sends and semantic
verification. The [runbook](../../runbooks/chiado-fee-load.md) records commands and
the additional independent blob verification required for the full result.
The native verifier SHA-256 was
`da99270dabdd0f2c8f3b28c5bfcdac91506149955fd40c27cdc0867b0e4b88af`.
The first measurement process preceded a verification-only public-provider rate
limit; its loaded script and the revision difference are retained in
`driver.measured.mjs` and `measurement-driver.json`. Later corrections affected
verification/setup, not the recorded offered-rate generation or runtime.

| Check | Evidence type | Result |
| --- | --- | --- |
| `forge build --root eez-core-protocol` at the initialized pin | Source build | Passed, cached outputs |
| `node --check` on primary and expanded drivers/preparation | Static | Passed |
| `bash -n` on primary/expanded wrappers | Static | Passed |
| `python3 -m unittest discover -s scripts/ci/tests -p test_fee_load_summary.py -v` | Unit | 3/3 passed; failures, omissions, success-latency and drain denominators covered |
| Quote/admission and canonical semantic checks | Live | All intended sources settled; expanded verification retry recorded above |
| Full native blob/source binding | Live independent verifier | 69/69 postings, 831/831 source envelopes passed |
| Independent L2 follower replay | Not run | Not established by this benchmark |
| Rust workspace / full CI / new Kurtosis matrix | Not run | No production Rust, SDK or runtime change in this load-test task |

## Operations and remaining limits

The task authorized test traffic and funding. One **1.1 test-xDAI** bridge funded
the primary workload; the expanded setup reused unspent actor balances and
distributed **0.384 test-xDAI** to 256 fresh senders without another bridge.
Unused keys/balances and all original evidence were retained. There was no
node/signer restart, policy change, contract-system upgrade or runtime deployment.
The dirty operator checkout and its configuration/datadirs were preserved.

Canonical-at-read evidence is not a finality guarantee. This exercise does not
cover a long production soak, a public gateway/wallet extension, Falcon crypto,
independent L2 replay, controlled reorg/restart recovery, or a matched pre-fee
baseline. Quote capacity above 100/s and sustained settlement under other mixes
remain unmeasured. A longer same-mix run with independent replay and explicit
tail-latency targets is needed before declaring a production capacity limit.
