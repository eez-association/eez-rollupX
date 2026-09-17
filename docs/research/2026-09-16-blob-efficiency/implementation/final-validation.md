# Final implementation validation

Date: 2026-09-17. Current integration base and freshly fetched fork `origin/main`:
`fc0ee0516a1dbbdf9f785a3fa3d2f1789b6d2af7`. Protocol gitlink:
`3c0cdf4955941a5c23639346932b55b33babebbd`. Reviewed source manifest:
[`implementation-source.sha256`](implementation-source.sha256), manifest digest
`2c9c5d9dea5b9083931aee82e4d987c893402794e0507ed2c3046ad02902c27d`.

## Static and component checks

- `cargo +1.94.0 fmt --all --check`: passed.
- `git diff --check`: passed.
- Affected-package `cargo check --offline --locked --all-targets -j3`: passed.
- Strict affected-package Clippy with `--all-targets -- -D warnings`: passed.
- Mandatory policy Compose propagation passed for Chiado and Kurtosis; the
  Kurtosis configuration unit suite passed 16 tests with one explicit live probe
  skipped.
- The complete rebased CI/tooling Python suite passed 241 tests; four opt-in real
  Docker/Kurtosis probes were explicitly skipped because their behavior was
  exercised by the live runs recorded below.
- Proof signer: 317 passed, zero failed, two explicit measurement/inventory
  utilities ignored.
- Prover client: 30 passed.
- Composer: 203 passed after removing disabled-policy-only tests.
- Control RPC: 6 passed.
- Payload codec: 32 passed after removing 13 tests for deleted tags 0 through 2.
- Deriver: 48 passed.
- Driver: 56 passed.
- Node: 44 passed.
- Parser/iterator property validation: 10,000 generated cases each for arbitrary
  canonical parser acceptance, independent dense environment iteration, and
  sparse transaction placement.

## Process and native checks

- Mandatory-policy partial consumption: 5 passed.
- Same-height resumption: 1 passed.
- Native L1 queue reorg: 1 passed.
- Chained interstate ordering: 5 passed.
- Cross-chain suite: 22 passed before the follower representation-only
  simplification; its bidirectional smoke passed again on the final source.
- Final-source retained maxima: Ethereum 65,535 and Gnosis 65,536 blocks, one
  blob each, zero misses, 20 warm repetitions, exact digest/signature recovery;
  see [structured retained result](native-final-retained-checkpoint.json).
- Native post plus clean-follower smoke passed before the simplification for
  Ethereum 60 and Gnosis 65 blocks, one blob each, producer-prepared, zero
  misses and exact follower replay. Both profiles passed again on the final
  source with their focused clean-follower scenarios.
- Earlier maximum live posts and clean followers: Ethereum 65,536 and Gnosis
  65,535 blocks; see [live structured result](native-live-prepared-checkpoint.json).
- L1 component suite: 135 passed with loopback permission.
- Node-common: 6 passed.

## Post-review simplification

After assessing upstream PR #152, runtime v3 decoding was simplified to return
the bounded sparse `Payload` directly. The follower now replays its allocation-free
`BlockPlan` iterator without materializing legacy block-count/transaction vectors,
the unused dense v3 compatibility adapter was removed, and the deriver no longer
accepts local cadence/default-header inputs. Focused validation passed: payload
codec 45, deriver 48 and Node 44 tests; Ethereum and Gnosis clean-follower native
scenarios; the bidirectional cross-chain smoke; and same-height resumption. See the
[assessment and adoption record](upstream-pr-152-assessment.md).

The final cleanup deleted tags 0 through 2, `DecodedBatch`, their encoders,
decoder, feature flag, legacy-only error variants and obsolete signer test
verifier. Runtime consumers now call `Payload::decode` directly. It also reused
the admitted terminal block rather than decoding its RLP again in fresh follower
reconciliation. After the obsolete tag-2 test verifier was removed, the full
signer suite passed 317 tests with two explicit utilities ignored. The
Ethereum/Gnosis clean-follower plus native-capacity scenarios passed again.

Follower replay lists now retain Alloy `Bytes` from sparse records and Sync
reconstruction instead of copying every transaction into `Vec<Vec<u8>>`.
Deriver 48/48 and the three focused native scenarios passed after this final
change.

The driver composer interface now exposes one required explicit Sync outcome.
The optional compatibility adapter, pool-driven outcome, disabled-policy
branches, and fixed-fee posting fallback were removed. Policy, posting, admission,
and selection are concrete runtime paths for the new network. Driver all-features
56/56 and Composer 203/203 passed.

The mandatory-policy cleanup also passes all five retained native
partial-consumption scenarios, including first/middle/last omission, restart,
same-height resume, receipt-cost accounting, and clean-follower convergence.

## Release-evidence boundary

## Live Kurtosis validation

Disposable local deployments on the same source and pinned protocol passed the
inbound, outbound, mixed and mixed-pure modes, state chaining, safe-head/root
convergence, and all 28 selected upstream protocol scenarios. The independent
collector reported complete accounting, causal coverage and authenticated
semantic blob coverage with zero failed or skipped scenarios. A separate pure
L2 smoke verified physical Beacon blob/KZG evidence, transaction-to-batch
correlation and equality between the safe L2 hash and L1 registry root. Clean
node restart restored RPC and both correlation directions in 10 seconds while
preserving that commitment. The partial-consumption canary produced the exact
`Committed`, `SourceNonceSpent`, and `SourceReverted` outcomes and then proved
subsequent safe-head progress.

The first full run preserved as evidence exposed a stale collector assumption
that derivable intermediate records contain hashes. The verifier now validates
sequential recipe records and uses the canonical terminal hash as the ancestry
commitment; malformed-window tests distinguish the new rule. A fresh complete
run passed after that correction. See the
[Kurtosis validation record](../../../../audits/2026-09-17-derivable-operations-kurtosis/README.md)
and its checked-in compact result.

The final tooling inventory also found that the new `derivable_operations`
integration target was not selected by the native artifact runner. It is now
archived and executed in the cross-chain lane, and the exhaustive integration
target inventory regression passes.

## Release-evidence boundary

These checks performed disposable local deployment only; they do not perform or
authorize a public or production deployment. The coverage ledger keeps
integrated maximum-backlog signer restart to cross-chain readiness, release
pruning/state-history settings, a distinct live K=6 profile, maximum follower
crash injection, and portable full resource artifacts open as
release-environment evidence.
