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
`0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076`, at exactly
`block.timestamp == cutoff + updateDelay`, with `cutoff == nextCutoff`.
It appends consecutive nonzero hashes after the current endpoint and advances
the cutoff by one interval. An empty import still advances the schedule.
Existing records, including identical ones, cannot be overwritten. Overflow,
invalid scalar state and an import beyond the configured limit revert. It
emits no logs and calls no other contract.

The execution protocol, not this Solidity contract alone, authenticates L1
headers, chooses the maximal source, derives exact calldata and enforces the
runtime and storage profile. It invokes the import before applications, at
the mandatory scheduled position, and invalidates the entire block on any
failure. The system call has its own fixed budget; it creates no transaction
receipt or fee, does not consume transaction gas, and must preserve the system
account's balance and nonce. The L2 header and EVM PREVRANDAO must both equal
the selected L1 header's value. These rules apply from genesis during live
execution, stateless validation and follower replay.

The RollupX integration fixes the predeploy at
`0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077`, the maximum import at 64 hashes,
and the call budget at 2,000,000 gas. Its profile commits to the runtime,
bootstrap, schedule and limits. The [RollupX proposal](../docs/proposals/l1-origin-and-offline-proving.md) specifies evidence, anchoring,
public inputs and offline proving. The predeploy does not authenticate L1
consensus or grant signing authority by itself.

Genesis generation compiles this local source in a disposable tree at the
canonical Solidity source name `src/L2/L1BlockhashContract.sol`, with the pinned
core compiler settings and libraries. That source name is part of the runtime
metadata and thus its hash. The generator never modifies the core checkout.
The committed runtime and genesis fixtures are checked by
`bash scripts/update-eezl2-genesis.sh --check`.
