# Chiado 6291 staged performance workload

Status: completed successfully, exit 0; 870/870 source transactions have successful
canonical receipts. The operator requested `scripts/run-chiado-staged-performance.sh`
on the current network. The unchanged helper selects 14 scenarios, 770 jobs and
870 source transactions at scale 1. It uses `--no-verify`; this run measures
execution/receipt progress and posting outcomes, not scenario semantic correctness.
The separate [32-case protocol suite](../2026-09-23-events-staged-all/README.md)
completed semantic validation on this deployment before this workload.

## Identity and settings

Fork base `1832f970884ca9460646a4cde87cca70eaffb3e1` plus the recorded local
feature/events port and settlement/timing work; clean protocol checkout
`820e50f749f63165be6a5797d4c5fe52cbbbbe34`. L1 10200, L2 6291,
rollup 1 under registry `0x5d6b8fc56e43528c219de7eed257564c05923e08`.
Runtime images and operational configuration were not changed for this run.

Foundry 1.7.1 / Solidity 0.8.34; local L1/L2 RPCs and existing Composer fronts.
Preparation workers 4, send workers 2, 1.5-second pause per worker, MultiSend
batch 220, setup receipt timeout 1800 seconds and source receipt timeout 600
seconds. The owned runner has a 6 GiB memory/swap-total limit, 2 CPUs and a
512-process limit. The helper retains its full-run wallet lock. This matches the
lighter scheduling limits of the prior historical-target repeat, but protocol,
network state and RPC location differ; it is not a controlled performance
comparison. No database copies or service restarts were requested or performed.

## Readiness correction

The first preflight correctly refused to start because a non-workload outbound
request was blocked on rollup escrow. Transaction
`0x94253911fdcf4642a06fd0c640464fe004d81ae0f190435d71e92b82ce16839c`, sender
`0x0aEbf98eAC89b28982690F8643Bcf34bcDc710Ea`, nonce 2, required
1,000,000,000,000,000 wei; the escrow held only 10,000,000,001,000 wei.
The sender is not in the test worker pool. The validation was not bypassed.

A 0.001 test-xDAI inbound deposit from the existing public development funding
account replenished rollup escrow through the deployed minimal bridge. Deposit
`0x48e82c430b8b65d48cb5f9e68b71f32ed29eabdd2753d0ff5f8f3d217e8cca86`
and the previously blocked withdrawal both succeeded, and the queue emptied.
The second preflight passed queue, source/worker nonce alignment, funding,
chain identities, fresh heads and deployed-code checks. L1 faucet funds sufficed;
the genesis-funded source topped up the L2 faucet for new worker allocations.
No user transaction was discarded and no active poster nonce was used.

## Evidence and limits

Private local operation records: `artifacts/events-performance-20260923/` in the
active source checkout. Run files:
`eez-core-protocol/tmp/e2e-staged-net/20260923-200859/`. Runtime/runner image IDs,
initial restart counts and settings are in `identity.json`; resource/queue samples
are collected about every 20 seconds. Logs, worker keys and profiles remain
private and are not committed. Ignored local artifacts have no external retention
guarantee. Final sanitized results, exact identities, phase timings, resource observations
and evidence hashes are in [results.json](results.json).

## Completed results

- All **770/770 jobs prepared**, with no preparation or deployment-send failures.
- All **2,850/2,850 setup transactions** and **870/870 source transactions** have
  status 1 receipts whose block hashes match the current canonical chains.
- Source split: **520 L1-origin / 350 L2-origin**. All 350 L2 sources are covered
  by L2 safe block **4625**. At the independent check, L1 safe block 23197149
  covered 508 of the 520 L1 sources; the last source was in canonical block
  23197151. Full L1 finality is not claimed by the receipt check.
- Every source transaction hash is unique and each of the 770 jobs sent the
  exact source count in its manifest. No resend/resume run was needed. The final
  cross-chain queue is empty: zero queued, in-flight, blocked or unknown.
- Total runner time **1,253.721 seconds** (20m 54s rounded); source send phase
  **664.928 seconds** and final receipt monitor **7.133 seconds**. Preparation
  took about 390 seconds, setup broadcast 28 seconds and setup mining 137 seconds.
  Exact per-phase measurements are in `results.json`.
- No Composer or Signer restarts occurred; the runner exited 0 without OOM.
  Across 56 samples about 20 seconds apart, host available memory stayed above
  **39.89 GiB**. Sampled memory maxima are observations, not guaranteed peaks
  between samples. This run did not reproduce the prior host resource exhaustion.

## Posting outcomes

The source window runs from the measured send-phase start until the final
canonical receipt check, so it includes the settlement tail after the runner
first sees receipts. These are **network-wide posting attempts** in that window;
concurrent public traffic is not excluded.

| Outcome | Source window | Entire run and settlement tail |
| --- | ---: | ---: |
| Included at target | 114/136 (83.82%) | 119/142 (83.80%) |
| Network slot skipped | 12/136 (8.82%) | 12/142 (8.45%) |
| Block produced without inclusion | 10/136 (7.35%) | 11/142 (7.75%) |
| Preparation failed | 0 | 0 |

All 142 collected attempt histories have intact event sequences. Skipped-slot
classification uses adjacent canonical headers bracketing the absent timestamp;
it is not inferred solely from a timeout. Produced-block exclusion means a
canonical block exists but did not include that attempt; this run did not inspect
builder/proposer traces to assign its cause. The retries left no workload source
unsettled. This result does not prove every possible consecutive-skip/reorg case.

No fresh semantic verification or independent follower replay was run by this
performance helper. The preceding full protocol suite remains separate evidence;
successful receipts here do not replace its scenario assertions. The older
network/protocol run is not a controlled baseline for performance improvement.

## Scenario inventory

| Scenario | Jobs | Successful source transactions |
| --- | ---: | ---: |
| `counter` | 100 | 100 |
| `nestedCounter` | 50 | 50 |
| `multi-call-nested` | 50 | 50 |
| `multi-call-twice` | 50 | 50 |
| `counter-multi-tx` | 50 | 150 |
| `reentrant` | 20 | 20 |
| `revertCounter` | 50 | 50 |
| `revertFromOtherChain` | 50 | 50 |
| `counterL2` | 100 | 100 |
| `nestedCounterL2` | 50 | 50 |
| `multi-call-nestedL2` | 50 | 50 |
| `multi-call-twiceL2` | 50 | 50 |
| `revertCounterL2` | 50 | 50 |
| `revertFromOtherChainL2` | 50 | 50 |
