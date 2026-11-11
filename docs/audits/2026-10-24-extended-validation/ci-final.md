# Final runtime CI: 28049ff7

[CI run 40123208325](https://github.com/eez-association/eez-rollupX/actions/runs/40123208325)
completed on 2026-10-24 at 20:44 UTC. Every execution lane passed. The aggregate
CI result remains **failed** because Lint rejects the inherited
`crypto-bigint 0.5.5` advisory `RUSTSEC-2026-0387`; no exception or promotion was
added. See [dependencies](dependencies.md) for that unresolved integration gate.

The [portable inventory](ci-final.json) records all jobs, 174 distinct process
tests, 38 producer checks, the requested concurrency regressions, exclusions,
commands, artifact identities and log hashes. It covers head
`28049ff734ab931cbde01246c5d238cce6ef65bf`, checked out by CI as synthetic merge
`c35cf5bc71548f74b8d72e3fa2a0a66113eab0c8`. Its tree equals the PR head's tree;
the other parent is `a50c326d99f7cd1c21406eedbf370a344015a6b5`. The protocol pin
is `84a5b5207880f192a6a5896cbfe0bac5249db47f`. Later working-tree test/documentation
changes are outside this CI identity.

## Executed checks

| Lane | Result and selection |
| --- | --- |
| Runtime producer | PASS; 38/38 vectors, codec, causal-evidence and stateless-prefix tests |
| Workspace debug | PASS; 1,571/1,571, with three explicit utilities/profiles ignored |
| Reth debug regressions | PASS; shutdown cleanup 1/1 and aggregate tracing quota 1/1 |
| Embedded process tests | PASS; four partitions, 43/43 |
| Cross-chain process tests | PASS; four partitions, 131/131 |
| Kurtosis waves/state chaining | PASS; four wave modes, independent signatures and nine state-chain cases |
| Pinned protocol on Kurtosis | PASS; 34/34, followed by independent evidence verification of all 34 |
| Quote trace viewer | PASS; ten model and two browser tests |
| Lint / aggregate gate | FAIL; the known crypto-bigint advisory |

The producer took 24 min 29 s to compile shipping binaries and 15 min 38 s to
compile the test archive. Only six workspace crates compiled during archiving;
the external-dependency guard and frozen shipping-byte comparisons passed.
Consumers used that producer's immutable runtime/test payloads without rebuilding
Rust. The local consumer also verified seven binary hashes, thirty archived
files, payload linkage, image bytes and genesis/profile outputs. Its completed
handoff digest and both immutable payload references are recorded in the JSON.

Both root and nested Reth locks use Tokio 1.53.3. CI actually selected the three
requested signer lifecycle tests, two prover-client cancellation/deadline tests,
two node RPC cancellation/quota tests, Reth shutdown and Reth shared tracing
quota. The JSON preserves their individual PASS records rather than inferring
coverage from a successful build.

The final public pending/debug process regression passed in **9.697 s**. The
unchanged delayed-start mixed-wave admission regression passed in **76.571 s**.
The earlier b019 admission failure remains an unresolved intermittent failure;
this pass does not diagnose or erase it. See [admission evidence](dynamic-admission.md).

## Kurtosis scope and exclusions

Kurtosis completed at 20:44:18 UTC. The four wave modes exercised 4 inbound,
4 outbound, 8 mixed and 24 mixed-pure operations, with four independent signature
checks. The state-chain checks covered source, destination and mixed state in
both directions, poison-call recovery, mixed directions and repeated identical
calls. Canonical commitment heights and the distinct ordered heights used by
the fee-funded mixed directions are retained in the JSON.

The upstream scenario runner reported 34/34 PASS. Its independent verification
then reported complete evidence, causal coverage and semantic blob coverage,
with valid accounting. The names exactly match the default selection at the pin:
**34 selected of 64 discovered**, with **30 explicit-only exclusions**. Every
excluded scenario and its upstream reason is listed in the JSON. The separate
[local campaign](kurtosis/README.md) additionally selects cases explicitly;
its coverage must not be folded into this default CI count.

Five tests are outside the eight process partitions: the four-day stress run,
Ethereum/Gnosis large-history cold checkpoints, retained-history cold checkpoint,
and the Kurtosis cold-follower replay. The per-shard skipped counts mostly denote
other shards and must not be summed as extra omissions. This Kurtosis CI lane
checks stateless execution of protocol prefixes but does not consume the test
archive or invoke that ignored cold-follower process. Explicit local replay and
sustained-workload results remain separate evidence.

The optional Kurtosis OpenTelemetry sidecars could not start because their
Docker client requested API 1.51 while the daemon supported at most 1.48.
The lane continued and passed; it does not validate that optional telemetry path.

## Remaining limits

Lint stopped before `cargo audit`, tooling/configuration checks and Clippy in
this run. Separate local results are recorded in the owning
[validation audit](README.md); they are not relabelled as CI passes. The three
debug exclusions are fixture generation, fixture inventory and a manual timing
profile, each named in the JSON. No process test was retried in this CI run.

Raw logs are retained locally under ignored
`artifacts/pr-readiness/ci-candidate/40123208325/lanes`; another checkout does not
contain them. The JSON supplies their hashes and public job links, subject to
GitHub retention. The local read-only observer once exited with signal status
143 and was restarted; it did not cancel or retry any remote job. The completed
logs, rather than an inferred intermediate state, establish these results.
