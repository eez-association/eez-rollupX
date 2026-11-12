Review target: PR head `68af64a7fb025aa0ffe8117a3082a5504e703522`.

This is the source-only report returned by Claude Fable 5.2. It precedes the
follow-up corrections and new validation results. See the adjacent README for
finding dispositions and `validation.json` for executable evidence.

# Adversarial review of PR #166 (ETH accounting map and two regressions)

**Model:** Claude Fable 5.2 (`claude-fable-5-2`), single agent, no delegation. Read-only: nothing was executed, so every statement below is from source inspection, not from a test run.

## Verdict

No production accounting defect is established by current evidence, and both new regressions are arithmetically sound. The PR is not ready as written because its evidence map states the accepted failed-row profile incorrectly (finding 1). That is a documentation and evidence-classification defect; no production change is implied.

## Confirmed findings, by impact

### 1. The README treats failed rows with descendants as unsupported; current accepted sources say they are supported

- **Where the PR says it**
  - `docs/audits/2026-11-12-eth-conservation/README.md`, "Callback / nested success and terminal failure" row: "broader than the accepted fork profile … not qualification of that unsupported profile".
  - Same file, "Selected qualification": "the fork's supported failed-row case".
  - Same file, closing paragraph: anything outside the "accepted two-chain/failed-terminal-row profile needs a separate policy decision".
  - `contracts/test/ProtocolGroupFunding.t.sol:185`: the new comment is true in itself (a terminal row is a subset), but it sits next to the pre-existing stale comment at `:160-161`.
- **Accepted rule**
  - `crates/eez-proof-signer/SPEC.md:76-79` lists "failed mutable calls with bound descendants" as supported; `:896-899` and `:1001-1004` define their ETH rule (failed frames discard all nested transfers, including successful descendants).
  - `crates/eez-control-rpc/VALIDATION_IDENTITY.md:20-23`, normative for validation contract 9: "Version 8 extends the accepted cross-chain profile to … failed mutable calls with descendants".
  - `docs/proposals/full-protocol-e2e.md:51-54` (owner-authorized, implemented).
- **Current behaviour agrees**
  - `crates/eez-protocol/src/entries/verify.rs:797-805` checkpoints facts for a failed row, then executes its calls.
  - `crates/eez-protocol/src/entries/mod.rs:691-692`: "failed frames retain their resolved descendants".
  - Tests in the PR's own fresh 323-test run cover it: `call_patterns.rs:401` and `call_patterns/rollback_value.rs:85,90`.
- **Where the stale wording comes from**
  - `crates/eez-proof-signer/docs/settlement-pipeline.md:116-125` still says failed calls with descendants reject, while deferring to `SPEC.md`.
  - `docs/audits/2026-10-27-group-funding/README.md:86` predates the 2026-10-30 extension.
  - AGENTS.md requires reporting such disagreements; the README picks the stale side silently.
- **Effect**
  - A supported, accounting-relevant shape is filed as a future policy decision instead of residual evidence. In that shape, funded callbacks inside a failed frame return to EEZL2 and leave only via cleanup, which is exactly the gross-versus-net case of oracle 5.
  - The stated reason for choosing the terminal-row test is wrong.
- **Smallest fix (docs and comments only)**
  - Correct the three README passages and cite `SPEC.md` §1 and the validation identity.
  - Move "failed row with value-bearing descendants through the real manager and production cleanup" into the residual list.
  - Record the `settlement-pipeline.md` versus `SPEC.md` disagreement.
  - Do not claim `test_CoreRevertedNestedRetriesRestoreFundingBeforeReuse` is profile-conformant: I did not establish whether its two-retry table is.

### 2. The new Solidity table is hand-built; nothing ties it to the fork's producer or verifier

- **Where:** `ProtocolGroupFunding.t.sol:174-207`.
- **What holds:** the pinned `EEZL2` and proxies are really exercised.
  - A row miss would fold `CALL_NOT_FOUND` and fail `_consume`.
  - Value is in the row key.
  - The SYSTEM transfer at `EEZL2.sol:275-278` is unwound by the revert at `:447-451`.
  - Call gas is 0 and `USE_GAS_LEFT` is false, consistent with the fork's zero-gas rule.
- **What does not:** "matching the fork profile" is asserted, not checked.
  - No Rust test materializes or verifies this shape (L2 root → value-bearing L1 callback → failing value-bearing L2→L1).
  - The nearest fresh Rust value evidence is the opposite direction: `materialize.rs:2511-2573`, inbound, `ether_delta == -1`.
- **Map omission:** the README rows for "L2-origin target failure caught by source" and "Callback / nested" do not cite fresh shared-crate value evidence that already exists:
  - `materialize.rs:2511` (failed nested row, value rolls back);
  - `materialize.rs:2698` (failed L2 root, zero `etherDelta`);
  - `rollback_value.rs:95` (failed value root still needs execution funding).
- **Effect:** residual evidence, not a defect. The README already says the test does not prove signer acceptance.
- **Smallest fix:** cite those tests in the two rows, and say the Solidity table's profile conformance is by construction.

### 3. The Solidity "closed ledger" line adds no independent check

- **Where:** `ProtocolGroupFunding.t.sol:201,205`.
- **Problem**
  - Line 205 is the sum of the assertions at `:196` and `:197`.
  - It runs after `_clearProtocolBalances()`, whose `vm.deal` cannot change either operand.
  - `RECIPIENT.balance == 0` at `:201` is vacuous, because nothing in this table targets `RECIPIENT`.
- **What still holds:** the pre-clear assertions (this, `nestedCaller`, manager = 0, SYSTEM = 1 ether) do close the ledger at `opening + value` by EVM conservation.
- **Effect:** the README sentence "closed ledger checks opening + one modeled load credit − one modeled cleanup" overstates what is measured.
- **Smallest fix:** assert the four-account sum before clearing, or reword the README.

### 4. The Rust paid-group test is sound, but one map row over-reads it

- **Where:** `crates/eez-evm/src/block/tests/groups.rs:136-219`.
- **Checked by hand**
  - The stub pays 7 and retains 30.
  - Receipt indices and the `paid_gas` derivation are right.
  - Collector gets 10·g, beneficiary 3·g, user pays 13·g.
  - Closing total is `OPENING − 7 = OPENING + 37 − 39 − 5`.
  - It uses the production `EezBlockExecutor`, `clear_protocol_balances` and `finish`.
- **Over-read:** the user transaction calls `TARGET` directly and never touches EEZL2 or SYSTEM. In the README row "Whole signed user revert/halt after load", "loaded inventory remains after user rollback" is therefore trivially true in this fixture.
- **Where the real interaction is evidenced:** only in the real-manager Solidity tests, which use manual cleanup. No single test combines real consumption, the production cleanup hook and paid fees.
- **Smallest fix:** say so in that row. The "Selected qualification" paragraph already discloses the stub.

## Production defect search

None confirmed.

- **Inspected:** `block.rs`, `evm.rs`, `balances.rs`, `stateless.rs`, the group cursor and `committed_l2_table_value` in `system_tx.rs`, the failed-row path in `verify.rs`, `EEZL2.sol`, `CrossChainProxy.sol`.
- **Consistent with the native profile and Core H.2:**
  - mint and failed-credit removal;
  - fee credit capped at the base fee, with no fee on native transactions;
  - cleanup limited to SYSTEM and EEZL2, placed before commit and at `finish`;
  - the unmodified-terminal root check.
- **Not inspected, so no claim either way:** Composer `build.rs`/`slot.rs`, signer settlement, deriver, L1 `EEZ.sol`.

## Unverified hypotheses

- **Recovery fixture:** `BaseL2.t.sol:33` sets `RECOVERY_ADDRESS == SYSTEM_ADDRESS`. A future real-manager recovery test in this suite would send recovered ETH into a cleanup sink. I did not check the production genesis recovery recipient. This does not affect the new test.
- **Block-end ordering:** the block-end leg of the Rust test uses an ordinary transaction after a completed group. The executor allows it; I did not verify that a canonical Composer or deriver body can take that order.
- **Hashes and provenance:** I could not check any SHA-256, executable identity, log or the base-versus-association-main claim in `validation.json`.

## What I did confirm in the record

- **Test counts match source:**
  - EVM: 46 default and 49 with stateless;
  - Solidity: 154 total, 18 FeeVault, 9 group funding;
  - Protocol: 233 unit, 31 `call_patterns`, 11 causal, 3 and 2 vector tests.
- **Fuzz runs:** 256 is the Foundry default; `contracts/foundry.toml` has no override.
- **Fresh versus historical labels:** every test cited as fresh exists in a crate the PR ran. The Composer, signer and e2e rows are labelled source or historical, correctly.
- **Links and symbols:** the README link targets I checked exist. The symbols named in the map exist, except `build_outbound_group` and `execute_call_inspected`, which I located but did not read.

## Residual process cases

- The committed README and `validation.json` still record workspace Clippy, full signer/RPC/Composer suites, `eez-node-e2e` process scenarios and Core/Anvil as not run. Replace those entries with the actual rerun results, including failures; do not substitute historical passes.
- The second EVM check is logged as `baseline-evm.log` but already includes the new test. This is disclosed; keep it clear that there is no pristine-base EVM run.
- The README's own four remaining cases stand. Add the supported failed-row-with-descendants ledger from finding 1.
