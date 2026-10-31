# Complete pinned protocol E2E support

## Scope and source identity

- Fork baseline: `c702f90da19cc149890d0a17a4ef6c200dcb6a21`.
- Unchanged protocol: `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Qualified implementation: `6808f13b45176291b8cfaa8e5effbc31ee01f504`.
- Qualified PR head: `f0f40401d491ddaf4e1190e408dba92963b9118f` (audit-only addition).
- [Design and compatibility](../../proposals/full-protocol-e2e.md),
  [64-scenario inventory](scenarios.md), [signer review](signer-review.md).

The baseline default runner omitted 30 of 64 scenarios. Running all scenarios
against baseline binaries produced 33 passes and 31 failures. The implementation
adds nested static/mutable callbacks, caught-revert continuation, rollback ownership
and reusable static candidates throughout composition, independent validation,
settlement and follower reconstruction. No pinned scenario assertion was weakened.

## Qualification before simplification

[Hosted CI run 41165154202](https://github.com/eez-association/eez-rollupX/actions/runs/41165154202)
passed all 13 jobs. Its merge tree exactly matches `f0f40401`.

| Evidence | Result |
| --- | --- |
| Complete pinned reference Anvil suite | 64 passed, zero omissions |
| Local and hosted Kurtosis protocol suites | 64 passed each, zero failures/skips/exclusions; complete semantic and causal evidence |
| Local independent cold follower | Canonical L1 only through L2 block 966; 84 scenario-range blocks and 101 receipts checked across 12 batches |
| Hosted native E2E inventory | 192 passed exactly once: 149 cross-chain + 43 embedded, all eight shards successful |
| Workspace units | 1,641 passed; six optional fixture/timing utilities skipped |
| Release protocol/blob evidence | 55 passed, zero skipped |
| Integration contracts | 153 passed |
| Applicable lint, codecs, RPC, SDK, configuration and dependency checks | Passed; existing audit warnings and optional exclusions recorded below |

[Compact local validation record](candidate-validation.json) retains exact source,
artifact identities, commands, optional skips and limitations.
[Cold follower result](candidate-independent-replay.json) records the replayed safe
head. Bulk logs/datadirs remain local-only under the task worktree's
`artifacts/full-protocol/candidate/revision-6808f13b`; they are not available from
another checkout. Compact evidence and hosted logs remain independently accessible.

## Simplification and signing identity: 2026-10-31

The initial implementation unnecessarily coupled RPC validation version 8 to
on-chain key rotation. Review found no changed ECDSA verifier, core protocol pin,
key-derivation algorithm, production Solidity ABI or genesis. Two retained native
request fixtures are byte-identical; the caught-lookup fixture adds authenticated
manager witness material without changing its execution history.

The correction keeps RPC identity 8 for runtime compatibility and preserves the
existing signing-domain version 7 and registered vkey. Independent Foundry ABI/hash
checks reproduced both baseline profile vectors and all five baseline key vectors.
This does not remove network/registry/proof-system binding or authorize signing
before independent execution. No contract/key/genesis migration is required by
this compatible extension. Earlier pre-baseline deployment boundaries still apply.

**Current validation: pending for the simplified revision.** The green results
above apply to the stated source identities, not automatically to later changes.

## Material limitations

- ECDSA is a trusted attester, not a ZK proof or binary attestation. The signer
  review is a separate manual adversarial review, not an external audit.
- Upgrade Composer, signer and followers together; discard old proving sessions.
  Older request witnesses may lack manager data now required for lookup replay.
- Old followers cannot replay newly supported canonical shapes. Downgrade after
  their inclusion is unsupported. No arbitrary live-network migration was rehearsed.
- Pending requests/retry counters remain in memory. This work adds no durable queue.
- Passing the pinned 64-scenario suite is not support for every Core configuration.
  The signer still supports one L2 and one proof system; explicit cross-chain call
  gas, sender-bound batches and other L2 targets remain unsupported. Existing
  gas/frame/witness and nesting bounds remain. See signer SPEC section 1.
- Optional mainnet fork, stress, fixture-generation and tooling probes are listed
  in the validation record; they are not omissions from the pinned protocol suite.

## Preserved investigation history and first failures

The [original dated investigation](https://github.com/eez-association/eez-rollupX/blob/f0f40401d491ddaf4e1190e408dba92963b9118f/docs/audits/2026-10-30-full-protocol-e2e/README.md) is immutable at the
qualified commit. It retains intermediate failures and corrections: initial Anvil
port collisions, baseline quote/replay failures, temporary checkpoint ownership,
static-candidate reuse and cached-depth findings. Local qualification also recorded
Git ownership/export prerequisites, missing `kill`, and shell-wrapper exit 127
after passing cross-chain tests. The full embedded rerun and all hosted native
shards passed; those earlier harness failures are not relabeled successful.

The detailed scenario/evidence JSON, source/stack probes and diagnostic excerpts
remain in that same commit. Their removal from the current tree reduces review
noise; their contents and history have not been rewritten.

## Isolation

All work used `codex/full-protocol-e2e-20261030` in a separate worktree. Existing
operator changes, deployments, wallets, configuration and datadirs were preserved.
Owned final and diagnostic enclaves were stopped, not deleted. No Chiado/public
network migration, funding or deployment was performed.
