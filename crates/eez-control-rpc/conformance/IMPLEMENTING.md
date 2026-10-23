# Implementing an external v2 prover

This is the integration boundary for Nethermind/C# or another independently
implemented prover. Nethermind's team supplies execution and the server. RollupX
does not require Rust libraries, Rust build metadata or a chain RPC in that server.

## Minimum service

Generate messages/services from [prove.proto](../proto/prove.proto) and
[prove_stream.proto](../proto/prove_stream.proto) with your language's protobuf/gRPC
tooling. Preserve protobuf `uint64` as unsigned 64-bit integers and `bytes` as
raw byte arrays. Do not substitute JSON/hex strings for protobuf bytes.

Expose `prove.v1.Prover/Capabilities` for discovery and the bidirectional
`prove.v2.Prover/ProveStream` for proving. The v1 package contains shared types;
there is no v1 proving method. Configure one fresh-network profile locally,
including genesis/L1 rules, deployment addresses, rollup and proof-system policy.

The Composer already selects its server through `EEZ_PROVER_URL` and checks proof
signatures against `EEZ_ATTESTER_ADDRESS`. Keep that address consistent with the
registered verifier key. The supplied node Compose files explicitly set a local
bundled-signer URL; an external deployment must override the Composer service's
effective environment/topology, not assume a different URL in `.env` wins.
No language/backend selector or new runtime setting is required.

Implement the [portable identity](../VALIDATION_IDENTITY.md) using independent
local inputs. Advertise profile 2, operations tag 0, that identity, and finite
positive limits/timeouts as specified by [RPC discovery](../SPEC.md#21-validation-identity-and-capability-discovery).
The Composer consumes these limits for planning. In particular, a block ceiling
must fit at least one complete network Sync interval. Limits need not equal the
Rust daemon's defaults.

The smallest implementation keeps one active sequential prefix per stream:

```text
Composer                         C# prover
  |--- Capabilities ------------->| report local identity and limits
  |    compare local identity     |
  |--- stream + identity -------->| reject mismatched configuration
  |--- Begin -------------------->| bind predecessor; create session ID/epoch
  |<-- Ready ---------------------|
  |--- Block + complete witness ->| execute and validate; append only on success
  |<-- Validated ------------------|
  |          repeat Block/ACK     |
  |--- Finalize + batch + blobs -->| validate exact range, anchor, settlement,
  |                               | DA and independently derived public input
  |<-- Proof, then EOF -----------| sign only after every check
  |    verify digest/signature    |
  |--- submit exact batch to L1 -> getCustomData binds real anchor/root/rules;
                                  registry verifies the attestation and batch
```

A caller-supplied Begin root is not trusted state. Bind it to the witness parent,
reconstruct the state transition and retain the conditional accepted-root binding
that the manager authenticates at settlement. L1 headers arrive through
`ExecutionWitness.l1_headers_rlp`; its `headers` field remains L2 ancestry.

The execution witness contains Ethereum data, not serialized Rust objects:
`state` contains RLP trie nodes (including nodes needed when deletes collapse
branches), `codes` raw contract bytecode, and `keys` raw hashed-key preimages.
The existing native fixtures exercise this augmented witness format. Missing
trie/code/header data must fail closed; no provider lookup repairs it.

A baseline server may use epoch 1, always set `reused = false`, and forget the
session when the transport ends. Resume, Rewind, Cancel and subrange Finalize are
optional; return a correlated `UNIMPLEMENTED` rejection with the correct binding
and unchanged cursor when unsupported. Resume of an unknown session can return
`NOT_FOUND` with no authorized binding. The existing client then begins/backfills
or retries the exact range on a new stream. Unsupported controls do not permit
omitting witnesses, changing the requested range, or widening settlement inputs.

The Composer can pipeline a bounded number of frames. A sequential server must
continue consuming/responding with backpressure; it must not wait for the client
to finish sending the entire request before emitting ACKs. A successful Proof is
followed by EOF. Half-close drains accepted work; EOF without Proof is failure.

## Required validation

The [signer specification](../../eez-proof-signer/SPEC.md) defines the accepted
validation profile independently of how the Rust daemon implements it. An external
engine must implement the same native transaction, system-call, state/receipt,
rollback, grouped cross-chain and static-position rules. An ordinary Ethereum
execution engine alone does not implement the RollupX profile.

Use [streaming evidence rules](../STREAMING_SPEC.md#3-offline-evidence-and-settlement-input)
for complete canonical L1 headers, bridge-only headers, source maximality,
delayed history imports and matching PREVRANDAO. Both modes are required:
actual-parent binds the exact future parent/time; historical ordinary catch-up
binds an explicit recent anchor and forbids all cross-chain actions. Every
catch-up part carries its complete bridge, even when intermediate headers were
not selected for execution. No L1 or L2 RPC fills missing evidence.

Recompute native blob commitments, the accepted block-hash binding, ABI/operation
bytes and the public-input hash from independently validated execution. The
[public-input specification](../../../eez-core-protocol/docs/MULTI_PROVER_SPEC.md#per-ps-publicinputshash-two-stage)
and [signer gates](../../eez-proof-signer/SPEC.md) are the authority. Return the
configured ECDSA attestation format; a different proof system requires a separate
authorized extension. An ECDSA signature is a trusted attestation, not a ZK proof.
L1 verifies its context/signature, not whether your engine executed correctly.

Errors must not advance validated progress or produce a signature. Preserve
[typed failure and retry rules](../SPEC.md#6-failure-handling): only validated
request-local attribution may identify a removable candidate. A configuration
error must have no actionable details; transport `FAILED_PRECONDITION` with
`eez-capabilities-changed: 1` also explicitly prevents any attached details from
being interpreted as a candidate failure. Ordinary opaque errors retain queued
work; the only automatic transport retry codes remain UNAVAILABLE,
DEADLINE_EXCEEDED and ABORTED. Failed proofs never authorize submission.

## Conformance handoff

1. Verify the three RLP/Keccak identities and JSON acceptance/rejection cases in
   [validation-identity.json](validation-identity.json). These have independent
   encoding/hash oracles and cover integer width, leading zeros, escaping,
   UTF-8 key order and duplicate-key rejection.
2. Start a fresh server with one native fixture's genesis/deployment inputs,
   public fixture attester key and the vector's declared resource profile.
   Follow the [corpus instructions](README.md); compute expected identity locally.
3. Run the baseline native transcripts using the wire-only replayer, or implement
   its documented JSON transcript format in C#. Test negative inputs and the
   exact expected proof digest/attester; accepting protobuf frames is insufficient.
4. Run cases for every optional control/resource policy your implementation
   advertises. The complete 29-case Rust policy suite is not mandatory for a
   server that deliberately omits those optimizations.
5. Run real Composer settlement and independent follower replay with the C#
   prover denied outbound network access, covering both anchor modes and partial
   catch-up. This establishes execution interoperability beyond wire conformance.

The current Rust tests and portable vectors are integration material, not a claim
that an external C# server has passed them. Shipping Nethermind support requires
the integrating team to execute these checks against its implementation.
