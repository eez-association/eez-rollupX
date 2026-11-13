# Minimal Quinn audit remediation

This is the owner's authorized follow-up to fix the dependency-audit failure in
[PR #171](https://github.com/eez-association/eez-rollupX/pull/171). It starts at
`4ffe60879b609725fbcf97327ef42b180cb606d3`; the Core gitlink remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. The earlier process tests, executable
identities and review reports remain evidence for their original revisions.

## Change and reproduction

Only two package versions/checksums change in the root and vendored-Reth
lockfiles:

| Package | Before | After |
| --- | --- | --- |
| `quinn-proto` | 0.11.18 | 0.11.19 |
| `quinn-udp` | 0.5.14 | 0.5.16 |

These are the minimum fixed compatible versions for
[RUSTSEC-2026-0524](https://github.com/RustSec/advisory-db/blob/4ef2df8e806a65e2f1ecc14f56f51249062b23d6/crates/quinn-proto/RUSTSEC-2026-0524.md),
[RUSTSEC-2026-0523](https://github.com/RustSec/advisory-db/blob/4ef2df8e806a65e2f1ecc14f56f51249062b23d6/crates/quinn-proto/RUSTSEC-2026-0523.md) and
[RUSTSEC-2026-0538](https://github.com/RustSec/advisory-db/blob/4ef2df8e806a65e2f1ecc14f56f51249062b23d6/crates/quinn-udp/RUSTSEC-2026-0538.md).
Quinn 0.11.9, both Reqwest versions, Reth/Stateless/Gnosis/REVM/Alloy, every
other package record and all dependency edges remain unchanged. No manifest,
production/test source, feature or advisory-policy exception changes.

Cargo's precise offline updates also reselected unrelated Windows and macro
dependency edges. Those unrelated edits were removed by retaining only the
resolved versions/checksums in the original package records. Both full
`cargo +1.95.0 metadata --locked --offline --format-version 1` calls accept the
result, including the vendored manifest. A structured comparison verifies that
all other fields and package records are identical.

An owned checkout of the official RustSec database at
`4ef2df8e806a65e2f1ecc14f56f51249062b23d6` supplies the same database for both
checks. Root `cargo audit --no-fetch --db <owned-db> --json` reproduces the three
unignored findings before the update (exit 1) and reports none afterward
(exit 0). The repository's existing accepted-advisory policy stays unchanged;
this does not establish absence of every vulnerability. `cargo deny --locked
check advisories bans sources` passes with that policy and an owned database
path; warnings remain visible.

The additional audit of the vendored test lockfile goes from four unignored
findings to one: pre-existing `libz-sys 1.1.28` / `RUSTSEC-2026-0418` remains.
That lockfile is not the root CI audit input. No exception or unrelated libz
upgrade is included, and its audit is explicitly **not passing**.

## Runtime and compatibility boundary

The dependency path is Reqwest -> Quinn -> the two updated crates. Both locked
Reqwest versions gate Quinn behind optional HTTP/3 support. The supported Linux
shipping graphs do not enable it. Before/after `cargo tree --locked --offline`
feature trees are byte-identical for the production package selection, that
selection including dev dependencies, and the three vendored Reth packages
including their test dependencies. None contains Quinn. These comparisons use
`x86_64-unknown-linux-gnu`, not arbitrary custom features or every platform.

All shipping binaries and examples build with Dockerfile.runtime's production
package/target selection, Rust 1.95.0, LLVM 22, two jobs and the checkout-owned
target. This local build uses the normal optimized dev profile. The resulting
Composer, follower and signer ELF digests are byte-identical to the recorded
pre-update files. No local release Docker image was rebuilt, and the prior
process matrix is not relabeled as a rerun with the new lockfiles.

The upstream fixes change QUIC behavior if HTTP/3 is enabled later. The unchanged
runtime conclusion here applies to the captured shipping configuration. The
vendored shutdown ordering and shared RPC-quota patches were reviewed and are
unchanged. There is no execution, decoding, witness, settlement, storage, ABI,
bootstrap or network-profile change. Validation version 9 and signing domain 7
remain unchanged. The [portable identity specification](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md)
explicitly records removal of the historical lockfile review guard; stale
AGENTS wording does not require inventing a new guard or semantic version bump.

No migration or coordinated signer upgrade is needed for this lockfile change;
the protocol identity remains compatible. CI must still produce and validate
new exact-revision artifacts with the new lock hash. The byte-identical local
ELFs do not authorize substituting old CI payloads. Rolling back the lockfiles
restores the audit failures.

## Validation and review

The [structured evidence](quinn-remediation-evidence.json) retains commands,
exit codes, source/lock/graph/executable identities and log hashes. It records:

- Root audit: reproduced 3 findings / exit 1, then 0 unignored findings / exit 0.
- Dependency policy: advisories, bans and sources passed with unchanged policy.
- Both locked metadata checks and all three before/after feature comparisons passed.
- All production-selected binaries/examples built; three runtime ELF hashes matched.
- RPC unit tests: 6 passed, zero failed/ignored/filtered, including both identity tests.
- Frozen-vector integrity: 29 vectors, fixture inputs and schemas verified; no regeneration.
- Workspace format and Clippy with warnings denied passed; diff checks passed.
- Vendored audit: 4 findings before, 1 afterward; the unrelated libz finding remains.

The seven new process tests and five existing process regressions remain the
earlier executed evidence. No process suite, full release/Kurtosis matrix,
external-client replay, public network or deployment is claimed for this
lockfile follow-up. At the preceding head, all eight embedded/cross-chain CI
slices passed; lint failed at the root audit and Kurtosis was still running.
Those are observations of that head, not results for the next revision.

Ponytail self-review uses the previously recorded guidance revision
`96608e078b2d5ea607c5c3b8b5a54d1c1fba6846`.
What this change does: it removes the three root audit findings by updating two
unused optional transport packages, preserving every other lock record/edge.
The intended load and worker/queue ownership remain unchanged because no
compiled dependency or production code changes. The original failure is
reproduced, the same database verifies the correction, and shipping graph/ELF
comparisons establish the bounded runtime-impact conclusion. No additional
abstraction, fallback, policy exception or version bump is needed.

The final exact-model adversarial review will be recorded separately after
reviewing this frozen scope.
