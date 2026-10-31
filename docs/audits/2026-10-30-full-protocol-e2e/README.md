# Complete pinned protocol E2E support

## Scope and source identity

- Fork baseline: `c702f90da19cc149890d0a17a4ef6c200dcb6a21`.
- Unchanged protocol: `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Locally qualified simplified implementation: `e51a2477d98662be2b33c0f17b3bc4c0f0921d20`.
- Previously hosted-qualified PR head: `1ff8619d0be29c0f1ab4376d2a76737552d3c61f`.
- [Design and compatibility](../../proposals/full-protocol-e2e.md),
  [64-scenario inventory](scenarios.md), [signer review](signer-review.md).

The baseline default runner omitted 30 of 64 scenarios. Running all scenarios
against baseline binaries produced 33 passes and 31 failures. The implementation
adds nested static/mutable callbacks, caught-revert continuation, rollback ownership
and reusable static candidates throughout composition, independent validation,
settlement and follower reconstruction. No pinned scenario assertion was weakened.

## Current local qualification

The cleanup removes 86 production lines, six temporary metadata vectors, repeated
coverage totals and discarded call hashing. Successful nested calls no longer copy
rollback facts they never restore. Host-shape validation and independent execution,
receipt and commitment binding remain mandatory. The [performance record](performance.md)
contains the matched measurements and their limits: no regression in seven
partitioner workloads; this is not a whole-node throughput claim.

| Check | Result on `e51a2477` |
| --- | --- |
| Workspace units | 1,641 passed; six optional fixture/timing utilities skipped |
| Affected packages / signer | 1,149 / 338 passed, respectively |
| Native E2Es | All 192 executed exactly once: 191 first-run passes and one timing flake; targeted baseline and candidate diagnostics passed |
| Release protocol/blob checks | 55 passed, zero skipped |
| Network traffic and state chaining | All four traffic modes and state-chaining assertions passed |
| Complete protocol confirmation | 64 passed, zero failures/skips/exclusions/omissions; semantic and causal evidence complete |
| Independent cold follower | Canonical L1 only through L2 block 1,974; 150 scenario-range blocks and 101 receipts checked across 25 batches covering all 64 scenarios |
| Real proof path | 97 signed windows, 97 remote attestations, zero canonical divergence events |
| CI tooling | 423 passed; seven optional infrastructure probes not run |
| Formatting, Clippy, signer rustdoc, SDK, configuration, genesis, artifact identity and PR-history secret scan | Passed |

The native flake missed a fixture's explicit L1 submission target. Its unchanged
test passed on both prior and candidate binaries in isolated diagnostics; the
original full run remains failed (`flaky-result = fail`). The first 64-worker
protocol run passed upstream assertions but failed strict evidence collection:
one scenario retried an underfunded fee quote, leaving an unreconciled `ERROR`.
The complete confirmation used CI's four-worker concurrency and passed all checks.
No test, evidence rule, timeout or scenario selection was weakened. Both first
failures and successful confirmations are retained.

[Validation identities and results](candidate-validation.json),
[follower result](candidate-independent-replay.json) and the
[scenario inventory](scenarios.md) bind the evidence to this source. Raw logs,
datadirs and measurement samples are local-only under
`artifacts/full-protocol/candidate/revision-e51a2477` and
`artifacts/full-protocol/simplification-performance`; they are unavailable from
another checkout. The task-owned enclave was stopped with its data retained.
Hosted CI for this follow-up must still be checked on [PR #123](https://github.com/eez-association/eez-rollupX/pull/123).

## Signing identity and compatibility

RPC compatibility remains version 8; signing domain 7 and the registered vkey
remain unchanged. No verifier, core pin, production ABI, genesis or storage format
changes are introduced by this cleanup. No contract/key/genesis migration is
required for the baseline-compatible extension. Network, registry, proof-system
and independent execution checks remain mandatory. Earlier pre-baseline deployment
boundaries still apply.

The [prior qualified record](https://github.com/eez-association/eez-rollupX/blob/1ff8619d0be29c0f1ab4376d2a76737552d3c61f/docs/audits/2026-10-30-full-protocol-e2e/candidate-validation.json) preserves the
signing-identity vectors, all 64 scenario results, cold replay and earlier checks.
[Hosted run 41187306318](https://github.com/eez-association/eez-rollupX/actions/runs/41187306318)
passed all 13 jobs on that earlier source, not this follow-up.

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
