# Burn protocol balances at block start and remove unbacked genesis ETH

| Field | Value |
| --- | --- |
| Design status | Draft, implemented on `feat/zero-system-balances`; process validation pending |
| Date | 2026-09-30 |
| Source baseline | `eduadiez/rollupX` `main` at `b791d78` |
| Protocol baseline | `eez-core-protocol` `9d13218` (unchanged) |
| Related work | [Signer specification](../../crates/eez-proof-signer/SPEC.md), [Chiado deployment runbook](../runbooks/chiado-deployment.md#genesis-allocations) |

## Problem and scope

L2 ETH is minted by `eez-evm` for each native system transaction and must stay
backed 1:1 by the rollup's `etherBalance` on L1. Two places held ETH that no L1
value backs or that can never move:

- `EEZL2` receives the full committed value of every loaded table. With partial
  consumption, reverted incoming calls or table replacement, the unconsumed
  value stays in `EEZL2` (the protocol states "Table replacement retains unused
  ETH"; core spec §H.2 leaves L2 value to the prover).
- `SYSTEM_ADDRESS` receives EEZL2's outbound "burn" transfers, proxy
  pre-deployment sweeps (`RECOVERY_ADDRESS()` on L2) and arbitrary transfers,
  but can spend only freshly minted value, so its balance only grows.

The base `genesis.json` also funded public development keys, `0xdead…dead`, the
retired key-based system address `0xE3601f…` and a `BridgeReceiver` smoke
predeploy at `0x4200…0008`. Those balances are unbacked on L1.

Non-goals: changing `EEZL2` or other pinned protocol contracts, or the upstream
protocol's L2 value model.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | Every L2 block sets `balance(EEZL2)` and `balance(SYSTEM_ADDRESS)` to zero before its first transaction; nonce, code and storage are unchanged. | `eez-evm` block tests on nonzero balances. |
| R2 | Zero or absent accounts receive no state transition. | `eez-evm` block test with an absent `SYSTEM_ADDRESS` and zero-balance `EEZL2`. |
| R3 | L1 simulation keeps Ethereum semantics. | Same block test on `ExecutionLayer::L1Simulation`. |
| R4 | Prefix block-hash commitments stay consistent between Composer and signer. | The burn is a pre-execution change (included in every per-transaction state root), never a post-transaction change; `partial_consumption` and `cross_chain` process tests. |
| R5 | The production base genesis allocates no ETH. | `update-eezl2-genesis.sh --check` fails on a funded account; `eez-evm` genesis test. |
| R6 | Development networks keep their funded public keys. | Kurtosis and Chiado select `genesis.dev.json` through `EEZ_GENESIS_BASE`; Kurtosis CI workloads. |

## Design and alternatives

`EezBlockExecutor::apply_pre_execution_changes` runs the burn after the EIP-2935
history write. A table is consumable only in the block that loaded it
(`EEZL2._validateProxyAndGetDestinationInfo` requires `lastLoadBlock ==
block.number`), and inbound system transactions load and consume in one
transaction, so both balances are dead at the next block start.

Alternatives:

- **Burn in `finish()` (block end).** Rejected: the signer derives prefix block
  hashes from per-transaction state roots and requires no post-transaction state
  changes, while the Composer hashes finished prefix blocks.
- **Burn at `loadExecutionTable`.** Rejected: residue appears after the load,
  inbound calls also mint, and a native edit inside a transaction would change
  its state diff.
- **Remove EEZL2's transfer to `SYSTEM_ADDRESS`.** Rejected: it mixes withdrawn
  ETH into the pool that funds incoming calls and patches the pinned protocol.
- **Top-up minting.** Rejected: it bounds rather than removes residue and lets
  unbacked residue pay for backed calls.

Genesis: `genesis.json` becomes the production base (EEZL2 and EIP-2935 only).
`genesis.dev.json` adds the 20 public development accounts. `0xdead…dead`,
`0xE3601f…` and `0x4200…0008` are removed everywhere; the deployment's
`BridgeSender`/`BridgeReceiver` smoke step is removed because its L2 receiver no
longer exists.

## Compatibility and recovery

The burn changes execution and state roots from the first block in which either
account holds ETH. It is a fresh-network boundary like the native-transaction
format: pre-change nodes, followers and signers cannot replay a network created
with it, and it is not a binary-only upgrade for existing networks. Genesis
changes alter the rendered genesis identity; they affect new deployments only.

## Validation plan

- `cargo +1.95.0 test --locked -p eez-evm` (R1–R3, R5).
- `bash scripts/update-eezl2-genesis.sh --check` (R5).
- `forge test --root contracts` after removing the smoke bridge contracts.
- Workspace `clippy`, signer and Composer unit tests, and the
  `partial_consumption`, `cross_chain` and `queue_liveness` process tests (R4);
  these require the LLVM 22 toolchain from `scripts/ci/install-llvm.sh`.
- Kurtosis CI workloads with `genesis.dev.json` (R6).

## Implementation and follow-up

Implementation is on `feat/zero-system-balances`. Record the validation results
and any unmet criteria in the pull request before acceptance.
