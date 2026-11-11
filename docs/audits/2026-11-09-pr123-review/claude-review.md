<!-- Independent Claude Code review; model claude-fable-5-1, effort max. Unedited final response. Findings below are reviewer claims; see README.md for reconciliation and executable evidence. -->

# Independent review — PR #123 (`8b5cafd3`, base `c702f90d`, target `ff7a5cdc`, protocol `84a5b520`)

Static review only: nothing was compiled or run, so every finding below is unreproduced and needs the proposed test. Paths are under `head/` unless noted.

**Disclosure:** a grep surfaced three lines of `head/crates/eez-protocol/tests/pr123_value_review.rs`, which is not part of `pr.diff`. I did not open it. Finding 3 was derived from the sources, but it overlaps the topic those lines suggest.

## Findings

### 1. High — successful-root lookup entries now execute L2-authenticated callbacks on L1 (introduced)

An L1→L2 call that succeeds inside an L1 frame predicted to revert is settled as a lookup: no L2 delivery, no root change. The PR now lets that lookup's L1 entry carry executable L2→L1 callbacks. Nothing on L1 forces the caller's frame to revert, so if it does not, the callbacks persist with no L2 state transition behind them.

- **What changed:**
  - Base rejected lookups with descendants and multi-action rollback regions in `fifo_observation_claims` (removed at `pr.diff:9201-9224`).
  - Base's `is_lookup_only_inbound_entry` required empty call tables; head allows them (`crates/eez-protocol/src/settlement.rs:41-59`).
- **Table shape:**
  - `l1_applied_root_mask` marks the root non-applied (`settlement.rs:78-115`).
  - `execute_lookup_call` keeps the callbacks in the L1 source entry's `l2ToL1Calls` and discards the L2 delivery (`entries/materialize.rs:1147-1160`).
  - The entry keeps `success == true`.
  - No rollback region is opened for it, so the callbacks get no `revertNextNCalls` marker.
- **Signer accepts it:** the regression at `crates/eez-proof-signer/src/settlement/tests/semantics.rs:425-503` builds exactly this (root with `revert_spans = [2]`, one L2→L1 child) and asserts it is lookup-shaped and passes `verify_native_semantics_with_observations`.
- **On L1:** per the pinned spec, `_executeEntry` runs each callback through `proxy(L2 source).call(...)`, verifies, and returns when `success == true`. Only the caller's frame reverting undoes it.
- **The prediction is evadable:** the Composer simulates L1 with `prev_randao = parent.mix_hash`, `suggested_fee_recipient = parent.beneficiary` and `parent_beacon_block_root = ZERO` (`crates/eez-composer/src/composer/posting.rs:168-181`, `composer/estimate.rs:483-490`). A wrapper can revert only when the EIP-4788 root for `block.timestamp` is missing or zero, which holds in simulation and not on-chain.
- **Preflight does not check it:** `EffectInspector::verify` compares calls and outcomes, not whether a frame reverted (`posting.rs:903-913`).
- **Impact:** an L1 contract that authenticates `proxy(L2 contract)` can be called without the L2 side happening. Example: the L2 bridge burns in scratch and calls L1 `receiveTokens`; L1 releases, L2 is unchanged, and the attacker repeats.
  - Native ETH is protected by L1's per-entry ether check.
  - ERC-20 and message bridges are not.
- **Not affected:** failed-root lookups (`success == false`) revert themselves on L1.
- **Scope:** none of the pinned L1→L2 scenarios I checked needs this shape; the rollback-group scenarios are all L2→L1, where L1 enforces rollback with spans.
- **Existing, smaller variant:** in base a terminal source-reverted lookup could already return phantom success to the L1 caller. The callbacks are what is new.
- **Confidence:** high that the shape is built and signed; medium-high on end-to-end exploitability.

**Proposed tests**
- Unit: `materialize_actions(MAINNET, [R{revert_spans:[2]}, c(L2→L1, parent 0)])`; assert the source entry has `success`, one `l2ToL1Calls` element with `revertNextNCalls == 0`, and is lookup-shaped.
- Foundry on the pinned `EEZ`: post that entry, call the proxy from a wrapper that does not revert, assert the callback's L1 effect persists.
- Process: L1 wrapper that reverts only when the 4788 root is zero or missing.

**Fix direction:** reject successful-root lookups whose source entry has L1-executed calls (the base behaviour), or wrap those callbacks in a forced-rollback span.

### 2. Medium-High — unattributed failures are retried forever, and the PR adds deterministic user-triggered ones

The retry loop is existing; several of the triggers are new. One held transaction can stall cross-chain settlement until an operator intervenes.

- **Loop A (existing):** a non-actionable signer rejection becomes `PreparePostBatchError::Prover`. Survivors are pushed back with no attempt charged and an empty slot block is built (`crates/eez-composer/src/composer.rs:5722-5727`, `7662-7678`).
  - This contradicts `crates/eez-control-rpc/SPEC.md` §6.1, which says the implementation charges once per failed prover episode.
- **Loop B (existing):** an unattributed preflight `PostingError::Effects` gets `AttemptCharge::None`. Only an attributed singleton is charged (`composer.rs:2784-2806`, `crates/eez-composer/src/optimistic.rs:522-529`).

Triggers:

- **2a. Root-slot reuse doubles callbacks.** Partly existing, with a new route.
  - A retried L2-origin root reuses its source slot but appends its callbacks and trace events again (`materialize.rs:186-225`).
  - Nested rows got the fix in this PR (`materialize.rs:1256-1304`); root slots did not.
  - The verifier rejects the doubled entry (`entries/verify.rs:475-489`), as non-actionable `ExecutionTableMismatch` (`crates/eez-proof-signer/src/settlement/outbound.rs:274-300`).
  - The region-revert route exists in base. The new route is a failed root with descendants retried identically.
  - With idempotent callbacks, the Composer's own event check still passes (`composer.rs:3976-4007`).
- **2b. Missing-proxy reads that are not retried.** Introduced.
  - A static read whose callback proxy is missing becomes an app-visible `StaticCallProxyNotDeployed` revert, and its records are truncated (`crates/eez-protocol/src/composition.rs:401-425`).
  - If the app does not retry at the same key, the real table has no candidate. L1 reverts `ExecutionNotFound` instead.
  - Preflight then sees an unexpected call and fails unattributed.
- **2c. Inspector exemption lookalike.** Introduced.
  - The exemption at `posting.rs:974-996` drops a legitimately expected failed static read whose revert data is exactly `StaticCallProxyNotDeployed(X)`, where `X` is loaded with empty code (for example `tx.origin`).
  - Its "expected" check matches the failed read itself, so `verify` fails.
  - The existing test at `posting.rs:1554-1625` covers only the positive, proxy-has-code and trailing-byte cases.

**Proposed tests**
- 2a: materialize `[A{revert_spans:[2]}, cb, A, cb]` and `[A(fail), cb, A(fail), cb]` for an L2 origin; assert one `incomingCalls` element and that `verify_outbound_source_group` accepts.
- 2b: process test with one ordinary cross-chain call plus a caught, un-retried read with a missing callback proxy; assert eviction at compose time.
- 2c: `EffectInspector` unit test with an expected failed read returning the lookalike bytes.
- Loops: scripted prover returning `FAILED_PRECONDITION` without details repeatedly; assert bounded eviction.

### 3. Medium — the Composer's ether-delta projection is not rollback-aware (existing; more shapes reach it)

`entry_flat_value_sum` and the verifier disagree on which value-bearing calls count, and the signer rejects the difference as non-actionable, feeding loop A.

- **Composer:** `delta = entry_flat_value_sum(L2 tables) − entry_flat_value_sum(L1 entry)` (`composer.rs:6629-6651`, `6713-6717`). The function skips only marked spans, failed rows and failed entries (`entries/mod.rs:229-257`).
- **Verifier:** restores value at every rollback boundary (`verify.rs:641-711`, `765-782`, `472-506`), matching L1's ether rule.
- **Signer:** compares the two (`outbound.rs:656-665`, `crates/eez-proof-signer/src/settlement/inbound.rs:572-581`).

Divergent cases:

| Case | Composer | Verifier | Status |
| --- | --- | --- | --- |
| Value-bearing L2→L1 call that fails on L1 | −v | 0 | existing |
| Value-bearing callback inside a successful row whose enclosing frame reverted | +v | 0 | existing |
| Root-level reverted attempt with a value-bearing callback | +v | 0 | existing |
| Value-bearing call under a failed frame with descendants | counted | excluded | new |

- **Coverage:** every new fixture in `crates/eez-protocol/tests/call_patterns.rs` uses zero value. The existing test `naturally_reverted_l2_root_has_no_committed_event_or_value_outflow` asserts only the verifier side.
- **Related, existing:** lookups are forced to `etherDelta == 0`, but L1 checks the ether invariant before a failed entry reverts. A value-bearing lookup therefore reverts `EtherDeltaMismatch`, not the claimed outcome.
  - The fixture in finding 1 (values 7 and 3) is accepted by the signer yet could not execute as claimed.
- **Confidence:** medium. I found no Composer special-casing but did not run it.

**Proposed test:** parameterised property test over value-bearing trees (failed root, failed nested, successful-in-reverted-frame, root-region retry); assert the Composer delta equals `verify_*().ether_delta`.

### 4. Medium — table layout changed for base-accepted shapes; historical replay is unproven (introduced)

Base assigned one row or slot per occurrence. Head reuses them. Followers and the signer require byte-for-byte reconstruction with the current materializer.

- **Repeated identical static reads or callbacks at one host position:** base pushed a row per read and advanced the cursor (`main/.../materialize.rs:1090-1132`, `285-313`). Head reuses one row (`materialize.rs:1075-1123`).
  - Head's own test expects 3 host rows for 6 callbacks (`tests/call_patterns.rs:95-160`); base gives 6.
- **Identical failed mutable retries:** base's cursor always advanced (`main/.../materialize.rs:392-464`, `256-284`; `main/.../verify.rs:517`). Head restores and reuses (`materialize.rs:220-222`, `1306-1314`).
- **Consequence:** a head follower re-deriving a base-era batch with such a shape should fail `bind_settlement` with "mutable table mismatch" (`blob_semantics/reconstruction.rs:343-374`).
- **Evidence gap:** the audit's differential runs compare PR-internal baselines (`77f1113e`, `73458a03`, `1ff8619d`), not `c702f90d`.
- **Doc conflict:** `docs/proposals/full-protocol-e2e.md` says previously supported data retains its decoding rules.
- **Rolling upgrade:** there is no enable switch, and the vkey is unchanged because the signing domain stays 7. The first upgraded Composer and signer pair can settle shapes old followers cannot replay.
- **Confidence:** medium-high on the layout difference; whether any deployed history contains these shapes is unknown.

**Proposed test:** base-versus-head differential over base-accepted action sets, including `[static root, cb, cb]`, `[R, s, s]` and `[F(fail), F(fail)]`, asserting byte-identical tables. Add a frozen base-era fixture replayed through `from_semantics` and `bind_settlement`.

### 5. Medium-Low — specification disagreements

- **Mixed static groups.** Signer SPEC §10 says such a host "MUST NOT carry … reverted mutable roots or root revert spans" (`crates/eez-proof-signer/SPEC.md:903-907`). The `verify.rs:265-267` doc comment says the same.
  - The verifier no longer enforces it; base did (`main/.../verify.rs:353-367`).
  - The test `mixed_outbound_groups_accept_nested_calls_but_reject_failed_and_rollback_sources` asserts acceptance despite its name (`materialize.rs:3043-3094`).
  - The pinned scenarios need acceptance, so the normative text is stale.
- **Version split.** `AGENTS.md` still says one `VALIDATION_VERSION` covers everything and refers to "validation-contract-7 semantics". The PR splits RPC version 8 from signing domain 7 and removes the equality test; `AGENTS.md` is not in the diff.
- **After merging main.** Main adds a note that "`VALIDATION_VERSION` remains 7" to `VALIDATION_IDENTITY.md`; it will read as contradictory.

### 6. Low — unbounded rollback-scope nesting amplifies work on Composer-controlled input

- `decode_semantic_transactions` does not cap nested `Snapshot` scopes (`blob_semantics.rs:209-241`).
- `CallForest::new` validates each span over its siblings (`materialize.rs:595-659`), and `append_flat_call` walks every active region per call (`1343-1353`).
- Work is O(scopes × calls), repeated at least three times per request (decode, `from_semantics`, semantic binding), plus once per lookup root.
- Only a malicious Composer reaches large values; user traces are bounded by frame budgets. The existing regression covers 1,024 scopes only.
- Also: `call_array_cursor_slots` became O(n²) in its fallback path (`system_tx.rs:357-395`), and session checkpoints are now retained and cloned for the whole composition.

**Proposed test:** a few hundred thousand nested scopes over a few thousand calls, with a time budget; add an explicit scope cap.

## Checked and found consistent

- **Verifier model:** cursor and hash rollback ownership matches the pinned contracts for failed frames, forced spans and root scopes, including that L1 keeps hash and cursor across spans.
- **Static resolution:** memoisation, cycle and depth handling have no reachable panic that I found. Release builds use `panic = "unwind"`.
- **Observation overlay:** scope nesting and checkpoint arithmetic are correct under my reading of revm's `BundleState` API (from memory, not checked against the pinned source).
- **Public inputs:** recomputation is unchanged.
- **Source identity:** complete-group ordinal derivation is unchanged; only the partition inputs differ.

## Main #125 and #126

- **#125 deposit cap:**
  - No under-count path within an inbound transaction: every persisting L1→L2 value call is counted.
  - It over-counts value in rolled-back frames, which fails closed.
  - It does not cover L1→L2 value pulled by callbacks inside L2-origin transactions. This is an existing gap in #125, not caused by #123.
  - Main's new tests should hold under head semantics.
- **#126 retained history:**
  - Orthogonal.
  - Finding 4 does bear on boot-time re-derivation: a deterministic reconstruction mismatch is not `SourceIncomplete`.
- **Merge:** I expect no textual conflicts from the hunks, but did not perform the merge.

## Other uncovered cases

- L1-origin multi-root rollback groups mixing failed and successful value-bearing lookups.
- Residual SYSTEM/EEZL2 balance within an observation scope: scratch deliveries call the EVM directly, so I expect the group cleanup is skipped (not confirmed).
- A stack-depth probe at depth 1,024 on the actual follower and Composer threads.

## Limits

- I did not read the Composer's composition-time L1 inspector, the signer's `settlement_job.rs` and `validate/stateless.rs` beyond grep hits, `settlement_manifest.rs`, the blob codec, the CI scripts, or most of the e2e test diffs.
- The pinned Solidity was read through its specifications, not `EEZ.sol` or `EEZL2.sol` line by line.
- The existing audits were not treated as verification.
- Finding none in an area is not evidence of safety, in particular for erased-frame results, which neither the contracts nor the signer bind.
