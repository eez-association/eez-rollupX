# Data provenance

> This is an implementation guide. [`SPEC.md`](../SPEC.md) is authoritative for
> protocol behavior and compatibility requirements.

The service handles several representations of the same transition. A value's
name should say who supplied it or which local gate established it. Decoding by
itself never turns a Composer claim into validated evidence.

```mermaid
flowchart TB
    W[Composer v2 frames] --> A[AdmittedBlock and checked session binding]
    A -->|RLP and witnesses| V[Stateless validation]
    V --> VB[ValidatedStreamBlock plus L1 checkpoint]
    VB --> VW[ValidatedWindow selected at Finalize]

    W -->|Finalize calldata| C[CanonicalPostBatch]
    C --> B[Settlement binding and authorization]
    VW --> B
    E[Checked L1 ancestry and conditional context] --> B
    B --> R[RecomputedPublicInputsHash]
    R --> H[AttestablePublicInputsHash]
    H --> S[Attestation signature]
    S --> M[Manager binds actual L1 context at inclusion]
```

## Naming vocabulary

| Vocabulary | Meaning |
| --- | --- |
| `expected_*`, `configured_*` | Selected by the operator at startup |
| `wire_*` | Protobuf representation before conversion into a domain type |
| `claimed_*`, `submitted_*`, `declared_*` | Controlled by the Composer |
| `decoded_*` | Parsed locally, but not yet semantically validated |
| `recovered_*` | Obtained through fork-aware signer recovery |
| `computed_*`, `observed_*` | Calculated or extracted locally, but not necessarily bound to a composer claim |
| `validated_*` | Accepted by the consensus/execution validation boundary |
| `bound_*` | A composer claim matched to validated execution evidence |
| `authorized_*` | A cross-chain effect accepted by every applicable settlement gate |
| `recomputed_*` | A commitment calculated locally from checked inputs |

Aggregate types carry most of this information. Fields inside a strongly named
type do not repeat its entire state: for example, `AdmittedBlock.rlp` is still
Composer-controlled, while `ValidatedBlock.rlp` is the same exact byte string
retained only after successful replay.

Avoid the unqualified word `trusted` when a more precise source exists. Prefer
`expected_rollup_id`, `configured_chain_spec`, or
`validated_window_pre_block_hash` over `trusted_id`, `trusted_config`, or
`trusted_root`.

## Boundary guarantees

### `AdmittedBlock` and session admission

Identity metadata and Begin's rollup/hash widths are checked before admission.
Each Block has present, bounded witness fields and decoded fixed-width claims.
Quotas include its separate L1 header segment. Admission alone does not establish
block identity, execution, accepted ancestry or settlement correctness.

### `ValidatedStreamBlock` and L1 checkpoints

Exact consensus bytes and stateless execution agree with the wire hash, parent
and state root before an ACK is emitted. L1 segments exact-decode and extend one
consecutive hash-linked chain under local fork rules. Each retained block owns
its evidence checkpoint and selected source cutoffs. Rewind restores both L2
and L1 prefixes; a changed duplicate witness cannot overwrite them.

The chain establishes internal consistency. Finalize still checks all source
recorded origin membership and anchor termination, including inherited selections and the complete
bridge. Its endpoint is authenticated by the on-chain manager's context at
inclusion, not by trusting the Composer or querying a provider.

### Backend results

Stateless exact-decodes the block, recovers signers under the configured fork
rules, checks consensus, verifies the witness pre-state, re-executes the block,
and compares the locally computed post-state root with the header commitment.
The backend returns each block's computed identity, validated internal state
roots, receipt outcomes, selected prefix-block hash checkpoints, and settlement
evidence together.

Shared validation consumes that backend result and compares it with the
admitted stream. A backend result is not handed to settlement until this
contract check succeeds.

### `ValidatedWindow`

This is the settlement-facing output of execution validation. It contains exact
admitted block bytes, locally validated endpoint block hashes, system-sender flags,
and outbound receipt observations. The terminal block additionally retains
receipt outcomes and selected transaction checkpoints. It retains the terminal replay input needed to independently validate static and
lookup observations during each Finalize.

Execution validity alone does not authorize canonical ancestry. The checked
`CandidateContext` binds the exact replay parent, schedule, complete L1 ancestry
and selected mode to Core's conditional timestamp/hash context. The registered
vkey binds the local network/program; Core checks signed entry roots and immediate
consumption requirements. Begin, unsigned root pins and ACKs never establish
acceptance. The batch's leading root update instead binds B[0], the
terminal ordinary prefix after mandatory pre-execution and any ordinary users.

### `CanonicalPostBatch`

Canonical decoding proves only that the submitted calldata is a complete,
round-trippable ABI representation. The decoded root updates, effects, DA,
rollup assignments, and public-input fields remain untrusted claims.

### Bound and authorized settlement data

The effect-binding gate joins submitted entries to locally derived transaction
positions and prefix-block hash checkpoints. Inbound and outbound authorization then
checks the direction-specific transaction, receipt, call-hash, value, and
ether-delta evidence. DA verification consumes only those authorized bindings.

### `RecomputedPublicInputsHash`

`CheckedPublicInputProfile` constructs this digest by folding the canonical
batch with the configured proof-system vkey. The type proves local computation from a
profile-checked batch; by itself it does not prove that the state, effects, and
DA passed the complete settlement job.

### `AttestablePublicInputsHash`

The async RPC wraps the recomputed digest in this stronger type only after
execution, state-chain validation, effect/DA binding, observation replay and
offline context/evidence checks pass. On-chain verification binds the signature
to the actual anchor, signed entry roots and network-bound registered key. The signer performs no
network lookup before or after replay. `Attester::sign` accepts only this type rather
than `RecomputedPublicInputsHash` or an arbitrary `B256`, so a profile-only or
Composer-supplied digest cannot cross the signing boundary by accident.

## Comment guidance

Comments at these boundaries should answer three questions:

1. Where did the value come from?
2. What has already been checked?
3. What remains unchecked?

Inside a stage, comments should explain a security consequence or a surprising
implementation constraint. They should not merely restate an expression, cite
historical implementations, or label a value “trusted” without identifying its
authority.
