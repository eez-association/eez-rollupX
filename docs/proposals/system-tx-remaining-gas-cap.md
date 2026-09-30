# Native transaction budget from the remaining block gas

| Field | Value |
| --- | --- |
| Design status | Draft. Implemented on branch `feat/system-tx-remaining-gas-cap`; not yet accepted or deployed. Supersedes requirement N2's fixed 2,000,000 budget in [native system transactions](native-system-transactions.md). |
| Date | 2026-09-30 |
| Source baseline | `main` at `4f2da1d8aa7c881a77750559257a0d83aa590432` |
| Protocol baseline | `9d13218a355e7d543ca8a9927eea2962600b716d` (the pinned core specification defines no native-transaction gas budget; this is a fork execution rule) |
| Related work | [Native system transactions](native-system-transactions.md) (N2), [signer specification](../../crates/eez-proof-signer/SPEC.md), issue #35's original remaining-block-dependent gas requirement |

## Problem and scope

Native type `0x76` transactions executed with a fixed 2,000,000 gas budget. That
budget is too small for this fork's nested, reverting and grouped cross-chain
calls, which can legitimately need up to Osaka's per-transaction limit. Raising
the fixed budget to 16,777,216 would make Ethereum's block-gas rule reserve that
amount per native transaction: a transaction is admitted only while
`block gas used + declared limit <= block gas limit`, so a native transaction
could not be placed after 13,222,784 gas of a 30M block had been used, even when
it actually needs far less. The fixed 2M budget is removed entirely; there is no
configuration, fallback or legacy mode.

Non-goals: a per-entry L1-committed gas limit, a larger L2 block gas limit, or
changes to ordinary transaction rules.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| G1 | A native transaction executes with `min(16,777,216, block gas limit - cumulative gas used by preceding transactions)`, derived from block contents only | `eez-evm` tests: cap in an empty block; exact remainder after a 16M ordinary transaction; exact 30M fill by two out-of-gas calls; repeated execution agrees |
| G2 | No declared-limit reservation for native transactions; ordinary transactions keep Ethereum's rule | `eez-evm` remainder test; Composer draft refuses an ordinary transaction declaring more than the remainder |
| G3 | A budget below intrinsic gas (including EIP-7623's floor) invalidates the block; nothing is truncated or skipped | `eez-evm` intrinsic/floor rejection test |
| G4 | Every consensus execution path applies the same rule: block executor, Composer block build and prefix hashes, deriver/follower, driver witness/observation replay and signer stateless validation | All use `EezBlockExecutorFactory`; the Composer draft (direct `transact`) applies the shared `system_tx_gas_limit`; draft/build equivalence tests |
| G5 | The Composer never commits a packing-dependent native result: a clamped native transaction is accepted only if its result and state changes equal those under the full cap | Draft tests: budget-independent call accepted; halting call and `GAS`-storing call refused as capacity while an empty block accepts them |
| G6 | Capacity refusals defer whole groups without eviction or attempt charges; a group that cannot fit an otherwise empty block is evicted as unfittable | Composer unit tests for the refusal classification; `capacity_probe` runs before the prefix-hash rebuild so a misfit is never a transient slot abort |
| G7 | Composer and signer identities differ from the 2M profile | Validator revision `v5`, `system=0x76/nonce/remaining-gas-cap16777216` |

## Design and alternatives

`eez_primitives::system_tx_gas_limit` defines the budget. `EezBlockExecutor`
sets it on the transaction environment for native transactions on the rollup
layer and transacts directly, bypassing the inner executor's declared-limit
check; the rest mirrors the pinned `alloy-evm` executor. The native envelope's
`gas_limit()` and RPC `gas` field report the cap as an upper bound.

The Composer's live draft executes transactions directly rather than through the
block executor, so it applies the same budget. When the budget is below the cap
it first executes a reference run at the cap on the same pre-state without
committing, then the clamped run; a differing `ResultAndState` (status, gas,
output, logs, state) is a `BlockCapacity` refusal. Native transactions pay no
fees, so equal executions leave identical state. The cost is one extra execution,
only for native transactions placed after more than `30M - cap` gas.

Admission prechecks charge each native transaction only its 21,000 base cost.
`SyncBlockState::capacity_probe` then decides the fit before the canonical
prefix-hash rebuild: when declared upper bounds (cap per native transaction,
declared limit per ordinary one) fit, nothing is executed; otherwise the group
runs on a throwaway fork. A capacity refusal defers the source for a later slot,
or evicts it as unfittable when nothing precedes it in the block.

Alternatives considered:

- Fixed 16,777,216 budget: simplest, but reserves the cap and limits heavy
  blocks to one or two native transactions.
- Larger consensus block gas limit with a builder soft limit for ordinary
  transactions: retains reservation semantics, raises the worst-case proving
  and witness cost.
- Budget derived from calldata shape: helps loads but cannot bound arbitrary user
  code in nested deliveries.
- L1-committed per-entry gas limit (as OP deposits): best pricing model, but a
  core-protocol change across ABI, codecs, Composer, signer and deriver.

## Compatibility and recovery

This rule applies from genesis of a new network. Chains that executed native
transactions under the fixed 2M budget are not supported by this revision:
their history would not replay, and no activation height, migration or mixed-
version mode exists. Composer, signer, followers and genesis must come from the
same revision.

Execution outside a block (for example tracing or re-executing a single native
transaction with the envelope's default environment) uses the cap, not the
position budget, and can differ for a transaction that was clamped in its block.

## Validation plan

Unit: `eez-primitives`, `eez-evm`, `eez-composer` (draft and admission),
`eez-control-rpc`, `eez-proof-signer`. Process: `native_system`, `cross_chain`,
`settlement_policy_gas`, `chained_interstate`. CI runs the full matrix.

## Implementation and follow-up

Implementation and the checks actually run are recorded in the pull request for
branch `feat/system-tx-remaining-gas-cap`.
