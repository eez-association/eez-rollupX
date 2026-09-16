# Live devnet settlement-policy verification — 2026-09-13

This document records the historical validation run. Detailed run artifacts are
preserved locally under `artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/`;
generated logs, deployment snapshots, and screenshots are not part of the source distribution.

The requested deployment and verification passed. At the end of this run,
`eez-devnet` used empty-history consolidation every 150 L2 blocks and pure L2
mode `always`. It was subsequently stopped for the public Chiado deployment.
Three measured empty intervals were exactly 300 seconds; all 28 pinned protocol
network scenarios passed. A fresh follower independently reconstructed all
1,584 settled blocks and 267 transaction receipts in the evidence snapshot from
canonical L1 data.

The playground and network monitor passed checks. Deployment hostnames are
omitted from this public record. The existing
HTTPS RPCs and three explorer frontends passed checks after the composer
restart; the monitor reported matching L1 commitment and L2 safe block. See
public-health.json (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/public-health.json`, local evidence). HTTPS checks used the public hostnames
with `curl --resolve` to this host and certificate validation.

## Configuration and executable identity

The recreated enclave is `a9f374153bf644edbd20a3e07eb5ca56`, using
[`devnet-args.yaml`](../../../testing/kurtosis/devnet-args.yaml).

| Setting | Effective value |
| --- | --- |
| Settlement policy | Enabled |
| Pure L2 mode | `always` |
| General maximum unsettled history | 150 L2 blocks |
| Aggregate blob fullness trigger | `off` |
| L2 / L1 cadence | 2 / 12 seconds |
| Proof window / submission slack / bundle lead | 4,000 / 500 / 7,500 ms |
| Maximum speculative depth | 0 |
| L1 / L2 chain IDs | 7331 / 6290 |

The general rule is a block threshold: 150 × 2 seconds gives nominally five
minutes from the latest settlement. L1 inclusion determines the actual receipt
time. Included pure L2 transactions, including reverted transactions, trigger
an earlier post at an eligible opportunity. Valid cross-chain work also
triggers posting. An empty Sync block alone does not. Fullness is disabled so
it cannot shorten this profile's empty interval.

The fee profile uses the running chain's fee inputs, with explicit local PBS
settings: a 1 gwei priority floor, 100 gwei caps, 10,000 bps execution/blob fee
headroom, and a 2,000 bps execution gas margin. The active chain permits 15 blobs
per block and six per transaction; the effective transaction capacity was six.

The actual composer, signer and independent follower executables were checked
against the previous settlement-policy build before deployment. No shipping
Rust runtime changes were needed in this verification task. Image IDs, binary
SHA-256 values, actual composer environment, profile hash and verifier hashes
are in runtime.json (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/runtime.json`, local evidence). Its repository HEAD identifies the base
commit of the working tree; binary hashes identify the tested implementation.
The pinned upstream protocol commit was
`3c0cdf4955941a5c23639346932b55b33babebbd`.

## Results

| Requested behavior or correctness check | Result | Evidence |
| --- | --- | --- |
| Empty history settles every five minutes | Two consecutive 150-block / 300-second intervals before traffic; no intervening posts or included transactions in either range | Idle before traffic (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/idle-before-traffic.json`, local evidence) |
| Return to idle after traffic | Another 150-block / 300-second empty interval, spanning a composer restart | Idle after traffic (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/idle-after-traffic.json`, local evidence) |
| Always settle included pure L2 work | Five transfers in one L2 block settled together after 10 seconds; a lone reverted call and a subsequent successful call each settled after 6 seconds | Pure waves (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/pure-waves.json`, local evidence) |
| Contract deployment and state changes | Counter deployment, increment and add settled; final value 42; correlation RPCs and authenticated blob coverage passed | Pure contract (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/pure-contract.json`, local evidence) |
| Inbound, outbound and mixed waves | 40 cross-chain operations across six waves and six measured posts; six invalid calls evicted | Local evidence: `artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/wave-results.json` and `waves/` in the same archive |
| Pure work alongside cross-chain activity | All six ordinary filler transfers included exactly once, with successful canonical receipts and contiguous nonces | Wave results (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/wave-results.json`, local evidence) |
| Ordering and multiple events within one block/transaction | All nine state-chaining cases passed, including poison eviction, mixed directions and repeated identical calls in one L1 transaction | State chaining (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/state-chaining.log`, local evidence) |
| Complete pinned protocol contract suite | 410 passed, zero failed or skipped, across 26 suites | Foundry log (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/validation/protocol-foundry.log`, local evidence) |
| Fork contract suite | 19 passed, zero failed or skipped, across nine suites | Foundry log (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/validation/fork-contracts-foundry.log`, local evidence) |
| Complete pinned protocol network suite | 28/28 passed, no skips, missing cases or duplicates; independent causal and semantic blob checks complete | Protocol summary (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/protocol-summary.json`, local evidence) |
| Native blob authentication and complete posted history | 58 successful canonical posts, 59 authenticated blobs, contiguous L2 history 1–1,584 and all 267 included transactions | History receipts (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/history-receipts.json`, local evidence) |
| Independent execution from L1 | Fresh follower matched every one of the 1,584 block hashes and 267 receipt statuses, including the reverted outer transaction | Independent replay (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/independent-replay.json`, local evidence), test log (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/validation/independent-replay.log`, local evidence) |
| Restart recovery | Previously settled block preserved; scheduler reconstructed the 150-block trigger from the last canonical settlement | Restart (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/restart.json`, local evidence), decision (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/restart-decisions.json`, local evidence) |
| Blobscan agrees with canonical blob data | Indexed metadata, stored bytes and web proxy matched authenticated Beacon data | Blobscan (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/blobscan.json`, local evidence) |
| Runtime diagnostics | Zero commitment divergence events or panics; no unexpected errors after the first canonical post | Diagnostics (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/diagnostics.json`, local evidence) |

The empty settlements occurred at 16:00:21 → 16:05:21 → 16:10:21 UTC, advancing
the applied L2 endpoint 198 → 348 → 498. After the final traffic post at 16:41:33,
the next post was at 16:46:33, advancing 1434 → 1584. Each of the three measured
ranges contains only empty L2 blocks. The observer permits a limited inclusion
delay, but the actual measurements were exactly 150 blocks and 300 seconds.

The pure-mode probes were isolated from cross-chain work, and their endpoints
advanced by less than the general 150-block threshold. This prevents either
trigger from masking the `always` behavior. The five transfers shared L2 block
643 and one L1 post; the reverted-only and successful-call cases used L2 blocks
657 and 669. Reported 6–10 second latencies are measured from L2 block timestamp
to canonical L1 receipt timestamp, not guaranteed inclusion deadlines.

The network suite ran all 28 scenarios concurrently and completed its upstream
phase in 111.68 seconds. Independent verification then checked receipt outcomes,
cross-chain causality and authenticated payload semantics for every scenario.
Coverage includes bridges, ordinary calls in both directions, nested calls,
flash loans, reentrancy, static calls, multiple calls and transactions, and
reverts. The state-chaining checks additionally require exact ordered return
values, same-Sync placement, signer validation, and L1/L2 commitment equality.
In the repeated-call case, `postBatch` and the source transaction both landed
in L1 block 224.

The complete-history snapshot is L1 block 281,
`0x1380b4fb3fad55a6ca0efc9c33451aa0e9ab6458f7486babf4df413831c3daab`.
Its applied L2 endpoint is block 1584,
`0x3622bfaa5d6a9c0d6637a4bab534b7e36342be97052c10050d29c209c02ec585`.
The history collector authenticated every post against its canonical receipt
and Beacon/KZG data, checked there were no missing block heights, and matched
every included transaction to decoded data availability. The follower then
executed this history from genesis using only L1 RPC, Beacon data and deployment
configuration; it received no composer database or unsafe execution overlay.
The network continued producing and settling blocks after this fixed snapshot.

## Issues found and resolved

1. **UI deployment binding.** The pinned flash-loan fixture exports
   `PREDICTED_WRAPPED_TOKEN_L2`; `deploy-ui-demo.sh` referenced the old
   `WRAPPED_TOKEN_L2` variable and failed during the first deployment. The script
   now uses the deterministic address, and the corrected deployment image
   `eez-deploy:settlement-policy-devnet` completed a fresh full deployment.
2. **Blobscan verifier argument size.** A complete hex-encoded 131,072-byte blob
   exceeds Linux's per-argument limit. The verifier now supplies indexed bytes
   to `jq` through a file. Full-size regression fixtures cover both Beacon
   response layouts and reject mismatched bytes. The complete native blob and
   explorer check then passed.
3. **State-chaining fixture boundary.** Its original wait required a held-pool
   drain event, which the enabled policy's empty-Sync path does not emit. The
   fixture now observes committed Sync-block production. Regression tests
   require this signal and reject Live-block or pool-drain events alone. All
   original state, receipt, ordering and signer assertions remain. The complete
   nine-case state-chaining suite passed after the fix.

The initial cross-chain wrapper completed all four wave modes but failed at the
old state-chaining boundary wait. Its failure log is retained, and the corrected
full state-chaining suite was run separately. The replay test was also extended
to compare explicit recorded receipt statuses, allowing the intentional reverted
pure transaction to be verified without assuming every outer receipt succeeds.

Recreating the enclave detached the existing standalone gateway and monitor
from their Docker network. Both were reconnected to `kt-eez-devnet`; the monitor
was restarted to clear the previous chain's cache. Existing certificate volumes
and routes were preserved. The recovery procedure is documented in the
[gateway README](../../../testing/public-gateway/README.md).

During the composer restart, Docker changed the exposed L2 RPC from port 36254
to 36268. The original observer and readiness probe timed out on the old port;
the runtime had restarted successfully. After resolving current Kurtosis ports,
verification checked the preserved settled block and the complete historical
empty interval against canonical receipts. The restart helper now refreshes
ports automatically. `observer_reconnected_at` in `restart.json` is the observer
recovery time, not the runtime startup time.

The restart was requested at 16:46:17.094810, and the replacement runtime started
at 16:46:17.494973 UTC. At 16:46:27, its fresh policy decision used canonical L2
anchor 1434, 150 unsettled empty blocks and target L1 timestamp 16:46:33. The
single ERROR-level message after the first canonical post was the scheduler's
head source closing during this requested shutdown; its exact timestamp and
classification are retained in `diagnostics.json`.

## Validation and reproduction

The following checks passed in this task:

```bash
forge test --root eez-core-protocol --no-match-path 'script/**'
forge test --root contracts
forge build --root eez-core-protocol
cargo build --locked -p eez-l1 --example verify_blob_evidence
cargo build --locked -p eez-protocol --example e2e-causal-verify
cargo fmt --all --check
git diff --check
bash -n scripts/deploy-ui-demo.sh \
  testing/kurtosis/scripts/verify-blobscan.sh \
  testing/kurtosis/scripts/verify-state-chaining.sh \
  testing/kurtosis/scripts/verify-policy-replay.sh
cargo clippy --locked -p eez-node-e2e --test kurtosis_policy_replay -- \
  -D warnings -A clippy::duration_suboptimal_units
python3 -m unittest \
  scripts.ci.tests.test_blobscan_evidence \
  scripts.ci.tests.test_sync_boundary \
  scripts.ci.tests.test_kurtosis_ci.CiTimingTest \
  scripts.ci.tests.test_kurtosis_ci.SettlementEnvironmentTest \
  scripts.ci.tests.test_kurtosis_ci.BlobExplorerSelectionTest
```

The Python selection passed nine tests. The Clippy allowance is the existing
toolchain compatibility allowance for duration units. Live checks used the
existing `verify-eezl2-deployment.sh`, `verify-blob-da.sh`,
`verify-cross-chain-waves.sh`, `verify-state-chaining.sh`, `run-protocol-e2e.sh`
and `verify-policy-replay.sh` infrastructure. The protocol suite used
`EEZ_PROTOCOL_SCENARIOS=all`, `EEZ_PROTOCOL_MAX_PARALLEL=all`, and native causal
and blob verifier binaries. The waves enabled invalid-call probes and three
mixed/pure waves. The actual pre-restart URLs are recorded in the raw logs.

For another check against this running enclave, refresh endpoints through
`KURTOSIS_ENCLAVE=eez-devnet` and `testing/kurtosis/ports.sh`; host ports are not
stable across restarts. Use a new output directory to preserve these results.
The small helpers alongside this report have the following roles:

| Helper | Operation |
| --- | --- |
| `verify-idle.py` | Read-only canonical settlement observer; accepts a recorded `--anchor-l1-tx` to verify a historical interval |
| `check-pure.py` | Sends the three pure-L2 probes using the public local fixture account; requires the deployed Counter address |
| `check-mixed-fillers.py` | Accounts for the six transfers in the three mixed/pure fixture waves |
| `collect-history.py` | Reads and authenticates complete settled history, producing native replay input |
| `restart-and-check.py` | Restarts the recorded composer and refreshes its RPC port; `--verify-existing` only resumes observation |

For independent replay, the executed command was:

```bash
export EEZ_TEST_PREBUILT=1
export EEZ_TEST_RUNTIME_BIN_DIR="$PWD/target/policy-native-release"
export EEZ_TEST_PROOF_SIGNER_BIN="$EEZ_TEST_RUNTIME_BIN_DIR/eez-proof-signer"
# Set EEZ_L2_SYSTEM_KEY to the public fixture value in devnet-args.yaml.
bash testing/kurtosis/scripts/verify-policy-replay.sh \
  artifacts/devnet-policy-20260913/history-final/summary.json
```

The complete raw output is in `artifacts/devnet-policy-20260913/`, including
failed attempts, deployment logs, all 28 scenario logs, receipt/causality/semantic
evidence, every native blob input and decoded result, runtime logs and build
logs. The permanent raw evidence manifest (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/raw-evidence-sha256.json`, local evidence) records
selected raw file hashes and sizes; SHA256SUMS (`artifacts/audit-evidence/2026-09-13-settlement-policy-devnet/SHA256SUMS`, local evidence) covers this curated
report bundle. Large blob inputs stay in the local artifacts directory. Private
generated scenario account files are not part of this evidence bundle.

## Measurement scope

This run exercises the requested `always` / 150-block configuration on the
local Ethereum Kurtosis devnet. The first relay inclusion required validator
registration; startup attempts are excluded from steady-state idle timings.
The wave fixture also required one isolated public-account L2 self-transfer to
refresh its builder warmup before the inbound wave baseline. Its receipt is
retained in `inbound-builder-warmup.json` in the raw output and is excluded from
the six measured wave posts. The bootstrap dropped-bundle count remained
unchanged throughout the traffic waves.

The tests authenticated a two-blob startup post and subsequent one-blob posts;
they did not saturate the six-blob transaction limit in this run. Recorded costs
are receipts from a subsidized local PBS fixture, not Ethereum mainnet price
estimates. Other policy modes, transport limits and network profiles have
separate evidence in the
[implementation audit](../2026-09-13-settlement-policy/README.md).
