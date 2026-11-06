# Run a Composer with several attesters

This extends the existing native `prove.v2` setup. Complete the applicable
[deployment procedure](chiado-deployment.md) first. It does not deploy or register
attesters, change network rules, or give the Composer a proof-signing key.

## Prerequisites

- Run matching Composer and signer versions for the same network identity.
- Deploy a separate ECDSA proof-system contract for each attester.
- Register each proof system on the reference Core Rollup manager with its exact
  network-profile verification key. Each key binds the network, registry,
  proof-system address and signer address. An address-derived key is insufficient.
- Set the manager's threshold to the required number of distinct proof systems.
  Register and check the new members before raising the threshold.
  The threshold counts proof-system contracts. Use separate signing keys for
  independent attestation.
- Keep the existing legacy proof-system setting for deployment and discovery.

## Configure and start

Supply `EEZ_PROVERS` to the Composer as a comma-separated list:

```text
EEZ_PROVERS=http://signer-a:50061=0x<SIGNER_A>=0x<PROOF_SYSTEM_A>,http://signer-b:50061=0x<SIGNER_B>=0x<PROOF_SYSTEM_B>
EEZ_ATTESTATION_GRACE_MS=250
```

Replace the placeholders with nonzero 20-byte addresses. URLs can contain `=`;
the two address fields are split from the right. Do not put authenticated URLs
or private keys in committed configuration. Provision and secure each signer
endpoint through the existing signer procedure. This setting does not start
additional signer containers.

The list takes precedence over `EEZ_PROVER_URL` and `EEZ_ATTESTER_ADDRESS`.
An explicit empty list fails startup. Unset keeps the existing single-prover
configuration. At most 16 distinct proof systems are supported. Duplicate or zero
proof-system addresses and zero attesters fail startup.

Startup requires every configured endpoint to agree on the locally computed
validation identity within 30 seconds. The smallest reported block ceiling limits
the Composer's settlement range. Limits cannot change silently after launch.
After startup, an unavailable member need not stop settlement when the remaining
valid members can meet the on-chain threshold.

Restart the Composer to apply configuration changes. The standard Compose node
profiles forward both settings. Use their existing start/check procedure; no
different node database or follower configuration is needed.

## Check settlement

- Check `eez.composer.attestation.quorum_reached` for threshold and proof count.
- Check `eez.composer.attestation.member_skipped` for unregistered members,
  wrong network-profile keys or signer mismatches.
- Check successful native L1 receipts and canonical commitment advancement.
  A signature, block ACK or relay acceptance alone does not prove settlement.

Every collection reads policy at its settlement anchor. Every included proof must
verify; invalid responses are excluded. A threshold of zero still requires one
proof, as required by the registry. If the threshold is unreachable, work remains
owned and is retried through the existing settlement procedure.

The grace setting controls collection after threshold is met. It defaults to
250 ms, accepts 0 through 60000 ms, and is capped by the remaining proof deadline
and post-proof reserve. Pending RPC exchanges are canceled when collection ends.
A started blocking blob-commitment calculation can finish after cancellation;
it owns its input and cannot publish a settlement by itself.

Fee and wallet gas estimation use the usable member set at their pinned L1
snapshot. They still execute the verifier bytecode in disposable estimation state.
Actual submission uses the collected real proofs and native preflight.

## Recovery and rollback

Keep the existing [settlement recovery boundaries](chiado-deployment.md).
No database or chain migration is introduced. The queues remain in memory.
Do not assume restart preserves unsettled requests. Before downgrading to a
single-prover Composer, restore a manager policy that its signer can satisfy.
Do not reduce the threshold merely to hide a broken or incompatible deployment.

See the [design and acceptance checks](../proposals/attestation-quorum.md).
