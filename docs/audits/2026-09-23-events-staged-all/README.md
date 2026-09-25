# Full pinned-protocol staged suite on Chiado 6291

Status: completed; the full run passed **32/32** with semantic verification.
Requested command: `bash script/e2e/run/network-staged.sh all`.
This is the unchanged upstream runner at protocol
`820e50f749f63165be6a5797d4c5fe52cbbbbe34`, with semantic verification enabled.
The `all` target selects exactly 32 scenarios, once each, including the four new
read/write/read cases. No scenario is excluded and `--no-verify` is not used.

## Network and controls

The [new official network](../2026-09-23-events-network/README.md) uses L1 10200,
L2 6291, rollup 1 under registry
`0x5d6b8fc56e43528c219de7eed257564c05923e08`. Its locally validated release
Composer and Signer were not rebuilt or restarted for this test. Protocol sources
were clean before the run; the parent fork has the previously recorded local port.

Foundry 1.7.1 and Solidity 0.8.34 are used with the protocol’s own compiler settings.
Preparation concurrency is 2, verification concurrency 2, send workers 4 with
0.25-second pacing per worker, and funding balance workers 4. This is a functional
validation run, not a performance comparison. The runner’s full-run wallet lock
is held at `eez-core-protocol/tmp/.eez-network-e2e.lock` in addition to its internal
faucet lock. There were no competing upstream runners. The public fixture source
and existing faucet fund the isolated workers; the active poster nonce is unused.

Preflight verified chain IDs, synchronized/fresh RPC heads, deployed managers,
an empty cross-chain queue and aligned latest/pending/ingress nonces for the 32
workers plus faucet/source. Existing L1 faucet funds suffice; fresh L2 workers
are funded from the genesis-funded fixture account. Operator configuration,
service databases and protocol test assertions are unchanged.

## Evidence

Local-only private operation records are in
`artifacts/events-staged-all-20260923/` in the active source checkout. Raw per-case
logs/manifests and timing/receipt CSVs are in
`eez-core-protocol/tmp/e2e-staged-net/20260923-214411/` (complete run), with the
initial failed setup retained at `20260923-214019/`. Wallet/profile files contain
credentials and must not be committed. These ignored artifacts are unavailable
from another checkout and have no external retention guarantee. Sanitized identities, every scenario’s semantic check markers, canonical source
receipts and log hashes are retained in [results.json](results.json).

## Results and retained first failure

The unchanged pinned runner completed from **19:44:11 to 19:45:12 UTC**:

- **32 passed, 0 failed, 0 not verifiable**, exit 0; no skipped scenarios.
- **132 setup transactions** and **34 source transactions** were mined.
- The runner verified matching L1 settlement entries/calldata, L2 tables/calls
  and scenario-specific state assertions as applicable. It did not merely count
  receipt successes. Per-scenario check markers are retained in `results.json`.
- The four new cases (`staticReadWrite`, `staticReadX2Write`, and both `L2`
  counterparts) passed, including duplicate reads around a real state change.
- A subsequent independent RPC check refetched all 34 source receipts, required
  status 1 and matching canonical block hashes, and checked the safe heads.
  All L2 source transactions were covered by L2 safe state; the post-run
  cross-chain queue had zero queued, in-flight, blocked or unknown requests.
- Preparation and verification both used concurrency 2. The complete run took
  60.937 seconds with warm compiler artifacts and already funded wallets. This
  elapsed time is not a baseline-comparable performance measurement.

The **initial attempt failed**: public `rpc.chiadochain.net` returned HTTP 429
while planning five cases and sending deployment transactions for another 22.
Only five cases became verifiable, and those five passed. Its overall result
remains **5 passed / 27 not verifiable**, exit 1. That attempt is not counted as
full coverage or silently converted to a pass. Its logs and partial deployments
were preserved. No test assertions or network binaries were changed.

After checking that the embedded L1 RPC returns transaction-by-hash and receipt
lookups for these newly indexed transactions, the complete suite was run afresh
against that local endpoint. Every worker’s pending nonce matched its canonical
nonce before starting. The second run had no preparation failures, and all 32
cases passed; this was an RPC configuration correction, not a runtime code fix.
The existing operational indexer’s separate enrichment endpoint was not changed.

## Scope and limits

This covers every scenario selected by this protocol revision’s `all` target:
ordinary calls and native bridges in both directions, repeated/different calls,
multiple source transactions, nested/deep calls, flash loans, reentrancy, handled
reverts and static/read-write-read calls. It is not a proof of every possible
contract interaction, sustained load, adversarial soundness or restart recovery.
No fresh independent follower was started for this live run; exact-release
independent replay was previously checked in the deployment’s local process test.
Protocol/runtime source, deployed images and databases were not changed or copied.
Test contracts and funding transactions remain on-chain as expected.

## Per-scenario outcome

| Scenario | Outcome |
| --- | --- |
| `multi-call-twice` | PASS |
| `multi-call-two-diff` | PASS |
| `multi-call-twiceL2` | PASS |
| `multi-call-two-diffL2` | PASS |
| `counter-multi-tx` | PASS |
| `deepNested` | PASS |
| `flash-loan` | PASS |
| `multi-call-nested` | PASS |
| `nestedCounter` | PASS |
| `multi-call-nestedL2` | PASS |
| `nestedCounterL2` | PASS |
| `bridge` | PASS |
| `counter` | PASS |
| `bridgeL2` | PASS |
| `counterL2` | PASS |
| `reentrant` | PASS |
| `nestedCallRevert` | PASS |
| `revertCounter` | PASS |
| `revertFromOtherChain` | PASS |
| `nestedCallRevertL2` | PASS |
| `revertCounterL2` | PASS |
| `revertFromOtherChainAndCallAgainL2` | PASS |
| `revertFromOtherChainL2` | PASS |
| `revertFromOtherChainNested` | PASS |
| `nestedStaticCounter` | PASS |
| `staticReadWrite` | PASS |
| `staticReadX2Write` | PASS |
| `topLevelStaticCounter` | PASS |
| `nestedStaticCounterL2` | PASS |
| `staticCounterL2` | PASS |
| `staticReadWriteL2` | PASS |
| `staticReadX2WriteL2` | PASS |
