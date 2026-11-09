# Implementing an external v2 prover

This is the integration boundary for Nethermind/C# or another independently
implemented prover. Nethermind's team supplies execution and the server. RollupX
does not require Rust libraries, Rust build metadata or a chain RPC in that server.

The accepted [fresh-network profile](../../../docs/proposals/native-system-transactions.md)
and pinned core define interoperability. Shared type/version names alone do not:

| Boundary | Exact local contract |
| --- | --- |
| Native transactions and receipts | Cross-chain `0x45` has six fields and no version, with execution-group source identities; optional HistoryUpdate `0x46` version **1** has only chain ID and 1–64 explicit increasing positive number/hash pairs after its version. Receipts retain the corresponding type. |
| `ChainOperation` payload | Direct RollupX V0 (`0x00`), including its [compact count grammar](../../../docs/proposals/native-system-transactions.md#direct-v0-wire-contract); this differs from the draft's per-block uvarints. |
| Execution and settlement | The single `NetworkProfile` bound by the registered verification key, genesis, L1 rules and [signer validation contract](../../eez-proof-signer/SPEC.md). |
| Prover transport | `prove.v2.Prover`, capability profile 2 and validation-contract identity version 7, as defined below. |

These are the fixed contracts of one network profile, not selectable compatibility
modes. The [compliance review](../../../docs/audits/2026-10-24-specs-compliance/README.md)
records differences from the reference draft; it does not override these contracts.

## Minimum service

Generate messages/services from the single [prove.proto](../proto/prove.proto)
with your language's protobuf/gRPC tooling. Preserve protobuf `uint64` as unsigned 64-bit integers and `bytes` as
raw byte arrays. Do not substitute JSON/hex strings for protobuf bytes.

Expose `prove.v2.Prover/Capabilities` for discovery and the bidirectional
`prove.v2.Prover/ProveStream` for proving. Both methods and all messages are in
one package and service. Configure one fresh-network profile locally,
including genesis/L1 rules, deployment addresses, rollup and proof-system policy.

The Composer already selects its server through `EEZ_PROVER_URL` and checks proof
signatures against `EEZ_ATTESTER_ADDRESS`. Keep that address consistent with the
registered verifier key. The supplied node Compose files explicitly set a local
bundled-signer URL; an external deployment must override the Composer service's
effective environment/topology, not assume a different URL in `.env` wins.
No language/backend selector or new runtime setting is required.

Implement the [portable identity](../VALIDATION_IDENTITY.md) using independent
local inputs. Its ten-element RLP has one validation-contract version covering
encoding, canonical JSON and execution/settlement semantics together. The network
profile domain is `EEZ_ROLLUPX_NETWORK_PROFILE_V1`. There are no independent schema
or canonicalizer versions to configure. Use the published identity/profile vectors
before testing streaming; generated protobuf bindings alone do not implement this hash.
Validation-contract version 7 includes execution-group source identities,
optional nonempty `0x46` history and [complete-group cleanup](../../../docs/proposals/native-system-transactions.md#complete-group-balance-cleanup).
An external executor clears SYSTEM/EEZL2 in the completing transaction's state
before its checkpoint, retaining load/user adjacency through previews/rollbacks.
Cleanup adds no gas, receipt or log; final-block-only cleanup is insufficient.
Block finalization additionally clears any ordinary donations. Those remain
visible during in-block execution until the block closes or a complete group
clears them. Derive canonical prefix hashes by closing copies; replay FIFO
observations at their selected in-block transaction boundary before finalization,
even for a full terminal prefix. An `eth_call` on the closed state is not
equivalent. Independently check the unmodified terminal execution output against
its header root. The version-7 identity distinguishes these semantics without
changing the v2 protobuf.
For each native group compute standard ABI
`keccak256(abi.encode(string("SOURCE_ENTRY_V1"), uint256(chainId),
bytes32(l1ParentHash), uint64(entryIndex)))`. Use the L2 EVM chain ID from the
envelope and the independently authenticated intended inclusion parent. Index
zero is the first complete native group in the full Live Sync block. An outbound
load plus signed user counts once, including a committed reverted user; an
applying inbound delivery counts once. Nested/static roots within a group add
no indices; ordinary/history transactions, anchors and inbound lookups without
a native transaction do not count. Derive the entire sequence before selecting
a consumed suffix; never reset it per posting or reproof. Do not count raw L1
success events: one root carrier may commit several failed outbound groups.

The full witness/semantic stream supplies the count; accepted-root and exact
prefix/checkpoint checks authenticate it at inclusion. Removing the prefix from
sourceHash does not remove those checks. A changed incompatible accepted root
requires reconstruction, not a trusted counter override. SourceHash alone is
neither authorization nor global deduplication; chain-ID reuse can produce equal
identifiers across deployments. No new RPC field or chain RPC is needed.

Advertise capability profile 2 (the fixed v2 wire discriminator), operations tag 0, that identity, and finite
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
  |--- submit exact batch to L1 -> getCustomData binds the actual L1 anchor;
                                  Core verifies the attestation and entry roots
```

A caller-supplied Begin root is not trusted state. Bind it to the witness parent,
reconstruct the state transition and bind its starting block hash in the signed
entries. Core checks each entry's starting root when consuming it. L1 headers arrive through
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

The [history transaction contract](../../../contracts/L1_HISTORY_SPEC.md) also
applies to an external engine: execute the optional first-position `0x46` update
only at an eligible genesis-phased timestamp. Decode exactly version 1, chain ID
and 1–64 increasing positive number/hash pairs; derive the delayed cutoff,
fixed sender/destination/zero value and the 2M
remaining-gas-bounded budget. Preserve the system nonce and meter execution.
The store starts empty behind a genesis-bound proxy; inherit the configured
bootstrap origin until the first import. Execute the implementation authenticated
by state and witness code, including ordinary authorized upgrades. Gaps in stored
numbers are allowed and cannot be backfilled. Check the complete consecutive
ancestry (including unstored headers) and delay, and set both header and opcode
PREVRANDAO from that recorded choice. Omission inherits origin and PREVRANDAO
without a storage write; validate that inheritance even in eligible empty blocks.
Never infer a new choice from newer witness headers or perform an invisible import.

Use [streaming evidence rules](../STREAMING_SPEC.md#3-offline-evidence-and-settlement-input)
for complete canonical L1 headers, bridge-only headers, recorded origin membership,
delayed history imports and matching PREVRANDAO. Both modes are required:
actual-parent binds the exact future parent/time; historical ordinary catch-up
binds an explicit recent anchor and forbids all cross-chain actions. Every
catch-up part carries its complete bridge, even when intermediate headers were
not selected for execution. No L1 or L2 RPC fills missing evidence.

Recompute native blob commitments, the signed block-hash transitions, ABI/operation
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
