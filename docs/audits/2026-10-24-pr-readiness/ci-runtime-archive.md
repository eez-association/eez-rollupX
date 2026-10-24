# Release archive feature mismatch

The [first CI runtime producer](https://github.com/eez-association/eez-rollupX/actions/runs/40092563758/job/120868076808)
correctly failed its archive guard after rebuilding 136 external crates.
Shipping compilation succeeded in 26m34s; archive compilation succeeded in
22m26s, then the guard rejected it. This is a dependency-feature mismatch,
not a Rust compilation failure. The failed Docker stage did not export the
candidate or failure build metadata; its downstream runtime/native lanes did
not execute. The full log is local-only; its digest is retained in the
[portable record](ci-runtime-archive.json).

## Cause and correction

The exact selections in [Dockerfile.runtime](../../../Dockerfile.runtime) and
[native-tests.py](../../../scripts/ci/native-tests.py) produce different external
units at `d1838526480b89651f759d34e27bfbcfca43fb55`:

| Dependency | Shipping-only feature | Correction |
| --- | --- | --- |
| Tokio 1.52.3 | `test-util` | Remove the signer's dev-feature; keep its normal Tokio dependency. |
| Reth trie-common 2.7.0 | `default` | Select `std` explicitly, with defaults disabled. At this pin, `default = ["std"]`; both selections already included `std`. |

The shipping invocation mixes binaries and examples and activates these
signer dev-dependencies. They entered in `b6dca63a2c956ddba82ed2874c9286e4b91a146e`,
before the pending-RPC adapter. Tokio and trie-common are also the first two
external crates the CI log shows rebuilding. Adding test features to the
archive would preserve the contamination of shipping and is not the remedy.

Only one signer test used virtual time. It now holds the admission permit,
sets a 50ms budget for that blocked admission attempt, checks the exact
`DeadlineExceeded` rejection, and restores the fixture's original replay budget
before releasing the permit. The same session/epoch and retained tip remain
intact; no attestation occurs on rejection, and the retry still must return a
65-byte proof and exactly one signing call. There is no worker racing this
admission timeout. `Arc::get_mut` limits the test's mutation to its uniquely
owned runtime between operations. The independent trie fixture remains intact.

## Evidence and remaining gate

Cargo 1.95.0 unit graphs for the exact shipping and archive target selections
contain the same 1,136 external units after correction, with no differences in
features, profiles, target attributes or normalized dependency edges. The
before graphs differ directly only in the two features above; their transitive
effect changes exactly 136 external packages, matching the CI rejection.
The comparison preserves unit multiplicity and recursively hashes dependencies,
including distinct host/target profiles. This supersedes an initial count of
1,061 keys that collapsed 75 distinct units. The diagnostic uses
`RUSTC_BOOTSTRAP=1` solely to permit `--unit-graph`; it executes no compiler
units and is not a shipping-build flag. Full command arrays and source/graph
hashes are in the portable record; raw graphs are local-only.

The targeted debug regression passed: **1 passed, 321 filtered**, 2.17s after
a 3m34s debug build. The same retained test executable then passed the full
signer suite: **320 passed, 0 failed, 2 ignored**, 9.95s, without recompilation.
The ignored cases are fixture/vector generation utilities, explicitly named
in the portable record. Compilation used one job, LLVM 22 and a private temporary
directory; the memory guard observed at least 15.25 GiB available. Workspace
format and whitespace checks also passed. The source hashes stayed unchanged
through both runs.

The subsequent [remote producer](ci-final-runtime.md) confirmed release archive
cache reuse and passed the actual dependency guard. It then failed two unrelated
example fixture tests before publishing runtime/test payloads; that failure and
its [local correction](ci-prefix-fixture.md) remain explicit. Existing live
results identify the previous shipping binaries and do not validate new shipping
bytes. This feature correction changed no guard, exception, production Rust,
lockfile, version, protocol pin, schema or fixture expectation.
