# Signer soundness review

Reviewed source: `6808f13b45176291b8cfaa8e5effbc31ee01f504`; protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. This is a separate manual adversarial
source review, not an external audit. [Original findings, source hashes and dated
follow-ups](https://github.com/eez-association/eez-rollupX/blob/f0f40401d491ddaf4e1190e408dba92963b9118f/docs/audits/2026-10-30-full-protocol-e2e/signer-review.md) retain the full investigation.

## Authorization path

`service/settlement_job.rs::settle_validated` checks L1 context, native DA,
reconstructed tables and effects, then independently replays observations. Only
after replay, cancellation and deadline checks can it construct attestation material.

The driver executes mutable lookup tables through authenticated manager state,
requires manager success, and separately compares the actual target's success/revert
and exact output. Missing proofs/code reject. Rollback observations share a
canonical-prefix scratch overlay; temporary writes never become canonical state.

Static candidate selection binds call identity, position, callback execution,
return bytes and success. Only the selected callback graph counts toward coverage.
Memoization is scoped to a pending read; cached suffix height is checked on reuse.

## Findings and regressions

| Threat/finding | Evidence and disposition |
| --- | --- |
| Forged outcome copied into native semantics and settlement | Service regression reaches `observation_replay`, rejects, asserts zero signing calls |
| Missing authenticated manager bytecode | Same service boundary rejects before signing |
| Delivery failure mistaken for target revert | Manager must succeed before the target frame outcome is accepted |
| Altered reusable static candidate | Independently assembled authorizer fixture rejects output, callback commitment and calldata mutations with `ExecutionTableMismatch` |
| Memoized suffix bypasses nesting bound | Reproduced during development; memo now checks stored descendant height at every reuse |
| Full-cache snapshots amplify nested observation memory | Replaced with bounded journal positions; 1,024-scope regression passes |
| Temporary reentrant checkpoint survives session replacement | Retire descendant checkpoints and retain the outer rollback anchor; focused before/after regression passes |
| Static pool candidate ambiguity | Distinct top-level host positions separate local-write contexts; repeated static callbacks reuse only matching rows |

Final qualification ran 339 signer tests, codec/commitment oracles, real process
rejection/settlement tests and all 64 network scenarios. A 2 MiB stack probe passed
at depths 64, 256, 512 and 1,024 for three shapes; 1,025 rejected. This is evidence
for the tested bound, not a general resource/performance guarantee.

## Signing-domain simplification

RPC compatibility and on-chain attestation identity serve different purposes.
Retain RPC version 8 while preserving signing domain 7, network/genesis, registry,
proof system and attester bindings. No invalid previously issued attestation was
identified that requires domain revocation. An incompatible rule change or signer
compromise would require a separate revocation decision.

The ECDSA contract authenticates a key holder, not the binary performing validation.
The simplified revision `0ffc0b8d` passed 338 signer tests, including positive
fixtures, forged-outcome/missing-code rejection and RPC mismatch checks. All three
retained native signatures/public-input hashes and the independent baseline
profile/key vectors are unchanged. Only the obsolete RPC/signing-domain version
equality assertion was removed. All 64 network scenarios and independent follower
replay passed again; see [the audit summary](README.md).
