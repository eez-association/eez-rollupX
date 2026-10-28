# Native transactions, source identities and fee collection

| Field | Value |
| --- | --- |
| Status | Accepted fresh-network contract. Complete-group cleanup and optional HistoryUpdate are implemented. The execution-group source index and versionless `0x45` amendment are tracked in the [current audit](../audits/2026-10-27-source-counter/README.md). Earlier native-v1 and prefix-bound identity results do not validate these amendments. The core is unchanged. |
| Scope | [Accepted branch scope](l1-origin-and-offline-proving.md#accepted-branch-scope-after-divergence-review), requirements A1–A11 |
| Implementation baseline | PR #103 head `9c1ab488e1b421cdc59c7e82bd95f080f7332b3e`; the current source/envelope amendment replaces its earlier identity rule. |
| Pinned core | `feature/e2e_edge_cases`, `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Draft reference | [System transaction design at 2c6d346](https://github.com/eez-association/specs-draft/blob/2c6d346bea0b3736f7453c35174291495694d1fc/docs/rollup0-spec/F-system-transaction-design.md); the pinned core prevails where its ABI or execution differs |

There is one canonical format per transaction kind and
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
settlement of fresh captures. The accepted amendments change execution rules
and history encoding; protobuf messages and the pinned core are unchanged.
These commitments are for fresh networks only, without old
domains, alternate endpoints or compatibility decoders.

## Fixed profile

| Item | Value |
| --- | --- |
| Cross-chain transaction and receipt type | `0x45` |
| HistoryUpdate transaction and receipt type | `0x46` |
| Envelope version | No field for `0x45`; integer `1` for history `0x46` |
| Implicit system sender | `0xfffffffffffffffffffffffffffffffffffffffe` |
| EEZL2 destination | `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C` |
| History destination | `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077` |
| Cross-chain native gas cap | `16_777_216` |
| History native gas cap | `2_000_000` |
| Manager profile domain | `EEZ_ROLLUPX_OFFLINE_PROFILE` |
| Prover transport | Existing `prove.v2.Prover.ProveStream` |
| Portable identity | Validation contract `3` (encoding, canonicalization and semantics) |

The validation identity binds the grouped outbound and indexed multi-root rules.
The L1 registry remains
independently configured and authenticated; sharing an address with EEZL2 is not
a universal assumption. Core proxies, selectors, call hashes and table execution
remain those of the pinned revision.

## Exact envelope, execution and RPC

```text
transaction = 0x45 || RLP([chainId, sourceHash, gasLimit, to, value, input])
transactionHash = Keccak256(transaction)
receipt = type || RLP([status, cumulativeGasUsed, logsBloom, logs])
```

Require exactly six fields, complete input consumption and minimal canonical
RLP. There is no version field. chainId is a uint256 equal to the configured L2 chain
ID (this runtime's configured chain IDs remain u64); gasLimit is uint64; value
is uint256. Integer zero is the empty RLP byte string. sourceHash is exactly 32
raw bytes, to exactly 20 raw bytes, and input a byte string. Reject wrong widths,
leading-zero integers, extra fields, trailing bytes, unsupported types, destinations
or selectors, and any decode/re-encode mismatch. Contract creation is unsupported.

The `0x45` kinds are inbound delivery and outbound table load. History uses the
separate `0x46` encoding below. Validate each kind's exact destination, calldata,
value, occurrence and execution position. Shape validation alone does not authorize execution, temporary
funding or signing. Do not reinterpret arbitrary calldata as a native kind.

Immediately before each native transaction, derive remainingBlockGas using the
existing block executor's gas accounting. For `0x45`, the encoded gasLimit MUST equal
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
user nonces are unchanged. Apply end-of-block SYSTEM/EEZL2 balance cleanup,
EIP-2935 pre-execution calls, checked temporary inbound funding and outer-failure
rollback. The accepted complete-group cleanup below limits shared protocol
inventory to one complete group; it does not adopt the draft's per-inbound
neutrality or residual-balance policy.

For both EEZL2 deliveries and table loads, value is the checked
committed_l2_table_value of the lowered table: sum incomingCalls and the
incomingCalls inside expectedOutgoingCalls with checked arithmetic. It is not
necessarily root.value, and an outbound load is not necessarily zero-value.
Inline callbacks still need this committed funding. Only history is always zero.

RPC transactions expose type `0x45`, sourceHash, implicit from,
encoded gas, actual to/value/input and the ordinary inclusion fields. For generic
Ethereum RPC clients, nonce, gasPrice, v, r, s and yParity are `0x0` placeholders;
accessList, blobVersionedHashes and authorizationList are empty arrays. Omit
maxFeePerGas, maxPriorityFeePerGas and maxFeePerBlobGas. These compatibility fields
do not enter the native hash or authorize signing. Native receipts preserve their type (`0x45` or `0x46`), effectiveGasPrice `0x0`, contractAddress null and ordinary status, gas,
log and inclusion fields; omit obsolete state-root and blob-gas fields.
Reject native input at public signing, raw-transaction submission, pool and user
transaction gossip boundaries. Engine/import paths still validate the full profile.

## Canonical positions and lowering

Positions come from the authenticated, complete native EEZ semantic stream.
Enumerate complete cross-chain execution groups starting at zero before selecting
a consumed suffix. These are not `PostBatch.entries` row numbers: the ordinary
anchor does not count, nor do nonexecuting inbound lookups. Validate the supported
two-chain profile and every nonexecuting action before deriving this sequence;
never filter foreign roots into an apparently valid local stream.

Lower L1-origin brackets to one native delivery for each applied, successful,
mutable root. A failed, reverted or static-only root produces no standalone
native delivery and does not advance the source index. Retain all required table
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
transaction with source index 0, then one delivery with source index 1.
The failed and static inbound roots do not advance the source index. Consuming
the outbound group does not renumber the delivery's original index 1.

## Complete-group balance cleanup

Accepted for fresh networks on 2026-10-27, addressing
[issue #105](https://github.com/eez-association/eez-rollupX/issues/105). The core
manager pools ETH and leaves issuance/burning to the node/circuit. RollupX gives
each complete table its full checked allocation, including all flat and nested
incoming values; it never subtracts earlier residual inventory.

Immediately after executing a complete cross-chain group, set the balances of
SYSTEM_ADDRESS and EEZL2 to zero. Preserve code, nonce and storage. FeeVault and
all other accounts are unaffected. The boundaries are derived from the exact
transaction body, never a Composer-supplied index:

- Outbound: one native `loadExecutionTable`, then its immediately adjacent signed
  user transaction, including every root and inline callback in that transaction.
  The load does not clean. Never clean inside callbacks or between roots.
- Inbound: one native `executeIncomingCrossChainCall`, including all callbacks.
- History imports and ordinary transactions outside a load/user pair do not clean.
  A native transaction cannot replace the user after a load; a block or effect
  checkpoint cannot end inside that pair. Exact source/user correspondence and
  the existing outbound-first semantic profile remain independently validated.

Cleanup is an unconditional part of the completing transaction's state delta,
**before commit and before its state/block-hash checkpoint**, whether its receipt
succeeds, reverts or halts. Native failure first removes its temporary mint as
before. A failed load still requires its paired user at the execution boundary;
failed native/user effects remain ineligible for settlement under existing checks.
Validation/DB errors produce no committed transaction or cursor advancement.
Dropping an uncommitted result drops its cleanup. Reverting/discarding a candidate
or a complete replay group restores account state, gas, nonce and grouping cursor
together. An enclosing EVM call revert does not undo cleanup after its transaction.

This is a bounded execution transition, not a call or transaction: no additional
receipt, log, gas charge or refund. Preserve the EVM receipt's type, status, gas,
logs and output. The next native limit is derived from that unchanged cumulative
gas; later transactions can observe empty protocol accounts and therefore have
different account-creation costs. Every producer, follower, witness builder and
Stateless prover MUST apply the same transition before taking a checkpoint.
Block finalization also clears both balances, including in empty and ordinary-only
blocks. It replaces beginning-of-block cleanup. Ordinary donations remain visible
to later transactions until the block closes or a complete cross-chain group
clears them. Never clear after a standalone ordinary transaction.

A complete prefix has two distinct uses. Continuing its execution, including FIFO
synthetic observations, uses the unfinalized state. Computing its canonical block
hash finalizes a disposable copy, so the prefix state root includes cleanup.
The hash authenticates the prefix body and its execution from the parent; an
observation pin selects that exact **in-block transaction boundary**, before
block-finalization cleanup. It is not an `eth_call` against the closed block's
state root. This distinction matters for ordinary donations; it does not change
which transaction boundary the signer independently derives. Synthetic calls
never commit and never trigger cleanup. After complete cross-chain groups the
balances are already zero, so both views agree.

Composer retains unfinalized prefix snapshots; follower reconstruction closes
whichever complete prefix actually settles. A resumed same-height settlement
replays the retained body and appended groups from the original H-1 parent,
replacing the closed prefix at H; it does not append transactions to that closed
state. A newly composed candidate uses its canonical parent. No continuation
reuses a finalized speculative cache. The offline prover derives finalized prefix roots on
copies without cleaning the running EVM state. Its final-block check must also
compare the **unmodified execution output** with the header root; projecting
checkpoint balances must never conceal missing finalization or a wrong root.

Acceptance checks: ordinary donations remain visible within a block; completed
blocks and independently rebuilt prefixes have zero protocol balances; storage,
nonces, code, fees and receipts remain intact; observations reject earlier or
closed-state outcomes when they differ from the selected in-block boundary;
partial settlement and follower replay reproduce the committed prefix. Retain
all complete-group, reverted-transaction and discarded-preview regressions.

This changes execution and commitments. Use matching new-network artifacts;
pre-change histories/databases are unsupported, with no mixed-version execution,
activation switch or migration. Use the six-field cross-chain envelope and keep the
`prove.v2` protobuf unchanged; validation-contract version 6 identifies the new
semantics. The manager profile preimage also includes this fixed version before
the history address, binding the cleanup rule into on-chain public inputs without
a new profile struct, configurable mode or alternate decoder. Solidity/Rust and
independent vectors use the same word. Deploy the matching fresh manager; an old
manager's commitment is incompatible even with the same configuration/genesis. No new L1 or L2 RPC dependency is introduced for the prover.

Acceptance requires positive and rejected complete groups, nested/multi-root
funding, included failures and discarded candidates, ordinary donations, history,
partial consumption, identical per-prefix/full hashes, independent cold signer
replay and follower derivation. Unit tests alone do not close those process gates.

## Exact source identities

Both cross-chain directions use the single execution-position identity:

```solidity
sourceHash = keccak256(abi.encode(
    string("SOURCE_ENTRY_V1"),
    uint256(chainId),
    bytes32(l1ParentHash),
    uint64(entryIndex)
));
```

Use Ethereum Keccak-256 and standard Solidity ABI, including the literal dynamic
string. `chainId` is the target L2 EVM chain ID, equal to the native envelope's
chain ID; this implementation admits uint64 EVM chain IDs and ABI-encodes that
value as uint256. `l1ParentHash` is the authenticated parent of the intended L1
inclusion block. Do not use a prehashed/padded tag, packed encoding, RLP,
hexadecimal text, NIST SHA3 or the registry's rollup ID in place of chainId.

`entryIndex` is the zero-based ordinal of a complete native cross-chain group in
the full Live Sync block. It continues across all postings in that L1 block.
Derive it from the complete reconstructed execution before selecting consumed
suffixes. Never reset it per batch, successful event or retry. The
[source-identity rules](#source-identity-simplification) specify counting,
authentication and partial settlement.

Cross-chain actions remain restricted to actual-parent mode. Prefix commitments,
exact bytes, signed origins, tables, values, effects, target time and deployment
identity remain independently authenticated. Removing the prefix from sourceHash
does not remove any of those checks. One outbound load is followed immediately
by its exact signed user; inline callbacks have no separate native transaction.

### Dedicated optional HistoryUpdate (accepted 2026-10-27)

```text
transaction = 0x46 || RLP([version = 1, chainId, [[number, blockHash], ...]])
1 <= blocks.length <= 64
```

Numbers are positive uint64 values in strictly increasing order, with nonzero
32-byte hashes. Gaps are allowed in storage, not in authentication ancestry.
Bounded canonical complete decoding rejects malformed pairs, extra RLP fields,
nonminimal integers, unsupported versions and the previous hash-only format.
Chain ID equals the configured L2 chain ID. There is no sourceHash, calldata,
gas, destination or value field supplied by the producer.

The [history contract](../../contracts/L1_HISTORY_SPEC.md) owns the exact storage,
proxy authority, ABI, timing, evidence and application semantics. It starts empty;
execution inherits the genesis-configured origin until the first import. The
engine derives `importL1Blocks`, permits an optional update only at transaction
zero at an eligible delayed timestamp, and authenticates every selected pair.
Omission remains valid even in eligible empty blocks and retains PREVRANDAO.
A first import may publish the positive bootstrap number; later imports must
advance beyond the stored endpoint. No imports regress the logical origin.

Meter execution with `min(2_000_000, remainingBlockGas)`, preserve system nonce
and require a successful 0x46 receipt, empty output and no logs. History is
fee-free, mints nothing, does not end a cross-chain group and remains in the
ordinary prefix. Exact bytes survive anchor changes and catch-up partitions.
Repeated bytes still require valid parent state, schedule and ancestry.
RPC exposes `blocks: [{number, blockHash}]` and fixed Ethereum placeholders;
input is empty. No legacy decoder or selector alias is retained.

### Authorization, retries and partial consumption

sourceHash identifies an occurrence; it is not an L1 inclusion proof, global
nonce or global deduplication key. Sibling candidates with the same live context
and occurrence can have the same sourceHash while having different execution
results, calldata, transaction hashes, receipts or block commitments. The accepted
root, exact public inputs and semantic execution checks distinguish those states.
Do not add an acceptedRoot-dependent source hash: re-proving the same candidate
after partial consumption must preserve its bytes.

For suffix replay of the same candidate, retain the original complete group
ordinals and validate the consumed prefix/checkpoint with existing rules.
When actually recomposing a new candidate, derive its context, complete group
sequence and execution checkpoints together. Retries of the same candidate do
not create a new occurrence.

The v2 service can execute Block inputs before Finalize supplies the full
settlement manifest. A Block ACK is not settlement authorization. Complete all
source/context, semantic-stream, exact-byte, checkpoint and public-input checks
before returning Proof. No new protobuf fields or original-L1-transaction replay
are required.

## Source-identity simplification

| Field | Value |
| --- | --- |
| Status | Accepted execution-group ordinal on 2026-10-27; implementation and validation tracked in the [source-counter audit](../audits/2026-10-27-source-counter/README.md). |
| Scope | One sourceHash formula for fresh networks; no alternate identity or legacy validation. |
| Fixed boundaries | Unchanged core contracts/ABI/public-input algorithm/gitlink; unchanged prove.v2; offline prover; no L1 allocator, posting wrapper, trusted counter or additional RPC. |
| Baseline | PR #103 head `9c1ab488e1b421cdc59c7e82bd95f080f7332b3e`, base `af0a000f12b0772892058066706465c84a00fd17`, core `84a5b5207880f192a6a5896cbfe0bac5249db47f`. |

### Counting contract

Count complete groups in the fully reconstructed Live Sync block, starting at
zero, in the existing outbound-before-inbound order:

- One outbound bracket contributes one load/signed-user group. Multiple mutable
  or static roots, nested callbacks and a canonically included reverted user do
  not split the group. A static-only outbound bracket still contributes one.
- Each applying inbound delivery contributes one group, including its callbacks.
  Static, failed or source-frame-reverted inbound lookups without a native L2
  transaction contribute zero. Their semantic and settlement validation remains.
- Ordinary transactions, history imports and root-only anchors contribute zero.
  An anchor or successful entry can carry the checkpoints of failed outbound
  groups: those physically committed groups still count individually.
- Unconsumed candidate suffixes are not counted as canonical execution. Reposting
  already consumed groups does not allocate additional indices. Derive their
  original ordinals before selecting the suffix that L1 actually committed.

Use checked uint64 conversion. The ordinal is an execution position, not a
PostBatch array index, L1 log index, transaction index or success count. The
native load and its signed user count once together. Raw event counting is
incorrect because a root carrier can cover several groups and a lookup can emit
a no-op event without a native transaction.

### Authentication without a counter oracle

The schedule fixes exactly one Live Sync endpoint for an intended L1 inclusion
timestamp. Historical/catch-up candidates cannot carry cross-chain actions;
intermediate replay blocks cannot carry native cross-chain groups. V0 and the
semantic stream reconstruct the complete terminal ordinary prefix and native
suffix. Consequently the offline prover derives all group ordinals from existing
execution evidence, starting at zero; it accepts no external starting counter.

Before signing, retain exact native reconstruction, complete semantic/DA binding,
independent execution and whole-group checkpoints. The accepted-root claim must
be the authenticated execution predecessor or one of those independently replayed
checkpoints. At inclusion, getCustomData binds the actual accepted root, exact L1
parent/time and deployment; core also checks per-entry currentRoot on execution.
A root hash alone does not reveal an ordinal: complete execution opens that
commitment and establishes the count. These gates must remain even though the
prefix hash is absent from the sourceHash preimage.

### Partial settlement, competing posts and follower replay

A same-target reproof retains full original execution, semantics and DA. Only
its accepted-root assertion/proof changes. Groups keep their original ordinal
and bytes; the selected suffix never restarts at zero. A compatible preceding
post can consume some of the same candidate, or a shorter compatible candidate,
without changing those identities.

An intervening root advance makes the old proof stale. Reproof against the new
root is permitted only when that root is a whole-group checkpoint in the fully
replayed candidate. An incompatible prefix requires a rebuilt candidate and new
proof; it cannot be repaired by supplying a different counter. A different L1
target likewise requires reconstruction under its new parent/time.

Followers combine canonical ordered BatchPosted, L2ExecutionPerformed and
L2TxSkipped observations with exact posted entries and native blob semantics.
Existing carrier attribution identifies which complete groups became committed,
including failed outbound groups carried by another entry. Reconstruct the full
source sequence before selecting that range. Authenticated entry/final block
commitments select append versus already-applied replay; verify retained prefix
and exact suffix bytes. SourceHash is never a global deduplication key.

### Identity scope and compatibility

SourceHash identifies a position within the configured network and L1 target.
It does not independently authorize execution, prove inclusion/finality, bind an
original settlement-row number or distinguish every competing candidate.
Deployments reusing an EVM chain ID may share identifiers; deployment/genesis,
registry/rollup, exact prefix, tables and bytes remain independently bound by
validation. Do not use sourceHash alone as a cross-network identifier.

The single validation-contract version is 6, in both capability identity and
manager profile; cross-chain envelopes have no version; history remains version 1 and the transport remains
prove.v2 with unchanged protobuf. New source bytes change native transaction and
block hashes. Use matching new-network artifacts, fresh genesis/profile and
fresh datadirs; mixed versions, migration, activation flags and rollback to old
binaries over new history are unsupported. No core changes or new environment
variables are required. Existing networks are not changed by this PR.

### Acceptance

Require independent ABI vectors; full-group/static/failed/root-rollback counting;
append-stable indices; different chain IDs and L1 parents; malformed source/gas
and duplicate/reset-index rejection; same-L1-block partial reproof with unchanged
bytes; incompatible-root rejection; and cold follower/restart derivation. Refresh
captured no-network proofs and portable conformance transcripts. Earlier prefix-
identity captures do not validate this formula. Keep protocol, RPC schemas and
locked dependencies unchanged and remove superseded identity-only machinery.

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
    uint64(2),
    address(0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077),
    uint64(64),
    uint64(2000000),
    Profile(profile)
);
protocolConfigHash = K(profileBytes);
```

The domain and validation-contract version identify the fixed rules above,
including both native encodings, gas caps, sender, cleanup, count codec and fee behavior; these are not additional operator
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

Use one ten-element portable identity RLP. Validation-contract version 6 covers
its encoding, canonical JSON rules and accepted execution/settlement semantics:

```text
K(RLP(["eez.validation-identity", 2, rollupId, l2ChainId,
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
optional advances and inheritance, PREVRANDAO, both anchor modes, bounded imports and multipart
ancestry. Ordinary local block production must not wait for L1 arrival, proofs or
settlement. Missing new headers omit the update; later headers cannot make
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
- History advance/omission, unchanged bytes across anchors and catch-up
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
