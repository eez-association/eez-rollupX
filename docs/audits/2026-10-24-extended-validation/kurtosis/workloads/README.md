# Exact offered workloads

These are the exact generators selected for both isolated networks. They sign
with the existing UI checkout's pinned `viem` package; no package was installed
for this audit. The package version is retained in each run's metadata. The
sequential sender ran from its identical copy under ignored `artifacts/`; the
pipeline is a separate control and does not alter the sequential measurement.
Digests are in [source-identity.json](source-identity.json).

Both require `EEZ_LOAD_CONFIG` (JSON), `EEZ_LOAD_OUTPUT` (new directory),
`EEZ_LOAD_UI_ROOT` (the existing package directory), `SOURCE_PK` (the dedicated
public-test workload account), and explicit `KURTOSIS_ENCLAVE`. The config binds
`enclave`, `runtime_source`, `chain_id` (696990), exact `genesis`, ordinary `l2`
and cross-chain `front` endpoints, `sender`, and a fresh empty `recipient`.
No key or configuration is included here. The separate preparation funded one
fresh account with 0.05 test ETH; the generators never fund or deploy.

```bash
export KURTOSIS_ENCLAVE=eez-main-ab-20261024
node ordinary-sequential.mjs
# After the mixed workload and canonical drain, with a new EEZ_LOAD_OUTPUT:
node ordinary-pipeline.mjs
```

The surrounding supervisor holds the pinned protocol checkout's
`tmp/.eez-network-e2e.lock`, checks exact owned container UUIDs and unchanged
runtime identity, samples only those containers every ten seconds, and stops
only its child workload process groups below 5 GiB available host memory. The
same threshold is enforced by each generator; starting requires 12 GiB.
The mixed cross-chain workload is the unchanged
`testing/kurtosis/scripts/cross-chain-wave.sh` at `b0195b0b`, with `mixed`,
12 waves, a 20-second gap, nonce barriers enabled and reverting sources disabled.
It uses independent fresh source accounts; the workload sender never shares a
nonce domain with setup or cross-chain actors. The pipeline starts after these
96 sources settle. No Cargo or Docker build runs during measurement.

The planned rate is an offer schedule, not a throughput claim. Record expired
schedule slots and backpressure separately; do not reinterpret them as accepted
transactions. A rejected or ambiguous RPC stops production without resending.
A successful result requires canonical successful receipts, exact gas prices,
nonce and both-account balance changes, and safe-head coverage for every
accepted transfer. The separate evidence collector authenticates blob payloads
and preserves canonical settlement claims for a cold follower replay.

The [recovery sender](ordinary-recovery.mjs) is a separate 72-second control:
24 ordinary transfers, one every three seconds, one RPC at a time. It reuses the
same account after complete drain, anchors its starting balances at the safe
block, and has a maximum gas reserve of 0.0000504 ETH. Its supervisor pauses only
the UUID-checked owned signer and always restores it in `finally`. Both versions
must recover canonical receipts and exact account balances. Baseline head
continuity is observed; the candidate must demonstrate block and transaction
execution progress during the interruption. Record exact wall-clock/head ages
and commit gaps, and investigate any pause against runtime logs. No arbitrary
wall-clock SLA is inferred from the two-second protocol timestamp interval;
host scheduling delay and protocol gating are separate explanations.

The read-only [inclusion analysis](inclusion-metrics.py) joins each final receipt
to the first `Canonical chain committed` log with that exact block hash. It
requires a match for every accepted transfer and rejects negative elapsed
times. This measures local inclusion separately from the existing hash-bound
L1 settlement timings; header timestamps alone are not used as arrival times.
Apply the same script to both revisions after their measurements finish. The
baseline result is in [baseline-inclusion.json](../baseline-inclusion.json).

The candidate [read-only collector](collect-mixed.py) preserves the baseline
receipt, fee and full-claim checks. Its optional `EEZ_DELAYED_L1_OUTPUT` and
`EEZ_MISSED_L1_OUTPUT` inputs point to the respective `ordinary` output directories,
alongside `EEZ_RECOVERY_OUTPUT` for the signer control. Each must contain a
successful 24-transfer summary with the same runtime identity and all 24 receipts;
duplicate transaction hashes across controls are rejected. The final endpoint
and fee interval include every supplied control. These inputs do not establish
that fault injection succeeded: the separate supervisors retain that evidence.
The [input-selection checks](../collector-preparation.json) preserved all 8,241
baseline receipts exactly and rejected inconsistent synthetic additions, without
RPC access or a new live execution. The baseline collector's original digest
and results remain unchanged.
The optional `EEZ_PREPARATORY_RECEIPTS` input preserves the two accepted transfers
from the aborted bootstrap attempt as a separate replay group. Their original
receipts, canonical block hashes, sender, recipient and gas costs are rechecked;
they do not enter measured workload counts, latency or fee-interval statistics.
The total actor budget includes these two transactions: at most 20,474 ordinary
intents and 0.0429954 test ETH of gas at the configured maximum fee, within its
0.05 test ETH funding. The initial failure remains recorded separately.

The [metrics reader](summarize-load.py) joins signer windows and Composer attempts
by their exact canonical block hashes. Final signer events omit request context
and window duration; these stay unavailable, with the first parser failure
preserved. Composer roundtrip timing is measured independently. The final
[campaign result](../candidate-load.json) records actual execution and the slower
sequential settlement p95 without a performance-parity claim.
