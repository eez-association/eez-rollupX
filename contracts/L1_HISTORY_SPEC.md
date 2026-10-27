# Permanent L1 history predeploy

Status: accepted fresh-network implementation contract for this fork, adopting
[issue #113](https://github.com/eez-association/eez-rollupX/issues/113). This is not
an activation or migration specification for existing chains.

## Public contract and storage

[L1BlockhashContract](src/L2/L1BlockhashContract.sol) starts empty. It stores
positive, strictly increasing L1 block-number/hash pairs permanently. Gaps are
allowed; skipped numbers cannot later be backfilled. It does not store headers,
timestamps, randomness, L1 state roots, or scheduling parameters.

| Slot | Contents |
| --- | --- |
| 0 | `mapping(uint64 => bytes32)` of imported L1 hashes |
| 1 | `uint64 latestImportedBlockNumber`; zero means empty |

`getL1BlockHash(uint64)` returns the exact stored hash, or `bytes32(0)` when
absent, including block zero and skipped numbers. `getLatestImportedL1Block()`
returns the latest stored number and hash, or `(0, 0)` before any import.
A zero result is never an authenticated L1 hash. `L1StorageProof` rejects it
before authenticating a supplied header. Getters make no claim that every number
below the endpoint was imported.

`importL1Blocks((uint64 number, bytes32 blockHash)[] blocks)` is callable only by
`0xfffffffffffffffffffffffffffffffffffffffe`. It requires a nonempty list,
every number strictly greater than the previous maximum, and every hash nonzero.
Errors are `UnauthorizedImport(address)`, `EmptyImport()`,
`BlockNumberNotIncreasing(uint64,uint64)` and `InvalidBlockHash(uint64)`.
A failing element reverts the entire call. The implementation has no constructor
or initializer, makes no external calls and emits no logs. Schedule, record count
and L1 authentication are enforced by the execution protocol below.

## Proxy and authority

The public address is `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077` and contains
the pinned core `EEZProxy`. Its initial implementation is at
`0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee007a`. Its dedicated ProxyAdmin is
`CREATE(history, 1)`, `0x58aD165846a897f4aba3EC8f07a4a7dB434324Bf`. The proxy has
nonce two; implementation and admin have nonce one. Genesis application storage
is empty; proxy storage contains only the ERC-1967 admin and implementation
pointers. The admin owner initially comes from `EEZ_L2_UPGRADE_OWNER`. Genesis
validation checks exact code, nonces, storage, zero balances and a nonzero
canonical owner distinct from the system address.

Execution pins proxy bytecode, including its immutable admin. The implementation
pointer and delegated code are read from authenticated L2 state/witnesses.
`historyCodeHash` commits to the proxy; the genesis hash also commits to the
initial implementation and authority. Historical replay executes the historical
implementation. Ordinary signed L2 transactions can upgrade through ProxyAdmin
or transfer its ownership independently of EEZL2's admin.

The owner is trusted with contract behavior. Proofs establish execution of the
selected code, not correctness of arbitrary upgrades. Compatible upgrades retain
slots 0/1, the ABI and query semantics, system-only increasing imports, and the
native call's gas/no-output/no-logs requirements. An upgrade cannot override
engine rules. Storage changes are not undone by restoring an old code pointer.

## Native transaction and execution

The dedicated HistoryUpdate is
`0x46 || RLP([version=1, chainId, [[number, blockHash], ...]])`, exactly three outer
fields. Each pair contains a positive uint64 and an exact nonzero 32-byte hash.
There must be 1–64 strictly increasing pairs. Bounded, canonical, complete RLP
and JSON decoding reject excess records, malformed pairs, nonminimal integers,
unsupported versions, extra RLP fields, unknown pair fields and the previous
hash-only body.
There is no sourceHash or producer-supplied calldata, destination, value or gas.
The executor derives system sender, predeploy destination, zero value and exact
`importL1Blocks` calldata.

An eligible block MAY contain one update as transaction zero. Other positions or
timestamps are invalid. The schedule and delayed cutoff derive from the fixed
genesis profile. The import must match every supplied pair to checked L1 headers,
link the prior execution origin to the selected endpoint through every intervening
header, and select an endpoint timestamp no later than the cutoff. Gaps in stored
numbers do not permit gaps in ancestry evidence. Freshness/maximality is not a
validity condition. The default producer imports consecutive available pairs, up
to 64; replay follows the recorded pairs instead of rerunning that policy.

Before the first import, the public store remains empty while execution uses
`config.rollupX.l1Reference` as its authenticated logical origin and PREVRANDAO.
The first update may include that reference when its number is positive, and
cannot import any number below it. Block zero is never stored. Once nonempty,
updates must advance beyond the stored endpoint. Omission inherits the logical
origin and PREVRANDAO without writes, including at eligible timestamps and in
empty blocks; delayed or unavailable evidence does not force an update or stop
L2 production.

Execution never checks or changes the system account nonce. It produces a
successful `0x46` receipt and charges execution gas to the block, with zero fees
and no mint. RPC exposes version, chainId and `blocks: [{number, blockHash}]`, plus
fixed Ethereum placeholders; input is empty. The gas limit is the smaller of
remaining block gas and 2,000,000. Existing EIP-2935 and balance-cleanup rules
remain in force. Runtime, parent state, header authentication, timing and
PREVRANDAO checks precede execution. Native execution requires success, empty
output and no logs; failure invalidates the block. The selected header controls
both L2 mixHash and the EVM PREVRANDAO before applications execute. These rules
are shared by Composer, follower and stateless replay.

The exact transaction travels in the V0 ordinary prefix and v2 `Block.rlp`.
It is neither an EEZL2 cross-chain action nor a deposit. History-only Sync blocks
remain ordinary catch-up endpoints. No metadata is added to extraData and
invisible history calls are rejected. The
[offline proving proposal](../docs/proposals/l1-origin-and-offline-proving.md)
specifies anchoring and public inputs. The store alone does not authenticate L1
consensus or authorize signing. Stored-pair limits and complete-ancestry resource
limits are separate; catch-up can be proved and settled in bounded parts.

## Application polling

Applications query inside each transaction and treat zero as unavailable, never
as a valid commitment. An earlier off-chain simulation cannot replace that
check. Scheduled delay, omitted updates and catch-up can leave stored history
behind the L1 head; no waiting duration guarantees availability.

A sequential consumer can return without advancing on zero, but must account
for permanently skipped numbers: the contract cannot backfill them. Applications
that require every produced L1 number must arrange an appropriate import policy
or use a separately authenticated source. The latest getter reports the maximum,
not a proof of contiguous coverage. A duplicate collector mapping is unnecessary.

## Compatibility

This is validation-contract version 6 from fresh genesis. The history ABI,
storage layout, proxy/genesis code, native 0x46 body, state/block roots and
validation identity change together. Use matching node, follower, signer and
deployment artifacts with newly rendered genesis and manager. `prove.v2` and
cross-chain 0x45 are unchanged; no chain RPC is added to the prover. There is no
legacy decoder, selector alias, migration or mixed-version execution mode.
Old sessions, genesis or datadirs cannot be reused. Compatible future proxy
upgrades execute through state without changing the pinned proxy or binaries;
changing protocol execution rules still requires a coordinated release.

## Ethereum RPC simulation

On public HTTP, WebSocket and IPC transports, `eth_call`, `eth_estimateGas`,
`eth_createAccessList`, and `debug_traceCall` with the explicit `pending` tag serve
an existing executed pending payload with Reth's normal state and environment.
Each method remains available only when its namespace is enabled. `txIndex`
tracing retains Reth's replay of an actual block and its transaction bounds; it
does not create transaction positions for a derived pending view.
If no such payload exists, the node snapshots the latest L2
parent and checked local L1 headers, then executes the real pre-application
prefix in request-local state: EIP-2935 and the native history transaction only
when ready evidence allows an import at an eligible position. The simulated
application continues before block finalization.
Omission leaves storage and the genesis-configured phase unchanged; the inherited or newly
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
The generator executes the pinned proxy constructor, retains its immutable admin,
and records the proxy, implementation and ProxyAdmin accounts. The three Rust
runtime assets and generated Solidity proxy-code identity must agree with genesis.
The committed bytes and genesis fixtures are checked by
`bash scripts/update-eezl2-genesis.sh --check`.
