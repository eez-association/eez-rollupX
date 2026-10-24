# Selective Tokio security update

Status: implementation and validation in progress, 2026-10-24 UTC. The starting
branch and protocol identities are in [the campaign record](README.md).

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
Signer/client lifecycle, shipping artifacts and live tests remain pending on
this update. Earlier binary results do not validate it.
