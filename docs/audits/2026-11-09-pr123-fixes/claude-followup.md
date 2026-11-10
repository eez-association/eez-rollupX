# Claude targeted follow-up

Read-only review with `claude-fable-5-1 --effort max`. Original response below;
see README for final disposition. No tests were executed by Claude.

**The fixes to findings 1, 2, 3 and 6 hold up; I found no concrete bug in the fixed logic itself, and three low-severity residuals around it.** Nothing was run: every result below is a hand trace, including the two new regression sequences.

## Remaining findings

**1. Low: the fee quote still gates on surviving withdrawals, while admission now gates on gross**
- **Where:** `crates/eez-composer/src/composer/fees/quote.rs:1039-1055` against `crates/eez-composer/src/composer.rs:3612-3627, 3663-3678`.
- **Mechanism:** the quote sums `l2_to_l1_value` and compares it with escrow; admission compares `l2_to_l1_executed_value`. That field has no other consumer.
- **Trace:** a 5 ETH root that fails, then a 5 ETH root that succeeds, with 7 ETH escrow. The quote sees 5 and issues; admission sees 10 and defers on `SourceCondition::Escrow` every slot.
- **Impact:** fail-safe, but the quote promises a transaction the composer will hold.
- **Fix:** use the executed value in the quote comparison.

**2. Low: append-order slot allocation still rejects one valid retry shape after a reverted scope** (not introduced by the fix; the earlier single-cursor version rejects it too)
- **Where:** `crates/eez-protocol/src/entries/materialize.rs:194-214, 240-242`.
- **Trace:** roots `[B ok in a reverted scope, A terminal fail, B ok, A ok]`:
  - B takes slot 0 and the scope restores the cursor to 0; the failed A is appended at slot 1.
  - The committed B moves the cursor to 1, so the final A selects the failed slot 1.
  - Materialization returns "L2 source cursor is reused with a different outcome".
- **Why it is valid:** table order `[A fail, B, A ok]` serves all four roots, and by my trace `verify_outbound_tables` accepts it. The builder cannot produce it because it only appends.
- **Impact:** fails closed; the transaction is evicted. The same happens if the last A fails with different revert data.
- **Decision for you:** document it as unsupported, or place failed slots ahead of later-consumable ones.

**3. Low (doc): the SPEC still defines `etherDelta` over all calls**
- **Where:** `crates/eez-proof-signer/SPEC.md:878-880, 982-984`.
- **Mechanism:** the verifier and signer use surviving values (`verify.rs:1200-1204`; the test at `materialize.rs:2641-2646` expects zero for a failed 7-wei root). This is not one of the three statements I flagged before.

## Validated, no finding

- **Fix 1, slot allocation:**
  - Successful and callback-bearing roots always rebuild the runtime-selected slot, so the verifier marks them.
  - A terminal failure only reuses a byte-identical copy at or after `next_source_slot`, which never falls below the runtime cursor.
  - Every pushed terminal slot has its own invocation, so duplicate coverage always completes.
  - Nested root scopes restore both cursors in stack order. I traced a dozen sequences, including terminal failures before, inside and after nested scopes; none diverge.
  - Both regression sequences pass through materializer, finalizer and verifier.
- **Slot count after a retry:** once a success or callback failure reuses an earlier slot, a later identical terminal failure reuses an existing copy instead of adding one. So "one slot per failed attempt" is not literal outside the frozen `[fail, fail]` shape; these tables still verify.
- **Fix 2:** admission projects every applying root on every chain and mirrors the prepare-time pairing; only the cap is chain-gated. The one prepare-only value error left is the `int192` range check (`composer.rs:6650-6654`), which needs more than 2^191 wei crossing.
- **Fix 3:** executed value is outside the checkpoint, so it survives natural failures, root spans, nested failed frames and nested spans, and counts each retry. The requirement is never below the debit, and the debit at accept uses surviving value. Over-reservation on retries is the intended conservative bound.
- **Fix 6:** the three statements and the test name now match the code.

## Limits

- I could not inspect the baseline materializer; byte compatibility rests on the single frozen fixture plus my model of a single-cursor baseline.
- I did not re-review the physical-balance check in `local/slot.rs` or inbound nested withdrawals.
