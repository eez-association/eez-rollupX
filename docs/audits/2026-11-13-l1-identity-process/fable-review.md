**Static verdict: no blocking defects remain. Two small non-blocking corrections are worth making.** I executed nothing; this is a read-only trace of the frozen tree, the accepted contracts and the retained logs. It is not approval of runtime correctness or of executable provenance.

What this change does: it adds one process-test module (`l1_identity`) plus the small fixture support it needs. The tests launch the real Composer, follower and signer binaries and check where each wrong L1 identity is stopped. Production source is untouched; the CI module lists and an audit record are updated.

## Blocking

None.

## Should fix (non-blocking)

1. **The "at least 2" wait in the paired-genesis test passes on one guard hit** (`crates/eez-node-e2e/tests/l1_identity.rs:525-533`)
   - **What this is:** the test waits for two log lines containing `terminal witness and canonical proof anchor disagree`, then checks no signature, dispatch, eviction or batch.
   - **Problem:** one rejected attempt writes that text twice: a Composer ERROR (`minimal batch unavailable…`) and a sequencer WARN (`retained ordinary settlement deferred`). The final run has exactly one hit (paired-node log lines 117 and 127, 13:53:48.195 and .390), and the test ended on it.
   - **Fix:** use `> 0`, or count only one emitter (for example lines that also contain `minimal batch unavailable`) if two attempts are really wanted. The drift leg already pairs two strings this way.
   - **If we skip it:** the claim still holds, since one hit proves the pre-signing fence. But the number misleads readers, and the negative checks cover one attempt, not two.

2. **The audit does not say the anchor fence is not genesis-specific** (`docs/audits/2026-11-13-l1-identity-process/README.md:62`, `:82-85`; `evidence.json` `limits`)
   - **What this is:** the row "Embedded G, canonical A, signer/deployed key G" records the hash comparison in `append_anchor_bridge` (`crates/eez-composer/src/composer.rs:485-493`).
   - **Problem:** that comparison fires on any hash difference at the anchor height. The embedded chain here is also independently mined, so the run does not isolate "different genesis" from "different blocks". The preliminary review raised this limit (B1); it was not recorded.
   - **Fix:** add one sentence to the README and one `limits` entry, in the same style as the `TargetChanged` qualifier.
   - **If we skip it:** a reader may take the row as proof of a genesis check that production does not have.

## Nice to have

3. **Table label** (`README.md:58`): the "Canonical G" startup row uses a fresh Anvil, not the G derived from A. Line 75 says so; the label could too.

## Preliminary findings: all corrected

| Item | Status at head |
| --- | --- |
| B1 unreachable later-simulation guard | Fixed. Waits on the pre-proof anchor message and asserts zero attestations (`:525-553`). No fee-sample assertion remains. |
| B2 mirror took the old genesis | Fixed. `native.rs:104` passes `None`; `lib.rs:1361` takes the last override; test asserts L2 block 0 equals `dep.profile.genesisHash` (`:713-721`). The other two `spawn_composer` callers resolve to the same genesis as before. |
| B3 wrong dispatch counter | Fixed. Uses posting-lifecycle `dispatched` and `Chain::new(&canonical_url, &dep)` (`:703-712`). |
| 1 `target_changed` attribution | Fixed. Needs `repriced` then `excluded/target_changed` on the same attempt across two targets (`:887-907`). Log shows attempts `:1:0` (target 11) and `:1:1` (target 12). |
| 2 single-instant negatives | Fixed for both drift legs. Canonical leg shows two compose failures 5 s apart (13:51:48, 13:51:53). |
| 3 vacuous ownership check | Fixed. State must be `queued`, `composing`, `in_flight` or `awaiting_evidence`; `terminal` and `unknown` fail. |
| 3b no charged attempts | Fixed. Last hand-off for the hash must show `attempt_counts == "[0]"` (`:973-989`); `TargetChanged` does not charge (`submitter.rs:260-262`). |
| 4 rejected-node datadirs | Fixed. `rejected()` calls `fixture_ready()`; the final run retained no datadirs. |
| 5 endpoints | Fixed. Mirror target is `fault_rpc_url()`; README states both drift legs are endpoint-identity tests. |
| 6 finality acknowledgement | Fixed. The driver writes number and hash; the test compares them with RPC `finalized`. |
| 7 follower wording | Fixed (`README.md:64`, `evidence.json` `limits.followers`). |
| 8 proxy upstream errors | Fixed. Returns a JSON-RPC error (`:79-89`). |
| Follower probe | Verified. The watched line is emitted only inside `poll_cycle` (`l1_watcher.rs:471-478`). Polling starts last in `eez-follower/src/lib.rs:275-278`, after `Deriver::new` (authentication) and boot catch-up, and needs a seeded ring, so RPC binding alone cannot pass. |

## Process-qualification checks

- **Paired G/A:** the message cannot come from missing state, because a missing height returns a different error through `?`. It needs `header.number + 1 == target` (`composer.rs:7396`), so heights were aligned. The hit happened while canonical was held and embedded had reached `held.number`. Signer log has no signing event.
- **Nonzero control:** mirror log shows `repriced → dispatched → included`; its signer has one `window_signed`; both followers reached watcher polling and converged on the safe head.
- **Wrong reference:** Composer log shows agreement, then readiness, then `finalized genesis reference header`; the follower shows the same rejection. Only the hash was changed, so the hash term is isolated.
- **Startup matrix and database case:** every expected error is present in the retained logs for the sampled roles and profiles. The reth stored-genesis error precedes any handshake line.
- **Counts:** 7/0/0/214, 5/0/0/216, 454 (1 ignored)/216/34/21, and 607 with 9 skips all match the logs. First and corrected failures match the README.

## Evidence and scope limits (not defects)

- The registered key does not bind the L2 reference hash; the test itself shows equal genesis hashes for good and bad documents. Only the document digest and canonical L1 authentication catch it, which `VALIDATION_IDENTITY.md:115-118` already states.
- Neither Composer nor signer compares a block-zero reference against the local L1 genesis at startup; the paired case depends on that. Per your brief this is not a defect.
- The nonzero reference is block 1 with zero PREVRANDAO. No test asserts that a `0x46` import ran or that it included the reference.
- Follower G has no positive observable that it loaded G; only the env wiring shows it.
- `AGENTS.md` says validation version 8; the spec and code say 9. This predates the change and the README discloses it.

## Read

- **Contracts:**
  - `l1-origin-and-offline-proving.md`, `L1_HISTORY_SPEC.md`, `VALIDATION_IDENTITY.md` and control-rpc `SPEC.md`, all in full.
  - Signer `SPEC.md` §1–6.1 and §12–14.
  - `request-lifecycle.md` and `data-provenance.md`.
  - Core `CORE_PROTOCOL_SPEC.md` Settlement Model, `postAndVerifyBatch` and I.1.
  - `CAVEATS.md` and `Rollup.sol` in full.
- **Change:** `final.patch` (code and CI parts), `preliminary.md`, `review-SKILL.md`, audit README and `evidence.json`. The on-disk `l1_identity.rs` matches the patch by line count and spot lines.
- **Testkit:** `native.rs`, `l1_reorg.rs`, `artifacts.rs`, `native-l1-consensus.py`, and the relevant parts of `lib.rs`.
- **Production:**
  - `eez-node/src/lib.rs` (launch, capabilities, readiness, Deriver, watcher order) and `l1_embedded.rs`.
  - `eez-follower/src/lib.rs` and `eez-node-common` readiness.
  - `derivation.rs`, `chain_config/offline.rs` and `deriver.rs:120-175`.
  - `composer.rs` (anchor bridge, proof target, compose failure, observer) and `posting.rs` quote/simulate.
  - `submitter.rs` target gates and `posting_fees.rs:397`.
  - `connection.rs`, signer `service.rs:118-167`, `held_pool/status.rs` and `l1_watcher.rs`.

## Not verified

- Any SHA-256 in `evidence.json`; I cannot hash with these tools. The final run rebuilt the runtime crates in-process (`process-final.log:5-84`), so digest equality across runs is taken from the record only.
- That the final run post-dates the last source edit, and that the working tree is clean.
- Signer Finalize internals beyond the startup profile and key check; no negative case here reaches Finalize.
- `MULTI_PROVER_SPEC.md`, `STREAMING_SPEC.md` and Core `EEZ.sol` itself.
- Added CI time (454 s serial in the final run) against partition budgets.
- Clippy, fmt and build logs beyond their recorded exit codes.

Reviewer model: `claude-fable-5-2`, no fallback or sub-agents. The prompt hook's Rust-skills output format was not applied: this is a review, and no Skill tool exists in this session.
