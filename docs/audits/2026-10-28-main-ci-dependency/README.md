# Main CI dependency-policy failure

Status: diagnosed and narrowly repaired locally, 2026-10-28 UTC. Candidate CI
remains required. This investigation does not establish runtime or deployment
validation for the candidate.

| Identity | Value |
| --- | --- |
| Repository / baseline | `eez-association/eez-rollupX`, `b832ad6a4764c9796eb61f86dcd6a29d85f72c77` |
| Protocol gitlink | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged; no core changes or local protocol tests |
| Isolated branch | `codex/main-ci-failure-20261028` |
| Main failure | [run 40659694662, Lint job 122719326039](https://github.com/eez-association/eez-rollupX/actions/runs/40659694662/job/122719326039) |
| Prior green PR #115 | [run 40613598671](https://github.com/eez-association/eez-rollupX/actions/runs/40613598671), head `f965725872be23dc514911abe73877053b4f7f65`, base `d5b6d3c9efa6ecdd8147edab532ff913f425426f` |
| Prior green PR #116 | [run 40628160846](https://github.com/eez-association/eez-rollupX/actions/runs/40628160846), head `f0ed4d8cca9b541d93945e6216d0fe6ef07ad5be`, base `6215df1a4395f58bb58bdb78bf4dc22f907c4169` |

## Cause and timing

The main job fails on **RUSTSEC-2026-0414**, affecting locked
`pulldown-cmark 0.13.4`. Its final dependency-policy result is `advisories FAILED,
bans ok, sources ok`. The later `cargo audit` and Clippy steps therefore did not
execute in that job. The existing zstd exceptions are unrelated and unchanged.

The [official RustSec advisory](https://github.com/RustSec/advisory-db/blob/40f5be6fb12ed93fc591bf4ab23b7d793b3f2239/crates/pulldown-cmark/RUSTSEC-2026-0414.md)
reports exponential parser event growth when wikilinks are enabled and identifies
0.13.5 as the patched release. Its advisory date is October 18, but it entered the
database much later:

| Event | UTC time, 2026-10-28 |
| --- | --- |
| PR #115 dependency policy passed | 01:17:26 |
| PR #116 dependency policy passed | 03:46:54 |
| Advisory content added, commit `ee8c4fd031654f4fc56f02e571186fcb3a5a5129` | 08:43:48 |
| ID assigned, commit `40f5be6fb12ed93fc591bf4ab23b7d793b3f2239` | 08:50:03 |
| Main dependency policy failed | 08:55:19 |

All three jobs used cargo-deny **0.19.8**, cargo-audit **0.22.2**, Rust **1.95.0**,
and Ubuntu 24.04 image **20261012.334.1**. PR #115 and main have the identical
Git tree `f25fb93a15a8d4b8ff84cc67b32f5f3ea3027aec`; PR #116's lockfile, audit
configuration and CI workflow also match. All three logs contain the same 112
cargo-deny `unresolved-workspace-dependency` diagnostics, including the green
jobs. They do not explain the new advisory failure.

The green logs report 1,380 advisories but do not record a database commit. A
local reconstruction at pre-publication database commit
`99670cfc0bc574433034b75876a4b791ccaa2efb` also reports 1,380 and passes the
baseline lockfile. The same lockfile fails at
`40f5be6fb12ed93fc591bf4ab23b7d793b3f2239` (1,381 advisories), with 0414 as its
only non-exempt vulnerability. This establishes a newly published advisory
against an existing dependency, rather than a merge-induced dependency change.

## Narrow correction and semantic review

`cargo +1.95.0 update -p pulldown-cmark --precise 0.13.5 --offline` selected the
compatible patch. It also reselected an unrelated `data-encoding-macro-internal`
edge from syn 2 to syn 1. That incidental change was discarded: the exact
Cargo-resolved pulldown-cmark package block was retained in the original lockfile,
and full `cargo +1.95.0 metadata --locked --format-version 1` accepted the result.
A structured comparison confirms that only this package's version/checksum
change; every dependency edge and other package record remains identical.
The nested Reth lockfile does not contain pulldown-cmark and is unchanged.

The resolved graph reaches this parser only through
`eez-control-rpc [build] -> tonic-prost-build 0.14.6 -> prost-build 0.14.3`, also
through `pulldown-cmark-to-cmark 22.0.0`. The enabled `cleanup-markdown` feature
parses protobuf comments with `Options::all()` minus smart punctuation, so the
affected wikilink option is enabled. The input is the checked-in
[`prove.proto`](../../../crates/eez-control-rpc/proto/prove.proto), processed by
[`build.rs`](../../../crates/eez-control-rpc/build.rs); there is no resolved
shipping request-parser path through this dependency. Optional dependency
declarations elsewhere are not selected graph edges.

The official 0.13.5 crate archive checksum matches the lockfile and records
upstream commit `48a2767b5b8ab0a4893e80d942e510065889fc6b`. The inspected source
diff changes Markdown parsing internals, including wikilink and list handling;
it does not change this crate's dependency list. Regenerating with each locked
version produces byte-identical `prove.v2.rs`, including generated documentation:
SHA-256 `d69840cb22b68e83cb7250a566a58599c4d6b269ac301fb0505e4101ddeea966`.

Under the [validation identity contract](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md),
only the local reviewed-lock digest guard is updated. `VALIDATION_VERSION = 6`,
the protobuf schema, canonical identity vectors, validation/execution rules,
Reth/Stateless/Gnosis/REVM/Alloy pins and security exception lists are unchanged.
No new advisory ignore or check bypass is introduced. No wire/storage migration,
upgrade order or rollback restriction is introduced by this build-only patch.

## Executed validation and evidence

The same pinned advisory database and CI audit-tool versions were used for both
lockfiles, in task-private caches. Builds used Rust 1.95.0, one Cargo job, and
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.

| Check | Baseline | Candidate |
| --- | --- | --- |
| `cargo deny --locked --offline --format json check advisories bans sources --hide-inclusion-graph` | Exit 1: only fatal advisory 0414 | Exit 0: all three policy groups pass |
| `cargo audit --db <pinned-db> --no-fetch --json` | Exit 1: 0414 | Exit 0: zero non-exempt vulnerabilities; existing warnings retained |
| `cargo +1.95.0 test --locked --offline -p eez-control-rpc --lib` | 7 passed, none ignored/filtered | 7 passed, none ignored/filtered |
| `cargo +1.95.0 check --locked --offline -p eez-control-rpc` | Generation captured during baseline tests | Passed; generated Rust/docs identical |
| `cargo +1.95.0 clippy --locked --offline -p eez-control-rpc --all-targets -- -D warnings` | Not repeated | Passed |
| `cargo +1.95.0 fmt --all --check` | Not repeated | Passed |
| Bounded upstream wikilink reproduction: 16 copies of `[[a\|]] `, 112 bytes | 327,676 parser events | 65 parser events |

The reproduction compiles the same small program against each actual built
parser library. Its full five-input results and executable/library hashes, lock
digests, audit summaries and log hashes are recorded in [results.json](results.json).
It is an upstream-parser diagnostic, not a new runtime or network test.

Raw logs, metadata, generated files and the diagnostic program are retained in
the isolated checkout's ignored `artifacts/ci-investigation/` directory and are
**local only**. Public CI logs remain retrievable from the runs above subject to
GitHub retention; their SHA-256 digests are in the portable result record. An
initial post-change cargo-deny invocation put `--metadata-path` in the position
used by local version 0.20.2, which CI's 0.19.8 rejects; its exit-2 CLI diagnostic
is preserved separately. The recorded before/after checks use the same supported
command and each ran once.

Full workspace Clippy/tests, signer/process suites, independent follower replay,
Solidity, genesis and live-network checks were not repeated locally for this
build-only change. Focused compilation, wire/identity assertions and generated
source comparison passed; candidate release/native/Kurtosis CI remains separate
evidence. No deployments, service restarts, wallet operations or operator checkout
edits were performed.
