# Chiado deployment and settlement validation — 2026-09-13

This document records the historical validation run. Detailed run artifacts are
preserved locally under `artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/`;
generated logs, deployment snapshots, and screenshots are not part of the source distribution.

Deployment and validation passed. Chiado remains running with empty-history
consolidation due every 1800 L2 blocks (nominally thirty minutes), pure L2 mode
`always`, and a 20% margin on the network priority-fee estimate. The complete
history snapshot contains 77 posts, 91 authenticated blobs, 5770 L2 blocks and
294 receipts. A fresh follower reproduced every block hash and receipt outcome,
and the final post and matching L2 endpoint reached finality.

The previous `eez-devnet` enclave was stopped at 21:55:56 UTC with
`kurtosis enclave stop eez-devnet`; its containers and chain data were preserved.
The fresh deployment uses Chiado L1 chain ID 10200 and EEZ L2 chain ID 6290.
The existing HTTPS gateway served this deployment through the monitor and
application. Deployment hostnames are omitted from this public record.

## Deployment and policy

| Component | Fresh binding |
| --- | --- |
| Registry | `0x2fbd78894e327a38b69a3c2f539b92d926610331` |
| Registry deployment block | 23043686 |
| Proof contract | `0xddf60080c3451613953727b1c0d5722ac6ed8016` |
| Manager | `0x9bfd73e7109993f3db26a39d8668df27756f5595` |
| Rollup ID | 1 |
| Genesis hash | `0x3f15022d42399129fa675b7458eda62afa2330e65e06e753602eb218440edb9d` |
| Genesis state root | `0xf652af83d311e9a3b3895ee73701744066eee209e7d618f80691311d0b915993` |

The deployment uses the implemented settlement-policy and network-fee-margin
binaries. No new compression, bloom omission or execution-derived header
encoding was implemented or deployed in this task.

| Setting | Five-minute validation profile | Active final profile |
| --- | --- | --- |
| Policy enabled | `true` | `true` |
| Pure L2 mode | `always` | `always` |
| Maximum unsettled L2 blocks | 300 | 1800 (active since 23:14:36 UTC) |
| Aggregate fullness trigger | `off` | `off` |
| L2 block time | 1000 ms | 1000 ms |
| Chiado L1 slot time | 5000 ms | 5000 ms |
| Proof window / slack / bundle lead | 1500 / 700 / 7500 ms | unchanged |
| Maximum speculative depth | 0 | 0 |
| Signer window guard | 2048 blocks | 2048 blocks |
| Effective blob allowance | 2 per transaction and block | 2 per transaction and block |
| Priority fee floor | 1 gwei | 1 gwei |
| Network priority estimate margin | 2000 bps (20%) | 2000 bps (20%) |
| Execution / blob fee caps | 20 / 10 gwei | 20 / 10 gwei |

Chiado's native timing makes 300 blocks five minutes and 1800 blocks thirty
minutes. The clock starts at the latest canonical settlement. Transactions can
trigger earlier posting, and empty Sync blocks alone do not. The block rule
does not promise inclusion at an exact wall-clock instant: the public builder
can exclude a bundle and the composer retries.

Unlike the earlier private devnet, Chiado permits only two blobs in this
profile. A thirty-minute empty range therefore needs capacity-limited posts
after the general threshold becomes due. Raising the signer guard cannot
raise that network blob allowance.

## Traffic and protocol results

| Check | Result |
| --- | --- |
| Fork contract tests | 19 passed, nine suites |
| Pinned protocol contract tests | 410 passed, 26 suites |
| Complete protocol network suite | 28/28 passed, zero missing, duplicated or skipped scenarios; upstream execution 143.07 seconds |
| Independent protocol evidence | Canonical receipts, causal execution and authenticated semantic blob coverage for all 28 scenarios |
| Cross-chain waves | Six waves: inbound, outbound, mixed and three mixed/pure; 40 valid operations, six poison calls evicted and ten measured posts |
| Pure fillers during mixed waves | Six unique successful receipts with contiguous nonces |
| State chaining | Nine cases passed, including same-Sync ordering, exact returns, poison recovery, mixed directions and repeated identical calls within one L1 transaction |
| Isolated pure transfers | Five transfers in one L2 block; one post; four seconds to L1 inclusion |
| Isolated reverted pure call | One status-zero receipt; independently triggered one post; nine seconds to inclusion |
| Successful call after revert | Counter increment succeeded and settled in four seconds |
| Traffic history | 69 posts / 75 authenticated blobs; all 3255 L2 blocks and 294 receipts, including the intentional revert |
| Fresh follower replay of traffic | All 3255 block hashes and 294 receipt statuses matched, from L1 and genesis only; 18.66 seconds |
| Public-network harness regressions | 22 tests passed, including typed/legacy raw transaction recovery and safe receipt movement after bundle exclusion |
| Empty five-minute consolidation | Two consecutive 300-block / 300-second intervals, each with one two-blob post and no intervening traffic or posts |
| Policy restart | Canonical L1, safe/latest L2 hashes, counter value, account nonce and historical receipt preserved |
| Protocol evidence after restart | All 28 scenarios independently reverified in 29.676 seconds |
| Thirty-minute consolidation | Due at 1800 blocks; six two-blob posts applied 1915 empty blocks, including work produced during draining; first inclusion 50 seconds after due, completion 120 seconds after due |
| Finite drain completion | No further post while L1 and L2 continued advancing after the due range was covered |
| Complete final DA history | 77 canonical posts, 91 KZG-authenticated blobs, contiguous L2 history 1–5770 and all 294 receipts |
| Final fresh follower replay | Every block hash and receipt status matched from L1-only inputs; 26.14 seconds |
| Finality | L1 finalized through 23044732; L2 finalized through 5770; registry commitment and L2 hash agree |
| Final receipt fee checks | 107 network quotes; 61 included posts and 51 companion receipts in captured cost telemetry; 14 retained excluded observations and five observation retractions; no double charges |
| Fee-observer regressions | Five tests covering delayed observations, future/orphaned receipts, duplicate charges and retracted report replacement |

The pure probes use ranges shorter than the 300-block general threshold and
contain no cross-chain system transactions, so neither rule can mask the
`always` trigger. Latencies above are observed receipt timestamps, not inclusion
guarantees. Full-history verification checks both KZG data availability and
canonical L2 block/transaction coverage; the follower independently executes
the same history without the composer's database or unsafe execution overlay.

## Timing, fees and final evidence

The two five-minute cycles advanced the applied endpoint from 3255 to 3555 to
3855 at 23:04:05, 23:09:05 and 23:14:05 UTC. Every block in both measured
ranges was empty. Each cycle used one post containing two blobs. See
idle-five-minutes.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/idle-five-minutes.json`, local evidence).

After the configuration restart, the thirty-minute general rule became due for
L2 endpoint 5655 at target L1 time 23:44:05. No intervening empty post occurred.
One initial preparation missed its submission window, and several bundle
attempts were excluded by the public builder. The first post landed at
23:44:55; subsequent applied endpoints were 4185, 4515, 4845, 5175, 5505 and
5770, with the final post at 23:46:05. All six posts used both available blobs.
The last post included additional blocks produced during the drain, for a total
of 1915 empty blocks. This is measured completion at 32 minutes, not a claim of
exact thirty-minute inclusion. The finite drain then stopped. See
idle-thirty-minutes.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/idle-thirty-minutes.json`, local evidence),
drain-stopped.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/drain-stopped.json`, local evidence) and restart.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/restart.json`, local evidence).

The complete-history snapshot is L1 block 23044734,
`0xeccfba65e8e93e029220bc2b70deb6fda3ae706e7c51321bc1f3db27018181b1`.
Its applied L2 endpoint is 5770,
`0x3a4b606d0855e4611b9d58440228ad844f1c6c6473d65f1caa1ae371adf7c38f`.
The independent follower matched all 5770 blocks and 294 receipts. Both the
last post and that L2 endpoint were finalized by 23:49:04 UTC. See
history receipts (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/history-receipts.json`, local evidence), independent replay (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/independent-replay.json`, local evidence),
replay log (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/validation/independent-replay.log`, local evidence) and finality (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/finality.json`, local evidence).

| Receipt scope | Execution cost (test xDAI) | Blob cost (test xDAI) |
| --- | ---: | ---: |
| Each measured five-minute empty post | 0.000113305000793135 | 0.000262144 |
| Six posts completing the thirty-minute drain | 0.000679770004758390 | 0.001572864 |
| All 77 posts in the complete-history snapshot | 0.031835356610226163 | 0.011927552 |

These are actual Chiado receipts, not mainnet price estimates. The table covers
posting transactions only. The separate captured runtime cost ledger includes
61 of those posts plus 51 companion receipts, totaling
0.035895155571666964 test xDAI. Its earlier-log coverage is incomplete, so that
subtotal must not be treated as the cost of all 77 bundles. Contract deployment
and actor funding are separate. The 107 checked quotes include network-derived
priority estimates both below the floor and at 1 gwei; the latter become
1.2 gwei after the configured 20% margin. See fees.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/fees.json`, local evidence).

Repeated excluded attempts with the same post hash caused five older cost
observations to be retracted and replaced. These are accounting revisions,
not evidence of five chain reorganizations. The final ledger checks canonical
receipt ancestry and companion ordering and charges each paid transaction
once. Diagnostics (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/diagnostics.json`, local evidence) retains all classified errors and the
observed builder exclusions.

The complete protocol results are in protocol-summary.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/protocol-summary.json`, local evidence)
and the post-restart recheck (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/protocol-after-restart.json`, local evidence). Additional evidence
includes pure transactions (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/pure-waves.json`, local evidence), waves (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/wave-results.json`, local evidence),
state chaining (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/state-chaining.log`, local evidence), runtime identities (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/runtime.json`, local evidence), and
normal-DNS HTTPS health (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/public-health.json`, local evidence).

## Issues found and resolved

1. **Snapshot/client compatibility.** The older standalone snapshot-sync image
   could not consume the current Gnosis checkpoint. Sync used the pinned
   `reth_gnosis` revision `7e588a7b` and Gnosis's legacy state-root mode, followed
   by the existing embedded client. A fresh Lighthouse checkpoint replaced the
   old consensus snapshot. Failed attempts and original data copies remain.
2. **Signer health command.** The unified image does not contain `nc`. Its
   deployment healthcheck now tests the actual gRPC listening port using Bash
   TCP support. The signer process and all live attestations were verified.
3. **Missing diagnostic APIs.** The public-network test profile now explicitly
   enables L2 `debug` and `trace`, matching the devnet's independent prefix
   verification requirements. State and canonical hashes survived the restart.
4. **Pruned transaction lookup.** The inherited snapshot pruned transaction
   indexes, making historical receipts disappear after in-memory caches were
   cleared. With the client stopped and backed up, the transaction lookup index
   was rebuilt from the deployment boundary through the persisted head. Future
   transaction lookup and receipts are retained. Existing sender pruning and
   account/storage/body history distance remain unchanged; this is not an
   archive conversion and does not restore pre-deployment pruned history.
5. **Sender-retention correction.** An initial attempt also removed sender
   pruning, which conflicted with the snapshot's existing static-file cursor
   and stopped persistence. The error and L1 head lag were detected; the node
   was stopped and its original sender-pruning setting restored. Subsequent
   head progress and historic receipt queries were rechecked. The interrupted
   mixed wave was rerun successfully.
6. **Raw transaction evidence.** If mined raw-by-hash RPC data is absent, the
   verifier recovers the typed or legacy envelope from its exact raw-block
   position and requires the expected transaction hash. Full blocks use
   stdin/files to avoid operating-system argument limits. Native semantic/KZG
   assertions remain mandatory.
7. **Idle bootstrap assumption.** Wave readiness accepts an existing canonical
   post. It no longer requires an extra idle post that may be intentionally
   delayed for thirty minutes. Every measured wave still gets a fresh baseline.
8. **State-chaining fixture assumptions.** L1 log queries now begin at this
   deployment, avoiding pruned older history. L2 receipt/log positions are
   recorded only once safe and canonical: excluded bundles can correctly move
   transactions to a later Sync block. The original exact ordering, return,
   same-Sync and signer assertions remain, and all nine cases passed together.
9. **Delayed fee observation.** One post was included at L1 23044238 and
   accounted for at canonical observation 23044239. The runtime's receipt cost
   was correct. The audit helper now distinguishes observation from inclusion,
   checks receipt ancestry and ordering, and rejects duplicate active charges.
   Regression probes reject future and orphaned receipts and double charges.
10. **Public HTTPS routing.** The initial host-network gateway worked through
   loopback but timed out through normal public DNS. The existing UFW profile
   allowed the UI proxy ports while relying on Docker publishing for 80/443.
   Restoring Docker-published HTTP/HTTPS and forwarding through the existing
   host UI proxy restored normal-DNS HTTPS. No firewall rules or node settings
   were changed. Certificate volumes and the old gateway were preserved.
11. **Monitor currency spelling.** Its configuration requires `XDAI`; correcting
   the initial mixed-case value restored healthy native-currency reporting.

After the sender-retention correction, eight traffic proof attempts reached
an opportunity cutoff. Each error explicitly retained the complete range and
source ownership; subsequent retries and final receipt/replay checks passed.
These events are recorded as retryable deadlines, not erased or described as
an error-free run. The retained logs contain no panics or commitment divergence.

The original protocol wrapper's on-chain phase passed all 28 scenarios, but its
first independent-evidence phase failed on unavailable RPC history/APIs. Those
failed results are retained. `protocol-complete/summary.json` records the final
passing independent verification, bound to the original runner log hash. No
scenario was removed or weakened to obtain the final result.

## Runtime and reproduction

Runtime files are under `.runtime/chiado-settlement-20260913/`; generated private
inputs are in its mode-0700 `private/` directory and are not published. The
checkout's original `.env`, `.env.chiado` and `deployments.env` remain unchanged.
Always select the new deployment explicitly:

```bash
docker compose -p eez-chiado-validation \
  --env-file .runtime/chiado-settlement-20260913/private/runtime.env \
  -f .runtime/chiado-settlement-20260913/runtime.compose.json ps

docker compose -p eez-chiado-public \
  -f .runtime/chiado-settlement-20260913/public/compose.json ps
```

The node uses persistent L1 and L2 bind mounts; changing policy requires
recreating the node with the updated environment, not redeploying contracts.
Update the monitor's displayed policy in its public Compose file too. The
pre-Chiado gateway, old enclave and old monitor remain stopped and preserved.

The public endpoints are `/rpc/l1`, `/rpc/l2`, `/composer/l1` and `/composer/l2`.
Host-local ports are 18645, 18688, 18999 and 18998 respectively; the Beacon API
is port 6152. The UI uses the fresh registry and standard wallet mode. Public
devnet demonstration keys are not exposed or funded on Chiado.

Native blob verification is enabled. No separate Chiado Blobscan or L2
Blockscout was deployed, so optional explorer-backed blob decoding and the old
private-network demo fixtures are outside this deployment. The monitor links
to Chiado's public L1 Blockscout. HTTPS checks pass both with the public hostname resolved
to loopback and through normal public DNS from the deployment host, with
certificate verification. This is not an independent off-host WAN probe.

Raw outputs are retained under `artifacts/chiado-validation-20260913/`. The
existing network scripts were reused with explicit deployment/RPC inputs; see
the [public-network test instructions](../../../testing/kurtosis/README.md).
Test funding uses a separate generated account, initially funded with five
test xDAI and ten genesis L2 units. Funding transfers and contract deployment
costs are separate from settlement fees.

The executable build profile is optimized `dev`, not a release build. Exact
running image/binary identities are in runtime.json (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/runtime.json`, local evidence). Generated
public deployment bindings and Compose/genesis files are under `deployment/`.
SHA256SUMS (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/SHA256SUMS`, local evidence) covers the curated bundle, and the
raw evidence manifest (`artifacts/audit-evidence/2026-09-13-chiado-settlement-policy/raw-evidence-sha256.json`, local evidence) identifies retained local
artifacts, including failed attempts. Configured private keys and JWT material
are excluded and checked before this bundle is assembled. Public-builder
inclusion variability and long-term snapshot retention still warrant
operational monitoring beyond this finite test run.

## Validation commands

The live checks used the existing repository infrastructure with the generated
Chiado deployment file, the four explicit endpoints, the deployed native
genesis/timing profile and private test-account inputs. The small audit wrappers
in `artifacts/chiado-validation-20260913/` retain timestamps and exit codes and
redact the test key from child output.

```bash
forge test --root contracts
forge test --root eez-core-protocol --no-match-path 'script/**'
cargo test --locked --offline -p eez-node-e2e \
  --test kurtosis_policy_replay --no-run

python3 artifacts/chiado-validation-20260913/run-check.py deployment
python3 artifacts/chiado-validation-20260913/run-check.py blob
python3 artifacts/chiado-validation-20260913/run-check.py protocol
# Wave runs select inbound, outbound, mixed, and mixed-pure through
# EEZ_WAVE_MODE / EEZ_WAVE_COUNT, with EEZ_INCLUDE_REVERTS=1.
python3 artifacts/chiado-validation-20260913/run-check.py \
  state-chaining state-chaining-canonical
python3 artifacts/chiado-validation-20260913/reverify-protocol.py \
  protocol-after-restart

python3 -m unittest \
  scripts.ci.tests.test_public_network_checks \
  scripts.ci.tests.test_sync_boundary \
  scripts.ci.tests.test_blobscan_evidence \
  scripts.ci.tests.test_kurtosis_ci.CiTimingTest \
  scripts.ci.tests.test_kurtosis_ci.SettlementEnvironmentTest \
  scripts.ci.tests.test_kurtosis_ci.BlobExplorerSelectionTest
python3 artifacts/chiado-validation-20260913/test_fee_observation.py -v
git diff --check
```

Shell syntax checks passed for the changed port/library, deployment, protocol,
wave, state-chaining, native/semantic blob and replay scripts. The isolated pure
probe uses `check-pure.py`; idle observations use `verify-idle.py` with
`--general 300 --cycles 2`, then `--general 1800 --cycles 1 --allow-split`, both
with `--l2-seconds 1 --blob-limit 2`. No transactions are submitted during the
measured idle intervals. `change-consolidation.py` requires the first idle test
to pass before changing the policy and records state before and after restart.

`collect-history.py` builds the canonical authenticated replay input.
`run-replay.py` invokes `verify-policy-replay.sh` with the exact frozen follower
binary, the native Chiado profile and fresh temporary L2 storage. `check-fees.py`
reconciles network quotes, included/excluded attempts, companion ordering and
all paid amounts. It checks canonical receipt positions at the observation
snapshot and rejects duplicate active charges.

The initial protocol wrapper is deliberately listed above even though its
independent-evidence phase failed. The final result uses the original complete
28-scenario execution plus the successful independent rechecks described in
the issues section. Existing output directories are not overwritten by these
wrappers; choose a new run label when repeating a check.

## Repository changes made for this deployment

The pre-existing settlement-policy and fee implementation remains in the
working tree. This deployment task needed no changes to shipping Rust code.

| File | Purpose |
| --- | --- |
| `testing/kurtosis/ports.sh` | Require four explicit RPC/front endpoints in public-network mode; prevent accidental fallback to a stopped enclave |
| `testing/kurtosis/scripts/lib.sh` | Select deployment artifacts and runtime logs explicitly, allow bounded test funding, and recover/hash-check canonical raw transaction envelopes |
| `run-protocol-e2e.sh`, `verify-eezl2-deployment.sh` | Use the shared deployment selector while preserving their existing assertions |
| `cross-chain-wave.sh` | Use private setup accounts, configured funding and container logs; accept canonical bootstrap evidence with a fresh per-wave measurement baseline |
| `verify-state-chaining.sh` | Use public-network inputs, bounded historical queries and safe canonical L2 receipt/log positions across recomposition |
| `verify-blob-da.sh` | Check the public builder method while retaining inclusion, native KZG, semantic and correlation checks; make only explorer checks optional |
| `verify-semantic-settlement-blob.sh` | Use hash-verified raw transaction retrieval |
| `verify-policy-replay.sh` | Pass the actual public-network genesis, chain profile, system identity and cadence into the existing fresh-follower integration test |
| `scripts/ci/tests/test_public_network_checks.py` | Test default and explicit routing, missing/invalid inputs, funding, typed/legacy raw envelopes, hash mismatches and replay configuration |
| `scripts/ci/tests/test_sync_boundary.py` | Cover unsafe/orphaned receipts that move to a later canonical Sync block |
| `testing/kurtosis/README.md`, `testing/public-gateway/README.md` | Document explicit public-network testing, retention/API prerequisites and the working Chiado HTTPS topology |

Script names without a directory in this table are under
`testing/kurtosis/scripts/`. Deployment-specific operational helpers, generated
Compose/genesis/bindings and raw evidence are outside the shipping runtime.
