# Permanent L1 history predeploy

Status: implementation contract for the RollupX fresh-network profile in this
fork. This is not an activation or migration specification for existing chains,
nor a requirement on other rollup profiles using the core registry.

[L1BlockhashContract](src/L2/L1BlockhashContract.sol) stores every produced L1 block hash from the
genesis-configured first number through the current selected endpoint. It
retains all installed records. Queries outside this range, or for an absent
record, revert; zero is not a successful absence result.

The runtime has no initializer. Fresh genesis installs the exact runtime and
authenticated bootstrap storage. Its layout uses full-width scalar slots:

| Slot | Contents |
| --- | --- |
| 0 | `mapping(uint64 => bytes32)` of exact L1 hashes |
| 1 | First covered L1 number |
| 2 | Current selected endpoint |
| 3 | Next source cutoff |
| 4 | Update interval |
| 5 | Update delay |
| 6 | Maximum hashes in one scheduled import |

`current()` returns the selected number and hash; `getL1BlockHash(number)`
returns an exact historical hash. The contract stores neither header bytes nor
historical timestamps, randomness or L1 state roots.

`importHistory(cutoff, newHashes)` is callable only by system address
`0xfffffffffffffffffffffffffffffffffffffffe`, at exactly
`block.timestamp == cutoff + updateDelay`, with `cutoff == nextCutoff`.
It appends consecutive nonzero hashes after the current endpoint and advances
the cutoff by one interval. An empty import still advances the schedule.
Existing records, including identical ones, cannot be overwritten. Overflow,
invalid scalar state and an import beyond the configured limit revert. It
emits no logs and calls no other contract.

The execution protocol authenticates ancestry and checks the origin recorded in
transaction zero, rather than inferring it from when headers arrive. At every
scheduled update the native `0x45` transaction must target the history predeploy,
with zero value and canonical `importHistory(cutoff, hashes)` calldata. An empty
array explicitly repeats the inherited origin. At other positions the transaction
is forbidden. Selected origins cannot regress, skip a produced header, or exceed
the delayed timestamp cutoff. Freshness/maximality is not a validity requirement.

The version-1 transaction binds its sourceHash to the L2 parent, number, timestamp
and exact import calldata. It never checks or changes the system account nonce.
It produces a successful receipt and charges its execution gas to the block,
with zero fees and no mint.
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

Applications must use the imported endpoint returned by `current()` as their
availability boundary. The latest produced L1 block is not necessarily imported
into the L2 state where the application executes. Scheduled delay, empty repeats
and multipart catch-up can all leave the endpoint behind the L1 head; there is no
fixed waiting duration that guarantees availability.

For a sequential consumer, check the endpoint **inside each polling transaction**.
When its next block is above that endpoint, return a no-progress result without
changing the cursor or application state. Otherwise call `getL1BlockHash`, process
the returned hash and advance only after success. Two polls in one L2 block can
then consume the last available record and return no progress, respectively.
An earlier off-chain simulation does not replace this in-transaction check.

`L1BlockNotImported(uint64)` has selector `0x2cbf90b9`. It covers requests below
the genesis bootstrap, above the endpoint, or for a missing stored hash; the
selector alone does not establish temporary unavailability. Do not catch every
getter revert, or turn a missing covered record into a zero hash. Propagate
invalid history state, malformed ABI results and other unexpected failures.

[L1BlockHashCollector](src/L1BlockHashCollector.sol) is a separately deployed
reference application. Its constructor requires an already imported start block;
`poll()` consumes at most one hash and returns `(false, 0)` only while its cursor
is beyond the endpoint. Its mapping/event demonstrate application work, not
another protocol history store. The cursor can represent the number after
`uint64.max` without wrapping. Applications that do not track work can read the
history predeploy directly without deploying a collector.

This consumer pattern preserves the history ABI/runtime, genesis, `prove.v2`
and offline replay rules. It does not install a collector into genesis or alter
existing application deployments. Consumers on new networks must use this pattern;
existing deployed collectors need their own application upgrade or redeployment.

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
transaction when scheduled. Both an advancing import and an empty repeat advance
`nextCutoff`; the selected header supplies PREVRANDAO. The ordinary application
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
