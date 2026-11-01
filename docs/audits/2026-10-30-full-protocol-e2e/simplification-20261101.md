# Execution-table cleanup, 2026-11-01

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

## Validation status

Qualification is in progress; older green results do not qualify this cleanup.
The task-local differential harness already matched exact results/errors on 28,672
inputs spanning ordinary, nested, failed, rollback and static groups. The baseline
verifier and its private ABI-lifting helper were copied unchanged except for module
paths; no legacy implementation is shipped. Benchmarks, final package/process
checks, complete protocol scenarios and cold replay are recorded after execution.

Raw evidence is local-only under `artifacts/full-protocol/simplification-20261101`.
The reproducible release harness is
[`entry_bench.rs`](../../../crates/eez-protocol/examples/entry_bench.rs); it measures
materialization including input cloning and verification of prebuilt tables.
These are component measurements, not whole-node throughput or a comparison of
the entire feature PR against fork main. Existing operator state is preserved.
