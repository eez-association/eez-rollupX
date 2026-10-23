# Portable v2 prover compatibility

| Field | Value |
| --- | --- |
| Design status | Accepted for this feature branch by the request to support externally implemented provers; implemented and locally validated; see the evidence record. Upstream PR #73 remains a draft design, not an accepted protocol. |
| Date | 2026-10-23 |
| Source baseline | RollupX `codex/l1-history-offline-proving`, `21fc597e8b3c6ee36525f1a0d1cce2c1e676b69a` |
| Protocol baseline | Published `feature/e2e_edge_cases`, `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Reference | [PR #73](https://github.com/eez-association/eez-rollupX/pull/73), head `d8f89eed6ffd8d287326c507931d8693105a7c40`, base `2febeb1003f98e6660fdf18a05cdec9b38d0e1c8` |

## Scope

Nethermind's team will implement its C# prover. RollupX must specify a portable
compatibility contract without requiring that implementation to reproduce Rust
build metadata. Keep the existing v2 schemas, complete offline witnesses, both
anchor modes and the fresh-genesis execution profile. This work does not provide
a Nethermind backend or expand the accepted proof-system/entry profile.

The reference PR changes documentation only and has one commit, no reviews and
no discussion as inspected. Its unary/preparation assumptions predate this
branch. Its useful requirement is independent implementation of the same rules.

## Requirements and acceptance

| ID | Requirement | Integration and acceptance |
| --- | --- | --- |
| P1 | A prover without persistent sessions can serve full requests. | Existing v2 Begin/Block/Finalize baseline; preserve and test bounded fallback for optional Resume, Rewind and subranges. No unary or preparation API. |
| P2 | Compatibility is independent of implementation language/dependencies. | Schema 6 hashes a specified RLP tuple with a protocol version and complete local configuration bindings. Fixed independently encoded vectors cover normal, zero-byte and full-width integer inputs. |
| P3 | Never omit execution or deployment rules to simplify identity. | Retain canonical full L2 document digest, offline profile commitment, rollup, system/EEZL2, registry and manager. Test every component and portable JSON edge cases. |
| P4 | Reject mismatching configuration before execution/signing. | Exact local comparison at Capabilities and every stream; service test sends missing/wrong identities before any frame. No identity learned from the remote response. |
| P5 | Keep resource planning coherent and finite. | Existing mandatory limits, shared capability snapshot and changed-limit rejection remain; existing client capability tests. |
| P6 | Semantics changes cannot silently reuse old execution facts. | Explicit protocol-version discipline plus a test-only Rust lockfile review guard. Changing dependencies requires review, not automatically a wire-identity change. |
| P7 | Configuration errors cannot evict or charge users. | Existing generic prover-error path requeues survivors unchanged. Preserve non-actionable capability errors and closed transport-retry classification. No new queue policy. |

## Design and alternatives

Adopt PR #73's portable compatibility objective while retaining its identity as
the existing 32-byte wire field. The normative preimage and vectors make it
implementable with ordinary Ethereum RLP/Keccak facilities. No new RPC version,
protobuf descriptor, identity selector or alternate execution mode is needed.
The transport hash deliberately cannot name which remote input differs; operators
compare the documented local inputs when diagnosing a mismatch.

The proposed small explicit field list is insufficient here: the full L2 rules,
history bootstrap/runtime, L1 rules and deployed manager must remain bound.
Keep canonical JSON version 1, documenting its exact byte rules; replacing it with
a second normalized chain schema would enlarge this change and risk dropping rules.
A protocol version replaces implementation-specific validator text and the runtime
Cargo.lock digest. The Rust dependency digest remains only in a local test guard;
an unrelated dependency change no longer makes another language incompatible.

Do not adopt optional/unbounded resource limits or revive the removed preparation
API. V2 already permits a sequential implementation and optional acceleration.
Keep all independent execution/settlement checks. In particular, PR #73's suggestion
that different execution rules cannot produce a valid proof does not hold for an
authorized ECDSA attester: the contract authenticates the attestation and bound
context, not its execution trace.

## Compatibility and recovery

The identity bytes change from schema 5 to 6; mixed peers fail before proving.
Roll out Composer and prover together with matching locally provisioned rules.
Discard retained sessions on a protocol version/configuration change. This is a
handshake change only: v2 wire fields, contracts, L2 execution, genesis, historical
decoding and database format do not change. The broader offline feature remains
new-network-only. Queue/retry state remains in memory; coordinated restart is
not durable recovery. A rollback requires both endpoints to agree again.

## Validation

Run identity/canonicalization vectors, existing v2 client recovery/capability tests,
all frozen native transcripts without regenerating expectations, affected protocol,
L1 and signer suites, workspace formatting/clippy and a network-denied real-signer
process regression. Existing full native vectors are portable acceptance material;
passing them against Rust is not evidence that Nethermind has implemented them.
External C# execution/proof interoperability remains the integrating team's check.

## Evidence

[The validation record](../audits/2026-10-23-portable-prover/README.md) maps the
requirements to source and tests, including the incomplete broad workspace run
and the external C# interoperability checks that remain with the integrating team.
