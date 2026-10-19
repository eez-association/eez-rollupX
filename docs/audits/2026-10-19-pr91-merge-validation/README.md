# PR 91: protocol pin and fee-enabled integration

Status: all local validation passed, including the full fee-enabled Kurtosis
lane and both dedicated fee-recovery exercises. The user authorized updating
the protocol pin, running the complete local fee-enabled Kurtosis lane, pushing
the fixes and merging PR 91 before hosted CI finishes. This does not authorize
promoting deployment tags without their existing CI gate.

## Identity and scope

- Tested RollupX source: `2362dbd256275a6d97ec3bfeeee30e94fdbf7635`.
- Pinned protocol PR 54 head: `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`;
  the [latest-commit review](../2026-10-19-protocol-pr54-latest-review/README.md)
  records the fee-client changes and controlled regressions.
- Previously published PR 91 head: `e5e1d97ff49e69821ba403a9ae723ab4afa2db21`.
- Target `main` at preparation: `2c19e36bed2720c00f89841ccfca47ec7c921e06`.
  `git merge-tree --write-tree origin/main HEAD` produced the tested HEAD tree,
  `d8c83e83ed433a9c76346e12bf3f96e78e02dd0c`, without conflicts.
- The local runtime producer built and sealed node, signer, deploy image and
  test artifacts for the exact source/pin. The artifact consumer verified their
  hashes and OCI identities before use. Runtime binary hashes equal the
  [previously qualified Composer nonce fix](../2026-10-19-composer-nonce-snapshot-fix/README.md).
  The updated deploy image bundles the new protocol tooling.

The root operator checkout, public deployment and existing enclaves were
preserved. Work uses `/tmp/rollupx-pr54-fees-20261019` and a new owned enclave,
`eez-pr91-merge-20261019`. No protocol source edits were made. After capturing
final logs and verifying the runtime identities, only this owned enclave was
stopped; its containers/data were retained. Both runtime containers recorded zero
restarts during validation.

## Local validation

- Core contracts: 778 passed; three fork-upgrade tests skipped. The documented
  `script/*`, `test_Doc_OriginalValue_SeedInSetup` and
  `test_PostCost_Incremental` exclusions were retained.
- Release producer vectors/blob evidence: 30 passed, zero skipped.
- Tooling: 379 passed, eight skipped on the first run. After locating the pinned
  Gitleaks 8.30.1 binary, its two skipped scanner regressions passed separately;
  six optional infrastructure probes remain unexecuted.
- SDK: six passed. Full-history secret scan passed.
- Full local Anvil selection: 62 passed, zero failed/timed out/not run.
- Full fee-enabled Kurtosis lane: passed on its first run. All four traffic modes
  and nine state-chaining cases passed. All 34 live-enabled protocol scenarios
  passed with zero failures/skips and complete independent receipt, causality and
  native-blob verification. The lane recorded 82 signed windows and 82 accepted
  remote attestations, with canonical commitment/safe-head convergence.
- Dedicated L1 fee exercise: four individually funded nested-call/Safe sources
  passed in legacy/EIP-1559 formats, followed by two four-user batches. The
  underpaid source was rejected without queue ownership. Both batches used one
  post and one refund from their largest measured payer; both had an actual
  shortfall. The second batch and a subsequent quote succeeded after observing
  the first shortfall. Safe coverage uses Safe 1.4.1 with an EOA owner.
- Dedicated L2 fee exercise: the underpaid signed source was rejected without
  ownership or execution. Both paid nested sources settled in one native block;
  canonical receipts, target effects, beneficiary, final queue disposition and
  pooled payment covering the complete posting/blob tariff all passed.

The completed checks used these commands from the isolated worktree:

```bash
forge build --root eez-core-protocol
FOUNDRY_ISOLATE=false forge test --root eez-core-protocol \
  --no-match-path 'script/*' \
  --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
BASE_PORT=28545 BASE_CHAIN_ID=51337 RAYON_NUM_THREADS=2 TOKIO_WORKER_THREADS=2 \
  bash scripts/ci/run-protocol-anvil.sh
python3 -m unittest discover -s scripts/ci/tests -v
node sdk/l2-fees.test.mjs
```

The separately repeated `test_secret_scan.py` used Gitleaks 8.30.1 on `PATH`.
The full-history scan used the repository configuration with
`--log-opts='--all -m' --redact=100 --ignore-gitleaks-allow
--gitleaks-ignore-path /dev/null`. Source-specific Rust unit/process regressions,
formatting and Clippy remain recorded in the nonce-fix audit; they were not
rerun for this test-tooling-only protocol update. The binary equality check is
recorded separately from this run's live integration results.

The full Kurtosis lane uses `testing/kurtosis/ci-args.yaml` with both collection
switches true, 1:1 conversion, 20% quote margin, 90% largest-source refund,
separate recipients and no excluded senders. These values were independently
read from the running node's environment. The local host uses two concurrent
protocol workers, matching the no-publication CI lane. No scenario filter or
fee fallback is enabled.

The retained copy of `testing/kurtosis/run-ci.sh` changes only its source-directory
binding and final teardown: it leaves the owned enclave available for diagnostics.
All assertions and scenario selections are unchanged. The lane includes inbound,
outbound, mixed and mixed/pure traffic, state chaining, the complete eligible
protocol selection, proof-flow checks, canonical commitment convergence and the
independent protocol evidence collector. Excluded live scenarios remain excluded;
the separate Anvil run explicitly selects all 62 local scenarios.

After loading the verified runtime environment, the lane was invoked with:

```bash
export KURTOSIS_ENCLAVE=eez-pr91-merge-20261019
export KURTOSIS_ARGS_FILE="$PWD/testing/kurtosis/ci-args.yaml"
export EEZ_CI_RESULT_DIR="$PWD/artifacts/pr91-merge-20261019/kurtosis"
export EEZ_PROTOCOL_MAX_PARALLEL=2
unset EEZ_PROTOCOL_SCENARIOS
bash artifacts/pr54-fees/run-ci-retain.sh
```

The subsequent fee exercises ran sequentially, using this enclave's ports and
deployment bindings and its public Hardhat account 2, following the
[fee exercise procedure](../../../testing/kurtosis/README.md#generate-test-traffic):
`node testing/kurtosis/scripts/verify-l1-fees.mjs`, then
`node testing/kurtosis/scripts/verify-l2-fees.mjs`. Their evidence paths were
separate, and both exit codes were zero.

The protocol collector authenticated and decoded native blobs and bound their
semantics to the canonical transactions in all 34 scenarios. It also verified
lookup results/root bindings where applicable. This run did not perform a new
independent follower execution replay of all payloads; the evidence's
`payload.execution_verified` fields remain false. Earlier native process replay
results retain their original scope and source identity.

Initial relay registrations were rejected as unknown during normal cold startup;
the relay subsequently accepted them within the existing timeout. The script's
existing housekeeper refresh and canonical-settlement gate then admitted traffic.
No timeout, assertion or service configuration was bypassed.

## Evidence and completion boundary

Raw logs are local-only in
`/tmp/rollupx-pr54-fees-20261019/artifacts/pr91-merge-20261019/`.
Runtime/test archives are local-only in
`/home/edu/Development/rollupX/artifacts/pr91-merge-20261019/`.
Neither location is available from a fresh clone. [Sanitized results and
digests](validation.json) bind the checks and fee measurements to the source,
images and retained logs. The final evidence commit changes documentation only.

This run does not establish a Falcon-owner workload, durable queue recovery,
mainnet builder qualification, a 770-job stress result or a new public deployment.
The classic protocol runner's setup/reorg guard remains a separate follow-up.
Hosted CI status and the merge outcome are tracked by
[RollupX PR 91](https://github.com/eez-association/eez-rollupX/pull/91).
