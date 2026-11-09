# Single profile and validation contract

Status: implementation and local validation complete; CI/external-engine limits below.
This records the incremental simplification after the
[fee-vault/RPC amendment](../2026-10-24-fee-vault-rpc/README.md).
Earlier results do not validate these new commitments.

## Source and accepted scope

- Worktree: `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`.
- Branch: `codex/l1-history-offline-proving`.
- Base HEAD: `3e23955dc477c9e675364776f118b1cecda75774`, plus the existing native/history/treasury work.
- Core gitlink and checkout: `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged.
- Rust 1.95.0, locked dependencies, Foundry 1.7.1, Solidity 0.8.34, via IR and optimizer runs 1.
- Fresh networks only; no deployment, publication, original-checkout edit or compatibility path.
- The [accepted proposal](../../proposals/native-system-transactions.md#manager-profile-and-portable-identity)
  and [identity contract](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md)
  define the resulting encoding and semantics.

## Changes and coverage

| Requirement | Implementation | Verification |
| --- | --- | --- |
| One profile without historical naming | Solidity `Profile` / Rust `OfflineProfile`; domain `EEZ_ROLLUPX_OFFLINE_PROFILE` | Exact ABI bytes and Keccak vectors; manager commitment and context tests |
| One validation-contract version | `VALIDATION_VERSION = 1`; ten-element RLP; remove schema/canonicalizer version fields and all caller arguments | Independent Python/Foundry RLP/Keccak vectors; every identity component and future-version mismatch tests; unchanged canonical JSON acceptance/rejection tests |
| One API with unchanged messages | Existing `prove.v2.Prover`, Capabilities and ProveStream | Proto digest equals the amendment's starting snapshot; no v1 module, alias or service |
| Recognize the current manager for fee estimates | Compiler-derived runtime fixture and normalized fingerprint | Solidity fixture/runtime equality and Rust changed-execution rejection tests |
| Retain independent offline validation | Existing exact identity comparison, execution, ancestry, modes, public inputs and signing checks | Three source process cases and all 29 frozen transcripts pass |

The version covers identity encoding, canonical JSON and validation semantics
together. Implementations cannot independently select them. Numeric wire
discriminators for native transactions, DA and capabilities retain their own
meanings; removing them would change those formats, not simplify this identity.
Domain separation, genesis/configuration binding and rejection of mismatched
identities remain mandatory. Historical audits retain their original identifiers
as evidence of their recorded revisions, not executable compatibility support.

## Executed checks

Logs under `artifacts/profile-simplification` are local-only ignored artifacts.
The starting source digests and status were recorded before editing. Tests using
fixed `/tmp` paths run with a private `bwrap` bind to worktree-local `TMPDIR` because
the host temporary-directory quota is exhausted; no shared directories are cleaned.

- `forge test --root contracts`: **142 passed**, no skips. The first run passed
  141 and correctly failed the runtime/fixture equality after the domain changed.
  Regenerating the actual compiler artifact and its Rust fingerprint fixed that
  failure; no assertion was removed.
- `cargo +1.95.0 test --locked -p eez-control-rpc -p eez-protocol -p eez-prover-client -p eez-l1 -p eez-composer --lib`:
  **851 passed**, one ignored Composer manual timing profile. This is not performance coverage.
- Independent Python RLP construction plus Foundry `cast` checks: all **27 native/profile encodings**,
  **13 relationships** and the three portable identities updated; Rust compares its encodings
  and hashes to the independent vectors. JSON acceptance/rejection vectors are unchanged.
- Starting-snapshot digests confirm that `prove.proto`, production/development
  genesis, Cargo manifest and lockfile are unchanged by this amendment.
- `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-protocol -p eez-blob-codec -p eez-payload-codec`:
  **581 passed**, including 320 signer cases and all 29 TCP/Stateless transcripts
  with 24 expected proofs. The two ignored signer utilities were separately run
  explicitly: capture inventory for each scenario and transcript generation.
  The 206 protocol unit cases overlap the earlier command; these are not 581
  additional distinct tests. Public-input, rolling-hash and Ethereum KZG oracles pass.
- `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings`: passed.
- Rust/Foundry format checks and `git diff --check`: passed.
- `python3 crates/eez-control-rpc/conformance/check_vectors.py`: all 29 vectors,
  fixture inputs and schemas match; all 169 recorded source digests also match.
- Edited relative Markdown file links resolve. No shell scripts, protobuf fields, dependencies, runtime options or submodule
  revisions changed.

## Source process evidence

Each scenario ran serially with prebuilt overrides cleared and
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. The harness built the independent signer
from source; L1 was local Anvil. Each command selected one real process test,
with 178 unrelated tests filtered and none ignored:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::minimal_bidirectional_cross_chain_smoke --exact --test-threads=1 --nocapture
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes --exact --test-threads=1 --nocapture
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::outbound_changing_static_reads_and_write_settle_and_replay --exact --test-threads=1 --nocapture
```

All three pass. They cover actual signature/context acceptance by the manager,
independent public inputs, caught-revert behavior and follower replay. Logs and
datadirs are retained in the local-only artifact directory above. Per-scenario
`EEZ_TEST_PROVE_FIXTURE_DIR` directories retained complete proving exchanges;
the repository's inventory utility selected the unique capture beginning at
block 1: bidirectional **1–90**, caught revert **1–105**, changing static **1–90**.
The latter contains all five outbound calls; the caught-revert capture contains
exactly one failed destination observation.

[Captures](captures.json) record request digests/ranges. [Executable identities](executables.json)
were recorded after each scenario, before the next Cargo invocation could replace
its binaries. The portable native fixtures contain complete requests, responses,
chain/genesis inputs and public test identities. Transcript expectations came
from those on-chain captures and the existing request-side subrange oracle,
not from accepting the replay service's output as the expected result.

## Fresh correctness review

Re-read the profile/identity specifications and both Solidity/Rust encoders,
the Composer and signer's independently loaded configuration, discovery and
stream admission, bytecode recognition, canonical JSON rules and fixture
provenance. The ABI profile tuple and every execution input remain bound;
the identity removes only independently versioned bookkeeping. A future contract
version still changes the identity. All malformed JSON tests remain, and absent,
empty, wrong-width or mismatched stream identity is rejected before session
creation without signing or attributing a removable source failure.

The review also corrected a documentation ambiguity left by the service merger:
identity metadata is required on `ProveStream`; `Capabilities` is the discovery
method used before agreement. No validation rule or runtime behavior changed
for that correction. All active historical profile/version names and v1 service
paths are absent. The profile domain remains distinct from context/rule domains.

[Source digests](source-sha256.json) identify the full uncommitted branch state
outside dated audits. [Incremental digests](incremental-sha256.json) distinguish
this amendment from the preserved starting worktree; old audit results are not
relabelled as evidence for the new bytecode or commitments.

## Compatibility and limits

The profile domain changes the manager bytecode, `protocolConfigHash` and proof
public inputs. The shorter identity preimage also changes the 32-byte validation
identity. Deploy matching manager, Composer, follower and signer artifacts for a
fresh network. Clients with a different identity fail the exact comparison;
there is no fallback, migration, retained-session reuse or rollback compatibility.
The protobuf schema, genesis contents and execution/fee/history rules do not change.
External provers update their identity calculation using the published vectors;
the gRPC messages and routes need no changes.

The full workspace test/E2E matrix, CI release/Kurtosis matrix, live deployment
and actual Nethermind/C# execution were not run for this amendment. Validation
uses the affected-package, protocol-oracle and source-process checks above;
the wire schema is unchanged and no external engine implementation is present.
No new signer RPC dependency is introduced.
