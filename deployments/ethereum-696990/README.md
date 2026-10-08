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

## Provenance

Copied from the mainnet node's `data/genesis.json` (supplied by the operator on
2026-10-08). On 2026-10-08 its computed state root matched block 0 served by
`https://mainnet.eez.dev/rpc/l2`, as did the chain ID, timestamp, gas limit,
`mixHash` and `extraData`.

## Changes after genesis

None recorded. An upgrade of EEZL2 to protocol `b75dddf` is planned; see
[the upgrade plan](../../docs/proposals/protocol-b75dddf-upgrade.md).
