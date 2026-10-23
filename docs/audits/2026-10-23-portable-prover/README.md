# Portable prover compatibility and scoped cleanup

| Field | Value |
| --- | --- |
| Date and status | 2026-10-23; implementation and scoped local validation complete; ready for PR review subject to CI |
| Source | Dedicated branch `codex/l1-history-offline-proving`, clean baseline `21fc597e8b3c6ee36525f1a0d1cce2c1e676b69a`, plus this patch |
| Worktree | `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`; original checkout untouched |
| Protocol | Published `feature/e2e_edge_cases` at `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean |
| Scope | Four approved cleanups and adaptation of PR #73 for an external Nethermind/C# prover; no external backend, new RPC version, deployment or existing-network migration |
| Sources | [PR record](upstream.json), [proposal/requirements](../../proposals/minimal-prover-interface.md), [previous feature review](../2026-10-23-offline-proving-pr-readiness/README.md) |

## Implementation and review

PR #73 has one documentation commit, no implementation/tests and no review
discussion at the inspected head. Its baseline predates the current v2 profile.
The portable compatibility objective applies; its removed unary/preparation
interface does not. [The proposal](../../proposals/minimal-prover-interface.md)
records each adaptation and the distinction from upstream R1–R7.

The production identity retains all network/deployment/profile bindings, replaces
Rust revision/lockfile inputs with a versioned validation contract, and leaves the
protobuf schemas unchanged. Canonical JSON rules are specified byte for byte;
independent Python RLP encoding and Foundry Keccak produced three frozen identity
vectors and Unicode/large-number JSON vectors. A Rust-only lockfile guard requires
semantic review without imposing Rust build metadata on external provers.

The full request remains mandatory, with complete offline L1 evidence, both anchor
modes and delayed PREVRANDAO. Optional session control fallbacks are preserved.
No signature, settlement or consensus check was removed. An authorized ECDSA
attester is trusted to execute these checks; upstream's suggestion that L1 itself
prevents a differently executing attester from signing an incorrect transition
does not hold for this proof backend.

The four cleanups remove the client-only StreamingError wrapper, share the
unchanged bounded rule-epoch predicate/constant, correct a block-hash comment and
duplicate testkit docs, and remove the one-off obsolete fixture deletion.
The manager fixture is refreshed because a Solidity source comment changes CBOR
metadata; its 6,372-byte executable is exactly unchanged (6,425-byte full runtime).
Genesis, core gitlink, dependency locks and v2 field definitions remain unchanged.

An initial suspected configuration-error accounting defect from PR #73 was
disproved by tracing the current production caller: `PreparePostBatchError::Prover`
requeues the complete survivor list without modifying attempt counts.
The client already suppresses actionable details on a marked configuration error.
No new error variant or queue policy is needed. The draft's old charging path is
not silently reintroduced.

## Requirement coverage

| Requirement | Implementation path | Evidence |
| --- | --- | --- |
| P1 / upstream R1–R2 | Existing v2 Begin/Block/Finalize and bounded optional-control fallbacks in `eez-prover-client` | Client optional-control/restart/pipeline tests; native service transcripts |
| P2–P3 / R3 | `eez-control-rpc/src/identity.rs` and `VALIDATION_IDENTITY.md`; canonical full L2 document and offline profile retained | Frozen RLP/Keccak vectors, every-component mutation test, portable canonical JSON vectors |
| P4 / R4 | Existing client capability comparison and signer stream metadata admission | Malformed/mismatching capabilities; pre-frame service rejection without signing |
| P5 / R5 | Existing effective limits/snapshot check, unchanged schemas | Capability bounds/change tests; fixed native quota transcripts |
| P6 / R6 | `VALIDATION_SPEC_VERSION` plus test-only reviewed lock digest | Dependency review guard; version namespace separation |
| P7 / R7 | Existing Composer generic prover error retention and client typed-error protection | Client mismatch-details and closed-status tests; production-call review; native opaque rejection retention |
| Cleanup | Shared epoch checks; direct ProverResult; no obsolete generator migration | Epoch boundary cases; v2 recovery suite; frozen fixture integrity; contract tests and executable equality |

The difference from R4 is explicit: preserving the wire hash means mismatches
cannot name a differing remote field. Operators compare the documented local
preimage fields. R5's optional/unbounded limits are not adopted. R6's local review
guard is conservative over the full lockfile; dependency changes with demonstrated
equivalent semantics need not change the portable protocol version.

The final fresh review traced identity construction from local configuration through
capability discovery and stream admission, checked unchanged signing/settlement
bindings, optional-control fallbacks and Composer ownership, and compared the
patch with every PR #73 requirement. It found no further applicable omissions or
unrelated implementation changes. The external engine and CI limits below remain
explicit; passing Rust conformance does not establish C# interoperability.

## Compatibility and remaining work

Schema 6 changes only the handshake namespace; Composer/prover must agree before
proving and retained sessions must be discarded on a namespace change. No L1/L2
RPC, protocol hard fork, schema/database migration or historical decoding change
is introduced by this follow-up. The larger feature remains fresh-network-only.
Contract metadata changes, so any deployment mechanism deriving addresses from
that manager's full initcode must use the rebuilt artifact.

Nethermind's team must still implement and validate its engine/server using the
[external integration guide](../../../crates/eez-control-rpc/conformance/IMPLEMENTING.md).
C# message generation does not establish C# service compilation or correct execution.
No dotnet SDK or external prover implementation was available here. Release-artifact,
full native and live Kurtosis/deployment validation remain CI/integration work.
The optional four-day stress test is excluded; it is not required to validate an
expired L1 blockhash. Existing shorter multipart tests exercise that boundary.

## Executed checks

Completed checks:

- Focused RPC/client/L1/protocol packages: 455 passed.
- Composer library: 406 passed, one manual profiling utility ignored.
- Signer binary crate: 327 passed, two explicit fixture utilities ignored.
  All 29 frozen v2 cases were matched to actual passing test names; expectations
  were not regenerated.
- Workspace/all-target clippy, signer all-target check, signer all-feature clippy,
  signer rustdoc with warnings denied, and formatting passed.
- Integration contracts: 120 passed. Manager executable equality passed.
- Both schemas generated C# messages with `ulong` and `ByteString` fields;
  this checks generation, not .NET compilation or external execution.
- Frozen-vector integrity, changed Markdown file links and three conformance
  shell examples passed their checks.
- Five selected source-based native cases passed with 173 filtered out and zero
  ignored: bidirectional settlement, malformed batch rejection, opaque rejection
  retention, bounded multipart catch-up, and expired-source multipart catch-up.
  Signer outbound network access was denied. Both anchor modes, complete independent
  follower replay, delayed history/PREVRANDAO application observations and malformed
  evidence rejection were exercised. The optional four-day stress was excluded.
  The process test selected exactly these five cases, not a passing empty filter.

The broad `cargo test --workspace --exclude eez-node-e2e` invocation exited 143
(SIGTERM) during compilation, before any tests executed. Its cause was not
established; this is not a green workspace run or a test failure reproduction.
The smaller follow-up with `--lib` selected only Composer: the signer is a binary
crate, so its coverage was obtained with a separate unfiltered package invocation.
No runtime test needed a retry. The first identity-vector failure against schema 5
is the intended before-change regression evidence, not a flaky result.

A scratch documentation-placeholder assertion rejected pre-existing section signs
(§6/§8); those are valid prose. No source correction was needed. The actual
file-link and Bash syntax checks passed.

Exact commands/results, source and executable identities, and sanitized evidence
digests are recorded in [validation.json](validation.json). The three runtime
executables are source-built debug/test artifacts; they are not the shipping
release artifacts. The harness rebuilt the standalone signer before use; its
pre-run binary hash is not the identity of the signer that executed the tests. Large logs and
datadirs under ignored `artifacts/` are local-only, not retrievable from another
checkout. This record does not replace the earlier dated protocol/E2E evidence.

## Continuous-production requirement

A subsequent user requirement makes uninterrupted ordinary L2 production during
L1 delivery failures mandatory. The [continuity investigation](../2026-10-23-l2-continuity/README.md)
identifies baseline production gates and a proposed protocol revision. It supersedes
PR readiness for that expanded scope; the passing checks here remain evidence for
this record's source, not proof that the new liveness requirement is implemented.
