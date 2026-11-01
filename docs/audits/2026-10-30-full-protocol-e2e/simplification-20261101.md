# Execution-table cleanup, 2026-11-01

Implementation: `372cb0741bd7e3a925137836d12d79911c6da573`.
Baseline: `77f1113e679865c129cd7db00d98c99f5378c263`; fork base:
`c702f90da19cc149890d0a17a4ef6c200dcb6a21`; unchanged protocol:
`84a5b5207880f192a6a5896cbfe0bac5249db47f`.

## Scope and independent soundness review

The public APIs, accepted inputs, exact commitments, wire/storage formats and
signing identity are unchanged. Tables are borrowed for verification; mutable
execution cursors never mutate the tables. Checkpoints retain hash-prefix length
and both value totals instead of copying accumulated hashes. Hashes only append
between properly nested checkpoints. Coverage of executed rows/calls is separate
and is not rolled back. Mutable and static materialization borrow recorded actions
and child indexes. Delivery entries share entry-opening bookkeeping but retain
successful delivery with an empty result when the destination call fails.

A separate manual adversarial review traced both signer callers in
`settlement/inbound.rs` and `settlement/outbound.rs` through independent table
verification and observation authorization. This is not an external audit.
Borrowing does not promote decoded input into execution authority. The removed
second root-hash comparison is implied by selecting the source with that exact
hash; no intervening operation changes the immutable root or source. The host's
single call cursor replaces redundant traversal bookkeeping. Every nested frame
closes before the root loop resumes; final cursor, coverage, rolling-hash, routing,
value, depth and static-candidate checks remain mandatory. No check against actual
execution, signed transactions, receipts or independently recomputed public inputs
was removed. The rejection/signing boundary is unchanged.

The new regression commits a prefix, executes nested rollback scopes, then commits
a suffix. It checks exact surviving event hashes and net value, retains all six
rows' coverage, and rejects forged return bytes inside the rollback region. It
also passes on the unchanged baseline. The existing signer forged-outcome and
missing-code regressions remain required before qualification.

## Validation

| Check | Result on `372cb074` |
| --- | --- |
| Workspace unit/bin tests | 1,641 passed; six optional profiling/fixture utilities ignored |
| Affected packages | 683 passed; five fixture utilities ignored; includes 338 signer units |
| Native process suite | 192 distinct intended cases, all first-run passes across eight shards; no omissions or duplicates |
| Release protocol/blob checks | 56 passed, zero skipped |
| Differential verifier comparison | 28,672 exact result/error matches: 3,847 accepted and 24,825 rejected |
| CI tooling | 423 passed; seven optional infrastructure probes skipped |
| Formatting, workspace/signer Clippy, signer rustdoc, SDK, configuration, RPC vectors and secret scan | Passed |

The new rollback regression also passes against the unchanged baseline. Existing
signer forged-outcome, missing-code and rollback rejection regressions passed;
they assert rejection before signing. The differential harness copied the old
verifier and private ABI-lifting helper unchanged except for module paths. No
parallel legacy verifier ships.

The complete pinned suite passed all 64 scenarios with zero failures, skips,
exclusions or omissions. Accounting, causal coverage and semantic native-blob
verification are complete. All four traffic modes and state-chaining checks passed.
A cold follower replayed canonical L1 data through L2 block 1,458, independently
checking 102 scenario-range blocks and 101 receipts across 17 settlement batches
covering all 64 scenarios. The proof logs contain 76 signed windows and 76 accepted
remote attestations, with zero canonical divergence events. The owned enclave was
stopped with data retained; existing deployments and operator state were preserved.

[Prior compact qualification evidence](https://github.com/eez-association/eez-rollupX/blob/5d74a42ae8ae9fe2a79ab02fdf7c6e5c247e698d/docs/audits/2026-10-30-full-protocol-e2e/simplification-validation.json) binds these results
to the runtime source. [Hosted CI](https://github.com/eez-association/eez-rollupX/actions/runs/41313363564)
also passed all 13 jobs on documentation follow-up `5d74a42a`; see the parent audit.

## Matched component performance

Ten alternating release-build pairs on CPU 21 compare the previous PR head with
this cleanup, using Rust 1.95.0, locked dependencies and identical workloads. The
final measurements ran after compilation and before process tests. Existing
operator workloads remained running. Values below are median microseconds.

| Workload | Materialize baseline → candidate | Verify baseline → candidate | Verify change |
| --- | --- | --- | --- |
| failed_frames | 692.532 → 688.685 | 430.423 → 404.410 | -6.04% |
| large_results | 3033.900 → 3018.646 | 1831.308 → 1756.516 | -4.08% |
| many_roots | 517.433 → 521.060 | 462.365 → 345.584 | -25.26% |
| nested | 694.054 → 690.102 | 429.894 → 406.797 | -5.37% |
| recursive_static | 386.885 → 381.347 | 224.627 → 216.656 | -3.55% |
| rollback_roots | 525.370 → 523.841 | 463.761 → 342.517 | -26.14% |
| single | 9.400 → 9.320 | 8.444 → 6.596 | -21.88% |

All verifier sample ranges are separated. Materialization ranges overlap: changes
from −1.43% to +0.70% are within the observed noise. The reproducible harness is
[`entry_bench.rs`](../../../crates/eez-protocol/examples/entry_bench.rs). It includes
input cloning in materialization, excludes deferred L1 trace finalization, and
verifies prebuilt tables. This is neither whole-node throughput evidence nor a
comparison of the entire feature PR against fork main. Production code is 14 lines
smaller in this pass; the regression, benchmark and evidence increase total PR size.

## Retained diagnostics and evidence limits

The first sandboxed workspace run could not bind 31 local mock-server sockets;
the unchanged suite passed with local networking enabled. The artifact-build tool
session reported exit 143 after successful export. Its cause is unresolved. A
cached producer confirmation exited zero and produced identical runtime and test
manifests; all validation uses those verified bytes. The first E2E payload export
omitted `CARGO_TARGET_DIR`; correcting the extraction path exported the existing
verified archive without rebuilding. Exploratory benchmark samples overlapped
compilation and are retained but excluded from the performance claim. The network
runner also wrote its complete passing result before the outer tool
session returned 143; the wrapper's final exit file was not written. This outer
session termination remains unexplained and is not relabeled a clean process exit.
A separate proof-path/zero-divergence check and the cold follower test each exited
zero against the retained network and exact artifacts. No test or
validation rule was weakened to obtain these results.

Raw logs, samples and datadirs are local-only under
`artifacts/full-protocol/simplification-20261101` and
`artifacts/full-protocol/candidate/revision-372cb074`; they are unavailable from
another checkout. Earlier dated failures remain in the parent audit and do not
qualify or invalidate this source's independently executed results.

SHA-256 identities:

- Baseline benchmark: `a5b720e94f35f36305458bdcf0cdaac83a141ab8340007a6d9c3863c85104c50`.
- Candidate benchmark: `e63522a7c03ea34e41cbfbbe15859e631b009f5fa2d0f715f209d9acc4f9114d`.
- Final benchmark results: `99ff860554cb2106fe0c5843703b63acfaa3486b8cc57b85df75b625b8c28389`.
- Differential results: `cd0c25225360b6d5529f3829eb821c8535396564e96640b2ef4a8594b58d10bb`.

## Root-planning experiment against `73458a03`

Both requested prototypes were implemented. Inline L2 root-slot assignment removes
79 production lines, two planning structs and intermediate assignment/slot lists.
Existing rollback scopes now restore the source cursor for root-level regions;
nested callback scopes must not reset that root cursor. Matching slots still require
identical outcomes, successful roots advance the cursor, and static reads observe
its pre-call position. Independent verification and signer authorization are unchanged.

Shared construction/finalization folding saved only five further production lines.
It was rejected after ten alternating release measurements found approximately 3%
higher median finalization time for `many_roots` and `rollback_roots`. The existing
benchmark now measures finalization explicitly so this cost remains observable.
The retained root-only prototype's materialization medians varied −0.56% to +1.07%
against the baseline, with overlapping sample ranges; independent verification
varied −0.25% to +0.53%. These are component measurements, not whole-node claims.

A separate manual signer review found no change to authorization gates: exact
source outcomes, position-bound rows, replay observations and independently computed
commitments remain mandatory. Table construction cannot authorize signing. The
rollback-observation regression asserts the specific outcome mismatch and zero
signing calls. This review is not an external audit.

The retained candidate passed 1,643 workspace tests (six optional utilities ignored),
685 affected-package tests (five fixture utilities ignored), formatting, workspace/
signer Clippy and signer rustdoc. Differential comparison matched all 16,384 cases:
10,536 accepted with identical tables/traces and 5,848 rejected with identical errors.
The new rollback/static-read regression also passed on unchanged `73458a03`.
Runtime `fbd0d238b9e75c1af68bbb31e7f210c49cc9ad8c` also passed all 56 release
checks and all 192 distinct native cases on their first run, with no missing or
duplicate cases. All 64 protocol scenarios passed with zero skips/exclusions and
complete accounting, causal coverage and semantic blob verification. A cold follower
replayed canonical L1 through L2 block 1,500, checking 150 scenario-range blocks
and 101 receipts across 24 batches covering all scenarios. Proof logs contain 82
signed windows and 82 accepted remote attestations, with zero divergence. The
runner and independent replay both exited zero; the owned enclave was stopped
with data retained. See [current compact evidence](simplification-validation.json).

Tooling finished with 423 passes and seven optional infrastructure probes unrun;
the two initially skipped secret-scan tests passed with the existing Gitleaks on
PATH. SDK/configuration and redacted commit scanning passed. Artifact initialization
first failed because the new task-local temporary directory did not exist; creating
it allowed the unchanged producer to pass. No product test required a retry.
The net production reduction is 79 lines; 80 regression-test lines, benchmark
coverage and this evidence increase the total diff. No contract, key, wire, storage
or genesis compatibility change is introduced by this cleanup. Raw prototype sources, differential harnesses, benchmark samples
and logs are local-only under `artifacts/full-protocol/structural-simplification`.
The retained prototype's benchmark result SHA-256 is
`5e8517855d23c6f434f940581bbb63ddeb03b24a7ffd2f93dd5652d623080208`.
