# FeeVault operation and upgrades

This procedure describes new networks generated with the upgradeable treasury
installed in genesis. All roles must start with the same generated genesis/profile
and fresh datadirs. There is no legacy-vault migration or compatibility path; see
the [deployment scope](../proposals/fee-vault-upgrades.md#compatibility-and-deployment).

## Inspect before acting

Read the deployment's genesis profile: `feeCollector`,
`feeVaultImplementationAddress`, `feeVaultProxyAdminAddress`, initial owner and
recipient. Inspect current ERC-1967 implementation/admin slots at the collector,
then read treasury `owner()`, `pendingOwner()`, `recipient()`, `pendingRecipient()`,
`minWithdrawalAmount()` and `totalWithdrawn()`. Read `owner()` on the ProxyAdmin
separately. Genesis describes initial state; current implementation and owners
can have changed. Verify deployed code against the selected source/build.

## Payments and governance

Anyone may call `withdraw()`; it pays the complete balance to the active
recipient. It reverts on an empty vault, below the configured minimum, or if the
recipient rejects payment. The counter and event roll back on failure. The
transaction's own base fee is credited after execution, so a successful withdrawal
may leave a new balance. `totalWithdrawn` includes donations and any repeatedly
withdrawn funds sent back to the vault; it is gross payments, not fee income.

The treasury owner calls `proposeRecipient(newRecipient)`. The proposed recipient
must call `acceptRecipient()`; a recipient contract must support making that call.
Until acceptance, payments still go to the old recipient. The owner may cancel
with `cancelRecipientChange()` or replace the pending proposal. Review the exact
destination and verify acceptance before treating rotation as complete.

`setMinWithdrawalAmount(wei)` changes the payment threshold; its initial zero
permits any positive balance. This is treasury state, not a node setting.

Treasury ownership uses `transferOwnership` and `acceptOwnership`. ProxyAdmin
ownership uses its separate standard `transferOwnership` call. Transferring one
does not transfer the other. Pending recipient proposals also remain until
accepted, cancelled or replaced; inspect them during an ownership handover.
A multisig can hold both roles. Loss of the upgrade
authority cannot be repaired by changing an environment variable.

## Compatible implementation upgrade

1. Review the complete storage layout against the active implementation. Preserve
   inherited namespaced storage and existing treasury slots; append new fields.
   Test existing balances, counter, threshold, owner and pending recipient/owner
   through the upgrade. Reject an implementation with incompatible semantics.
2. Compile and test the selected implementation using the pinned build settings.
   Deploy the exact implementation and verify its runtime, source and disabled
   initializer. A local test alone does not authorize a live deployment.
3. From the ProxyAdmin owner, call that admin's `upgradeAndCall(proxy,
   implementation, data)`. Use empty data when no migration is required; otherwise
   test the exact migration. Do not call the original initializer again.
4. Check the canonical receipt and `Upgraded` event, current implementation slot,
   treasury state/balance and an independent follower's matching block/state.
   Verify the new implementation in the explorer. Retain source/artifact identity
   and the transaction hash in the operation's audit.

Compatible upgrades execute as ordinary L2 transactions and need no hard fork or
prover RPC change. The administrator can nevertheless replace all treasury logic;
its authority includes control of the funds. A downgrade is safe only if old code
understands the resulting state; an irreversible migration is not undone by
pointing the proxy back. Atomic migration failure leaves the old implementation
and state in place.

## Validation status

Local implementation and validation are tracked in the
[proposal](../proposals/fee-vault-upgrades.md). No network deployment or live
upgrade has been executed by this change. Fee-credit visibility belongs to the
[explorer](explorers.md#rollupx-fee-accounting); governance/payment events do not
record the EVM's direct balance credits.
