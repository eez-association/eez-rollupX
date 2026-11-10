**Verdict:** I found no reachable soundness bug in the C13 (semantic boundary) or G02 (registry-destination guard) Rust logic. I did find one concrete regression in the fixture change, and the C13 claim holds only for posters that forward calldata verbatim. Nothing was built or run: this is a static review, and I had no shell, so no `git diff`.

## Findings

### 1. High — the `EEZPostBatcher` fixture edit breaks the Composer's pinned wrapper fingerprint (confidence ~0.85)

- **What changed:** `contracts/src/EEZPostBatcher.sol:17-19,29-31,59-79` adds storage, `setMetaCall` and a real hook body, which changes the runtime bytecode.
- **What it collides with:** `crates/eez-composer/src/composer/estimate/deployment.rs:59-65` pins that file's code ("at fork ab6e44c": length 4828, immutables at 157/222/1031/3141). `:163-180` rejects any other wrapper code.
- **Evidence:** the built `contracts/out/EEZPostBatcher.sol/EEZPostBatcher.json` has immutable references at 538/855/1901/4011.
- **Reach:** wallet estimation (`estimate/preparation.rs:267`) and fee quotes (`fees/settlement.rs:395-402`) return "unrecognized estimation deployment" when the wrapper is configured.
- **Expected failure:** the committed test `cross_chain::wallet_gas_estimates_post_batch_wrapper` (`crates/eez-node-e2e/tests/cross_chain.rs:2395`), because `deploy_post_batcher` (`crates/eez-testkit/src/lib.rs:3329-3344`) deploys this same artifact.
- **Do not fix by regenerating the fingerprint:** production would then reject the genuine Chiado wrapper and recognise a contract whose `setMetaCall` is unauthenticated.
- **Suggested fix:** keep the fixture byte-identical to upstream and add a separate test-only poster contract with its own testkit switch.
- **Smaller point:** `requireLastImmediate` is sticky, although the doc comment says "arms one meta-hook call".

### 2. Medium — C13 is closed only for verbatim-forwarding posters (confidence ~0.65, mostly pre-existing)

- A meta-hook retune needs a contract poster (`eez-core-protocol/src/EEZ.sol:444`). The follower still derives from the outer transaction's input, which it cannot check against what EEZ actually received.
- Canonical postings the follower still rejects:
  - any outer ABI other than `postAndVerifyBatch(batch)`, including upstream's own `postBatch(batch,bool)` (`crates/eez-l1/src/scan.rs:536`);
  - non-canonical or trailing calldata (`crates/eez-deriver/src/deriver.rs:2122-2128`);
  - extra or reordered transaction blobs (`crates/eez-l1/src/blob_source.rs:410`).
- Each rejection re-enters the resync loop at `deriver.rs:1465-1489` with the same error, so the follower stops progressing.
- The new count validation (`reconstruction.rs:291-304`) mirrors L1 only when the wrapper forwards verbatim; otherwise it checks unauthenticated fields and adds no safety.
- I would state this boundary in the proposal and signer SPEC rather than imply general retune support.
- This is by reading only; I did not execute the trailing-byte or decode-failure paths.

### 3. Medium-low — process regression gaps (`cross_chain/retuned_dispatch.rs`)

- **Composer after hook consumption is unasserted.** The two `consume=true` cases never resume bundles, so what the Composer does with a request whose effect landed without the user's L1 transaction (retry and deliver twice, or drop) is not checked. Only the unconsumed case asserts recovery (`:204-229`).
- **Narrow shapes.** The boundary is always 1 (no outbound group) and counts are all-or-nothing. There is no lookup-only entry and no mixed hook-plus-queue consumption. `deriver.rs` has no unit test of the split.
- **Small race.** `pause_bundles_after(0)` is armed after the hold is captured (`:120`), so a recomposition can slip through. Arming `pause_bundles_after(1)` together with the hold closes it.
- **Weak assertions and leftovers.** `:113-117` compares values with their own clones, and a debug `eprintln!("C13 …")` remains at `:147`.
- **No recorded evidence.** The audit README and process selection do not list these tests yet.

### 4. Low — signer guard

- **Version rule.** `crates/eez-control-rpc/VALIDATION_IDENTITY.md:150-155` says a settlement-semantics change MUST revise the validation version. The guard narrows acceptance while `VALIDATION_VERSION` stays 8 (`src/identity.rs:16`). Confirm v8 is unreleased, or record the rationale.
- **Test strength.** The pipeline test (`service/tests/derivation/captured.rs:756-790`) mutates only the L1 table, so later gates would reject it anyway. Its specific-variant assertion does pin gate order. No fixture has the guard as the only failing gate.
- **Placement.** The guard lives in `settle_validated` (`service/settlement_job.rs:116-119`), not in `CheckedPublicInputProfile`. That is fine today, since it is the only production path.

### 5. Low — drift

- `crates/eez-l1/examples/verify_blob_evidence.rs:210-225` says it binds "as the deriver does" but uses the strict `bind_settlement`, so it would reject a retuned posting that followers accept.
- `reconstruction.rs:281` says producers and signers "must use" `bind_settlement`, but no production code calls it. Signer strictness lives in `post_batch.rs:293-317` and `semantics.rs:194-215`.
- The follower upgrade-order note exists only in the proposal, not in the runbooks.

## Checked and found sound

- **Count acceptance equals L1's rules.** `immediate ∈ [boundary, len]` and statics non-zero only when the hook fires match `EEZ.sol:443-456,637-651`, given the proven table shape.
- **Boundary derivation.** Each L2-origin transaction materializes exactly one L1 host (`entries/materialize.rs:186`, asserted at `reconstruction.rs:158-169`).
- **Exact binding.** Every entry and static entry is still byte-bound, with no suffix accepted.
- **Canonical partial consumption.** Attribution is dispatch-independent: the window includes the posting transaction's own logs (`scan.rs:591-598,664-675`), so hook consumption reads the same as queued consumption.
- **Whole groups.** Outbound groups stay whole, ordinals are derived before suffix selection, and final root equality is retained.
- **Signer strictness.** The strict count checks listed above are still in place.
- **Guard coverage.** It covers all three L1 call arrays that `_validateCallDestination` sees (`EEZ.sol:1146,1493`) and compares against the registry proxy, which is the correct `address(this)`.

## Limitations

- **Diff inferred.** The diff was inferred from current file contents and the status list, so I could not see removed code.
- **Worktree changed during review.** `retuned_dispatch.rs`, `EEZPostBatcher.sol`, `reconstruction.rs`, `deriver.rs`, `post_batch.rs` and `settlement_job.rs` were all edited while I read them. Findings reflect my last read, and line numbers may drift.
- **Stale artifact.** The built artifact still has the two-argument `setMetaCall`, so it predates the current three-argument source.
- **Not reviewed.** The two `call_patterns.rs` changes and most of `cross_chain.rs`.
- **Hook format not applied.** The session's Rust-skills routing hook asks for a different output format; no Skill tool is available here, so I used the report format you asked for.
