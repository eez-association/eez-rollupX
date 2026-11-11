# Sustained mixed-load validation on an existing Chiado canary

This procedure spends **test funds**, deploys workload targets/proxies, and sends
transactions. It does not restart Composer/signer, alter scheduling, deploy the
protocol, or inject faults. Obtain authorization for the selected network first.
For normal deployment/recovery use [Chiado operation](chiado-deployment.md).

The checked-in harness replaces temporary helpers used by the
[initial mixed-load investigation](../audits/2026-10-09-conditional-slot-production/live-mixed-validation.md).
Its corrected checks have offline regressions and a complete live workload
with independent replay; that run required the documented manual test-funding
correction. Later preflight/security safeguards have offline coverage, not a
second funded soak. This is deliberately a Chiado canary procedure, not a generic load
framework: local RPC ports 18645/18688, fronts 18999/18998, Beacon 6152,
independent follower 19288, five one-second L2 positions, P=2 and maximum
unsettled span=150. Do not use it on a different topology without reviewing it.

## Prerequisites and bounds

- Matching pinned protocol checkout, initialized submodule, Foundry 1.7.1,
  Docker CLI, Bash, Python 3, jq, curl, openssl, flock and the existing wave
  runner's Kurtosis CLI dependency. Build the workload contracts beforehand.
- Running canary and an independently bootstrapped **L1/Beacon-only** follower
  on port 19288, with its own datadir, no sequencer overlay/peers/signing keys,
  and enough retained blob history to catch up. Starting that owned follower is
  separate; the harness will not create or restart services. Record whether it
  starts from genesis or resumes its own checkpoint and its exact source/image.
- Private deployment checkout with `.env` (independent `EEZ_L1_RPC_URL`),
  `deployments.env`, and `eez-core-protocol/chain.env` (`SOURCE_PK`). The source
  wallet must be the explicitly selected, non-poster workload wallet, funded
  on both chains, with no other concurrent sender. Do not print these files.
- The driver locks both its checkout's and the deployment checkout's existing
  `.eez-network-e2e.lock`. Coordinate any additional checkout/host using these
  wallets; these two locks cannot discover other users.
- Explicit per-actor funding amount; never inherit the wave runner's 10-unit
  default on a shared network. Setup deploys new targets and funds fresh actors
  on both chains. Their unused balances are not automatically recovered. Review
  the source balances, setup costs, and workload value transfers before starting.
- Ordinary self-transfers: 5/s for 900 seconds, exactly 4,500 required for a
  pass; gas limit 21,000, tip 10,000,000 wei, fee ceiling at most 100,000,000 wei.
  `STRESS_MAX_FEE_WEI=50000000` reserves at most 0.004725 native units for that
  stream alone. Setup costs and actor funding are additional. It never resends
  ambiguous submissions or automatically tops up a wallet.
- Thirty bidirectional waves, eight transactions each, 30-second inter-wave
  gaps plus submission time. Waves start just before ordinary load and may
  continue after its 15 minutes. This is not a capacity benchmark or a promise
  that every block contains transactions.

## Run

From the PR/configuration checkout, set explicit deployment identity inputs
using your private inventory, not a mutable image tag:

```bash
export STRESS_DEPLOYMENT_ROOT=/absolute/path/to/deployment
export EEZ_MIXED_OUTPUT_DIR=/absolute/path/to/new-private-evidence-directory
export STRESS_NODE_IMAGE_ID=sha256:REPLACE_WITH_VERIFIED_IMAGE_ID
export STRESS_SIGNER_IMAGE_ID=sha256:REPLACE_WITH_VERIFIED_SIGNER_IMAGE_ID
export STRESS_SOURCE_REVISION=REPLACE_WITH_VERIFIED_SOURCE_COMMIT
export STRESS_POSTER_ADDRESS=REPLACE_WITH_POSTER_ADDRESS
export STRESS_FOLLOWER_CONTAINER=REPLACE_WITH_OWNED_FOLLOWER_NAME
export STRESS_FOLLOWER_IMAGE=ghcr.io/eduadiez/rollupx-node@sha256:REPLACE_WITH_FOLLOWER_DIGEST
export STRESS_SENDER_ADDRESS=REPLACE_WITH_WORKLOAD_ADDRESS
export STRESS_L2_CHAIN_ID=REPLACE_WITH_CHAIN_ID
export STRESS_DELAY_MS=250
export STRESS_MAX_FEE_WEI=50000000
export EEZ_TEST_FUND_AMOUNT_ETH=0.006
bash scripts/run-chiado-mixed-validation.sh
```

Thirty inbound deposits total **0.00465** and outbound withdrawals total
**0.002325** native units, before fees. The funding value above exceeds that
value-only floor; it is not an automatic estimate of changing fees. Verify
adequate source and actor balances first. Preserve
all output in a private directory. For unattended execution use an owned service
with a bounded lifetime (75 minutes suffices for this procedure), retained stdout
and stderr, and the same environment. Do not launch another invocation while it
holds the wallet locks. Never run the Python checks with optimization (`-O`),
which disables their assertions.

The runner rejects funding at or below the complete deposit value and checks
the source's actor-funding plus worst-case ordinary-gas reserve before sending.
Deployment gas is additional; passing these necessary floors does not estimate
all setup costs. It uses the existing Foundry key CLI interface on a trusted
operator host, as does the wave runner. Do not use a multi-tenant host where
untrusted users can inspect process arguments. Only the public poster address
is needed; do not provide its signing key to the workload.

## Pass criteria and evidence

Require `run.exit`, `live-mixed.exit`, and `independent-verification.exit` all
equal **0**, the wave runner's semantic assertions passing, ordinary
`live-mixed-ordinary/summary.json` reporting `pass`, and
`independent-cross-chain-summary.json` reporting all **30 waves / 240 receipts**.

The ordinary helper records signed intents before sending and checks every
canonical receipt, nonce advancement, exact gas/balance accounting, covering
settlements, full blobs, and independent follower receipts/block hashes/state
roots. L1 evidence uses an independent provider. For mixed batches, it compares
the complete same-L1-block registry event stream: the posting receipt may be
only a prefix, with inbound companion transactions finishing execution later in
that block. The exact correlation endpoint and successful completion receipt
remain required. It does not treat partial consumption as completion.
This canary checker expects one posting for the rollup per L1 block. If another
posting advances the same rollup again in that block, the conservative endpoint
check can fail; investigate the complete event stream, do not weaken it to a
receipt-success check. The core protocol permits same-block re-touch, so this
is a harness limit, not a protocol invalidity.

The independent mixed checker requires the original workload to have succeeded,
checks all expected transaction identities and block roots against an independent
L1 provider or follower, and verifies setter values and exact value transfers.
The queue must be empty before sending and after draining, **not while mixed
traffic is intentionally active**. Sampled receipt movements are diagnostic
lower bounds, not a substitute for canonical checks.

The two corrections are covered by
`python3 -m unittest discover -s scripts/ci/tests -p test_settlement_evidence.py -v`.
Those are offline fixture checks, not live settlement evidence.

Before reporting success, the checker rechecks runtime identity and the pinned
blocks for both the ordinary and cross-chain results. This is canonical-at-read
evidence, not L1 finality. The independent follower has its own L2 execution
and state but may share the local canonical L1/Beacon infrastructure; the
external L1 RPC is an additional provider cross-check.

## Failure, cleanup and limits

Stop on failure; retain first logs, accepted hashes, signed intents, identities,
and all exit markers. No automatic resend, reset, funding retry, or "pass" for
a reduced transaction count is permitted. Reconcile accepted requests against
canonical receipts and the queue before another authorized run. A separate
read-only investigation may establish outcomes for a failed workload, but does
not change its original result. Stopping the driver cannot undo broadcasts.

After verification, stop only an owned follower that you started for this test,
retaining its datadir and logs. Do not stop Composer/signer or delete shared
resources. Record unchanged node/signer identities and restart counts, timing
headroom, workload results, failures, and checks not run in a dated audit.
Raw logs can contain private URLs and must not be committed.

This does not test intentional late proofs, stalled finalization, restarts,
controlled reorgs, exhausted replacements, durable queue recovery, or fresh
bootstrap of a newer follower. It does not repair the pinned protocol runner's
[optimistic receipt assumption](../proposals/canonical-network-test-receipts.md).
