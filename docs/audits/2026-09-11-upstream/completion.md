# Upstream backport completion

The code ports cover all **12 upstream mainline units, 43 source commits, and
217 file-change records** in the frozen [inventory](inventory.json). All **44
native integration cases** now have passing evidence. The implementation and
local validation are complete; full release/Kurtosis CI remains a merge gate.

The working branch is `backport/upstream-2026-09-11`, created after switching to
`main`, fetching the fork, and fast-forwarding to
`dec50f91bce40d5282b24d7a102beebf2ce3de05`. Fork PR #14 is now merged, so its
CI, native-blob, protocol, and correlation changes are the integration baseline.

The accepted upstream target is
`c97b83f142b947243c2d7ef6a76b627d4bfcb71b`. The protocol pin remains
`3c0cdf4955941a5c23639346932b55b33babebbd`, whose tree equals protocol `main`
at `a7a0d7eb1d57b28340d16bd53877b6a08526172c`.
The live fork, upstream, and protocol revisions were rechecked at **16:51 UTC
on 2026-09-11** and had not moved.

## Coverage and adaptations

[completion.json](completion.json) maps every source commit and changed file to
implementation commits, current target paths, and Git blob identities. It also
maps all **45 upstream E2E/testkit test functions** to their retained or adapted
tests. Removed paths, package moves, and split files have explicit dispositions.

| Upstream PR | Final disposition |
| --- | --- |
| #108 | Retain actionable failures and request-local index/hash attribution. |
| #126 | Retain Foundry action update and fork CI/toolchain pins. |
| #107 | Retain explicit roles; #128 supplies the final package layout. |
| #124 | Add bounded prover/relay failure accounting, invalid-hint handling, FIFO recovery and nonce-suffix eviction. |
| #115 | Retain settlement, timing, startup, identity and gas hardening with fork commitments. |
| #89 | Add the fork-correct RPC spec and enforce canonical signatures plus local request-hash equality. |
| #117 | Complete the minimum viable gas ceiling and cross-language gas-pin checks. |
| #118 | Complete startup catch-up, same-L1-block checkpoint guard, funding rescan and deployment configuration. |
| #123 | Retain checkpoint-quota removal and current resource-capacity descriptions. |
| #114 | Remove the legacy production mock and require a real prover; use the real verifier in gas fixtures. |
| #128 | Extract common/follower/E2E packages, remove dev node, preserve fork observation and correlation behavior. |
| #103 | Extract testkit and port all scenario coverage, structured signals, CI diagnostics and state-chaining scripts. |

The fork's protocol behavior requires several deliberate adaptations:

- Settlement commits block and prefix-block hashes. Registry/safe-head assertions
  and structured signals compare those hashes; execution state roots remain
  internal execution evidence.
- Native blobs remain mandatory. Osaka fixtures and resumption use EIP-7594
  sidecars. The Beacon fixture derives legacy full-blob proofs from the same
  captured physical blobs and checks their commitments.
- Multiple outbound proxy calls are supported. The adapted upstream test checks
  ordered results and destination effects instead of expecting eviction.
- Caught destination reverts are authenticated results. Both adapted direction
  tests assert exact revert bytes, rolled-back destination writes, and a later
  successful control transaction.
- Settlement-oracle counts use the mined batch's ordered successful carriers.
  Failed frames roll back their events while their outcomes remain in the
  virtual applied-step count. Evidence is bounded by the next same-rollup batch
  in that L1 block, preserving duplicate roots and excluding unrelated events.
  Fully settled scenarios also require each outbound transaction's successful
  receipt to be included in canonical safe history.
- The unauthorized-signer fixture gives composer and signer the same wrong
  authority and verifier key, preserving a mined L1 `InvalidProof` assertion
  after the new local request-hash check.
- The synthetic resumption fixture retains its exact L1 skipped-prefix and
  two-new-entry assertions. Its extra root-only steps lack derivation sidecars,
  so the defensive fork must reject them. The test requires that precise error,
  removes the injected L1 block through a reorg, and verifies safe convergence
  and healthy settlement afterward. It does **not** demonstrate a positive
  valid-sidecar resumed-execution case.
- Persistent correlation history and complete same-L1-block tail restoration
  remain available in both roles. The older single-record checkpoint fallback
  gains upstream's safety guard.
- CI retains frozen shipping binaries, exact artifact provenance, all pinned
  protocol scenarios, and failure on tests that pass only after retry.

Open and unmerged upstream branches remain the separate intake list in the
original audit. They are not accepted upstream `main` changes. Existing
production-readiness work outside these upstream units remains tracked in
[`PRODUCTION_READINESS_TODO.md`](../../../PRODUCTION_READINESS_TODO.md).

## Verification

Commands, per-case integration revisions, and log hashes are recorded in
[completion.json](completion.json). Results are combined across the recorded
runs; fixture corrections were validated by targeted reruns.

| Check | Observed result |
| --- | --- |
| Rust unit/vector tests | 767 passed, including composer and the new settlement-oracle regression. |
| Native node integration | All 44 cases passed on the recorded successful attempts: 22 cross-chain, 5 chained-state, 16 embedded-node, 1 resumption. |
| Integration contracts | 19 passed, including real-verifier gas pins. |
| Core protocol contracts | 408 passed with the existing CI exclusions. |
| Protocol Anvil scenarios | All 28 passed. |
| Python CI tests | 170 passed in the default run; all 4 opt-in cases passed separately (174 distinct cases covered). |
| Real packaging/resource fixtures | Rust 1.94 archive relocation, Docker runtime and data-only images, and a uniquely scoped Kurtosis resource probe passed. |
| Compilation and formatting | Rust 1.94 workspace/all-target Clippy with warnings denied, isolated follower all-target compile, Rust formatting and whitespace checks passed. |
| Configuration and artifacts | Genesis check, Chiado blob configuration, mandatory prover configuration and both roles' system-key/address checks passed. |
| Dependency checks | `cargo deny check advisories bans sources` and `cargo audit` passed under the existing policy. |

The advisory scan retained **14 allowed warnings**, all on crate versions
already present in baseline `dec50f9`; their identities are recorded in the
JSON. No dependency-policy exceptions were added.

Runtime tests used Rust **1.96.1** debug builds with node/follower package
optimization lowered to fit available memory, frozen before test feature
unification. Runtime sources are unchanged since `57a294f`; subsequent code
commits affect testkit and integration fixtures. The completion verifier checks
that boundary. The packaging fixtures use CI-pinned Rust **1.94.0**, release
profile, Foundry **1.7.1**, and nextest **0.9.143**, in a tiny isolated workspace.

The first optimized local runtime build was killed while compiling the node;
the serial build with lower role optimization succeeded. Integration runs found
the Osaka sidecar mismatch, the successful-carrier/event-count assumption, an
incomplete safe-inclusion wait, and the negative-fixture adaptations described
above. Those attempts are retained in the validation history and are not
reported as passing runs. Every integration case has separate passing evidence
after its applicable correction; retry-only success is not accepted.

Recheck the coverage ledgers with:

```bash
python3 docs/audits/2026-09-11-upstream/verify.py \
  --repo . --protocol-repo eez-core-protocol
python3 docs/audits/2026-09-11-upstream/verify_completion.py --repo .
```

The original verifier needs the historical protocol Git objects recorded in the
inventory, including the content-equivalent protocol main revision. A clone
without those objects must fetch them first or point `--protocol-repo` at an
audit mirror containing them.

Before merging, run the existing CI workflow on this branch and require its
`CI Result` gate: runtime artifacts, lint, contracts, embedded E2E, cross-chain
E2E, and the full Kurtosis E2E lane. A complete release build and full Kurtosis
network workload were not run locally. The packaging fixtures and resource
probe do not replace those checks.

Coverage checks establish inventory and code identity; they do not prove that
the software is free of bugs.
