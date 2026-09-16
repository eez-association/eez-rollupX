# Local settlement-policy evidence

This document records the historical validation run. Detailed run artifacts are
preserved locally under `artifacts/audit-evidence/2026-09-13-settlement-policy/`;
generated logs, deployment snapshots, and screenshots are not part of the source distribution.

This directory records completed local validation of the settlement policy. The
[coverage ledger](../../proposals/configurable-settlement-policy-implementation.md)
maps requirements and acceptance cases to their evidence. Subsequent funded
Chiado deployment and validation are recorded in the
[public-network audit](../2026-09-13-chiado-settlement-policy/README.md).

## Equivalent-workload cost measurements

`settlement_policy_cost.rs` uses the existing native `CrossChainWorld`, once
with legacy posting and once with enabled interval/general policy. Each
measurement begins at an already validated safe anchor and covers exactly 40
following L2 blocks. The idle workload contains no transactions; the second
contains two ordinary value transfers. Absolute heights/hashes differ between
the disposable chains; workload, range length, execution base fee, and each
network's blob fee fixture are controlled. No companion transactions occur in
these measurements, so poster and bundle spend are equal.

Execution base fee is zero in both runs. Legacy uses its retained implicit
10-gwei tip; enabled posting uses the network estimator with an explicit
1-gwei local minimum. Native Ethereum's blob floor is 1 base unit and native
Gnosis Prague's floor is 1,000,000,000. These are local fixture measurements,
not live fee forecasts or a promised percentage saving.

| Profile/workload | Legacy batches → policy | Execution gas, legacy → policy | Physical blob gas, legacy → policy |
| --- | --- | --- | --- |
| Ethereum, idle | 8 → 1 | 900,100 → 112,514 | 1,048,576 → 131,072 |
| Ethereum, two transactions | 8 → 1 | 900,076 → 112,502 | 1,048,576 → 131,072 |
| Gnosis, idle | 8 → 1 | 900,052 → 112,514 | 1,048,576 → 131,072 |
| Gnosis, two transactions | 8 → 1 | 900,112 → 112,514 | 1,048,576 → 131,072 |

[EthereumOsaka.json](EthereumOsaka.json) and [GnosisPrague.json](GnosisPrague.json)
contain range endpoints, receipt transaction hashes and exact native-unit spend
(hex-encoded integers). The tests validate canonical receipt/root identity,
range continuity, complete physical blob charges, and every measured formula.
Savings here come from fewer posts and the explicit tip change; reducing a
declared gas limit alone does not reduce charged execution gas.

## Kurtosis native DA and independent reconstruction

The isolated `eez-settlement-policy-release` enclave used the pinned client digests and
explicit timing/fee settings in
[settlement-policy-args.yaml](../../../testing/kurtosis/settlement-policy-args.yaml).
The existing blob verifier inspected an included two-blob transaction in L1
block 34, authenticated KZG and native payload data, checked both correlation
directions, and matched the applied registry root at L2 block 198. A fresh Rust
follower then reconstructed all 198 blocks using only canonical L1 data and
matched the deployment, increment, and add-41 transaction receipts. A subsequent
one-blob settlement in L1 block 49 covered L2 blocks 199–288 and passed the same
checks. Another fresh follower reconstructed through block 288 and matched
every hash in that second range. The two records cover L2 1–288 without gaps.

kurtosis-native-da.json (`artifacts/audit-evidence/2026-09-13-settlement-policy/kurtosis-native-da.json`, local evidence) and
kurtosis-native-da-followup.json (`artifacts/audit-evidence/2026-09-13-settlement-policy/kurtosis-native-da-followup.json`, local evidence) preserve those identities,
all covered L2 block hashes, KZG commitments, verification results and a digest
of the original full output. Repeated block lists and full block RLP are omitted.
The existing verifier's `abi_call_data_bytes` field means `PostBatch.callData`
length, not the complete ABI transaction input.

The initial wrapper completed native DA verification and independent replay,
then reported missing Blobscan endpoints because this profile disables explorers.
The wrapper now accepts an explicit `EEZ_VERIFY_BLOBSCAN=false`; its default
still requires Blobscan and propagates explorer errors. The subsequent complete
wrapper run exited successfully with that explicit setting. Native receipt,
KZG, root, correlation and replay checks are always required.

The local PBS profile explicitly supplies a 1-gwei minimum while retaining the
network estimator. It is a reproducible test setting, not a universal relay
minimum or a production default. The target fork permits 15 blobs per block
and six per transaction; the block maximum is not used as transaction capacity.

## Reproduction

Build and freeze the actual `eez-composer`, `eez-follower`, and
`eez-proof-signer` executables; record Cargo's `compiler-artifact.executable`
paths. `eez-node` is a package name, not the composer binary name. Tests require
both `EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` when
`EEZ_TEST_PREBUILT=1`. Use the same frozen runtime for a given run.
[runtime-build.json](runtime-build.json) records the final executable SHA-256
identities, Rust version, build profile and local image digest. Local integration
uses the optimized development profile; CI retains its existing release profile
and pinned Rust 1.94 / nextest 0.9.143 artifact checks.

```sh
EEZ_TEST_COST_EVIDENCE_DIR=/tmp/settlement-cost-evidence \
cargo test -p eez-node-e2e --test settlement_policy_cost -- --test-threads=1

KURTOSIS_ENCLAVE=eez-settlement-policy-release EEZ_VERIFY_BLOBSCAN=false \
bash testing/kurtosis/scripts/verify-blob-da.sh

# Supply the public test system key and the full summary.json from that run.
bash testing/kurtosis/scripts/verify-policy-replay.sh /path/to/summary.json
```

Existing native restart tests stop the process abruptly. Before those tests
restart, their workload is allowed to reach Reth's persisted canonical history;
recent in-memory execution can otherwise disappear on process termination.
This feature preserves retained pure history and uses the persistent witness
store. It does not introduce a new durability or transaction-ownership journal.
