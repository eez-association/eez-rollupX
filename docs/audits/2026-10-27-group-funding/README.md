# Complete-group funding independence

This is dated baseline evidence. For the implemented #105/#108 amendments and
the latest source-index scope, see the
[current integration audit](../2026-10-27-group-cleanup-and-history/README.md).

| Field | Value |
| --- | --- |
| Date/status | 2026-10-27 (Europe/Madrid); funding investigation complete; group cleanup is not implemented |
| Question | Can each canonical RollupX execution group run without residual SYSTEM/EEZL2 funding from a previous group? |
| Production source | `6c3106fb482bb323eefa8898a0bd922fc7520480`, PR #103; unchanged throughout this investigation |
| Review branch/base | `codex/review-protocol-issues-104-108`, `fcd86822460b17a78179b6264dbdcd2098833689`; only tests and audit records added |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`; initialized, unchanged and clean |
| Tools | Rust 1.95.0 with locked dependencies, Foundry 1.7.1, Solidity 0.8.34 |
| Evidence | [Results and executable/source digests](validation.json), [contract regressions](../../../contracts/test/ProtocolGroupFunding.t.sol) |
| Context | [Issue #105](https://github.com/eez-association/eez-rollupX/issues/105), [combined review](../2026-10-26-protocol-issues-review/README.md) |

## Finding

Yes for the current canonical RollupX funding policy: a complete group receives
its full committed incoming value without deducting earlier inventory. No funding
dependence on the preceding group's residual balances was found. This resolves
the funding concern for evaluating cleanup after a complete group; it does not
implement that cleanup or establish its checkpoint/replay correctness.

This conclusion concerns manager payout funding. It does not claim that clearing
balances is invisible to application code. A contract reading SYSTEM/EEZL2's
balance can observe the new policy, and execution commitments will change.
Ordinary user value/gas still require the user's own balance. FeeVault and other
application accounts are outside the proposed cleanup.

## Source argument

1. [`committed_l2_table_value`](../../../crates/eez-protocol/src/system_tx.rs)
   sums **all** incoming call values in every loaded mutable entry and every
   nested row's incoming array. It rejects uint256 overflow. It does not consult
   or subtract any current account balance or anticipated refund.
2. `build_outbound_group` funds the complete mutable table for the one paired
   signed user transaction, including all roots and callbacks. Static rows are
   checked to carry zero value. A checked
   [`InboundSidecar`](../../../crates/eez-protocol/src/entries/inbound.rs) uses
   the same sum for its single native delivery.
3. The [native EVM path](../../../crates/eez-evm/src/evm.rs) credits the native
   transaction's value to SYSTEM before transferring it to EEZL2. Native fees are
   zero. On failure, it removes that temporary credit rather than borrowing from
   prior SYSTEM funds. Existing tests cover zero and nonzero opening balances,
   reverts, out-of-gas and uncommitted execution.
4. The [pinned manager](../../../eez-core-protocol/src/L2/EEZL2.sol) spends its
   incoming allocation through the loaded flat/nested call arrays. Committed
   consumption moves forward. An ordinary enclosing revert that makes a row
   reusable also rolls back its payouts. Explicit revert spans restore funds
   even when their hash/cursor evidence survives. Static calls cannot transfer
   value. Forwarding an outgoing `msg.value` to SYSTEM forwards funds just
   received from that caller; it does not require earlier SYSTEM inventory.
5. Therefore the sum of the complete table's incoming values is sufficient from
   zero manager inventory for its represented payouts. Unused, failed and
   deliberately reverted calls can leave surplus; a later group receives its
   own full allocation again. Arbitrary core users can underfund a load and use
   pooled balances, but that is not the canonical RollupX construction.
6. Composer and deriver use the shared group builder. The signer independently
   checks [inbound native value](../../../crates/eez-proof-signer/src/settlement/inbound.rs)
   and [exact reconstructed native bytes](../../../crates/eez-proof-signer/src/settlement/da.rs).
   Earlier pooled balances cannot justify an underfunded canonical envelope.

This argument requires whole groups: multiple roots in one signed user share
their one allocation. Clearing after the first root would remove funds that
later roots still need. Canonical suffix reconstruction retains complete groups;
it does not create a new group starting inside a loaded user transaction.

## Executed validation

The new Foundry fixture executes the pinned EEZL2 and real proxies. At complete
group boundaries it uses `vm.deal` to model clearing both protocol accounts.
Each native load/delivery is given only its own allocation. Recipient balances,
caller state, cursor rollback and specific rejection data are asserted.

| Case | Result |
| --- | --- |
| Nested outbound, clear, then inbound | Passed, including 256 fuzz cases with positive bounded values |
| Nested inbound, clear, then outbound | Passed |
| Two roots sharing one outbound load | Passed; the first root leaves exactly the later root's allocation |
| Source frame succeeds then reverts; caller retries without another load/credit | Passed; payouts/cursor roll back and the original allocation funds the retry |
| Retained user failure followed by cleared balances and a fresh group | Passed |
| Explicit revert span leaves surplus; clear it before the next group | Passed |
| Omit the nested allocation | Rejected with `RollingHashMismatch`; the same table/call succeeds with exact complete funding |
| Failed nested row with descendants retried twice | Core-only stress test passed; this shape remains rejected by RollupX's narrower profile |

The fixture manually supplies allocations derived from the stated call values.
It does not invoke Rust encoding. Separate Rust tests exercise the production sum,
checked sidecar, complete grouping, static restrictions, overflow, suffix
reconstruction, native minting/rollback and the signer's exact-value rejection.

| Command | Final result |
| --- | --- |
| `FOUNDRY_ISOLATE=false forge test --root contracts` | 165 passed, 0 failed/skipped, including all 8 new funding tests |
| `FOUNDRY_ISOLATE=false forge test --root contracts --match-contract ProtocolGroupFundingTest -vv` | 8 passed, including 256 fuzz cases |
| `forge fmt --check --root contracts` | Passed |
| `cargo +1.95.0 test --locked -p eez-protocol --lib system_tx::tests::` | 10 passed, 198 filtered |
| `cargo +1.95.0 test --locked -p eez-protocol --lib entries::inbound::tests::` | 2 passed, 206 filtered |
| `cargo +1.95.0 test --locked -p eez-evm --lib native_mints_without_prefunding` | 1 passed, 36 filtered |
| `cargo +1.95.0 test --locked -p eez-evm --lib failed_validation_and_uncommitted_execution_do_not_leak_minted_value` | 1 passed, 36 filtered |
| `cargo +1.95.0 test --locked -p eez-proof-signer --bin eez-proof-signer strict_inbound_observation_` | 3 passed, 328 filtered |

There are 182 distinct passing test functions across the final contract and Rust
selections; the focused contract runs repeat tests included in the full suite.
The signer tests are validator unit tests, not an independent execution/signing
process. Existing compiler warnings were emitted for the ignored proxy recovery
call result and, on the first build, the EmptyCall fixture fallback.

The first new-fixture compile required explicit `bytes` casts for two conditional
hex literals; no tests executed on that attempt. An initial signer command used
`--lib`, but this package has only a binary target; it also executed no tests.
The formatter was rerun with `--root contracts` to use the owning configuration.
These were corrected test/setup issues, not production failures or flaky passes.
Logs and digests, including the first failures, are recorded in `validation.json`.

## Remaining implementation work

No funding change is needed to make the canonical groups independently funded.
Option 2 still needs a shared cleanup transition at the **complete-group** boundary,
with precise included-failure and speculative rollback rules, gas/receipt policy,
and checkpoints captured after cleanup. Keep block-start cleanup for ordinary
transfers outside groups. Update the normative execution/signer contracts and
validation identity together for a fresh network.

Validate Composer, follower and offline signer against identical post-cleanup
checkpoints, full block hashes, partial consumption and restart/replay. None of
those new-policy process checks, live operations or deployment were performed
here. The production branch, operator checkout and protocol gitlink were not
modified; no issue comments were posted.
