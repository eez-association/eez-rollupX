# Response to challenges

## A. Verified defects (in the proposal as written)

**F1 — accepted as you state it.** Proposed-path schema gap, not a deployed bypass; my "High" was scoped to package E. Two additions to your repair: the intermediate gate must run on the *final* recipe's block set (terminal promotion can turn a previously-intermediate block into the terminal, and vice versa), so cache the raw evidence, not a pre-computed "passes intermediate gate" bit; and `decoded_transaction_count` must be part of the cached evidence since `verify_preceding_block_transactions` compares it against the DA record.

**F3 — my "came from our builder" argument is withdrawn for adversarial inputs.** If pinned `validate_header` does not constrain `slot_number` / `block_access_list_hash` presence, then Stateless binds a header the follower's assembler can never reproduce (follower sets `slot_number=None`), and a field-only check that omits absence tests would attest a block the follower re-derives to a different hash. That is a real fork risk, not a complexity nit. I do not accept assembler equality as the *permanent* oracle, though:
- Assembler equality is only complete if the signer reconstructs `NextBlockEnvAttributes` byte-for-byte as the follower will, which is the same per-field audit in disguise; it merely derives the field list from shared code rather than from a written table.
- It re-introduces the dependency direction §4 (149-153) worries about, and it silently changes meaning whenever the pinned Reth assembler changes.
- Counter-proposal for the "later optimization": exhaustive destructuring of the pinned `Header` struct (no `..`) in the derived-field check, so a new header field is a compile error rather than an audit item. That gives the absence guarantee you want without a second assembly.
Your staging — assembler equality first, field-only later — is acceptable; I am asking that "later" require the exhaustive-destructure form, not just a conformance test.

**F13 — my `ordinary_block_count = 0` requirement was wrong and is withdrawn.** The recipe is fixed at signing; the applied range is learned from L1. Requiring zero would reject a valid payload whenever a competitor settles the prefix. Retained residue, which you have not addressed: (a) the follower must verify the resumed terminal's `parent_hash` against the *canonical* hash at H−1, since the competitor's prefix may be environment-divergent under v3 even at equal heights — `check_claimed_state` covers the claimed state root, and the Sync parent hash gives this, but the explicit test must cover a competitor whose prefix differs only in environment; (b) `window_pre_block_hash` vs. prefix anchor relationship is still unspecified and is not answered by "keep claimed recipe unchanged".

**Local: height-keyed witness UPSERT.** Agreed as a conditional-loss defect (not exploit): branch-B overwrite followed by pruned restart onto A makes A's witness unrecoverable if Reth has pruned A's parent state, so the fail-closed check turns into a permanent stall, not just a miss. Hash-keyed retention plus a bounded per-height branch count (to prevent a fork storm inflating the store) and the branch-switch test are the right fix; migration must keep the height index for the finalized-floor purge.

## B. Conditional risks

**F2/F7 — accepted with your correction.** `timing.rs:423-425` / `437-439` are test profiles; I should have said "the tested profiles", not deployment budgets. The structural point stands: the release config's `proof_time` is a hard bound on the whole `Prove` RPC, and nothing in §7/§8 authorizes changing it. Your replay-equivalence/cold-timing checkpoint before deciding RPC scope is exactly my step 5; I only add that the checkpoint must be run at the largest legal window on both profiles, because the chiado-class budget is where a 65k reference walk may not fit even with range references. Warm/cold SLOs measured, not derived from admission count — agreed.

**F8 — your correction is right and mine was wrong on the floor.** The last signed root is not settlement; retaining relative to it would break retries after exclusion/reorg. The floor must be the same finalized/applied floor the witness store already uses. On your design:
- "Refuse preparation at capacity rather than evict reserved history" is correct, but note it means the capacity is effectively a bound on the settleable window — which is the same consequence I described as "hard failure of preparation"; it must be documented against `EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS`.
- Lease semantics: reservation keyed by (composer attempt id, range), refcounted so overlapping attempts (retry after reorg) coexist; TTL ≥ `proof_time` + L1 inclusion window, renewed by the composer's periodic status call; explicit release on batch-applied or abandon; final-proof pin lives for the request only. Expiry must not evict — it merely makes entries evictable.
- On "anchors/lifetimes cannot be blindly trusted composer claims": the composer is already the signer's trusted operator of resource use (it supplies witnesses). Misclaimed anchors are a resource/liveness matter, not a safety one, because facts are hash-keyed and every reference is re-checked at final proof. Scope the distrust accordingly rather than building authentication for it.

**F11 — wording corrected.** Finite depth ≥ N+K−1 is permitted; "depth 0" was overstated. The consequence is unchanged: retention must cover the full window, and I did not claim production is misconfigured — only that the ledger should carry the row. Accepted as you frame it.

**F6 — the kind=2 record is a sound narrow extension; critique:**
- It is segment encoding (my option a) with the segment start expressed positionally, so §6 summaries must carry "environment at end of range" state, otherwise composition across a kind=2 boundary is wrong. Say so.
- Specify that the kind=2 block itself is derived under the *new* environment (not the old one with a change-after).
- Terminal promotion onto a kind=2 position: the environment installed by that record must survive for the next batch's leading gap, or the next batch's explicit environment must be the source of truth. Pick the latter (batch-level environment is authoritative; kind=2 is intra-batch only).
- Kind=1 not changing environment is fine, but a Sync boundary that coincides with an operator change forces the kind=2 onto the next ordinary block; document that the change is effective one block late in DA terms.
- The `prev_randao` roadmap is still open: per-L1-block randao would degrade to a kind=2 every ~6 blocks. Decide profile 2 explicitly.
- Admission sizing must reserve for kind=2 records.

**F9 — accepted.** One ordering requirement: check the claimed total logical count against the ceiling *before* walking, since the walk is the allocation.

**F10 — agreed.** A dedicated `ProverError::CacheMiss`, intercepted before retry/source-eviction, is necessary regardless of wire code. `FAILED_PRECONDITION` remains the right code for pre-fix clients (they terminate rather than spin); `UNAVAILABLE` is still wrong for them.

**Local: 65,536 native maximum vs. same-residue K=6 boundaries.** Plausible; I cannot re-verify the exact maximum (65,532) here. Splitting wire-count tests from legal maxima is correct either way.

## C. Design preferences

**F5 — your numbers are correct** (65,535 + 3-byte list header; 8.6% / 25.8% / 51.6% of 6/2/1 blobs of ~126,976 usable bytes). My "≤ half a blob" is your 51.6%. I do not dispute the arithmetic. The rationale I still contest: the cost that matters is marginal blob count, and a 36-hour window with any real traffic is multi-blob regardless, so the flat list rarely buys a blob. Sparse gap records are essentially run-length encoding with relative counts; that is what keeps §6 to "adjust the first gap". Fine — keep sparse, but state the byte table and the run-length invariant in the spec; that is a design choice, not a defect, as you say.

**F4 — agreed.** Facts carry actual header fields; recipe comparison at the final gate; generation replaced by identity + hash + structured miss.

**F12 — writer-only gating is an alternative, not necessary.** I overstated "protects nothing": the timestamp gives a deterministic protocol cut, which is a legitimate coordination choice. Retain it; my objection reduces to "one more permanent chain-config rule".

## D. Unsupported/withdrawn claims of mine

- F3's reliance on production-validated builder equality for adversarial inputs.
- F8's "signed root as anchor" floor.
- F13's zero-count requirement.
- F11's "depth 0 required".
- F2's presentation of `timing.rs` test profiles as deployment budgets.

Everything else in the original stands with the amendments above.
