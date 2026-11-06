# Composer attestation quorum

| Field | Value |
| --- | --- |
| Design status | Proposed implementation; upstream acceptance pending |
| Date | 2026-11-06 |
| Source baseline | `5cabdad9b2ba13c5cfb4c2f7dfa5623f862bd30a`, clean |
| Protocol baseline | `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Related work | [Multi-prover contract](../../eez-core-protocol/docs/MULTI_PROVER_SPEC.md), [RPC contract](../../crates/eez-control-rpc/SPEC.md), Rollup0 PR #171 |

## Problem and scope

The reference manager accepts M-of-N distinct proof systems, but the Composer
connects to one remote attester. Add bounded parallel attestation collection for
the existing native blob settlement and offline `prove.v2` profile. Keep Core,
the protocol pin, the wire schema, execution rules and signer authority unchanged.
This is a Composer feature; it does not deploy or register production attesters.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | Read the manager, threshold, member keys and accepted signers at one L1 anchor for each quorum attempt. Never count an unregistered or wrong-profile key. | Registration tests, threshold updates, native on-chain settlement. |
| R2 | Count only distinct proof systems with valid low-s ECDSA signatures over independently computed native public inputs. | Wrong digest, signer, key, signature encoding and duplicate-member rejection tests. |
| R3 | Give each prover the same settlement and physical blobs, naming only its own proof system. Merge verified responses in strict proof-system order. | Per-member request assertions and final on-chain verification. |
| R4 | Retry members independently within the existing shared proof deadline and post-proof reserve. A member must not outlive its attempt. | Controlled pending futures, cancellation and paused-clock deadline tests. |
| R5 | Continue after member failures when quorum remains possible. Candidate eviction requires a matching blocking set of attributable failures. | 2-of-3 failure, disagreement and blocking-set tests. |
| R6 | Preserve single-prover defaults, per-prover validation identity and cold backfill. Prevalidation failure cannot authorize signing or admission failure. | Existing single-prover tests, configuration precedence and fanout tests. |
| R7 | Budget calldata and verifier work for all configured members before selecting a batch. Retain final native execution validation. | Sizing regression and native process tests. |

## Design and alternatives

`EEZ_PROVERS` selects a bounded list of `url=attester=proof_system` entries.
Unset selects the existing single-prover settings. Explicit empty/malformed lists
fail startup. The new list takes precedence over the legacy endpoint and signer;
the configured legacy proof-system address remains the deployment/discovery default.
At most 16 distinct nonzero proof systems are allowed.

The launcher derives each member's network-profile verification key and checks
the shared validation identity before sequencing. It intersects member block
ceilings and fans best-effort prevalidation to independent sessions. Durable
witnesses remain the source of complete Finalize backfill.

The Composer owns quorum policy, registration reads, proof deadlines and failure
tallies. The remote client retains wire handling and its independent response
verification. Registration failures requeue work without charging candidates.
Every member request uses a singleton proof-system assignment, which preserves
the signer's existing structural gate. Native public-input hashes bind the same
physical blobs and proof context; only the per-system key accumulator differs.

Once threshold is reached, collect additional valid responses for up to 250 ms
(configurable), bounded by the remaining shared deadline. Dropping collection
cancels all member exchanges. A started blocking blob-commitment calculation can
finish with its owned input after cancellation. No detached RPC tasks or unbounded
member work are introduced.

Requiring every configured member would let one failed member block a valid
on-chain quorum. Copying another settlement profile would lose native blob and
anchor bindings. Changing the signer to accept a multi-system request would
unnecessarily enlarge its trust boundary.

## Compatibility and recovery

Existing single-prover configurations keep their behavior and wire contract.
Enable the member list only after deploying compatible signers and registering
each proof system with the correct profile key. The owner sets the on-chain
threshold. No chain or database migration is introduced. Rollback to an older
Composer requires restoring a threshold it can satisfy; unsettled requests remain
subject to the existing in-memory queue/restart contract.

## Validation plan

Run Composer, launcher and native public-input unit tests, then process tests
with separate real signers, 2-of-2 and 2-of-3 policies, and L1 proof acceptance.
Run existing signer/client/session tests unchanged. CI owns the full release and
Kurtosis matrix. Report static checks, Rust compilation, runtime tests and live
deployment separately; a written test is not execution evidence.

Operator procedure: [Composer attestation quorum](../runbooks/attestation-quorum.md).
