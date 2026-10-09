# Ethereum mainnet L2 (chain 696990)

| Field | Value |
| --- | --- |
| L1 | Ethereum mainnet (chain 1) |
| L2 chain ID | 696990 (`0xaa29e`) |
| Genesis block hash | `0xb8d94a6bdaee075d59df35f5577825944f7804dd8cb33c036832ed9daa1309e0` |
| Genesis state root | `0x162938959b86e35ccd215f51c57501ab39db5688dd3553fcb0586075e176eef0` |
| Genesis timestamp | `0x6ac36757` |
| L1 reference | block 26125137, `0x3efb8b2453f8dfb85b0e91eeab9cecd7b0f697d3b911e2f4e93326fb857f44d9` |
| Schedule | 2 s blocks, Sync every 6 blocks |
| `genesis.json` SHA-256 | `310a2cd98d2c4b21b38cb35d80296f84282767b4a2dc31f8ea76a2883df610a8` |

## Genesis allocations

| Address | Role |
| --- | --- |
| `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c` | EEZL2 (`EEZProxy`) |
| `0x4c3d971b1079e58843d0e1af155a2e54b9e54dba` | EEZL2 implementation at genesis (protocol `855fe06`) |
| `0x57bf29a07746a25b87c613e01d5da49f6119e96c` | EEZL2 `ProxyAdmin`, owner `0xceaf908c05b277e92e89b51b281ec76305b9a141` |
| `0x0000f90827f1c53a10cb7a02335b175320002935` | EIP-2935 block-hash history |

The implementation's immutables are rollup ID 1, system address
`0xeeee…0076`, `USE_GAS_LEFT = false` and recovery address
`0xceaf908c05b277e92e89b51b281ec76305b9a141`. No account holds ETH at genesis.

## Managers before the planned upgrade (read 2026-10-08)

Both managers sit at the same address on their chains, behind the same
`ProxyAdmin` address, with the same owner.

| Chain | Manager (proxy) | Implementation | Implementation code hash | `ProxyAdmin` / owner |
| --- | --- | --- | --- | --- |
| L1 (chain 1) | `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c` | `0x4c3d971b1079e58843d0e1af155a2e54b9e54dba` | `0x16c4c81a9b2259f9b1b082e7316b8d7b20d083404425d96bd17ff0a44e30cd91` | `0x57bf29a07746a25b87c613e01d5da49f6119e96c` / `0xceaf908c05b277e92e89b51b281ec76305b9a141` |
| L2 (chain 696990) | `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c` | `0x4c3d971b1079e58843d0e1af155a2e54b9e54dba` | `0x432e1c7af4d49fc907681f91e313965b17c3f35c857d8c58442cdbfca7925e4d` | `0x57bf29a07746a25b87c613e01d5da49f6119e96c` / `0xceaf908c05b277e92e89b51b281ec76305b9a141` |

`PROXY_INIT_CODE_HASH()` is
`0x26f77730044fbffb547593b1a676eb53cbbb9a57879c3f2cf090cee88a283a4a` on both.
The rollup manager on L1 is `0xb3fb7a12f1cfaa51fa39246cf17205872ca156d0`.

## Provenance

Copied from the mainnet node's `data/genesis.json` (supplied by the operator on
2026-10-08). On 2026-10-08 its computed state root matched block 0 served by
`https://mainnet.eez.dev/rpc/l2`, as did the chain ID, timestamp, gas limit,
`mixHash` and `extraData`.

## Changes after genesis

None recorded. An upgrade of both managers to protocol `v0.1.0-rc.2` is planned; see
[the upgrade plan](../../docs/proposals/protocol-rc2-upgrade.md).
