# Dependency gate: remediation assessment

Read-only assessment, 2026-10-24 UTC. The [gate evidence](dependency-gate.json)
records both advisories in the candidate and `origin/main` at
`a50c326d99f7cd1c21406eedbf370a344015a6b5`. Candidate initial HEAD is
`3e23955dc477c9e675364776f118b1cecda75774` plus the audited patch; protocol remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. This assessment used Cargo.lock SHA-256
`53fa3e897a06b1ddbac3a742630befae3e2cff45b15b7cdb6ceb37c991d1f696`.
The subsequent [pending-RPC correction](pending-rpc-dependency-edge.json) adds
only the existing `reth-tasks` dependency to `eez-node-common`, producing SHA-256
`aa8b6ba7699104a5923775aa94cff71a71c7f9ecbfc11d874359704819f32058`.
All external versions, sources and checksums remain identical; neither advisory
is remediated by that edge.
Advisory database revision: `21d909e4c8bd4e14c7aea58bf980c881a0053ab1`.
Inheritance from main does not waive either failure.

## Tokio: compatible selective-update candidate

[RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html)
lists `>=1.53.3` and `>=1.51.6,<1.52.0` as fixed. The 1.53.3 update adds unsafe
registration APIs and deprecates constructors whose safe interface cannot express
the required file-descriptor ownership. Deprecation does not establish the safety
of existing callers; inspect those obligations in addition to clearing the gate.
See the [reviewed fix](https://github.com/tokio-rs/tokio/pull/8587).

**Recommend a selective update from 1.52.3 to 1.53.3 in a dependency follow-up.**
The [workspace](../../../Cargo.toml) permits Tokio 1. Pinned Reth `3d592ec`
and reth_gnosis `9517ba3` specify caret `1.52.3`; all inspected cached registry
consumer manifests permit 1.53.3. The alternative 1.51.6 line violates those
minimums. The [tagged 1.53.3 manifest](https://raw.githubusercontent.com/tokio-rs/tokio/tokio-1.53.3/tokio/Cargo.toml)
fits Rust 1.95; its mio/socket2/tokio-macros requirements are already represented
in the lockfile. This is a static compatibility assessment, not a solver/build
result.

The resolved graph contains `interprocess 2.4.2` with old `AsyncFd` constructors
in its unnamed-pipe implementation. Reth IPC enables its Tokio feature but uses
local sockets in the inspected production code. Neither fact establishes an
exploitable path or absence of one. Review actual descriptor ownership before
changing callers. Retest signer cancellation/permit retention/shutdown, TCP
streaming conformance, prover-client witness ownership, Composer timers/retries
and socket shutdown/churn. The [release changelog](https://raw.githubusercontent.com/tokio-rs/tokio/tokio-1.53.3/tokio/CHANGELOG.md)
also contains timer and semaphore fixes relevant to these paths.

## Crypto-bigint: no compatible lock-only update established

[RUSTSEC-2026-0387](https://github.com/RustSec/advisory-db/blob/21d909e4c8bd4e14c7aea58bf980c881a0053ab1/crates/crypto-bigint/RUSTSEC-2026-0387.md)
specifies `>=0.7.6` as fixed. The current path is `eez-proof-signer` →
`eez-protocol` → `k256 0.13.4` → `elliptic-curve 0.13.8` →
`crypto-bigint 0.5.5`. The [protocol signer](../../../crates/eez-protocol/src/signer.rs)
actually signs with k256; indirect dependency status is not a safety argument.
The advisory concerns timing behavior; this review does not demonstrate an
exploit or prove its absence.

`elliptic-curve 0.13.8` requires crypto-bigint `0.5`, excluding 0.7.6. Alloy
signers and other consumers also require the old elliptic-curve/k256 family;
updating only the direct signer or adding a second fixed copy leaves old users.
The [k256 0.14.0 manifest](https://raw.githubusercontent.com/RustCrypto/elliptic-curves/k256/v0.14.0/k256/Cargo.toml)
shows an API-family migration, including elliptic-curve 0.14.1, ecdsa 0.17 and
signature 3. Its resolution through this pinned Alloy/REVM/Reth graph has not
been demonstrated.

**Recommend a dedicated cryptographic dependency migration assessment before
integration**, or a maintained API-compatible 0.5 security backport if one becomes
available and is recognized by the advisory. None is established by the current
fixed-version range. Do not force 0.7.6 under the 0.5 constraint or add an ignore.
Preserve existing digest, 65-byte signature, low-s, recovery-id and public-input
oracles; check transaction/EIP-7702 recovery and applicable ECRECOVER/P256
precompiles for semantic/gas changes. Repeat offline streaming and real accepted
proof/follower tests. Review the [upstream constant-time fixes and checks](https://github.com/RustCrypto/crypto-bigint/pull/1365)
for the deployment compiler/target; functional tests alone cannot prove timing
safety.

## Integration boundary

No Cargo command, dependency/lockfile update, advisory exception, binary rebuild
or exploit test was performed for this assessment. The graph was reconstructed
with Python `tomllib` from Cargo.lock and checked against cached pinned manifests;
feature presence alone is not proof of runtime reachability.

Both gates remain blocked. Remediation must preserve the currently retained
validation artifacts, then rebuild and rerun affected checks plus deny/audit on
the new lockfile. The [identity guard](../../../crates/eez-control-rpc/src/identity.rs)
requires a dependency-semantics review and explicit digest update; it does not
automatically require a wire-version change. Change the validation version only
if accepted execution/decoding semantics change. Existing binary results do not
validate a future dependency update.
