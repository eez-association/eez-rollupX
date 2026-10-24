# Selective Tokio security update

Status: Tokio update implemented and validated; the separate crypto-bigint
integration blocker remains unresolved, 2026-10-24 UTC. The starting branch and
protocol identities are in [the campaign record](README.md).

The workspace minimum and lockfile move Tokio from 1.52.3 to 1.53.3, the compatible
fixed release for [RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html).
The alternate 1.51.6 line is below pinned Reth/Gnosis requirements. The
[release changelog](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/CHANGELOG.md)
also includes timer, wakeup, semaphore and shutdown fixes; their behavior matters
to the existing streaming and worker-lifecycle regressions.

## Resolution and semantic boundary

`cargo +1.95.0 update -p tokio --precise 1.53.3` resolved the fixed package without
upgrading another package. Cargo also reselected unrelated dependency edges
within already locked versions. Those edge changes were not retained: the exact
Cargo-resolved Tokio package block was combined with the preceding lockfile's
other blocks, and full `cargo +1.95.0 metadata --locked --format-version 1`
accepted the result. A structured comparison establishes that only Tokio's
version and checksum change; its dependency list is unchanged.

| Identity | Before | After |
| --- | --- | --- |
| Lock SHA-256 | `aa8b6ba7699104a5923775aa94cff71a71c7f9ecbfc11d874359704819f32058` | `6eb07deb642781d89e044eecb4323fd238c389d244a0f2d4250837e3ee63bb04` |
| Lock Keccak-256 | `ee4e2fdddf84af928f69b8c730783d67c5edf4836a03ca6bee6cb2e72fb44071` | `a347d3b4c7b08028dc310b4b1f4af9635add6955ea3b7be612cd1ec649a21f35` |

No EVM, transaction codec, signature implementation, proof/public-input formula,
canonical JSON rule, profile, Solidity source or protocol gitlink changes. The
local dependency-review guard is updated, while `VALIDATION_VERSION = 1` and the
v2 service remain unchanged. This is a review conclusion about protocol inputs
and outputs, not a claim that scheduler timings remain identical. Runtime tests
and the new executable identity are required for operational validation.

The workspace has no direct `AsyncFd` construction. The inspected interprocess
2.4.2 unnamed-pipe adapters register a private `FdOps(OwnedFd)`, borrow it for
I/O, and consume `AsyncFd::into_inner` before transferring ownership out. This
review does not add new unsafe calls or claim that a deprecated API is safe for
arbitrary external callers. Pinned Reth's IPC path uses local sockets.
The source scan also found inotify 0.11.1's event-stream constructor. Its `stream`
feature is absent from the resolved graph; notify 8.2.0 uses the synchronous
inotify/mio path. This distinction is based on resolved features and callers,
not merely the package appearing in Cargo.lock.

## Remaining cryptographic dependency gate

The [previous assessment](../2026-10-24-pr-readiness/dependency-remediation.md)
still applies to `crypto-bigint 0.5.5`: elliptic-curve 0.13.8 requires 0.5, while
the advisory's fixed range begins at 0.7.6. A fresh crates.io index and upstream
tag check found no newer 0.5 release. Updating only the direct signer would leave
old transitive users, and changing the full pinned API family is not a selective
security update. No advisory exception, package-version disguise or unchecked
cryptographic patch is introduced. This remains an integration blocker.

The read-only registry recheck at **2026-10-24 20:16:41 UTC** still finds 0.5.5
as the latest 0.5 release; the latest registry version overall is 0.7.7. The
[crates.io index entry](https://index.crates.io/cr/yp/crypto-bigint) response has
SHA-256 `a841c4712064efd3059524b76ec189ce3a4a54e6bb4047e7173a2e945b42c64e`.
The local-only response summary is
`artifacts/extended-validation/dependencies/crypto-availability-final.json`.
This check changes neither lockfiles nor installed packages.

## Evidence

Local-only original/resolved lockfiles, resolver log, metadata and structured
lock comparison are retained under `artifacts/extended-validation/dependencies/`.
The [portable audit comparison](dependency-results.json) uses advisory database
`21d909e4c8bd4e14c7aea58bf980c881a0053ab1` for both lockfiles. Tokio's advisory
disappears. `cargo audit --no-fetch --json` still exits 1 for crypto-bigint and
reports its pre-existing informational warnings; `cargo deny --locked --offline
check advisories bans sources` exits 1 with crypto-bigint as its sole fatal
advisory. The original unsupported CLI option and summary-parser diagnostics
are retained separately from those actual results.

`cargo +1.95.0 test --locked --offline -p eez-control-rpc --lib` passes **7/7**,
including the frozen portable identity vectors, byte decoding and reviewed-lock
guard. It used one build job and the established task-private `/tmp` bind,
finished in 36.01 seconds and retained at least 15,791,149,056 available host
bytes. The memory guard did not cancel it. Workspace format checking also passed.
`cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings`
also passes, in 230.05 seconds with one build job and a minimum of 15,013,871,616
available host bytes. Its local process guard did not cancel it.

CI `40116791686`, head `053668df` and synthetic merge
`abf3d1bc82bebe6ac3bb3ea213d9e8c272df8c20`, passes **1569/1569** debug
unit tests with three explicit skips. The signer rewind/cancellation, shutdown
and backpressured-publication regressions, and the prover-client deadline and
witness-owner cancellation regressions, passed with Tokio 1.53.3. The local
selection record is
`artifacts/pr-readiness/ci-candidate/40116791686/debug-classification.json`;
the retained debug log SHA-256 is
`f1c8361785824fd8ef8a6e286a5933f35c6dd872dea441c2aee5d7f849f2fa7d`.
The separate Reth cleanup check passed **1/1**, but that
revision's nested vendor lock still selected Tokio 1.52.3: it does not establish
cleanup behavior with the updated scheduler. The pending-debug correction aligns
that nested lock. The repeated cleanup regression now passes **1/1** with
Tokio 1.53.3; its command, explicit profile and result are in the
[pending-debug verification](pending-debug.md#executed-local-checks).

The producer subsequently passed **38/38**, including its archive feature and
frozen-runtime checks. The exact merge checkout consumed and verified the
runtime payload `sha256:0dcb612bd8de95110ad355deb1805b929e80fbb6126a5911307755f77e70517b`
and test payload `sha256:8eec3afc676b5bd5e94de45335eb13200f01ef21dfb6786a915831f1997b3a59`
from the repository's `eez-rollupx-ci-runtime` and `eez-rollupx-ci-tests` GHCR
images. All seven extracted executables and all 30 test-payload files passed the
existing identity/integrity checks; the test runtime matches the packaged
runtime. The local-only handoff is
`artifacts/extended-validation/ci-40116791686/artifacts/ci-consumer/verified-handoff.json`.
Its first invocation omitted required `GITHUB_SHA` and stopped before checking
identity; supplying the exact workflow merge corrected that invocation.
All eight process partitions subsequently passed **174/174** (43 embedded and
131 cross-chain), with five explicit ignored tests outside selection. The live
Kurtosis lane passed its four initial workloads but was cancelled when the
pending-debug candidate superseded this CI; it did not finish protocol/replay
validation. These binaries do not contain the later pending-debug
correction; earlier binary results do not validate later changes.

The final runtime candidate `28049ff734ab931cbde01246c5d238cce6ef65bf`,
CI `40123208325` at merge
`c35cf5bc71548f74b8d72e3fa2a0a66113eab0c8`, passes **1571/1571**
workspace unit tests with three explicit skips. Both separate Reth regressions
pass **1/1** with Tokio 1.53.3 in the root and nested lockfiles, including the
shared tracing quota and cleanup-before-shutdown acknowledgement. The debug
log SHA-256 is
`632d8a3fccd04066b2e7fbffca089c20dfa762a68ec275dc64ef3334b65e41be`.
Its dependency gate still fails solely on crypto-bigint's advisory; bans and
sources pass. The fatal advisory prevents subsequent CI audit/tooling/Clippy
steps in that lane, so those steps are not CI coverage. Local checks retain
their separate identities. The final producer passes **38/38** and publishes
matching runtime/test payloads; process and live validation must bind to those
bytes, not the preceding Tokio-only candidate.

The [completed final CI](ci-final.md) subsequently passes all eight process
partitions (**174/174**) and the Kurtosis lane, including **34/34** pinned
protocol scenarios with independent semantic/DA verification. Its global result
remains failed solely because of the cryptographic dependency gate; execution
passes do not waive that security finding.
