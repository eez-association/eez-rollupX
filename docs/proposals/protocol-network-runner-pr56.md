# Protocol PR 56 network runner integration

Status: implementation authorized on 2026-10-19; validation and publication pending.
Baseline: RollupX `e5e1d97ff49e69821ba403a9ae723ab4afa2db21`, protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. Adopt protocol PR 56 at
`bcd77d8b0d23ed9086f12ac8a0c2e65c6a2c2688` with the focused runner corrections
described in the [review](../audits/2026-10-19-protocol-pr56-readiness/README.md).

## Scope and invariants

Retain the same contracts, ABI, genesis, fee policy, Composer execution and queue
ownership. This changes protocol test submission and independent CI evidence
collection. No deployment to an existing network is part of this migration.

Network preparation must retain each worker's deployment identity, verify its
canonical successful prerequisites and deployed code, and never turn an RPC
failure into nonce zero. A trigger may not replace or skip its own setup. Quote
the exact prepared nonce; same-account multi-trigger scenarios must still submit
bursts, without waiting for each trigger to settle. A typed fee-option rejection
must keep its diagnostic and must not sign, send or fall back to node prices.

Adopt PR 56's canonical receipt refresh and exact transaction links. The RollupX
collector must consume the changed outputs while retaining complete transactions,
canonical receipt checks, independent semantic/causal verification, blob KZG
authentication and follower replay. Summary labels or link-complete flags alone
cannot establish successful execution. Keep the same enabled scenario inventory;
PR 54's excluded scenarios remain excluded.

## Alternatives and compatibility

Merely raising the replacement fee can overwrite a prerequisite transaction.
Using pending nonce without validating setup does not establish that its code or
state exists. Disabling evidence collection or forcing legacy correlation would
hide migration defects. Use an isolated, bounded deployment check and retain
failure diagnostics instead.

Update the protocol gitlink and its RollupX consumers together. The production
Solidity sources and checked core/proxy bytecode match the old pin; this requires
no storage or genesis migration. Runtime binaries keep their existing fee/RPC
contract. Roll back the gitlink and runner consumers together if needed; do not
reset public-network state or discard accepted Composer requests.

## Acceptance

- Reproduce nonce-query failure and removed/reorged setup before the fix; prove
  no signing occurs until the original prerequisites are canonical and usable.
- Cover explicit quote nonce, typed option rejection, malformed RPC evidence,
  worker isolation and preserved multi-trigger nonce sequences.
- Exercise the actual collector with new transaction roles, multiple deliveries
  and settlements, and malformed/orphaned evidence; retain independent checks.
- Run protocol runner and Solidity checks, RollupX tooling checks, generated
  genesis consistency, and the full local Kurtosis lane before pushing.
- Record exact source/runtime identities and any exclusions in the audit. Local
  success is required before publication; remote CI remains the merge gate.
