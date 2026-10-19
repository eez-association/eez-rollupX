# Protocol PR 56 network runner integration

Status: implemented locally on 2026-10-19. Fee qualification and corrected
state-chaining checks pass; the full fee-enabled lane still fails on the
canonical protocol client's quote retries. Upstream gaps remain open, and
hosted CI on the final PR head remains required before merge. See the
[current validation record](../audits/2026-10-19-kurtosis-fees/README.md).
Baseline: RollupX `e5e1d97ff49e69821ba403a9ae723ab4afa2db21`, protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. Adopt protocol PR 56 at
`bcd77d8b0d23ed9086f12ac8a0c2e65c6a2c2688` with the focused runner corrections
described in the [integration record](../audits/2026-10-19-protocol-pr56-integration/README.md).
The protocol engineer subsequently integrated PR 56 in PR 54. The current
candidate pins canonical PR 54 revision
`f46af7c9833ac12a54f14d52f4da1f318a0f6525`, replacing the local protocol patches.
It fixes failed nonce reads and exact-nonce quoting, but does not include the
worker-local setup receipt/code guard described below. That acceptance gap must
remain explicit; a successful live run cannot establish reorg-safe preparation.

## Scope and invariants

Retain the same contracts, ABI, genesis, Composer execution and queue
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
authentication and the lookup collector's execution-prefix replay. Summary labels
or link-complete flags alone cannot establish successful execution. Keep the same enabled scenario inventory;
PR 54's excluded scenarios remain excluded.

On 2026-10-19 the user authorized enabling fees in Kurtosis. PR 54's latest
runner requires quotes and has removed its node-fee fallback. Enable both
directions in the default CI/developer profiles with explicit test assumptions:
parity conversion, zero fixed/risk costs, 20% profit target, separate nonzero L2
beneficiary and L1 refund recipient, and a 90% builder refund. Route normal waves
through the pinned quote client. Retain same-Sync state-chaining tests with
explicit funded bids and independent outbound senders, since signed admission
also rejects queued L2 predecessors. Disabled profiles retain same-sender L2
bursts and poison eviction; enabled profiles assert invalid-call rejection at
fee admission instead, with no queue ownership or source receipt.
The mixed-direction case must assert the existing funded-direction separation
and both proofs with fees enabled, retaining shared-Sync assertions when disabled.
Specialized policy profiles retain disabled collection. This authorizes no
change to public-network fee economics or to the quote API's nonce profile.
Wave submission retries known temporary admission refusals using identical
signed bytes under a 120-second deadline. Underfunding and unsupported profiles
remain terminal, as do ambiguous transport failures and changed response hashes.

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
- Verify both fee switches and all cost terms reach only Composer, normal waves
  reject failed quotes without signing, and actual collection/underpayment on a
  fresh fee-enabled enclave. Retain all state-chain and protocol assertions.
