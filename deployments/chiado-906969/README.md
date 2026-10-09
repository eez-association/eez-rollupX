# Chiado L2 at eez.asuscomm.com (chain 906969)

| Field | Value |
| --- | --- |
| L1 | Gnosis Chiado (chain 10200) |
| L2 chain ID | 906969 |
| Genesis block hash | `0x10c9ab15fb55a85ca101320e9bf842a67946704b36b98f5ae18ecac8ba8ffcd5` |
| Genesis state root | `0xc0082ef15873213ec8a9217c0bfd503c0058dd73422c8e318cc98625f0ebac5e` |
| L1 reference | block 23412901, `0x6fe1b5ba09ae3cf3c35536d4ec0205048610001df535703884f32a55ebfe3acc` |
| Schedule | 1 s blocks, Sync every 5 blocks |
| Deployed | 2026-10-07, protocol `855fe06`, deploy image `f61fbc1` |
| `genesis.json` SHA-256 | `ec2ca22f45a8801ca4e955c4ab4dd6f2cea268bd85a63a85a22d755cf48cc7bf` |

## Contracts

| Contract | Address |
| --- | --- |
| EEZ registry (proxy, Chiado) | `0x3c81284dae576d5152c9ca290668537618c44e5e`, deploy block 23412936 |
| EEZ registry `ProxyAdmin` | `0x91538a6710fae2abf7736efb0bf007f85c09c02f` |
| Rollup manager (proxy) | `0x049cc99c3875bd9b6eb997a9d551445c090a839c`, rollup ID 1 |
| EEZL2 (`EEZProxy`, L2) | `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c` |
| EEZL2 implementation at genesis | `0x4c3d971b1079e58843d0e1af155a2e54b9e54dba` |
| EEZL2 `ProxyAdmin` (L2) | `0x57bf29a07746a25b87c613e01d5da49f6119e96c`, owner `0x2ea2a5a1fe1ba4f4134c993e0c9a700706031e52` |

## Provenance

Copied from the node's `datadir/genesis.json` on 2026-10-08. Its computed state
root and block 0's hash match the network and the deployment's
`EEZ_INITIAL_BLOCK_HASH` / `EEZ_L2_GENESIS_STATE_ROOT`.

## Changes after genesis

None recorded. This network is the rehearsal target for the
[protocol `v0.1.0-rc.2` upgrade](../../docs/proposals/protocol-rc2-upgrade.md).
