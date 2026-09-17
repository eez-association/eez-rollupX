# Review: `docs/proposals/derivable-chain-operations.md` + coverage ledger

## 0. Root-cause check (the proposal's diagnosis is right, but its framing hides the cheapest fix)

- The ~30-minute forcing comes from tag `0x02`: every empty block is carried as a full header inside a `Vec<u8>` (RLP list of `u8` items), which `block_size.rs:84-106` bounds at roughly 2× the header RLP. That is ~0.6–0.9 KB per empty block; six blobs (~762 KB usable) fill after ~850–1,250 blocks ≈ 28–42 min at 2 s. `DueReasons::FULLNESS` (`settlement_policy/mod.rs:231-242`) then fires.
- The proposal never states that tags `0x00/0x01` **already** omit headers: an empty block costs one byte (`u16` 0 → `0x80`, `da.rs:593-607`), and the follower already derives such blocks from defaults (`deriver.rs:1171-1245`, `template == None`). Tag `0x02` exists only because those defaults are not bound by DA (`§3.1` lines 108-113). So v3 is really "tag `0x01` + explicit environment + exception map + signer derivability check". The sparse-gap encoding (`§3` lines 74-85) saves at most ~64 KiB per 65,535-block window over a flat count list — ≤ half a blob — while creating the entire `§6` "composable range summary / re-anchor adjusts the first gap" algebra (lines 343-351). See F5.

## 1. Findings

### F1 — Cached facts omit outbound-event observations → intermediate-block gate bypass
- **Severity: High. Blocks package E.**
- Spec: `§5.2` lines 276-280 lists "ordinary system-sender classification" but not observed outbound events. Code: `blocks.rs:338-360` rejects any intermediate block whose `settlement_evidence().observed_outbound_events()` is nonempty; that evidence is produced only during replay (`stateless.rs:351`, `824-849`) and carried in `SettlementBlockEvidence` (`validate.rs:158-164`).
- Scenario: block B in the middle of a window contains a user tx that calls EEZL2 and emits `CrossChainCallExecuted`. Cold path: rejected (`IntermediateOutboundEvent`). Cached path per the spec as written: the synthesized `ValidatedBlock` has empty observations → the signer attests a batch whose L2-origin outbound effect is silently unsettled.
- Repair: the cache entry must contain the complete `SettlementBlockEvidence` (system-sender flags **and** outbound observations, plus `decoded_transaction_count`), and the final pipeline must run `verify_validated_intermediate_blocks` over references exactly as over cold blocks. Add a ledger row (CO-16/17 do not name this).

### F2 — Final-proof latency is bounded by `proof_time`, which the plan neither changes nor models
- **Severity: High. Blocks rollout (not initial implementation).**
- `timing.rs:193-215` requires `proof_time + slack < L1 block time` and `≤ (K−1)·L2`; the tested profiles are 4,000 ms (mainnet, `timing.rs:423-425`) and **200 ms** (chiado, `timing.rs:437-439`). The whole `Prove` RPC — stream ingestion, reference resolution, v3 DA verification over 65k blocks (`da.rs` decodes every preceding block RLP twice: `verify_preceding_block_transactions` and `verify_validated_intermediate_blocks`), terminal replay, KZG, observation replay — must fit that budget. The proposal adds one gRPC message per referenced block (`§5.1` lines 232-234; client builds the whole `Vec<ProveChunk>` in memory, `prover-client/src/lib.rs:68-117`) and allows one in-flight preparation block to finish before the final proof is admitted (`§5.1` lines 254-256) — a 30M-gas block can take hundreds of ms alone.
- `§11.3` (lines 499-503) defers to measurement but says "no fixed hardware-independent seconds are invented"; the seconds already exist in config. Nothing in `§7`/`§8` authorizes raising `EEZ_PROOF_TIME_MS` or explains the consequence (larger `future_count`, fewer Live blocks; `timing.rs:254-268`).
- Repair: (a) range references (F9); (b) state a per-request cost model (µs per referenced block, per cold block) that composer prefix selection uses, alongside the block cap; (c) explicitly allow raising `EEZ_PROOF_TIME_MS`/`EEZ_PREPARATION_TIME_MS` per deployment and add the slot-layout change to the operator guide; (d) make "final proof for the largest window fits the *release* profile's `proof_time` on the named hardware" a hard gate with the chiado profile called out.

### F3 — Signer-side full-block reassembly via `EthBlockAssembler` is redundant
- **Severity: Medium (unnecessary complexity in B/D; also a dependency-cycle risk the proposal itself worries about, `§4` lines 149-153).**
- `§4` lines 154-160 and `§4.1` steps 3-4 (lines 172-178) require the signer to rebuild the block from execution outputs and compare canonical RLP. But the signer already holds the exact validated RLP, and Stateless has already bound every execution-output header field and every parent-derived field (state/receipts/tx roots, bloom, gas used, base fee, blob gas, requests hash) to it — the code relies on precisely this in `derive_prefix_block_hashes` ("every header field not affected by truncating the transaction list is inherited from the exact block accepted by Stateless", `stateless.rs:616-620`). The blocks were produced by the same Reth builder the follower will use (`payload.rs:156-158`, `deriver.rs:1247-1282`), so any builder/consensus divergence would already have failed production validation.
- The derivability check therefore reduces to a field comparison per derived block: `beneficiary`, `mix_hash`, `gas_limit`, `extra_data`, `parent_beacon_block_root` (presence per fork), `timestamp == parent.timestamp + step`, `number == parent+1`, empty ommers, withdrawals `None`/`Some(empty)` per fork, transaction bytes equal the record, no system sender (already checked). Parent timestamp for the first window block comes from the witness's parent header, which Stateless authenticates (see `authenticated_ancestor_hashes`, `stateless.rs:487`, `503-553`); `BackendBlockOutput` needs one new field (`parent_timestamp`).
- Repair: replace steps 3-4 with the explicit field list; keep "follower-built block == validated RLP" as a **test-time** conformance oracle per supported fork (this also satisfies `§3.1` lines 133-135). Evidence gap: the exact set of header checks performed by the pinned `stateless_reth` is not in the snapshot; the field list should be confirmed against it once.

### F4 — `PrepareOrdinary`'s "expected common environment" and per-environment eligibility marking are unnecessary
- **Severity: Medium (complexity).** `§5.1` lines 242-248, `§5.2` lines 279-280.
- The environment that will actually be published is decided at final composition (re-anchoring, terminal promotion, fallback). If cached facts hold the header fields (they must, for F3), the recipe check is a cheap comparison at the final DA gate, and no preparation-time environment or "ineligible under environment X" state is needed. Likewise the "cache generation" token adds nothing over hash-keyed facts + a structured miss (a miss is a miss; the composer re-prepares).
- Repair: `PrepareOrdinary` takes rollup id, validation identity, ascending `BlockWitness` values only; returns verified hashes. Drop generation.

### F5 — Sparse gap records vs. flat count list: decide with numbers before package A
- **Severity: Medium (design choice affecting A, D, G complexity).**
- Cost of a flat per-block count list (as in tag `0x01`): ≤ 1 byte per empty block, ≤ 64 KiB per 65,535-block window. Cost of sparse records: ~0, but `§6` needs first/last record positions, gap-prefix lengths whose RLP width depends on the anchor, re-anchor gap rewrites, and terminal-promotion removal (lines 343-351); the signer cannot reuse `DaPayloadCursor`/`verify_preceding_block_transactions` (`da.rs:83-131`, `375-409`), and the follower cannot reuse its `block_tx_counts` loop (`deriver.rs:2626-2670`).
- Recommendation: `0x03 = [profile, environment, block_tx_counts (legacy u16 list), transactions (Bytes list), exceptions [(ordinary_position, full_block_rlp Bytes)], terminal_legacy, l2_entries_legacy, outbound_group_sizes]`. Prefix sums stay one number per block as in `index.rs:120-146`. If half a blob per 36 h is judged material, keep sparse records but say so with the byte table.

### F6 — Environment change mid-window degrades every later block to a full record; no split rule
- **Severity: Medium. Blocks G design.**
- `§3.1` lines 137-139: a block whose environment differs becomes a full record and "the following block derives from that full block's actual header" — but the *common* environment is still the single published one, so every subsequent block also differs → ~0.6-0.9 KB each until the next terminal. `§6` only says configuration changes invalidate summaries (line 351). Additionally, `sequencer.rs:431-433` records the roadmap to derive `prev_randao` from L1 per block; profile 1 fixes it as a constant (`§3` line 68), which would collapse to all-exceptions.
- Scenario: operator changes fee recipient at block X with 20,000 unsettled blocks behind it; the composer's next candidate needs ~15 MB of exceptions; feasible-prefix selection must fall back to a boundary before X, which `§6` does not require it to find.
- Repair: either (a) encode the environment as segments `[(start_ordinary_position, env), …]`, or (b) specify that an environment change is a hard legal split point and the composer settles the prefix under the old environment first. Record the randao decision explicitly (profile 2 vs. per-record override).

### F7 — Volatile cache + restart has no recovery bound; cross-chain readiness depends on it
- **Severity: Medium. Rollout.**
- `§5.2` lines 295-298: restart invalidates everything; `§5.3` lines 320-323: cross-chain work for a slot requires the ordinary prefix prepared or cold-validatable within budget. After a signer restart with 40k unsettled blocks, ~625 preparation chunks of 64 must complete before any cross-chain slot is eligible; `§11.3` only "reports cold recovery separately". The witness store is deliberately durable for exactly this reason (`witness_source.rs:11-17`); the fact cache is small (RLP + evidence, ~1 KB/empty block) and could be too.
- Repair: persist validated facts (mdbx, keyed by block hash + validation identity, purged with the finalized floor) or bound `EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS` by measured re-preparation throughput × acceptable cross-chain deferral. Add a gate: "cross-chain settlement resumes within T after signer restart at the maximum tested backlog".

### F8 — LRU is the wrong eviction policy; capacity pressure produces miss/retry loops
- **Severity: Medium. Blocks E design.** `§5.2` lines 288-293.
- The blocks a final proof needs first are the *oldest* (anchor+1 …) — exactly LRU's victims once new blocks are prepared continuously. Pins apply only after admission; between the composer's "all prepared" decision and admission, concurrent preparation can evict → `cache-miss` → re-prepare → repeat at capacity.
- Repair: retain by height relative to the last signed/settled anchor (never evict ≥ anchor while under the hard cap; evict below anchor first, then non-canonical fork alternatives); treat the entry/byte caps as hard failure of *preparation*, not eviction of the pending window.

### F9 — Per-block `CachedOrdinaryBlock` chunks are 65k gRPC messages per proof
- **Severity: Medium (feasibility; feeds F2).** `§5.1` lines 232-234; `prover-client/src/lib.rs:68-117`, `window.rs:378-398` (per-chunk `push`).
- Repair: a `CachedOrdinaryRange { from_number, to_number, from_hash, to_hash }` chunk; the signer walks its cached parent links and verifies the chain, `from_hash`'s parent == previous chunk / anchor, count against the total ceiling. One chunk per contiguous prepared run.

### F10 — `UNAVAILABLE` for cache-miss collides with the client's retry allowlist
- **Severity: Low-Medium.** `§5.1` lines 258-265 vs. `prover-client/src/lib.rs:201-207` (`Code::Unavailable → Retryable`). The text says "do not repeatedly send the same unresolved reference through `prove_with_retry`" while choosing the one code that is retried verbatim.
- Repair: `FAILED_PRECONDITION` with a structured detail, reusing the existing `ProveFailure`-style actionable path (`settlement_job.rs:306-314`, `prover-client/src/lib.rs:211-216`).

### F11 — Speculative depth, Reth history retention and reorg exposure are consequences, not documentation items
- **Severity: Low-Medium. Rollout.**
- `settlement_policy/config.rs:92-111` requires `EEZ_MAX_SPECULATIVE_DEPTH ≥ N+K−1` or 0; a 36 h span means depth 0 (unbounded run-ahead). `sequencer.rs:52-56` and `witness_source.rs:11-14` tie that bound to Reth state retention; on-demand witness rebuild (`witness_source.rs:260-265`) and deriver replays need parent state as far back as the anchor. `§7` lines 401-404 says "document configurations" only. Also unstated: 36 h of L2 history becomes reorg-exposed to a competing composer's batch (`deriver.rs:1653-1654`) — a deliberate trade-off the user should see written down.
- Repair: a ledger row: required Reth pruning/history configuration ≥ max window; depth-0 consequence; safe-head lag.

### F12 — Reader-side activation-timestamp rejection is redundant
- **Severity: Low (simplification).** `§8` lines 414-418. The signer is the only thing that can turn a v3 payload into a settled batch; once readers are upgraded, rejecting "v3 before activation" protects nothing and adds a permanent chain-config rule. Keep: writer flag + signer compatibility check; readers accept v3 unconditionally once shipped. (Keep the timestamp only if there are third-party followers that need a hard cut.)

### F13 — Resumed/partial batch shape under v3 is unspecified (evidence insufficient)
- **Severity: Low-Medium until answered.** `batch_l2_range` yields `(H, H)` for a resumed batch (`deriver.rs:2936-2942`); reconcile uses `block_templates.last()` (`deriver.rs:2616`). `§4.2` lines 200-202 says "keep", `§3` lines 103-105 defers anchor/range to existing logic. v3 must state: `ordinary_block_count = 0` for a resumed batch, the terminal is the rewritten same-height Sync block, and how the signer's `window_pre_block_hash` (`validate.rs:695-700`, `state_chain.rs:112-117`) relates to the prefix-block anchor. The composer's resumed-window construction lives in `crate::local`, absent from the snapshot; I cannot confirm today's behavior.

### F14 — Admission conservatism vs. the §11 "fits six blobs" gate
- **Severity: Low.** `§6` lines 354-358 reserve worst-case exception records per admitted transaction (header-sized), while `§11.2`/CO-30 evaluate actual encodings. Under `sequencing.rs:37-67`/`213-218` the admission path will split earlier than the encoding suggests. State which sizing the sparse-transfer gate is measured under.

### F15 — Capabilities: identity digest undefined; per-connection recheck costs an RTT per proof
- **Severity: Low.** `§5.1` lines 221-226, 269-270. `RemoteProver` connects per call (`prover-client/src/lib.rs:127-134`), so "recheck at connection" = every request. Define the digest (e.g., hash of the chain-document bytes + validator crate version) and cache with a TTL / return it in `PrepareOrdinary` responses.

### F16 — Follower replay of a 65k-block batch is long, lock-holding and non-transactional
- **Severity: Low. Rollout.** `deriver.rs:1621` holds `begin_reconcile` for the whole batch; `deriver.rs:1981-1986` documents partial-failure half-state. `§11.6` only documents catch-up cost. Add a gate: follower replay throughput for the largest window and idempotent resume after a crash mid-batch (with CO-10's stronger match so re-derivation skips correctly).

## 2. Already covered (do not re-litigate)
Terminal/entries/groups byte identity (`§2`, `§3`); count ceiling before allocation (`§3` lines 90-93); explicit environment not operator defaults (`§3.1`); follower independence from signer cache (`§4.2` lines 204-207); `PrepareOrdinary` never signs (`§5.1`); hash+identity keying, never by height (`§5.2`); durable witnesses to the finalized floor and hash-checked hits (`§5.3`; matches `witness_source.rs:221-265`); L1-applied endpoint is a prefix, never the calldata claim (`§4.2` lines 200-202; matches `check_claimed_state`, `deriver.rs:2817-2853`); reconciliation must match env+ancestry (CO-10; fixes `local_block_matches`, `deriver.rs:2953-2981`); old signer only with v3 disabled (`§5.1` 270-272); rollback keeps v3 readers (`§8`).

## 3. Evidence insufficient in the snapshot
- Exact header/consensus checks performed by the pinned `stateless_reth` (needed to finalize F3's field list).
- `crate::local::build_sync_block` environment (whether empty boundary Sync blocks share the sequencer's environment — if not, every K-th block is an exception and F6 becomes High).
- Composer's resumed-batch window construction (F13).
- Per-block signer cost (cold and referenced) and empty-block witness size; both determine whether F2/F7 are Medium or High.

## 4. Ledger gaps
Add rows for: cached evidence completeness incl. outbound observations (F1); environment-change split/segments (F6); proof-time/profile changes and the reference cost model (F2); restart/re-preparation SLO (F7); anchor-based retention (F8); Reth retention + speculative depth + reorg exposure (F11); resumed-batch v3 shape (F13); follower replay/resume gate (F16). CO-14/CO-19/CO-21 currently encode the RPC/cache design as requirements; after F4/F9/F10 they should reference the simplified surface.

## 5. Minimal justified implementation sequence
1. **Decide shape first** (F3, F4, F5, F6): flat counts + environment (segmented or with a split rule) + exception map + legacy terminal; field-level signer check; range references; no prepare-time environment, no generation.
2. **A′ codec**: tag `0x03`, bounded decode, exact size function mirroring `self_contained_payload_len` (`eez-payload-codec/src/lib.rs:207-235`), golden vectors, legacy fixtures unchanged.
3. **D′ signer (reader)**: v3 branch in `verify_encoded_da_payload` (`da.rs:206-215`), derived-field check with `parent_timestamp` added to `BackendBlockOutput`, terminal path unchanged, default `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS=65536` (`config.rs:112-118`), tamper matrix. No RPC changes.
4. **C follower (reader)**: `execute_block_with_template` → explicit-environment path (it already takes `NextBlockEnvAttributes`, `deriver.rs:1218-1245`), v3 plan through `on_batch_posted`/reconcile/catch-up/correlation, CO-10 matching. Ship readers (3+4) before any writer.
5. **G writer behind a flag**: sizer/index on flat prefix sums, terminal promotion, feasible-prefix selection using the signer block cap. At this point the window is bounded by cold `proof_time`, not DA — measure µs/empty block here; it decides how much of step 6 is needed.
6. **E/F prepare-ahead, minimal**: `PrepareOrdinary` stream + durable (or anchor-retained) fact store + range references + `FAILED_PRECONDITION` miss detail; composer prepares from the witness store as blocks commit. Gate on F2/F7 numbers for both profiles.
7. **H/I**: writer activation flag, capability/identity check, evidence artifacts; readers accept v3 unconditionally.
