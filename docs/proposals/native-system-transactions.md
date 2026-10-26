# Native transactions, source identities and fee collection

| Field | Value |
| --- | --- |
| Status | Accepted fresh-network contract, implemented and locally validated. Results, independent review and CI/external-Nethermind limits are tracked in the [native-v1 audit](../audits/2026-10-24-native-v1/README.md). |
| Scope | [Accepted branch scope](l1-origin-and-offline-proving.md#accepted-branch-scope-after-divergence-review), requirements A1–A11 |
| Implementation baseline | RollupX `3e23955dc477c9e675364776f118b1cecda75774` plus the isolated branch's recorded-history changes |
| Pinned core | `feature/e2e_edge_cases`, `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Draft reference | [System transaction design at 2c6d346](https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/F-system-transaction-design.md); the pinned core prevails where its ABI or execution differs |

There is one format and
one execution profile, active from fresh genesis. No old-format decoder, database
migration, live-chain activation or mixed-version mode is required. Preserve
same-profile recovery, whole groups, partial consumption and block-hash commitments.
The actual fee-recipient address is a mandatory deployment input, not an open
design decision or a value to invent here.

The [accepted simplification](../audits/2026-10-24-profile-simplification/README.md)
uses one `Profile`/`OfflineProfile` and one
`prove.v2.Prover` service. The profile domain has no historical revision suffix.
One validation-contract version covers the identity encoding, JSON canonicalizer
and execution/settlement rules together; none can be selected independently.
Acceptance requires matching Solidity/Rust profile bytes, independent identity
vectors, rejection of mismatched identities, offline execution/signing and on-chain
settlement of fresh captures. Protobuf messages, execution rules and the pinned
core are unchanged. These commitments are for fresh networks only, without old
domains, alternate endpoints or compatibility decoders.

## Fixed profile

| Item | Value |
| --- | --- |
| Transaction and receipt type | `0x45` |
| Native envelope version | Integer `1` |
| Implicit system sender | `0xfffffffffffffffffffffffffffffffffffffffe` |
| EEZL2 destination | `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C` |
| History destination | `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077` |
| Cross-chain native gas cap | `16_777_216` |
| History native gas cap | `2_000_000` |
| Manager profile domain | `EEZ_ROLLUPX_OFFLINE_PROFILE` |
| Prover transport | Existing `prove.v2.Prover.ProveStream` |
| Portable identity | Validation contract `1` (encoding, canonicalization and semantics) |

Version 1 explicitly distinguishes our history, grouped outbound and indexed
multi-root rules from the draft's inbound-only version 0. The L1 registry remains
independently configured and authenticated; sharing an address with EEZL2 is not
a universal assumption. Core proxies, selectors, call hashes and table execution
remain those of the pinned revision.

## Exact envelope, execution and RPC

```text
transaction = 0x45 || RLP([version, chainId, sourceHash, gasLimit, to, value, input])
transactionHash = Keccak256(transaction)
receipt = 0x45 || RLP([status, cumulativeGasUsed, logsBloom, logs])
```

Require exactly seven fields, complete input consumption and minimal canonical
RLP. Version is integer 1; chainId is a uint256 equal to the configured L2 chain
ID (this runtime's configured chain IDs remain u64); gasLimit is uint64; value
is uint256. Integer zero is the empty RLP byte string. sourceHash is exactly 32
raw bytes, to exactly 20 raw bytes, and input a byte string. Reject wrong widths,
leading-zero integers, trailing bytes, unsupported versions/types, destinations
or selectors, and any decode/re-encode mismatch. Contract creation is unsupported.

The only native kinds are inbound delivery, outbound table load and history
import. Validate each kind's exact destination, calldata, value, occurrence and
execution position. Shape validation alone does not authorize execution, temporary
funding or signing. Do not reinterpret arbitrary calldata as a native kind.

Immediately before each native transaction, derive remainingBlockGas using the
existing block executor's gas accounting. The encoded gasLimit MUST equal
`min(remainingBlockGas, kindGasCap)`; reject a different supplied limit instead
of rewriting it. Meter actual gas, cumulative receipts and block capacity normally.
A resumed prefix retains its original gas usage and budget; do not restart the
position-derived budget. Native transactions pay no base or priority fee; nested
GASPRICE remains zero.
Preserve existing whole-group capacity/refusal and transaction-result validation:
in particular, a history import must succeed. A valid native envelope does not
make an invalid or mismatched cross-chain execution acceptable.

Native execution neither checks nor increments the system account nonce on
success, revert or halt. Disabling nonce validation alone is insufficient: the
underlying EVM's ordinary Call nonce increment must also be absent. Normal
user nonces are unchanged. Keep beginning-of-block SYSTEM/EEZL2 balance cleanup,
EIP-2935 pre-execution calls, checked temporary inbound funding, pooled intra-block
protocol balances and outer-failure rollback. Do not replace these with the
draft's per-inbound neutrality or residual-balance policy.

For both EEZL2 deliveries and table loads, value is the checked
committed_l2_table_value of the lowered table: sum incomingCalls and the
incomingCalls inside expectedOutgoingCalls with checked arithmetic. It is not
necessarily root.value, and an outbound load is not necessarily zero-value.
Inline callbacks still need this committed funding. Only history is always zero.

RPC transactions expose type `0x45`, version `0x1`, sourceHash, implicit from,
encoded gas, actual to/value/input and the ordinary inclusion fields. For generic
Ethereum RPC clients, nonce, gasPrice, v, r, s and yParity are `0x0` placeholders;
accessList, blobVersionedHashes and authorizationList are empty arrays. Omit
maxFeePerGas, maxPriorityFeePerGas and maxFeePerBlobGas. These compatibility fields
do not enter the native hash or authorize signing. Native receipts use type
`0x45`, effectiveGasPrice `0x0`, contractAddress null and ordinary status, gas,
log and inclusion fields; omit obsolete state-root and blob-gas fields.
Reject native input at public signing, raw-transaction submission, pool and user
transaction gossip boundaries. Engine/import paths still validate the full profile.

## Canonical positions and lowering

Positions come from the authenticated, complete native EEZ semantic stream.
Carry them as checked provenance through parsing/materialization, rather than
recovering them from filtered tables, hashes, submission queues or L2 tx indexes.

- `manifestIndex` is the zero-based ordinal of the top-level
  InitiateCrossChainTransaction bracket among all such brackets in that stream.
  Count both directions and brackets containing failed/static calls. ChainOperation
  and Close messages are not brackets. Do not reset at a blob boundary, phase,
  filtered list or canonically consumed prefix.
- `topLevelCallIndex` is the zero-based ordinal among root Call/StaticCall
  occurrences within that bracket: the call forest's roots, before filtering.
  Count failed, static and later-reverted roots. Nested calls do not increment
  this root counter. Snapshot/Revert/result messages do not erase or renumber it.
  This is **not** SourceActionManifest.source_index, which counts the mutable queue.
- Validate the supported two-chain profile. Do not filter foreign-chain brackets
  into an apparently valid local stream.

Lower L1-origin brackets to one native delivery for each applied, successful,
mutable root. A failed, reverted or static-only root produces no standalone
native delivery, but retains its original position and any required table
validation. The pinned core executes nested callbacks inside the group's tables.

Lower each L2-origin bracket to one native table-load transaction immediately
followed by its exact signed L2 origin transaction. All of that bracket's supported
mutable/static roots and cursor-reusing reverted attempts belong to this one
group. Never produce a separate load per root.

Retain the admitted stream's outbound-first rule: an outbound bracket after an
inbound bracket is invalid. The terminal native suffix is ordered load/user
pairs followed by inbound deliveries in their original bracket/root order.

There is no fourth, standalone callback-native kind. Callbacks execute inline
inside their parent group's tables. Reconstruction rejects independent callback
deliveries and preserves every inline callback and nested-table validation.

Example: bracket 0 is an outbound group with three roots; bracket 1 is inbound
with roots failed, static, successful mutable. Emit one load plus its signed user
transaction for bracket 0, then one delivery identified by `(1, 2)`. Consuming
bracket 0 does not turn that delivery into `(0, 0)`.

## Exact source identities

Let `K` mean Ethereum Keccak-256, and `ABI` mean Solidity abi.encode with the
types below. Tags are their exact ASCII bytes, with no trailing NUL. Never use
abi.encodePacked, RLP, JSON, hexadecimal text or NIST SHA3 for these preimages.
Validate existing chain/rollup/timestamp/index bounds before widening to uint256.

For cross-chain kinds define:

```solidity
domain(tag) = K(ABI(
    bytes32(K(bytes(tag))),
    uint256(1),                 // native envelope version
    uint256(l1ChainId),
    address(l1Registry),
    uint256(eezRollupId),
    uint256(l2ChainId)
))
```

T is the intended live L1 child timestamp and P its authenticated known parent
hash from the same verified settlement context. They are not later observed
inclusion coordinates. Cross-chain actions remain restricted to actual-parent
mode. The source hash does not replace that mode's exact target validation.

### Inbound delivery

```solidity
inboundDomain = domain("EEZ_ROLLUP0_INBOUND_SOURCE");
sourceHash = K(ABI(
    bytes32(inboundDomain),
    uint256(T),
    bytes32(P),
    uint256(manifestIndex),
    uint256(topLevelCallIndex),
    bytes32(crossChainCallHash)
));
```

crossChainCallHash is the pinned core hash of the exact admitted root call:

```solidity
K(ABI(bool isStatic, address sourceAddress, uint64 sourceRollupId,
      address targetAddress, uint64 targetRollupId, uint256 value,
      uint64 callGas, bytes data))
```

The root is mutable, and the currently admitted callGas remains zero. Do not use
the native envelope's gasLimit as callGas. Exact source/target identity, tables,
results, calldata, value and authorized execution remain separately validated.

### Outbound table load

```solidity
outboundDomain = domain("EEZ_ROLLUPX_OUTBOUND_SOURCE");
sourceHash = K(ABI(
    bytes32(outboundDomain),
    uint256(T),
    bytes32(P),
    uint256(manifestIndex),
    bytes32(K(exactSignedL2OriginTransactionBytes))
));
```

Require canonical signed origin bytes, valid signature/chain/profile and the exact
immediately following origin transaction. Table authentication and matching the
whole group's effects remain mandatory. Inline callbacks have no independent
source identity or system transaction.

### History import, including an empty repeat

```solidity
historyDomain = K(ABI(
    bytes32(K(bytes("EEZ_ROLLUPX_HISTORY_SOURCE"))),
    uint256(1),
    uint256(l1ChainId),
    uint256(l2ChainId),
    bytes32(l2GenesisBlockHash),
    address(historyPredeploy)
));
sourceHash = K(ABI(
    bytes32(historyDomain),
    bytes32(l2ParentBlockHash),
    uint256(l2BlockNumber),
    uint256(l2BlockTimestamp),
    bytes32(K(exactCanonicalImportCalldata))
));
```

The calldata is the exact canonical importHistory(uint64,bytes32[]) call, including
cutoff and the ordered array, even when empty. l2ParentBlockHash is the hash of
the block at l2BlockNumber minus one: the current block's parent, never the current
block hash or a same-height prefix variant.
Check number/timestamp/parent relations, mandatory transaction-zero placement,
zero value, exact gas, recorded ancestry and delay independently.

This source does not depend on a future settlement target, registry/manager,
anchor mode, accepted checkpoint, session or catch-up partition. Reposting the
same recorded history with another anchor or in different parts preserves its
transaction bytes and source hash. No self-referential block/state hash is used.

### Authorization, retries and partial consumption

sourceHash identifies an occurrence; it is not an L1 inclusion proof, global
nonce or global deduplication key. Sibling candidates with the same live context
and occurrence can have the same sourceHash while having different execution
results, calldata, transaction hashes, receipts or block commitments. The accepted
root, exact public inputs and semantic execution checks distinguish those states.
Do not add an acceptedRoot-dependent source hash: re-proving the same candidate
after partial consumption must preserve its bytes.

For suffix replay of the same candidate, retain the original complete manifest
positions, and validate the consumed prefix/checkpoint with existing rules.
When actually recomposing a new candidate, derive its new context, manifest and
identities together. Retries of the same candidate do not create a new occurrence.

The v2 service can execute Block inputs before Finalize supplies the full
settlement manifest. A Block ACK is not settlement authorization. Complete all
source/context, semantic-stream, exact-byte, checkpoint and public-input checks
before returning Proof. No new protobuf fields or original-L1-transaction replay
are required.

## Fee collector configuration and execution

### One authenticated configuration source

The accepted treasury amendment routes the shipped network's base fees to a
`FeeVault` predeploy at `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078`. The execution
rule still credits the authenticated `feeCollector` address without calling its
code. The vault accumulates funds and exposes an ordinary L2 `withdraw()` call
which anyone may trigger; the complete balance goes only to its stored recipient.
An empty vault or a failed recipient call reverts without losing funds, and
withdrawals are protected against reentrancy.

The owner proposes the recipient with `proposeRecipient(address)`; the proposed
recipient calls `acceptRecipient()` before it becomes active. The owner can cancel
a pending proposal with `cancelRecipientChange()`. Ownership moves
through OpenZeppelin's two-step transfer. Recipient and ownership changes are
ordinary L2 state transitions, so followers and the offline prover execute and
prove them from the existing witness. They change neither `feeCollector`, the
network profile nor the v2 proving frames, and require no hard fork. Recipient
selection rejects zero, the vault itself, SYSTEM and EEZL2. Withdrawal is local
to L2; it adds no bridge, withdrawal-network selector or automatic forwarding. The collector is an
atomically initialized transparent proxy. Its dedicated ProxyAdmin is initially
owned by `EEZ_L2_TREASURY_OWNER`; this upgrade authority is separate from treasury
ownership after deployment. See the [treasury amendment](fee-vault-upgrades.md).

The existing genesis renderer installs the vault proxy, locked implementation,
ProxyAdmin and initializer-derived storage alongside the other predeploys. Deployment requires
`EEZ_L2_TREASURY_RECIPIENT`; `EEZ_L2_TREASURY_OWNER` defaults to the deployment
owner and is passed explicitly to the renderer. `EEZ_L2_FEE_RECIPIENT` retains
its existing, separate meaning as the block beneficiary for priority fees.
The collector address is generated, not selected by an environment override.
Only new networks are in scope. The treasury amendment's executable validation
is tracked in the [treasury/RPC audit](../audits/2026-10-24-fee-vault-rpc/README.md),
separately from the earlier native-v1 results.

`config.rollupX.feeCollector` remains the checked execution parameter: exactly
20 bytes encoded as a `0x`-prefixed hex string, nonzero and distinct from SYSTEM
and EEZL2. All runtime roles load the same genesis document and propagate this
value through `DerivationConfig` to their EVM configuration. The shipped renderer
always selects the vault. Low-level execution rules do not call its code or
special-case its storage; the exact genesis state root authenticates both.
JSON canonicalizer 1 preserves string spelling, so the renderer emits lowercase
hex and every role loads the same document.

The renderer's `--render` mode requires exactly one `--treasury-owner ADDRESS`
and `--treasury-recipient ADDRESS`. Both must be nonzero 20-byte addresses,
distinct from the vault, SYSTEM and EEZL2. Missing, empty, malformed and duplicate
arguments fail before output is written. Deployment uses its `OWNER` only when
`EEZ_L2_TREASURY_OWNER` is absent; an explicitly empty owner is invalid. The
recipient is always explicit. Neither setting is a runtime node/signer override.

The update/check path uses the public fixture values in
`testing/kurtosis/l2-genesis-profile.json` (schema 5). The generated profile records
`feeCollector`, `treasuryOwner`, `treasuryRecipient`, the proxy/implementation addresses and runtime hashes, and
the complete genesis state root. Deployment checks its inputs against that
profile before broadcasting. Public fixture recipients are not production defaults.
Changing deployment settings after genesis does not mutate contract state.

Recipient updates take effect in transaction order, including for fees already
accumulated in the vault. A withdrawal before the proposed recipient's acceptance
pays the previous recipient; one executed afterwards pays the new one.
The withdrawal transaction's own base fee is credited after execution, so a
successful withdrawal can leave that fee in the vault. `totalWithdrawn` counts
successful gross payments, including donations. The owner may set a minimum
withdrawal in wei; its initial zero value allows any positive balance. Proposal,
cancellation, acceptance, threshold, withdrawal and standard upgrade events expose
these operations. Direct protocol credits execute no code and emit no vault event.
Owner-key loss can prevent recipient changes; ownership transfer is two-step.
The proxy is installed from genesis on a new network, with no legacy-vault
migration; see the [deployment scope](fee-vault-upgrades.md#compatibility-and-deployment).

### Per-transaction state transition

This rule applies only to L2 execution. The shared factory's L1 simulation mode
retains L1 fee rules and must not redirect fees to the L2 collector.
For every ordinary L2 transaction, credit the collector with:

```text
baseFeePerGas * finalChargedGas
```

finalChargedGas is the gas charged under the active EVM rules after refunds and
any applicable floor, including ordinary revert/halt outcomes that consume gas.
This redirects the base fee already debited from the sender. Do not debit it a
second time, credit the priority fee twice or change EIP-1559 base-fee updates,
fee caps, normal transfers or beneficiary rewards.

Credit immediately after each ordinary transaction and before the next
transaction or prefix checkpoint. A transaction-local execution revert does not
undo its fee; reverting its containing tentative block/group rolls back the fee
with the rest of that candidate. Native transactions contribute zero. Use checked
uint256 multiplication/addition and reject arithmetic failure rather than wrap.
Balance credit invokes no recipient code. Collector equal to an ordinary sender
or beneficiary is valid and must combine credits exactly once. Beginning-of-block
protocol cleanup must not erase collector funds. Apply the credit after the
transaction's account-deletion effects: if execution deleted the collector,
recreate an empty account holding the base-fee credit, with no previous code,
nonce or storage. Retain the active Ethereum rules for beneficiary tips.
Unsigned advisory execution with relaxed fee admission distributes only the
base-fee portion actually charged; synthetic uncharged frames distribute zero.
These simulation settings are never enabled for canonical block execution.

## Manager profile and portable identity

The exact Profile tuple is:

```solidity
struct Profile {
    uint256 l1ChainId;
    uint256 l2ChainId;
    bytes32 genesisHash;
    uint64 genesisTimestamp;
    uint64 blockInterval;
    uint32 syncInterval;
    uint32 updateOffset;
    bytes32 l1ConfigHash;
    bytes32 historyCodeHash;
    address feeCollector;
    uint64[] ruleTimes;
}
```

Only one execution profile is supported. Solidity uses `Profile`; Rust uses
`OfflineProfile`. `feeCollector` precedes the final dynamic array.
Canonical bytes and protocolConfigHash are exactly:

```solidity
profileBytes = ABI(
    bytes32(K(bytes("EEZ_ROLLUPX_OFFLINE_PROFILE"))),
    address(0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077),
    uint64(64),
    uint64(2000000),
    Profile(profile)
);
protocolConfigHash = K(profileBytes);
```

The domain denotes all fixed native-version-1 rules above, including gas caps,
sender, cleanup, count codec and fee behavior; these are not additional operator
selectors. Retain existing profile validity checks and also validate feeCollector
as above. Mirror this tuple in Solidity/Rust, initializer calldata, event encoding,
genesis/bootstrap and independently recomputed profile validation.
OfflineProfileInitialized publishes the complete canonical bytes. A genesis
header hash alone does not bind arbitrary chain-configuration fields, so explicit
collector binding is required.

Keep `EEZ_ROLLUPX_L1_HISTORY_V1` context and `EEZ_ROLLUPX_L1_RULES_V1` domains,
both anchor selectors and existing current-root checks. The context already
binds protocolConfigHash; require that exact commitment without additional
context variants. No pinned core ABI change is needed.

Use one ten-element portable identity RLP. Validation-contract version 1 covers
its encoding, canonical JSON rules and accepted execution/settlement semantics:

```text
K(RLP(["eez.validation-identity", 1, rollupId, l2ChainId,
       SYSTEM_ADDRESS, EEZL2_ADDRESS, l2DocumentDigest, protocolConfigHash,
       registryAddress, managerAddress]))
```

Use minimal integer RLP and raw address/hash bytes. l2DocumentDigest hashes the
complete source document, including feeCollector, with the canonical JSON rules in the
[identity contract](../../crates/eez-control-rpc/VALIDATION_IDENTITY.md).
No Rust/build fingerprint enters the identity. Independently load and recompute
it in Composer and prover; compare Capabilities and stream metadata before Block.
A session cannot resume under another identity. C# uses the same contract and
vectors; v2 transport and witness fields are unchanged.

## Retained derivation and recovery

### Direct V0 wire contract

Keep the direct compact V0 body:

```text
00 || uvarint32(block_count) || count_tokens
   || beneficiary_runs || extraData_runs
   || transaction_lengths || exact_typed_transaction_bytes
```

block_count is positive and at most u32::MAX, including the terminal position.
Positive counts 1..127 occupy one byte; 128..16,383 use two bytes with first byte
80..bf. Empty counts use 00 for one position, c0..fe for 2..64, or ff plus shortest
uvarint32 for at least 65. Empty runs are maximal and exactly cover the range.
Metadata runs are positive, maximal for equal values and cover the complete range;
extraData stays at most 32 bytes. Lengths are positive shortest uvarint32; all
transactions and the complete body must be exhausted. Reject overlong/overflowing
varints. Bound parsing/allocation by supplied bytes, not expanded empty positions.

The scheduled history transaction is recorded verbatim in the non-cross-chain
prefix. Reconstruct cross-chain native transactions and paired outbound users
from authenticated semantics; neither cross-chain natives nor L2 blob transactions
belong in that prefix. B[0] includes the terminal ordinary prefix and mandatory
pre-execution changes; B[i] adds whole authorized groups at the same height.
Retain failed-outbound carriers, exact tables, independent call/rolling/public
input/KZG oracles and canonical-only queue completion.

The [history proposal](l1-origin-and-offline-proving.md) owns delay, explicit
advance/repeat, PREVRANDAO, both anchor modes, bounded imports and multipart
ancestry. Ordinary local block production must not wait for L1 arrival, proofs or
settlement. Missing headers select a recorded repeat; later headers cannot make
that prior choice invalid. Keep stateless v2 witness authentication and exact
live-target rules, with no prover L1/L2 RPC.

<a id="wider-draft-conformance-dispositions-35-section-f"></a>

Queues/counters remain in memory; database persistence is not durable queue
recovery. General deep-reorg/bootstrap automation and the other explicitly
[deferred items](l1-origin-and-offline-proving.md#work-deliberately-outside-this-branch)
remain deferred. Deployment of matching contracts, genesis and binaries is a
fresh-network operation; reverting binaries cannot decode an incompatible history.

## Acceptance and validation

[Reference encodings](../../crates/eez-control-rpc/conformance/native-profile-v1.json)
freeze source/profile/identity/envelope examples. They are synthetic encoding
oracles, not valid signed execution or settlement fixtures. Runtime and conformance validation status is tracked in the
[native-v1 audit](../audits/2026-10-24-native-v1/README.md).

Implementation must cover the owning proposal's A1–A11 matrix, including:

- Exact native bytes/hash/RPC/receipt/storage, malformed RLP, unsupported types/versions,
  system-nonce absence, gas and temporary-funding/rollback behavior.
- Identical root calls at different original indexes; failed/static/reverted
  gaps; inline nested callbacks; standalone-callback rejection; changed live
  context; replay of a consumed prefix; recomposition and failed outbound carriers.
- History advance/repeat identity, unchanged bytes across anchors and catch-up
  partitioning, unavailable/late L1 and prover, and synchronized PREVRANDAO.
- Missing/invalid/conflicting treasury configuration, fixed vault rendering, full runtime propagation,
  Solidity/Rust profile equality, and mismatch rejection before signing.
- Ordinary fee success/revert/refund/floor, sender/beneficiary aliasing, no native
  fees, collector initially absent from state/witness, checked arithmetic,
  cleanup separation and every intermediate checkpoint.
- Generated runtime/genesis/profile outputs, portable identity vectors and v2
  accept/reject transcripts for the combined profile; stateful/follower/stateless
  equivalence and the existing full validation matrix.

The [code-context regressions](../../contracts/test/CrossChainProxyPolicy.t.sol)
cover the pinned proxy dispatch on both chains: unauthorized execution contexts
reject, manager-preserving delegation retains ordinary local forwarding, and
the `CALLCODE` self-probe can succeed without cross-chain dispatch. The inspector
does not reinterpret either opcode as CALL/STATICCALL.
E1/E2 define the implementation contract. E3 closes only after executable
conformance and integration evidence, not after a documentation or encoding check.
Actual Nethermind conformance remains unverified until its implementation runs.
