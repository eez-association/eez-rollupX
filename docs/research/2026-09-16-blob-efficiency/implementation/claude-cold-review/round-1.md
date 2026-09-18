# Independent security review: cold derivable-operations signer (tag 3 / profile 1)

Scope: the frozen snapshot only. No code was executed. Comments and SPEC/proposal text were treated as claims to be checked, not as evidence. Follower code, `validate/testing.rs`, `testkit`, `eez-protocol`, and the settlement unit tests are not in the snapshot; anything depending on them is listed as unverified.

## Summary

I did not find a concrete request or interleaving that reaches `Attester::sign` with an incorrect v3 window. The v3 DA gate (`verify_derivable_da_payload`) requires, at every logical position, a `ReconstructedBlock` fact that can only be built from successful Stateless output plus an authenticated parent, and it compares published environment/transactions (or full RLP) against that fact and the validated block. All pre-existing intermediate/terminal/effect/sidecar/semantic/KZG/digest gates run unchanged before the digest is wrapped. The findings below are a test-authority regression, two existing weaknesses, and several unverified assumptions that the soundness argument rests on.

---

## Findings

### F1 — Effect-bearing settlement is no longer tested on the shipping DA entrypoint (regression in test authority)

**Severity:** Medium (coverage, not a demonstrated exploit).

**Where:**
- `settlement/da.rs::verify_da_payload` — `#[cfg(test)] if legacy_da_oracle && encoded_payload.first() != Some(&eez_payload_codec::derivable::TAG) { ... return verify_encoded_da_payload(...) }`
- `service.rs::ServiceState` — `#[cfg(test)] legacy_da_oracle: bool` and `with_legacy_da_oracle`
- `service/tests/derivation.rs` — every positive v3 case uses `anchor_batch()` with `vec![], vec![]` sidecars/groups and empty blocks from `blocks(count)`.

**What is actually tested on the production path:** terminal-only and empty-gap windows (`cold_v3_empty_history_and_terminal_only_reach_the_attester`), one empty full record, rejections of old tags, six environment mutations, count/tx/terminal/sidecar/group/witness mutations, and the stub-without-fact case. All on one fork (the fixture chain, Cancun-active), all with zero effects, zero transactions in any derived position, no `EnvironmentChange` record accepted, and no exercise of the `!has_beacon_root` leniency in `ReconstructedBlock::matches_environment`.

**What is not tested on the production path:** every Sync-terminal case (outbound/inbound/mixed, callback groups, group sizes, `l2Entries` sidecars, `verify_reconstructed_sync_block`, lookup/static observation claims). Those fixtures exist only behind `with_legacy_da_oracle` and therefore exercise `verify_encoded_da_payload`/`verify_self_contained_da_payload`, which are `#[cfg(test)]` and compiled out of the binary. The gate map itself concedes: "new-format effect cases are still outstanding."

**Why this matters for soundness claims:** `verify_derivable_da_payload` is new code that hands `(terminal.number(), terminal.rlp())`, `payload.l2_entries()`, `payload.outbound_group_sizes()`, and the authorized effect sets into `verify_complete_effect_metadata`. By inspection this is the same body the v2 path used, so I judge current risk low — but the "unchanged terminal/sidecar/Sync gates" assertion for the shipping binary currently rests on reading, not on any executed test. A future edit to the v3 branch (e.g., passing the wrong block, reordering the effect-metadata call before the position loop, or dropping it) would not be caught by the effect regression suite.

**Repair / discriminating tests:** (1) Port at least one outbound, one inbound, one mixed/callback, and one lookup-observation fixture to v3 encoding (`Payload::new(..., terminal_rlp, l2_entries, groups)`) and run them through `TestServer` with `signing_calls()` assertions, without `with_legacy_da_oracle`. (2) Add a positive production-path case with a non-empty `Transactions` record and an `EnvironmentChange` record at a gap boundary, plus the mutations "tx list reordered", "tx moved to adjacent gap", "environment change one block early/late". (3) Add a pre-Cancun fixture asserting that a differing published beacon root is accepted only pre-Cancun and rejected at the first Cancun block. (4) Consider deleting the legacy oracle once ported, so the test suite cannot silently regress to non-shipping verifiers.

**Confidence:** High that the gap exists (visible from `cfg(test)` markers and test bodies). Low that it currently hides a bug.

---

### F2 — `AttestablePublicInputsHash` is constructed before observation replay runs (existing weakness)

**Severity:** Low.

**Where:** `service/settlement_job.rs::run_settlement_with_observations` ends with `Ok((AttestablePublicInputsHash(recomputed_public_inputs_hash), observation_claims))`; `validate_and_settle` then calls `state.validator.verify_observations(&validated_window, &observation_claims)` **after** the value exists.

**Why it's a weakness, not a bug today:** `validate_and_settle` returns the hash only after `verify_observations` succeeds, so production control flow is correct. But the documented boundary ("only completion of the full settlement job may construct it"; SPEC §13, proposal §1.1 item 1) is not what the type enforces: the value exists for requests whose lookup/static claims have not yet been replayed. The `#[cfg(test)] run_settlement` helper already returns it without replay. A refactor that moves `verify_observations` or adds an early return would not be caught by the type system.

**Repair:** return `settlement::RecomputedPublicInputsHash` (or a new non-attestable newtype) from `run_settlement_with_observations` and wrap into `AttestablePublicInputsHash` only after `verify_observations` returns `Ok` in `validate_and_settle`. **Discriminating test:** an RPC-level fixture with a lookup-only inbound entry whose `expected_output` is forged must return `FailedPrecondition` with `signing_calls() == 0` — the snapshot has unit tests for `verify_observations` outcome mismatches but no attester-boundary test for that path.

**Confidence:** High.

---

### F3 — Reconstruction failure reasons are discarded at `trace!` (existing/new operational weakness)

**Severity:** Low (diagnostics; fail-closed).

**Where:** `validate/stateless.rs::validate_blocks`: `Err(error) => { trace!(block_number, %error, "validated block is not reproducible by v3"); None }`. The only public effect is `DaPayloadError::MissingReconstruction` → `FailedPrecondition "settlement validation rejected"`.

**Why it matters:** For the new network every block must reconstruct. Any dependency drift (Reth assembler behaviour, `next_evm_env` change, fork allowlist mismatch such as an L2 config with BPO forks producing `SpecId::BPO*` which `next_attributes` rejects) will present as an opaque settlement rejection with the root cause invisible below TRACE. This is an availability/operability risk, and it also makes the mandatory "reconstruction never falsely fails" property hard to observe in production.

**Repair:** log at `warn!` (block number + reason, no witness contents) and/or retain the reason in `BackendBlockOutput` so `MissingReconstruction` can carry it. Behaviour must stay fail-closed.

**Confidence:** High.

---

### F4 — 128× larger default window magnifies single-slot occupancy (existing weakness, scaled)

**Severity:** Low (DoS/availability only).

**Where:** `config.rs` default `max_request_blocks = 65536`; `service/rpc.rs` single `Semaphore::new(1)`; `validate/stateless.rs::check_cancellation` only between blocks; observation replay and settlement gates are non-interruptible.

**Trigger:** any authorized composer (or anyone reaching the endpoint) streams a syntactically valid 65,536-block window with a doomed terminal. The slot is held for up to `request_timeout` (600 s) plus one running block; `wait_until_idle` also blocks shutdown until that block finishes. Bounds are all explicit (bytes/items/blocks/deadline), so this is not unbounded — but the proposal's "largest validated-window evidence is still outstanding" and `cold_span_feasibility` is `#[ignore]`.

**Repair:** none required for soundness. Record the measured per-block cost and confirm the 600 s deadline actually bounds a 65,536-block cold window before enabling the default; consider checking the absolute deadline in `check_cancellation` in addition to the token.

**Confidence:** High that the exposure exists; magnitude unmeasured.

---

## Non-findings worth recording (checked and found sound in the snapshot)

- **Reassembly inputs are independently recomputed.** `reconstruct` passes `replay.execution_output` and `replay.post_state_root` from `StatelessValidationOutput`; Stateless computes `post_state_root` from the trie and rejects on mismatch, and `validate_block_post_execution` checks receipts/gas/requests. No claimed header output is fed into `assemble`. `reconstruction_fact_is_built_only_from_exact_replay_outputs` mutates root/gas/requests and observes rejection.
- **Parent authentication.** `reconstruct` uses `witness.headers.last()`, requires canonical re-encoding, `parent.hash() == block.header.parent_hash`, and `parent.state_root == replay.pre_state_root`; Stateless already required `keccak(bytes) == parent_hash` for that same header. The clone is taken before `std::mem::take(&mut admitted.witness)`.
- **No-state-read adapter.** The pinned `EthBlockAssembler` reads nothing from `state_provider`; `NoStateReads` sets the flag on every trait entry including the infallible `hashed_post_state`, and `ensure_unused` runs before the block is returned.
- **Sparse/gap semantics.** `Blocks::next` and `Payload::new/decode` agree on record positions (`next + gap`, then `+1`); every gap position yields `Derived { environment: active, transactions: &[] }` and the DA gate checks environment and `transactions.len() == block.body.transactions.len()` at each position, so a non-empty block hidden as a gap, a wrong step, or a late/early environment change all reject. `EnvironmentChange` applies to its own block; `FullBlock` leaves the active environment unchanged on both encoder and decoder.
- **Exact terminal/full binding.** Terminal and full records require `rlp == validated.rlp()` byte-for-byte and a reconstruction fact. `block_count == preceding + 1` and `first_block_number + position == number` bind numbering.
- **Fact binding.** `check_backend_window_output` requires `fact.block_hash() == output.computed_hash`; `ReconstructedBlock` has no constructor outside `validate::stateless::reconstruction`.
- **Digest binding.** Public inputs use KZG versioned hashes of the physical blobs that carried the verified `native.operations`; the composer-claimed hash is destructured and discarded in `check_header`.
- **Quotas.** Count is bounded before variable-length decode (`checked_count` before `Environment::decode`); record positions `< count`; tx count `≤ u16::MAX` per record/full block; RLP `Vec<u8>` legacy items cannot expand beyond input length; window byte/item/block quotas are checked before retention.
- **Cancellation/deadline before signing.** `ensure_before_deadline` runs before and after `sign`; `WorkerGuard` drop cancels the token; a detached worker cannot sign because `sign` is only invoked by the RPC future; the permit travels through the closure and is released on completion, abort-before-start, or panic. The existing runtime test asserts `signing_calls() == 0` after a detached timed-out worker completes.

---

## Required validation gaps (must be closed before the cold path is called reviewed)

1. Production-path (no `legacy_da_oracle`) v3 fixtures for outbound, inbound, mixed/callback, static/lookup-observation, and reverted-effect actionable-failure cases, all with `signing_calls()` assertions (F1).
2. Production-path positive cases for non-empty `Transactions` records, `EnvironmentChange` records (including one immediately after a `FullBlock` record and one at the last ordinary position), and their positional mutations.
3. Fork coverage on the real backend: a pre-Cancun chain (beacon-root leniency), first-Cancun/first-Prague/first-Osaka derived blocks, and a rejected Amsterdam/BPO-spec block — currently only the driver-level fixtures cover forks, and only for empty blocks.
4. Independent non-empty block fixtures for the assembler beyond the single legacy-tx Cancun case in `nonempty_derived_block_uses_executed_transactions_receipts_and_gas` (typed txs, blobs → `blob_gas_used`, Prague requests, reverted txs). Signer/follower agreement via shared `next_attributes`/assembler is not evidence against a shared error.
5. An attester-boundary test for a failing `verify_observations` claim (F2).
6. Feasibility/cost measurement for the 65,536 default under the 600 s deadline; the `#[ignore]` test is not evidence (F4).
7. Evidence that `validate/testing.rs`'s stub cannot populate `BackendBlockOutput::reconstruction` (the field is `pub(crate)`; the type's privacy is what prevents this, and that module is not in the snapshot).

---

## Gate-by-gate soundness assessment (cold v3)

| Gate | Status | Basis / caveat |
|---|---|---|
| Header-first stream, bounds, widths, quotas | Sound | `window.rs`; quotas checked before retention; unknown proto fields uncharged (bounded by per-message limit). |
| Single active slot / worker lifetime | Sound | Permit travels through blocking closure; runtime tests cover timeout/abort/panic. |
| Posting chain/blob allowance | Sound (spec-accepted trust) | Composer selects `target_timestamp`; pre-existing. |
| Exact RLP/number/parent/hash, signer recovery | Sound | `decode_match_and_recover_signers` before replay. |
| Stateless replay (consensus, witness, roots) | Sound (pinned dep) | Not re-audited here; `stateless-4fc3806/validation.rs` computes and compares post-root. |
| Reconstruction parent/pre-state/hash binding | Sound | `reconstruct`; parent hash-authenticated by Stateless ancestor check. |
| Whole-block assembly equality, supported shape | Sound by inspection; **coverage-limited** | Fork allowlist, ommers/slot/BAL/withdrawal presence checks; independent fixtures only for empty blocks + one legacy tx. |
| State-root telescope | Sound | Unchanged. |
| Backend-output association incl. fact ↔ hash | Sound | `check_backend_window_output`. |
| Terminal checkpoint plan / prefix hashes | Sound (unchanged) | Not re-derived here. |
| Canonical PostBatch, profile pins, root chain | Sound (unchanged) | |
| Intermediate reserved-type/system-sender/outbound-event | Sound | Runs over all preceding blocks regardless of representation. |
| Terminal framing + inbound/outbound authorization | Unchanged; **not exercised with v3 DA** | Pre-existing complexity in `derive_effect_positions` (legacy fallback) not re-reviewed in depth. |
| Native carrier / KZG / identity `blobIndices` | Sound (unchanged) | |
| v3 position binding (env, txs, full RLP, count, numbering) | Sound | `verify_derivable_da_payload`; every position requires a fact. |
| Group sizes, sidecars, Sync reconstruction | Same body as before; **untested on v3 entrypoint** | F1. |
| Native semantics / FIFO observation claims | Unchanged | |
| Observation replay before return | Sound in control flow; **type boundary weaker than documented** | F2. |
| Digest recomputation and private attestable type | Sound | Only `run_settlement_with_observations` constructs it. |
| Deadline/cancellation before `sign` | Sound | Checked before and after signing; detached workers cannot sign. |

---

## Unverified assumptions the argument depends on

- Reth `evm_env_from_header`/`context_for_block` (used by Stateless) and `next_evm_env`/`context_for_next_block` (used by the assembler and the follower) are equivalent for a header the assembler produced. Only `ethereum-evm-build.rs` is in the snapshot.
- The follower actually derives with `next_attributes(parent, env)` + empty withdrawals/ommers and rejects on any mismatch; follower code is absent.
- `alloy_rlp` derive/decoders reject trailing bytes and non-canonical forms as the proptests assume (`any_accepted_form_is_canonical` is evidence, not proof).
- `public_input_post_batch_for` in the tests carries `callData` into physical blobs so the v3 tests exercise `verify_da_carrier_with_limit`; helper not shown.
- `blocks(count)` hard-codes `timestamp_step_seconds: 1`; if the block-13 fixture step differs, the count=3 positive test would not pass as written. Cannot be checked without the fixture.
- Any deployment L2 chain document does not enable BPO forks; if it does, `next_attributes` rejects every block (fail-closed, but total unavailability, surfaced only at TRACE — F3).

This is a source review of a bounded checkpoint, not a proof; caching/preparation and follower behaviour need their own review.
