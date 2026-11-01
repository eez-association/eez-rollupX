# Complete pinned protocol E2E support

## Source and result

- Fork baseline: `c702f90da19cc149890d0a17a4ef6c200dcb6a21`.
- Unchanged protocol: `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Locally qualified runtime: `fbd0d238b9e75c1af68bbb31e7f210c49cc9ad8c`.
- Prior hosted-qualified head: `5d74a42ae8ae9fe2a79ab02fdf7c6e5c247e698d`
  (qualification documentation added after the runtime commit).

The baseline runner selected 34 of 64 scenarios. Running all 64 against baseline
binaries produced 33 passes and 31 failures. This implementation adds nested
callbacks, caught-revert continuation, rollback ownership and reusable static
candidates across composition, signing, settlement and follower replay. No pinned
scenario assertion was weakened.

The current runtime passed 1,643 workspace tests, 685 affected-package checks,
56 release checks and all 192 native cases without retries. All 64 protocol
scenarios passed with complete accounting and causal/semantic blob evidence,
zero exclusions and zero skips. A cold follower independently reached L2 block
1,500 from canonical L1, checking 150 scenario-range blocks and 101 receipts
across 24 batches. The source-cursor cleanup removes 79 production lines;
shared trace folding was tested and rejected for its measured finalization cost.

[Prior hosted CI](https://github.com/eez-association/eez-rollupX/actions/runs/41313363564)
passed all 13 jobs on `5d74a42a`; that run does not qualify the newer runtime.
Fresh hosted results are tracked on the PR after the local qualification above.

## Review and evidence

| Record | Purpose |
| --- | --- |
| [Design and review map](../../proposals/full-protocol-e2e.md) | Implementation responsibilities, invariants and compatibility |
| [Scenario inventory](scenarios.md) | All 64 paths, source hashes and baseline/candidate results |
| [Signer review](signer-review.md) | Independent manual adversarial review and regression coverage; not an external audit |
| [Latest qualification](simplification-20261101.md), [compact results](simplification-validation.json) | Exact runtime identity, checks, performance measurements and retained diagnostics |
| [Partitioner performance](performance.md) | Earlier matched component benchmarks and their limits |

Raw logs/datadirs are local-only under `artifacts/full-protocol/candidate` and
`artifacts/full-protocol/simplification-20261101`; another checkout cannot retrieve
them. Component benchmarks do not establish whole-node throughput or performance
of the entire feature PR against fork main.

## Compatibility and limits

No contract, key or genesis migration is required for baseline-compatible
networks. RPC validation version 8 remains separate from signing domain 7.
Upgrade Composer, signer and followers together; discard old proving sessions
and rebuild witnesses requiring manager data. Old followers cannot replay newly
admitted canonical shapes; downgrade after inclusion is unsupported. Existing
pre-baseline deployment boundaries still apply; no arbitrary migration was rehearsed.

ECDSA remains trusted attestation. The supported profile is one L2 and one proof
system, with existing gas/frame/witness bounds; explicit cross-chain call gas,
sender-bound batches and other L2 targets remain unsupported. Pending queues are
still in memory. Optional fixture/tooling exclusions are recorded in the compact
results and are not omissions from the 64-scenario suite.

## Preserved history and isolation

The [previous qualification and first failures](https://github.com/eez-association/eez-rollupX/blob/5d74a42ae8ae9fe2a79ab02fdf7c6e5c247e698d/docs/audits/2026-10-30-full-protocol-e2e/README.md)
retains the native explicit-target timing flake, high-concurrency fee-quote evidence
failure and links to the original dated investigation. Its
[validation identities](https://github.com/eez-association/eez-rollupX/blob/5d74a42ae8ae9fe2a79ab02fdf7c6e5c247e698d/docs/audits/2026-10-30-full-protocol-e2e/candidate-validation.json)
and [cold replay result](https://github.com/eez-association/eez-rollupX/blob/5d74a42ae8ae9fe2a79ab02fdf7c6e5c247e698d/docs/audits/2026-10-30-full-protocol-e2e/candidate-independent-replay.json)
are preserved at that immutable commit instead of duplicated in the current tree.
Earlier failed runs are not relabeled passing. The prior qualification separately
records socket-permission failures and unexplained outer-session exit-143 results;
the current runtime build, network runner and cold replay exited zero.

Work used the isolated `codex/full-protocol-e2e-20261030` worktree. Existing operator
work, deployments, wallets, configuration and datadirs were preserved. Owned
enclaves were stopped with data retained. No Chiado/public-network migration,
funding or deployment was performed.
