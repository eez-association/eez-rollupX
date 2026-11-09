# Nested callback contract creation

| Field | Value |
| --- | --- |
| Design status | Requested PR #91 bug fix implemented and locally validated; required CI pending. This restores the pinned protocol's execution semantics, without changing its contract. |
| Date | 2026-10-18 |
| Source baseline | `eez-association/eez-rollupX` PR #91, `0bb1396d9cb660738b17d43a6cdbf9a564177823`; additional failing Gnosis CREATE/CREATE2 process tests |
| Protocol baseline | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Related work | [Reported issue](https://github.com/eez-association/eez-rollup0/issues/208), [inspector](../../crates/eez-evm-inspector/README.md), [execution-entry specification](../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md) |

## Problem and scope

An original L2 caller dispatches to L1, which calls an L2 factory. The factory
creates and reads a child successfully, but the original L2 caller sees no
code after the nested call returns. Both CREATE and CREATE2 fail through the
Gnosis process fixture; existing-contract controls settle. The overlay applies
storage and some balances but omits code and factory nonce updates. Its
outgoing snapshot also omits newly created accounts and constructor storage.

Fix this execution-state exchange in `eez-evm-inspector`, using REVM's journal
operations for rollback. Keep SELFDESTRUCT mutations in callback caches
unsupported, and preserve the existing protocol rejection of failed calls
with descendants. Transient-storage exchange and other protocol support
extensions are outside this fix. The original GPv2 transaction's unavailable
body prevents conclusive attribution of that historical incident.

## Requirements and acceptance

| ID | Invariant | Check |
| --- | --- | --- |
| R1 | A successful callback's created code, nonce, balance and storage are visible in the resumed EVM, including an address previously read as empty. | Journal tests; CREATE/CREATE2 process cases must settle and expose value 42. |
| R2 | Factory nonce changes survive the nested return and revert with the enclosing frame. | Journal tests and subsequent creation using the same factory. |
| R3 | An enclosing revert rolls back installed code, creation flags, nonce, balance and storage. | Explicit journal checkpoints and the existing overlay revert process controls. |
| R4 | A subsequent dispatch sees all live source changes, including new account code and constructor storage that never reached the DB cache. | Snapshot tests and repeated nested callback checks. |
| R5 | Unsupported destruction or unavailable new bytecode fails explicitly; unchanged/read-only accounts do not overwrite live state. | Negative and cold-account component tests. |

## Design

Build outgoing snapshots by applying the journal's live account changes to a
cloned cache, using REVM's cache conversion. This preserves new accounts and
their code/storage without reading stale database values or modifying the
source DB. The inspector already uses REVM's standard EVM state representation.

Refresh retained callback sessions before checkpointing the first callback in
each new source dispatch. The source may have written locally after the previous
callback returned. Once a callback has published within the current frame,
preserve the cumulative session cache, including queued rollbacks; replaying
either its original snapshot or its last published result would lose those
changes. The frame's existing result marker distinguishes these cases.

On return, compare against live source account information, including accounts
first loaded by the callback. Resolve changed bytecode from the returned
account or contract cache. Apply code, nonce and balance through journaled
account setters. Newly created contracts need a journaled creation checkpoint,
not just code installation, so later local execution and rollback retain the
account lifecycle. Preserve per-slot updates and the existing destruction gate.

Directly assigning account fields would break enclosing reverts. Copying only
the code would leave CREATE nonces, constructor storage and later re-entry
incorrect. Changing the protocol contracts or dependency versions is unnecessary.

## Compatibility and validation

There is no ABI, wire, storage-schema, configuration, protocol-pin, genesis or
contract-bytecode change. This changes Composer simulation, not native
canonical execution. Older Composers can still reject these newly supported
flows; followers and signers must continue validating canonical execution.
No data migration or public deployment is part of this change. An eventual
Composer restart retains the existing volatile-queue recovery limitation.

Run inspector and Composer unit tests, the Gnosis creation and follower-replay regressions,
applicable nested/rollback process tests, contract tests, formatting and
workspace Clippy. Retain the first failing baseline and compare against the
fixed source with exact executable identities. Record actual results separately
from this plan; do not treat the historical mainnet incident as solved.

## Implementation evidence

The [development validation record](../audits/2026-10-18-nested-created-account-fix/README.md)
retains the original missing-code failures, the first candidate's stale-session
failure, and the final passing checks: 28 inspector and 378 Composer unit tests,
89 integration-contract tests, six process regressions with semantic assertions
and independent follower replay, plus formatting and workspace Clippy. The
manual profiling test is excluded. The change has not been deployed.
