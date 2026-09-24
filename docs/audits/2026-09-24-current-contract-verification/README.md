# Current Chiado and L2 contract source verification

Status: eight full source matches on the active 2026-09-23 network; the L2
`BridgeReceiver` remains unverified. This record is for registry
`0x5d6b8fc56e43528c219de7eed257564c05923e08` on Chiado 10200 and L2
chain 6291. The September 18 verification inventory in the operator checkout
belongs to the retired 6290 network.

## Source and method

- Verification checkout: fork `5fb0a0ead7290b7906141e75fb0843a3a6b8e245`,
  clean before the audit edit; protocol gitlink and initialized checkout
  `820e50f749f63165be6a5797d4c5fe52cbbbbe34`.
- Deployed network source: fork base
  `1832f970884ca9460646a4cde87cca70eaffb3e1` plus the retained local
  feature/events patch, as recorded in the [deployment audit](../2026-09-23-events-network/README.md).
  The exact compiler artifacts are local-only in
  `artifacts/events-deployment-20260923/core-contract-artifacts/` and
  `artifacts/events-deployment-20260923/bridge/out/` in this checkout.
- All eight artifact source sets passed byte-for-byte Keccak comparisons with
  their metadata's `sources[*].keccak256` at the pinned checkout. Standard JSON
  used the original source keys and metadata settings, omitting only
  `compilationTarget` and adding `outputSelection`. The compiler was
  `v0.8.34+commit.80d5c536`, optimizer runs 1, `viaIR`, Osaka EVM.
- For EEZ, Rollup, ECDSAProofSystem and BridgeSender, each retained creation
  transaction's input began with the exact artifact creation bytecode; the
  remaining bytes were its constructor arguments. L1 CrossChainProxy and both
  periphery Bridges were also fully matched by Blockscout. The L2 EEZL2 live
  runtime hash matched active `EEZ_L2_EEZL2_CODE_HASH`
  (`0x1f7a76d0120aa9e25c1eb15d26a371404cb252acdbff381dfa2e26eb0d0bed13`).
- Submitted Standard JSON to each Blockscout verifier, then queried the public
  `/api/v2/addresses/<address>` records. For the genesis-installed EEZL2,
  submitted the same Standard JSON through Blockscout's Etherscan-compatible
  API with the exact `(uint64,address,bool)` constructor arguments and received
  `Pass - Verified`. Submission acknowledgments alone were not counted.

## Final explorer inventory

All `Full` rows returned `is_verified=true` and no partial-verification flag
from the public explorer API on 2026-09-24.

| Chain | Contract | Address | Source result |
| --- | --- | --- | --- |
| [Chiado](https://gnosis-chiado.blockscout.com) | EEZ registry | [`0x5d6b…23e08`](https://gnosis-chiado.blockscout.com/address/0x5d6b8fc56e43528c219de7eed257564c05923e08) | Full |
| Chiado | Rollup manager | [`0x0205…0ca8`](https://gnosis-chiado.blockscout.com/address/0x0205d8d23200b59120d2886979752af80af20ca8) | Full |
| Chiado | ECDSAProofSystem | [`0xb3c2…07b7`](https://gnosis-chiado.blockscout.com/address/0xb3c27735413385ba7149b48c6b528ef64eff07b7) | Full |
| Chiado | BridgeSender | [`0x8b81…7948`](https://gnosis-chiado.blockscout.com/address/0x8b81477d5925749b6a2b668f37156b431d727948) | Full |
| Chiado | CrossChainProxy | [`0x893d…764e`](https://gnosis-chiado.blockscout.com/address/0x893d0b5463e0f01ae911efd0d797453af9f4764e) | Full |
| Chiado | wallet Bridge | [`0xCa0a…c8fD`](https://gnosis-chiado.blockscout.com/address/0xCa0a7803C6d982628A48B1Cc82009D6493e3c8fD) | Full |
| [L2](https://explorer.eez.dev) | wallet Bridge | [`0xCa0a…c8fD`](https://explorer.eez.dev/address/0xCa0a7803C6d982628A48B1Cc82009D6493e3c8fD) | Full |
| L2 | EEZL2 | [`0x4200…0007`](https://explorer.eez.dev/address/0x4200000000000000000000000000000000000007) | Full |
| L2 | BridgeReceiver | [`0x4200…0008`](https://explorer.eez.dev/address/0x4200000000000000000000000000000000000008) | Unverified |

## Receiver mismatch and remaining limit

The retained `BridgeReceiver` artifact and active genesis each have 64 bytes of
runtime code. Their first 21 bytes agree, including all 11 executable bytes and
the CBOR/IPFS metadata header. Only the 32-byte IPFS metadata digest differs:
artifact `3ebf67407621a7ab8091c9cb20da1142c5c302c0545e94aa7c1ea98cc6dcff3`,
genesis `daea6b25f46ab7bb15282500f09415f984751ea95ce376de5eaea3366d828ac6`.
The genesis metadata CID is `Qmd5CuennDfsajLHP4mmuHhReAE8KRQDSrfHEVHNaenE4D`;
two public IPFS gateway reads returned HTTP 429. The previous network also had
only a partial receiver match, which does not verify this new genesis.

Automatic approval review rejected submitting the known mismatching artifact
to the current L2 explorer because it could create a misleading verification
record. No receiver verification request was sent. A full match requires the
original Standard JSON whose compiler metadata reproduces the genesis digest.
An explicitly approved partial-match submission would need to be labeled as
executable-code equivalence, not full source verification.

No contract was deployed, no wallet was funded, and no service was restarted.
This operation checked explorer source status and the bytecode evidence above;
it did not rerun contract tests or independent follower replay. Local-only
Standard JSON and constructor-extraction evidence is under
`/tmp/eez-contract-verify-20260924/` and is not durable across hosts.
