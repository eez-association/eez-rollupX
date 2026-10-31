# Partitioning simplification and performance

Comparison baseline: `1ff8619d0be29c0f1ab4376d2a76737552d3c61f` (the green PR before this cleanup).
Measured implementation: `e51a2477d98662be2b33c0f17b3bc4c0f0921d20`.
Protocol remains `84a5b5207880f192a6a5896cbfe0bac5249db47f`.

The cleanup centralizes complete-load metadata, removes six temporary metadata
vectors, and avoids call-hash cursor inference when complete groups already fix
the partition. Callers no longer repeat the guaranteed coverage-total check, and
successful nested calls no longer copy rollback facts they never restore.
All host-shape checks precede partition acceptance; the signer
still decodes its own loads and independently verifies execution and effects.
No wire, storage, signing-domain or RPC-version change is introduced.

## Matched measurements

Rust 1.95.0, locked dependencies, release profile, Intel(R) Core(TM) Ultra 9 185H, CPU 21.
Ten alternating baseline/candidate pairs used the same CPU, workloads and iteration
counts. Fixture construction, caller metadata extraction and nested-call snapshots are outside the timed
region. No operator workloads were stopped. Both binaries asserted expected results.

| Workload | Baseline median (µs) | Candidate median (µs) | Change |
| --- | ---: | ---: | ---: |
| single | 1.827 | 0.084 | -95.40% |
| many_hosts | 443.696 | 4.554 | -98.97% |
| large_calldata | 2275.195 | 0.784 | -99.97% |
| static_pool | 1.459 | 0.906 | -37.89% |
| nested_rollback | 571.418 | 2.261 | -99.60% |
| split_mutable | 109.989 | 108.973 | -0.92% |
| split_rollback | 572.963 | 570.658 | -0.40% |

The two split-host paths are unchanged within the observed run-to-run variation;
their sample ranges overlap. No slowdown was measured in these cases. Large
complete-group gains remove previously discarded calldata hashing; they are not
whole-node throughput or settlement-latency gains. This does not establish
performance for every workload or compare the entire feature PR against fork main.

Run the [microbenchmark](../../../crates/eez-protocol/examples/partition_bench.rs)
with `cargo +1.95.0 run --locked --release -p eez-protocol --example partition_bench`.
For the baseline, use the same harness with the old slice argument (`&groups`)
in place of `groups.iter().copied()`; keep fixtures and iteration counts identical.

## Correctness and qualification

A task-local differential harness compared the exact baseline partition function
with the candidate on 100,000 deterministic inputs: 21,091 accepted and 78,909
rejected, with identical result vectors or error strings in every case. The old
implementation is retained only as a local diagnostic artifact, not a production
compatibility path. Focused regressions cover malformed hosts on the optimized path.

All 1,149 affected-package tests and 1,641 workspace unit tests passed; five fixture
utilities and one manual timing probe were ignored as in existing CI. Formatting,
workspace Clippy, signer all-feature Clippy and warnings-denied rustdoc passed.
Release checks, the complete 64-scenario confirmation and independent follower
replay passed. All 192 native E2Es executed; one first-run timing failure passed
its diagnostic rerun. The first protocol run also failed evidence collection after
a fee-quote retry. See the [audit summary](README.md) for both retained failures,
confirmation evidence and hosted-CI status.

## Evidence

Raw samples, the differential harness and build/test logs are local-only under
`artifacts/full-protocol/simplification-performance`; another checkout cannot
retrieve them. The first benchmark build failed because its synthetic call fixture
used an unavailable `Default` implementation; explicit fields corrected the harness
before measurements. Lint checks also caught closure/cast style issues, corrected
before the final runs. A workspace test identified the obsolete signal declaration
for a removed duplicate check; its unused declaration was removed and all workspace
tests passed. No behavioral regression test was weakened.

A shared Cargo target directory initially reused the baseline executable alias for
the candidate. Hash comparison caught this before final timing; a scoped protocol
release clean/rebuild produced distinct binaries. The final ten pairs below use
those verified binaries, with identical harnesses apart from the old/new API argument.

- Baseline benchmark SHA-256: `d3848d3d6492dc365ba5c2826251ebff92996d8bdb3ddbe4ec94e7797aa7372f`.
- Candidate benchmark SHA-256: `674f0be613bae18985f2c71563921135dc4923f45671a8aec60cf172cf45f925`.
- Raw measurement JSON SHA-256: `af63082e64a9b8c5aa4cf92d8065228d79d617c08f6d202e96f0fdbdc9ec64af`.
