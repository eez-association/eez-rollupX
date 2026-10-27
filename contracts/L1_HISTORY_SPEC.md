# Permanent L1 history predeploy

Status: implementation contract for the RollupX fresh-network profile in this
fork. This is not an activation or migration specification for existing chains,
nor a requirement on other rollup profiles using the core registry.

[L1BlockhashContract](src/L2/L1BlockhashContract.sol) stores every produced L1 block hash from the
genesis-configured first number through the current selected endpoint. It
retains all installed records. A query outside the retained range returns
`bytes32(0)`: that block is not available in the queried L2 state. This includes
both pre-bootstrap and not-yet-imported blocks. Missing records within the imported
range and invalid endpoint state revert. Zero never represents an authenticated
L1 hash.

The runtime has no initializer. Fresh genesis installs the exact runtime and
authenticated bootstrap storage. Its layout uses full-width scalar slots:

| Slot | Contents |
| --- | --- |
| 0 | `mapping(uint64 => bytes32)` of exact L1 hashes |
| 1 | First covered L1 number |
| 2 | Current selected endpoint |
| 3 | Fixed first cutoff (genesis phase), never advanced |
| 4 | Update interval |
| 5 | Update delay |
| 6 | Maximum hashes in one scheduled import |

`current()` returns the selected number and hash; `getL1BlockHash(number)`
returns an exact historical hash or the unavailable sentinel above. The contract
stores neither header bytes nor historical timestamps, randomness or L1 state roots.

`importHistory(cutoff, newHashes)` is callable only by system address
`0xfffffffffffffffffffffffffffffffffffffffe`. It requires
`block.timestamp == cutoff + updateDelay`, `cutoff >= firstCutoff`, and
`(cutoff - firstCutoff) % interval == 0`. It appends 1–64 consecutive nonzero
hashes after the current endpoint. Empty imports, overwritten records, overflow
and invalid scalar state revert. The phase remains fixed. It emits no logs and
calls no other contract. The engine enforces the single first-position update;
this contract does not authenticate L1 consensus.

The dedicated HistoryUpdate is `0x46 || RLP([version, chainId, hashes])`, with
version 1 and exactly three fields. The hash array contains 1–64 exact 32-byte
hashes. Canonical complete RLP decoding rejects nonminimal integers, malformed
lists, unsupported versions, extra fields and old `0x45` history envelopes.
There is no sourceHash or producer-supplied calldata, destination, value or gas.
The executor derives the system sender, this predeploy, zero value, the eligible
cutoff and canonical import calldata from the block and authenticated parent.

An eligible block MAY contain one update, only as transaction zero. It MUST NOT
contain one at any other position or timestamp. Omission inherits the installed
origin and PREVRANDAO without writing storage, including at eligible positions
and in completely empty blocks. A later update does not need intervening empty
imports. Every supplied hash must match the complete consecutive checked L1
ancestry; the selected endpoint timestamp must not exceed the delayed cutoff.
Freshness/maximality is not a validity requirement. The selected header binds
both the header and opcode PREVRANDAO; omitted updates still require that check.

Execution never checks or changes the system account nonce. It produces a
successful `0x46` receipt and charges execution gas to the block, with zero fees
and no mint. RPC exposes version, chainId and hashes, plus fixed Ethereum
placeholders; `input` is empty because the actual call is context-derived.
Its gas limit is the smaller of remaining block gas and 2,000,000. Existing
EIP-2935 and protocol balance cleanup run first. The executor requires the exact
runtime, parent storage, checked header hashes, empty output and no logs; failure
invalidates the entire block. The selected header determines both L2 `mixHash`
and actual EVM PREVRANDAO before applications execute. A storage write alone
does not configure the opcode. These rules apply to producer, follower and
stateless replay from genesis.

The exact transaction is carried in the V0 ordinary prefix and in v2 `Block.rlp`.
It is neither an EEZL2 cross-chain action nor a deposit. History-only Sync blocks
remain valid ordinary catch-up endpoints. No origin metadata is added to
`extraData`, and invisible history system calls are rejected.

The RollupX integration fixes the predeploy at
`0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077`, the maximum import at 64 hashes,
and the call budget at 2,000,000 gas. Its profile commits to the runtime,
bootstrap, schedule and limits. The [RollupX proposal](../docs/proposals/l1-origin-and-offline-proving.md) specifies evidence, anchoring,
public inputs and offline proving. The predeploy does not authenticate L1
consensus or grant signing authority by itself.

## Application polling

Applications query `getL1BlockHash(number)` directly and treat a zero result as
no progress. The latest produced L1 block is not necessarily imported
into the L2 state where the application executes. Scheduled delay, omitted updates
and multipart catch-up can all leave the endpoint behind the L1 head; there is no
fixed waiting duration that guarantees availability.

For a sequential consumer, perform the lookup **inside each polling transaction**.
On zero, return without changing the cursor or application state. On a nonzero
hash, process it and advance only after success. Two polls in one L2 block can
then consume the last available record and return no progress, respectively.
Keep the cursor wide enough to represent exhaustion after `uint64.max`; do not
wrap it. An earlier off-chain simulation does not replace this in-transaction
check. `current()` remains available to inspect the imported endpoint; a separate
collector contract or duplicate history mapping is unnecessary.

`L1BlockNotImported(uint64)` has selector `0x2cbf90b9`. It identifies a missing
covered hash. Invalid endpoint state uses
`InvalidHistoryState()`. Neither error means normal waiting. Do not catch every
getter revert or interpret zero as a valid header commitment: consumers such as
`L1StorageProof` must reject zero before authenticating a supplied header.

## Compatibility

This behavior is active from fresh genesis under validation-contract version 4.
The history address, ABI selectors, storage layout, native transaction encoding,
import schedule and `prove.v2` wire format are unchanged. The runtime code hash,
genesis state/block roots, manager profile and validation identity change.
Use matching node, follower, signer and deployment artifacts with the newly
rendered genesis and manager. Old genesis or retained prover sessions cannot be
reused, and replacing binaries alone does not update an existing network's
predeploy. No migration or legacy execution mode is included. Existing networks
retain their deployed getter semantics; transitioning one requires a separately
specified coordinated upgrade or a fresh network. Rolling back to the old
artifacts requires the old network configuration/state, not the new datadir.

## Ethereum RPC simulation

On public HTTP, WebSocket and IPC transports, `eth_call`, `eth_estimateGas`,
`eth_createAccessList`, and `debug_traceCall` with the explicit `pending` tag serve
an existing executed pending payload with Reth's normal state and environment.
Each method remains available only when its namespace is enabled. `txIndex`
tracing retains Reth's replay of an actual block and its transaction bounds; it
does not create transaction positions for a derived pending view.
If no such payload exists, the node snapshots the latest L2
parent and checked local L1 headers, then executes the real pre-application
prefix in request-local state: EIP-2935, balance cleanup, and the native history
transaction only when ready evidence allows advancement at an eligible position.
Omission leaves the endpoint and genesis phase unchanged; the inherited or newly
selected header supplies PREVRANDAO. The ordinary application
simulation, Reth's gas/access-list algorithms and its selected debug tracer use
that same state and environment. Failure to prepare or execute the prefix returns an error, without
falling back to the unmodified parent state.

This derived view does not promise inclusion of pending pool transactions. It is
discarded after the request, cannot commit a block, and cannot authorize proving
or settlement. State and block overrides retain Reth's diagnostic semantics and
are applied **after** the valid prefix. A hypothetical number, timestamp or
PREVRANDAO does not reschedule the import, authenticate different L1 evidence or
certify a valid L2 block. Estimates remain advisory.

Omitted tags, `latest`, and explicit historical block selectors retain their
existing behavior, as do positional and named JSON-RPC parameters. Debug trace
options and application failure results retain Reth's semantics. Pending trace
preparation and execution share the registry's aggregate tracing quota with
other debug/trace/bundle methods; cancellation does not release that permit
while its blocking worker still runs.

Engine auth's separate `eth_call`, and simulation/debug/trace methods outside
these four public methods, are not adapted: their environment-only fallback still rejects a
derived view when it requires an unexecuted history import. This RPC adaptation
uses the node's existing local evidence; it adds no L1 or L2 RPC dependency to the prover
and makes no change to `prove.v2`.

## Genesis generation

Genesis generation compiles this local source in a disposable tree at the
canonical Solidity source name `src/L2/L1BlockhashContract.sol`, with the pinned
core compiler settings and libraries. That source name is part of the runtime
metadata and thus its hash. The generator never modifies the core checkout.
The committed runtime and genesis fixtures are checked by
`bash scripts/update-eezl2-genesis.sh --check`.
