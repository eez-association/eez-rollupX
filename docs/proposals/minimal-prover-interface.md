# Minimal prover interface

| Field | Value |
| --- | --- |
| Design status | Draft. Not accepted; no implementation exists. |
| Date | 2026-10-08 |
| Source baseline | `eduadiez/rollupX` `main` at `2febeb1003f98e6660fdf18a05cdec9b38d0e1c8`; line references below are at this commit |
| Protocol baseline | `eez-core-protocol` `855fe0602484750861b4c31502a1a91355d33ed6` (unchanged) |
| Related work | [RPC specification](../../crates/eez-control-rpc/SPEC.md) §2.1, §3, §6; [signer specification](../../crates/eez-proof-signer/SPEC.md); [validation identity contract](derivable-chain-operations.md#51-internal-rpc-extensions) (introduced in PR #26); [`identity.rs`](../../crates/eez-control-rpc/src/identity.rs) |

## Problem and scope

The Composer should be able to use any prover that executes the same rules,
not only the signer built from this repository. An external team (Nethermind)
has a prover ready to integrate. Three properties of the current interface
prevent that or make it unnecessarily costly.

1. **All four RPCs are effectively mandatory.** The client accepts a prover
   only if `preparation_max_blocks` is in `1..=64`, `maximum_leases` equals
   `MAX_PREPARATION_LEASES`, the cache fields are nonzero and the lease
   lifetime is at least `max(2 × request timeout, 120 s)`
   (`crates/eez-prover-client/src/connection.rs:107-128`). The node then runs
   a background worker that sends every ordinary block to `PrepareOrdinary` and
   `RetainOrdinaryRanges` (`crates/eez-node/src/lib.rs`, preparation worker
   around lines 160-380). A preparation failure makes the witness store report
   not ready (`crates/eez-node/src/witness_source.rs:167-181`), which pauses
   admission. The `prove` path already has a branch for a prover without
   preparation (`crates/eez-prover-client/src/lib.rs:627`), but the handshake
   makes that branch unreachable.
2. **The validation identity cannot be computed by another client.** It is a
   Keccak hash over an RLP tuple that includes `keccak256(Cargo.lock)` of this
   workspace (`identity.rs:19`), a digest of the chain document produced by the
   repository's canonical JSON rules, and an implementation-shaped revision
   string (`identity.rs:13-14`). The Composer requires an exact match at startup
   (`connection.rs:102`) and on every `Prove` header. The identity is also
   repeated in preparation messages, cached-range references and cache-miss
   details.
3. **`Capabilities` negotiates 14 fields, but the `Prove` path uses few of
   them.** `Prove` uses the operations tag and profile, the identity,
   `max_window_blocks` (window planning; `eez-node/src/lib.rs:144`) and
   `max_message_bytes` (the gRPC encode limit; `connection.rs:63`). The
   request byte and witness-item limits are used only to plan preparation
   chunks (`lib.rs:422-430`). The timeouts are used only to validate the lease
   lifetime. The cache and lease fields exist only for preparation.

A further defect found while tracing item 2: when the signer reports a
configuration change on a request (`FAILED_PRECONDITION` with
`eez-capabilities-changed`), the client returns `ProverError::Backend`
(`lib.rs:794-795`). The Composer handles that like any prover failure and
charges every candidate one settlement attempt. A misconfigured prover can
therefore cause user transactions to be charged and, after repeated windows,
evicted.

**Why a simpler check is safe.** Settlement correctness does not depend on the
identity. The signer re-executes every block, and L1 verifies the proof against
block hashes it commits itself. A prover that executes different rules cannot
produce a valid proof for a different state; it can only fail to prove. The
identity check is an operational guard: it turns a misconfiguration into a clear
startup error instead of failed windows that could be charged to users.

**Scope:**

- Make preparation an optional extension, so the minimum prover implements
  `Capabilities` and `Prove` (item 1).
- Replace the 32-byte identity hash with a few explicit chain identity fields,
  and check them only at the handshake and once per `Prove` request (item 4).
- Reduce `Capabilities` to a small required set, with the rest optional or
  extension-only (item 5).
- Never charge users for a configuration mismatch.

**Non-goals:**

- Removing the preparation path. Whether it is needed at all is a measurement
  question and a separate proposal.
- Changing the failure model (proof, retry, rejected entry). Separate proposal.
- Changing the witness format sent by `Prove`. Whether an external prover can
  consume Reth-style augmented execution witnesses must be confirmed with that
  team separately.
- Multi-prover quorums and on-chain verification of non-ECDSA proofs.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | A prover that implements only `Capabilities` and `Prove` and reports `preparation_max_blocks = 0` passes the handshake. The node does not start the preparation worker, admission readiness does not depend on preparation, and every `Prove` request streams all window blocks. | Process test: the signer runs with preparation disabled; `e2e::` and a `cross_chain::` subset settle every window. No `PrepareOrdinary`/`RetainOrdinaryRanges` call reaches the signer. |
| R2 | With `preparation_max_blocks > 0`, preparation behaves as today. The cache, lease and timeout fields are required only in this case. | Existing preparation unit and process tests pass unchanged. |
| R3 | The chain identity consists of explicit fields that another implementation can compute from the published genesis file and specification, without this repository's build or JSON canonicalizer. | Unit vectors: published genesis plus specification produce the expected field values. |
| R4 | The identity is compared at the handshake and once per `Prove` request, and nowhere else. A handshake mismatch prevents node startup. A request mismatch returns the existing configuration-change response. Every mismatch message names the differing field and both values. | Unit tests for each field; a process test with a wrong chain ID fails startup with a field-named error. |
| R5 | `Capabilities` requires only the identity, `max_window_blocks` and `max_message_bytes`. Request byte and witness-item limits are optional. Preparation fields are required only when preparation is enabled. | Handshake table tests for absent, zero, invalid and conflicting values in both profiles. |
| R6 | `spec_version` changes whenever validation or execution semantics change, including through a dependency upgrade. | A CI guard test fails when the locked versions of execution-critical crates change without a `spec_version` change. |
| R7 | A configuration mismatch, at the handshake or on a request, is never charged to user transactions. Survivors are requeued without charge. | Composer unit test: a configuration-change response leads to requeue without a settlement-attempt charge. |

## Design and alternatives

### Item 1: preparation as an optional extension

- **Client:** `validate` accepts `preparation_max_blocks = 0`. In that case the
  cache, lease and preparation timeout fields must be absent or zero and are
  ignored. Values `1..=64` enable preparation, and the current checks for the
  other fields apply.
- **Node:** start the preparation worker only when preparation is enabled.
  Durable witness capture still runs, because `Prove` needs the witnesses. With
  preparation disabled, `preparation_error` stays empty, so
  `WitnessStore::ensure_ready` does not depend on it.
- **Our signer:** keep preparation enabled by default. Add a setting that
  disables it, so tests and operators can run the minimal profile.
- **Specification:** `SPEC.md` §2 lists `Capabilities` and `Prove` as required
  and describes `PrepareOrdinary` and `RetainOrdinaryRanges` as an optional
  extension. A prover that does not offer it may return `UNIMPLEMENTED`; the
  Composer never calls those RPCs when preparation is disabled.
- **Trade-off:** without preparation, each `Prove` validates the whole window
  within the proof time budget, so long catch-up windows take longer. A prover
  should report a `max_window_blocks` it can prove within that budget. The
  existing startup rule still applies: a cap smaller than one Sync interval
  fails startup.

### Item 4: explicit chain identity, checked once

Replace `bytes validation_identity` with a message whose fields are compared
one by one:

```proto
message ChainIdentity {
  uint32 spec_version = 1;            // validation rules, protocol constants, EVM rules
  uint64 chain_id = 2;                // L2 chain ID
  uint64 rollup_id = 3;
  bytes  genesis_hash = 4;            // 32 bytes, L2 genesis block hash
  uint64 block_interval_seconds = 5;  // config.rollupX
  uint32 sync_interval_blocks = 6;    // config.rollupX
  uint64 l1_reference_chain_id = 7;   // config.rollupX.l1Reference
  uint64 l1_reference_number = 8;
  bytes  l1_reference_hash = 9;       // 32 bytes
}
```

What each input of today's identity becomes:

| Today's input | Proposed coverage |
| --- | --- |
| Genesis state, including predeploys | `genesis_hash` |
| `SYSTEM_ADDRESS`, `EEZL2_ADDRESS`, validator rules, protocol pin, EVM rules | `spec_version`, with a table in `SPEC.md` that maps each version to its semantics and protocol pin |
| Chain document fields used for derivation (`config.rollupX`) | Explicit fields. The L1 reference hash commits to that block's number, timestamp and randomness. |
| Chain ID, rollup ID | Explicit fields |
| `keccak256(Cargo.lock)` | Removed. Covered by the `spec_version` rule and the CI guard (R6). |
| Canonical JSON digest and canonicalizer version | Removed. The fields that affect validation are explicit. |
| Operations tag and profile ID | Folded into `spec_version` |

**Explicit fields rather than a hash:** nothing needs to be reproduced (no RLP
encoding, no canonical JSON), and a mismatch reports which field differs, for
example "prover chain_id 699069, Composer expects 906969".

**Where the identity is checked:**

- `CapabilitiesResponse.identity`, at the handshake. The node refuses to start
  on a mismatch.
- `ProveHeader.expected_identity`. The signer compares it before reading
  witnesses and returns the existing configuration-change response on a
  mismatch. This per-request check stays because the gRPC channel can reconnect
  to a restarted, reconfigured prover without a new handshake.
- Nowhere else. A signer's identity is fixed for the life of its process, and
  its prepared-fact cache is volatile (`prepared_cache.rs`). Repeating the
  identity in preparation messages, `CachedOrdinaryRange` and `CacheMiss` adds
  nothing; those fields are removed, and the cache is keyed by block hash.

**Not charging users (R7):** map a configuration mismatch to a distinct prover
error that the Composer handles like a construction failure, requeuing survivors
without charge. Today it maps to `ProverError::Backend`, which charges.

**Residual risk:** two implementations with equal identity fields that still
execute differently are no longer stopped at startup. They produce rejected
proofs at runtime instead. The `spec_version` rule and the CI guard keep this
unlikely for our own builds. How runtime rejections are attributed belongs to
the separate failure-model proposal.

**Open questions:**

- Does the L2 execution profile pin every EVM fork activation? If not, add the
  fork schedule to `ChainIdentity`, or require both sides to reject a chain
  document whose fork configuration differs from the profile.
- Do any other chain document fields affect validation? Implementation must
  list every field the validator reads. Each must be covered by `genesis_hash`,
  fixed by `spec_version`, added explicitly, or rejected when it differs from
  the profile.

**Alternative considered:** keep the current hash and only remove
`dependency_digest`. That removes the impossible input, but another client
would still have to reproduce the RLP tuple and the canonical JSON digest, and
a mismatch would still be an opaque hash difference.

### Item 5: smaller `Capabilities`

| Field | Proposal |
| --- | --- |
| `operations_tag`, `profile_id` | Removed; replaced by `ChainIdentity.spec_version` |
| `validation_identity` | Replaced by `ChainIdentity identity` |
| `max_window_blocks` | Required. Unchanged meaning and startup rule. |
| `max_message_bytes` | Required. Sets the gRPC message limit. |
| `max_request_bytes`, `max_request_witness_items` | Optional; zero means not reported. The prover enforces its own limits and rejects oversized requests with `RESOURCE_EXHAUSTED`, as today. Required when preparation is enabled, for chunk planning. |
| `preparation_max_blocks` | Zero disables the extension; `1..=64` enables it. |
| `stream_idle_timeout_millis`, `request_timeout_millis` | Extension-only (used for the lease lifetime check). The Composer's proof budget governs `Prove`. |
| `cache_max_entries`, `cache_max_bytes`, `maximum_leases`, `lease_lifetime_millis` | Extension-only |

Removed field numbers are reserved in `prove.proto`, not reused.

## Compatibility and recovery

- **Wire:** incompatible with the current interface by design. An old peer and
  a new peer fail the handshake: the new client finds no `ChainIdentity`, and
  the old client finds no 32-byte identity. Both fail at startup; neither signs
  or settles with a mismatched peer.
- **Upgrade order:** upgrade the node and the signer together. Production
  deployments already use one image tag for both. Rollback means rolling back
  both. Upgrading restarts the node, so requests held only in memory are lost,
  as with any restart.
- **Stored data:** none. The identity is not persisted, the signer cache is
  volatile, and the node's witness store is unaffected.
- **Documents to update with the implementation:** `SPEC.md` §2.1 and §3,
  the signer specification, `derivable-chain-operations.md` §5.1,
  `docs/runbooks/chiado-deployment.md` (identity schema note) and
  `docs/runbooks/ethereum-node.md` (a mismatch now names the differing field
  instead of implying different image commits).
- **External provers:** implement `Capabilities` and `Prove` as described in the
  minimal profile of `SPEC.md`, using the published identity fields and
  vectors.

## Validation plan

- **Unit:**
  - field-by-field identity comparison and error messages (`eez-prover-client`);
  - handshake validation tables for both profiles: absent, zero, invalid and
    conflicting values;
  - the signer's `Capabilities` and `ProveHeader` identity check;
  - requeue without charge on a configuration mismatch (`eez-composer`);
  - the `spec_version` CI guard.
- **Process (`eez-node-e2e`, serial):**
  - the minimal profile, with the signer's preparation disabled: `e2e::` and a
    `cross_chain::` subset settle every window;
  - existing suites unchanged with preparation enabled;
  - a wrong chain ID fails startup with a field-named error.
- **Checks** from the repository guide for signer or prover RPC changes:
  `cargo +1.95.0 fmt --all --check`; clippy with `-D warnings`;
  `cargo +1.95.0 test --locked -p eez-control-rpc -p eez-proof-signer -p eez-prover-client`;
  the signer contributor guide checks; `cross_chain` process tests.
- **External conformance (follow-up):** publish fixtures: identity field values
  for a sample genesis, a `Capabilities` response, and a recorded `Prove`
  session with its expected result.

## Implementation and follow-up

- **Status:** draft, not implemented.
- **Next action:** agree on R1-R7 and resolve the two open questions about the
  chain document.
- **Suggested delivery:** item 1 and R7 first (small, unblock a minimal prover),
  then items 4 and 5 together (one wire revision).
- **Follow-ups:** measure whether the preparation path is needed; simplify the
  failure model; confirm the witness format with the external prover team.
