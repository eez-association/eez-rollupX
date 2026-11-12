# ETH accounting boundaries and bounded qualification

This closes the evidence-mapping task in [#146](https://github.com/eez-association/eez-rollupX/issues/146),
handoff N12. No functional accounting defect was established in the inspected
paths. This change adds two qualification regressions; it does not change
production execution. Missing evidence alone is not a defect, and these bounded
checks do not prove conservation for every application or execution path.

## Fixed inputs and provenance

| Input | Revision / use |
| --- | --- |
| Frozen rollup0 source | `1b66d9bda675e6f8e83f514153a97136909acc06`, protocol `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Frozen rollupX target | `31e148f50dd79730b17d113ac78795eb377eb5c5`, protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Issue's subsequent current-code check | `716a8a3bad4415ade7e9a6bc3af4a1b35419b232`, same target protocol |
| Qualification base | Association main `601ebfa9c7e1a779f1afbd077bd31a3207e7a61b`, same target protocol |
| Candidate | This commit's test/documentation changes on that base; exact executable and check identities in [validation.json](validation.json) |

The scratch checkout initially had a clean status and a local-path `origin`.
GitHub operations explicitly select `eez-association/eez-rollupX`. The protocol
was initially uninitialized, then initialized recursively at the exact gitlink.
No pin, dependency, schema, profile, genesis or runtime source changes are made.

The original review's input provenance was **not independently checked**. This
qualification does not restart the 174-commit audit in #133 or repair that
limitation. The fixed [source EVM file](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-evm/src/evm.rs)
and its protocol gitlink were retrieved for this task. The [frozen target EVM](https://github.com/eez-association/eez-rollupX/blob/31e148f50dd79730b17d113ac78795eb377eb5c5/crates/eez-evm/src/evm.rs)
is byte-identical to the qualification base's EVM file (SHA-256
`b43a8ef0282c087b73bd0fa7a310e655e16db466f93a36979ede578519263bb7`).
Source rollup0#236 is later/unmerged and excluded: neither implementation
evidence nor an accepted target contract. No source patch is imported.

## Accepted accounting oracles

Read the pinned [Core H.2](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md#h2-ether-accounting-l1),
[execution entries](../../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md),
[static/reverted calls](../../../eez-core-protocol/docs/STATIC_ENTRY.md) and
[caveats](../../../eez-core-protocol/docs/CAVEATS.md) together with the accepted
[native execution profile](../../proposals/native-system-transactions.md) and
[FeeVault rules](../../proposals/fee-vault-upgrades.md). The fork narrows Core's
pooled-inventory flexibility; it does not add a Solidity per-load invariant.

1. **L1 entry boundary.** For a successfully committed entry, the sum of
   `rollupUpdates[].etherDelta` equals all protocol entry/reentrant value received
   minus successful mutable payouts that survive enclosing reverts. Revert-span
   transfers do not survive. `_applyRollupUpdates` rejects escrow underflow.
   For these accounted flows, held ETH and aggregate rollup liabilities change
   equally. Direct/forced donations change surplus separately. Posting gas,
   base-fee burn, tips and builder refunds belong to the L1 transaction ledger,
   outside the entry's escrow equation. Proof acceptance is not consumption.
2. **L2 execution boundary.** For a closed, enumerated balance ledger, with no
   unenumerated account transfers or other EVM balance-destruction effects,
   `closing = opening + successful_native_credit - specified_cleanup`.
   Transfers between included accounts cancel. Count SYSTEM, EEZL2, users,
   targets/proxies, beneficiary, collector and any recovery recipient actually
   touched. A failed native execution removes its temporary credit; a reverted
   subtree contributes no surviving transfer. Synthetic destination-call credits
   are provisional simulation funding, not additional canonical issuance.
3. **Fees.** For admitted ordinary transactions, final receipt gas `g` includes
   refund/floor rules. Sender payment is `g * effectiveGasPrice`; collector credit
   is `g * baseFee`, beneficiary credit is the priority portion. Count aliased
   addresses once. Native history/cross-chain gas is metered but fee-free.
   Discarded previews contribute nothing. Advisory fee relaxations are not a
   canonical monetary oracle; they cannot distribute unpaid base fees.
4. **Cleanup.** Measure the actual residual SYSTEM/EEZL2 balances immediately
   before each complete-group cleanup and block finalization. Those balances
   are specified sinks, including donations and unused/reverted allocation.
   Do not call them missing funds or collector fees. Never clean between a load
   and its signed user, between roots or inside a callback. FeeVault is excluded.
5. **Supply versus backing.** Gross committed-table funding includes allocations
   whose effects may later revert. It is not necessarily net issuance. Net
   surviving cross-chain transfers must also match the independently recomputed
   L1 `etherDelta`; the gross mint/cleanup equation alone cannot establish this.
   Production genesis is zero-funded; development fixture balances are explicit
   test seeds and cannot establish production backing. Deliberate L2 cleanup
   need not reduce L1 escrow by itself. No global equality between L2 supply and
   escrow is asserted after voluntary donations to cleanup accounts.

## Production paths and evidence map

**Fresh** means executed for this change, as detailed below. **Historical** means
the linked prior record, not a current pass. **Source** identifies the current
implementation and regression source only. These labels apply independently;
an old suite result does not prove every assertion mentioned in a row.

| Path / accounting boundary | Producers and consumers traced | Oracle, existing evidence and remaining limit |
| --- | --- | --- |
| Production genesis / permitted issuance | [genesis](../../../genesis.json), [profile](../../../crates/eez-evm/src/profile.rs), [genesis tests](../../../crates/eez-evm/src/tests.rs) | Every production allocation has zero ETH; required predeploys and proxy slots retained. Fresh EVM genesis regression. Generated-byte/state-root regeneration is historical #112, not rerun here. |
| Ordinary success, contract call and transfer | [EVM handler](../../../crates/eez-evm/src/evm.rs), full [block executor](../../../crates/eez-evm/src/block.rs), Composer [incremental builder](../../../crates/eez-composer/src/local/build.rs) | Value debits/credits plus receipt-based fees. Fresh handler fee matrix and new paid-group ledger; arbitrary application recipients are not exhaustively enumerated. |
| Ordinary revert, halt, refund and calldata floor | Same handler; [EVM tests](../../../crates/eez-evm/src/evm/tests.rs), [group tests](../../../crates/eez-evm/src/block/tests/groups.rs) | Value/storage roll back, included nonce/fees remain. Fresh success/revert/halt/refund/floor matrix, including collector=sender/beneficiary, and new paid-group success/revert/halt cases. These are small fixtures, not all contracts. |
| Base fee and beneficiary accounting | `EezHandler::reward_beneficiary`, `EezEvm::execute`; [native profile](../../proposals/native-system-transactions.md) | Credit final charged gas, preserve next-transaction visibility, no collector code execution, reject overflow; L1 simulation retains Ethereum distribution. Fresh low-level regressions include zero-price advisory and same-transaction collector selfdestruction. Other lifecycle/address combinations remain unqualified. |
| FeeVault withdrawal, failure and returned funds | [FeeVault](../../../contracts/src/L2/FeeVault.sol), [tests](../../../contracts/test/FeeVault.t.sol), [treasury process](../../../crates/eez-node-e2e/tests/native_system/treasury.rs) | Withdrawal moves the pre-call whole balance; counter includes donations. Failed payment rolls back; reentry cannot pay again; returned funds remain withdrawable. Fresh 18 Solidity cases. Real paid withdrawal may leave new base fees credited after execution; producer/prover/cold follower evidence is [historical treasury audit](../2026-10-26-fee-vault-upgrades/README.md). |
| L1-origin success / direct delivery | [applied-root selection](../../../crates/eez-protocol/src/settlement.rs), [native builder](../../../crates/eez-protocol/src/system_tx.rs), [EEZL2](../../../eez-core-protocol/src/L2/EEZL2.sol), [inbound signer](../../../crates/eez-proof-signer/src/settlement/inbound.rs) | Native delivery funds all committed flat/nested incoming values; successful surviving entry flows bind net `etherDelta`. Fresh executor issuance/group tests and real-manager following-inbound test; bidirectional settlement/replay is historical #112. |
| L1-origin destination failure / static lookup / source-frame revert | Same applied-root selection, [inbound reconstruction](../../../crates/eez-protocol/src/entries/inbound.rs), signer inbound and destination observation replay | Failed/static/source-reverted roots authorize no native delivery and no L2 root movement; fees remain on L1. Fresh shared protocol tests include `failed_inbound_root_retains_source_callbacks_without_canonical_delivery`; caught-destination and history-observation processes remain historical. Fresh low-level failed-native execution is defense in depth, not proof of correct non-delivery selection. Nonzero-value source failure with full live settlement is a remaining case. |
| L2-origin load plus signed user / multiple roots | Shared `committed_l2_table_value` and `build_outbound_group`; [EEZL2 consumption](../../../eez-core-protocol/src/L2/EEZL2.sol), block and incremental group cursors | One gross-funded load immediately precedes its user; all roots share inventory until group completion. Fresh real-manager multiple-root/nested funding cases and executor boundary tests. Signer [exact load-value/overflow regressions](../../../crates/eez-proof-signer/src/settlement/tests/outbound.rs) are source evidence, with historical signer suite results. |
| L2-origin target failure caught by source | EEZL2 outgoing value forwarding, [L1 execution](../../../eez-core-protocol/src/EEZ.sol), shared [effect traversal](../../../crates/eez-protocol/src/entries/verify.rs), signer outbound | Failed remote payment and source proxy transfer unwind together when the proxy frame reverts; an included source may still pay fees. New fresh real-manager terminal failed nested payment checks exact saved revert and restored value, with no descendants. Existing process failure checks mainly establish state/revert bytes; they do not by themselves establish an ETH ledger. |
| Whole signed user revert/halt after load | Handler credit rollback, EEZL2 EVM rollback, complete-group cleanup in both executors | Loaded inventory remains after user rollback, then is cleared as specified; native load gas is fee-free, included user gas paid. Fresh new paid-group ledger and existing revert/halt/discard tests. Real-manager user-revert and recovered-source retry tests also fresh; those use manual cleanup, not production hooks. |
| Callback / nested success and terminal failure | Composer [synthetic call owner](../../../crates/eez-composer/src/local/build.rs), shared traversal, EEZL2 `_resolveNestedReentrant`, signer outbound/inbound | Sum callback allocations once per committed table; actual payouts/refunds follow EVM rollback. Fresh real-manager nested fuzz (256 runs) and new terminal failed-row fuzz (256 runs). Existing `CoreRevertedNestedRetriesRestoreFundingBeforeReuse` has failed descendants and is explicitly broader than the accepted fork profile. It is not qualification of that unsupported profile. |
| Synthetic target funding, returned ETH and discarded forks | `transact_funded_call`, Composer `execute_call_inspected`, [fork/restore tests](../../../crates/eez-composer/src/local/build.rs) | Preserve prior proxy funds and returned value; remove credit on revert/halt/error, restore synthetic sender nonce, discard source-reverted forks. Fresh low-level funded-call/error tests; Composer cumulative-fork tests are source/historical evidence only. No simulation credit is counted as an extra settled transaction. |
| Revert spans / retries | EEZ/EEZL2 isolated `executeInContextAndRevert`, shared traversal rollback scopes | State/value rolls back while recorded outcomes/hash survive the deliberate context revert. Fresh real-manager revert-span residue and recovered-source retry cases. Core retry caveats still apply; general retries after changed local state are not authorized by this test. |
| Complete-group cleanup and discarded candidate | Shared [cleanup](../../../crates/eez-evm/src/balances.rs), [group state](../../../crates/eez-protocol/src/system_tx.rs), block preview/commit, Composer snapshots | Only SYSTEM/EEZL2; update cursor together with state. Fresh malformed/interleaved/incomplete, discarded-preview, failed-inbound/load and paid-group tests. Synthetic callbacks never independently trigger cleanup. Historical Composer regression covers incremental fork/unsigned/prefix rebuild parity. |
| Ordinary donations / empty block / finalization | Block `finish`, cleanup, Composer finalized copies | Donation visible to later ordinary calls; final residual sink does not add gas/log/refund. Fresh donation/empty-block/account-preservation regressions and new late donation in the paid-group test. Production FeeVault balance is never cleared. |
| Canonical prefix hash versus continuing execution | Composer prefix snapshots; [Stateless adapter](../../../crates/eez-evm/src/stateless.rs); [signer execution](../../../crates/eez-proof-signer/src/validate/stateless.rs) | Close a disposable copy only. Actual terminal bundle must independently reproduce header state root; projection cannot repair it. Three fresh adapter units check cleanup equivalence, untouched inventory and uncleaned-terminal rejection. #112's 19 independent prefix oracles, scheduled observations and donation process are historical, not replaced by the new balance test. |
| Partial settlement, replacement, restart and reproof | [partial-consumption processes](../../../crates/eez-node-e2e/tests/partial_consumption.rs), [deriver](../../../crates/eez-deriver/src), signer effect binding and group reconstruction | Replay same-height retained bodies from H−1; whole groups, exact bytes/roots and full-block ordinals precede consumed-suffix selection. Source hash does not authorize placement/consumption. Historical #112 partial/resume/restart evidence; new tests do not exercise the distributed replay path or durable queue recovery. |
| L1 escrow funding and transaction fees/refunds | [L1 target session](../../../crates/eez-composer/src/local/slot.rs), shared traversal `l2_to_l1_executed_value`, EEZ `_applyRollupUpdates`, [fee process tests](../../../crates/eez-node-e2e/tests/fee_quotes.rs) | Gross executed outflow is a conservative funding bound, surviving net flow establishes liability changes. L1 refunds cannot fund L2 admission. #112 follow-up and [fee audit](../2026-10-10-cross-chain-fee-research/README.md) are historical; no real builder ordered-refund exercise was run here. |
| Proxy prefunding/recovery / direct or forced ETH | [CrossChainProxy](../../../eez-core-protocol/src/base/CrossChainProxy.sol), Core caveats | Recovery is an internal transfer to the configured recipient; rejection may leave proxy funds. Include both accounts in an oracle. Neither account's balance alone proves duplication/loss. No fresh production-profile recovery/failure accounting process is claimed. |

The [group/history audit](../2026-10-27-group-cleanup-and-history/README.md),
[block-end audit](../2026-10-27-block-end-cleanup/README.md) and #112 are starting
evidence. In particular, #112 records 45 EVM, 419 Composer, 329 signer tests,
seven source process scenarios, independent prefix reconstruction and the
terminal-root negative check at its recorded revisions. Those numbers and its
excluded/ignored checks are historical. They are not added to this task's pass
counts or presented as fresh all-path coverage.

## Selected qualification

The missing intersection selected for execution was **paid included user
outcomes across native funding and group/block sinks**, and **a supported
terminal failed nested row carrying ETH**. Existing fee tests and zero-price
group tests covered the components separately; the broader failed-descendant
Core test did not qualify the fork's supported failed-row case.

- `paid_groups_reconcile_value_fees_and_both_cleanup_boundaries`: a production
  executor fixture uses nonzero base fee 10 and gas price 13. An ordinary SYSTEM
  donation precedes a native load of 37; the load pays 7 to another account and
  retains 30 until its signed user. User success transfers 11; revert/halt
  transfers none. Native gas earns no fee. Group cleanup removes 30+9; a later
  donation of 5 remains until block close. User debits, target/payout balances,
  collector/beneficiary credits and total `opening+37−39−5` reconcile exactly.
  The manager is a tiny bytecode stub, so this is production executor placement
  evidence, not real manager ABI/funding or cryptographic authorization evidence.
- `testFuzz_TerminalNestedRevertRestoresValueWithoutAnotherMint`: real pinned
  EEZL2/proxies execute one funded outer payment. Its value-bearing nested
  outgoing call matches a failed row with no descendants, returns the exact
  saved revert, and restores the caller's payment. SYSTEM retains only the
  successful outer outgoing value. The closed ledger checks opening + one
  modeled load credit − one modeled cleanup; a following inbound group needs
  no previous inventory. Manual `vm.deal` models native credit/cleanup and does
  not prove production hook placement or independent signer acceptance.

Neither regression exposes a failure of the unmodified production base. There
is no production patch, no before/after bug-fix claim and no new policy decision.

## Ponytail review

What this change does: It records the accounting boundaries and adds two bounded
monetary regressions. Production execution and the pinned protocol stay unchanged.

Looks good. Ship. The single-agent review followed the current
[Ponytail checklist](https://github.com/DietrichGebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md),
checking the changed fixtures, their production callers/consumers, rollback,
receipt-based equations and the distinction between manual funding and runtime
issuance. Expected additional test work is three fixed executor outcomes and
256 fuzz cases; there is no added shipping workload. No confirmed finding remains.

Not checked: a fresh distributed monetary process oracle, full workspace lint
and release/Kurtosis validation; the remaining cases below retain those limits.

## Validation and remaining qualification

Fresh checks passed: 49 EVM tests with Stateless, 323 shared protocol/codec
tests, 154 integration Solidity tests, focused EVM Clippy with warnings denied,
Cargo/Forge formatting and documentation checks. No executed behavioral tests
were ignored or skipped; empty doctest targets are not counted. The initial
unmodified-base contract selection passed 26 tests. The two new regressions pass
against unchanged production code; no pristine-base EVM failure is claimed.

See [validation.json](validation.json) for actual commands, counts, hashes,
environment failures and unrun checks. Raw logs and the test executable are
local ignored artifacts under `artifacts/issue146`; they are not portable CI
evidence. Rust tests use 1.95.0, locked dependencies, one job, debug information
disabled and opt-level 0 for this local resource-constrained run. Foundry is
1.7.1, Solidity 0.8.34, via IR, optimizer runs 1. Shipping profiles are unchanged.

Remaining narrowly scoped cases, in order of useful next evidence:

1. A source-based production Composer/offline signer/cold follower run of the
   value-bearing terminal failed row, with an independently enumerated L1/L2
   ledger and fee destinations. The Solidity test cannot establish the full
   builder/authorizer/replay path. Pair it with inbound source-frame rollback
   carrying value and assert that no delivery/mint occurs.
2. The same ledger across partial same-height consumption, intervening posting,
   retry and cold restart/reproof, measuring each cleanup only once per rebuilt
   canonical body. Existing historical replay correctness checks are useful but
   not a fresh exhaustive monetary oracle.
3. Paid FeeVault withdrawal with recipient return/donation and fee-recipient
   aliasing in production replay; include post-execution fee replenishment.
   Core proxy prefunding/recovery rejection needs its own complete account set.
4. Current exact-artifact CI, including workspace lint, signer/RPC sessions and
   the release/Kurtosis matrix. No performance, deployment or live-refund claim.

These are evidence gaps, not newly asserted defects or reasons to change Core,
encoding, funding policy, cleanup timing or source identities. Any scenario
outside the accepted two-chain/failed-terminal-row profile needs a separate
policy decision before implementation. No live network, wallet, deployment,
service, other task output or operator datadir was modified.
