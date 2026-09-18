# Signer authorization gate map

This maps the cold, prepared and mixed implementation to the signer specification.
The companion [focused soundness review](soundness-review.md) supplies the CO-53
trust, commitment and cache-refinement argument and records every finding and
disposition. Maximum native producer-prepared proofs, real posts and clean-follower
replay have passed on both supported profiles. Finishing this implementation does
not perform or authorize a production deployment.

## Trust and commitment argument

Operator-selected execution rules, system/attester identities, L1 posting rules
are trusted configuration. RPC bounds, blocks, witnesses,
calldata, blobs and recipes are untrusted. Stateless authenticates the execution
against the supplied pre-state; it does not prove canonical fork choice. The
settlement root chain binds the window's starting block hash, intermediate effect
commitments and actual validated terminal. KZG versioned hashes commit the exact
native DA bytes in the independently recomputed public-input digest.

For v3, the backend retains a private-constructor reconstruction fact only after
successful Stateless execution, authenticated-parent checks and complete Reth
assembly equality. Assembly receives the original input fields but derives all
output fields from actual replay output and trusted rules. State-provider access
is forbidden and checked even when an assembler might swallow an error. The
consuming validation boundary binds the fact's block hash to the corresponding
accepted block. A syntax decoder or canned backend cannot fabricate this fact.

The DA gate compares every sparse derived position's environment and exact
transactions to these facts and blocks, or compares full RLP for full records.
All positions require reconstruction evidence, including the retained terminal.
The beacon root is compared only where the active fork consumes it. No recipe
input comes from local follower defaults. No state-root value copied from a
submitted header is used as an independently computed assembly output.

Reconstruction facts confer no settlement authority. The unchanged intermediate
gate rejects privileged transactions AND outbound observations. Complete
terminal, observation, effect, semantic, DA and digest gates remain required.
Only the full settlement job constructs `AttestablePublicInputsHash`; the actual
attester still accepts only that private capability. Test-only counters observe
calls immediately at `Attester::sign`, without introducing another signing path.

## Authorization gate inventory

| Required gate | Implementation | Cold v3 treatment / evidence |
|---|---|---|
| Startup identity, key separation and strict chain rules | `config.rs`, `ServiceState::new`, `validate/stateless.rs` | Preserved. Reconstruction is always enabled for the new network; no activation option. Config tests cover default and lower block limits. |
| Header-first stream, contiguous bounds, widths and quotas | `window.rs`, `service/stream.rs` | Preserved. Block default becomes 65,536; bytes/items/messages/deadlines stay bounded and lower overrides remain effective. Native maximum windows pass through bounded preparation, final proof, posting and clean replay. |
| Single active final request and worker lifetime | `service/rpc.rs`, `service/preparation.rs` | Final requests remain single-flight. Preparation is separately single-flight and releases the shared fair worker after every block; final proving holds it through the full job. Cancellation, timeout and detached-worker tests preserve the no-signing boundary. |
| Posting chain ID and target-fork allowance | `ServiceState::posting_blob_limit` | Preserved; v3 does not select its own blob allowance. |
| Exact RLP/number/parent/hash and signature recovery | `decode_match_and_recover_signers` | Preserved before replay and reconstruction. |
| Witness ancestry, consensus, pre-state, transactions and output roots | pinned Stateless called from `validate_blocks` | Preserved, full execution of every block. Reconstruction uses outputs before they are discarded. |
| Parent hash and pre-state for reconstruction | `validate/stateless/reconstruction.rs::reconstruct` | Added; exact parent bytes, matching child parent hash, matching replay pre-state and computed child hash. |
| Full omitted-output equality and follower-supported block shape | `eez-driver::derivation`, `reconstruct` | Added. Full canonical RLP equality and explicit supported forks/fields. No-state-read barrier prevents a fabricated provider from affecting assembly. |
| State-root telescope | `validate_blocks` | Preserved. Reconstruction evidence never replaces this check. |
| Backend-output association, transaction/receipt/sender/event coverage | `check_backend_window_output` | Preserved and extended to reject a fact for another block hash. |
| Locally selected terminal checkpoints and canonical prefix hashes | `CheckpointPlan`, `derive_prefix_block_hashes` | Preserved. Terminal is replayed freshly and its observation witness retained. |
| Canonical PostBatch and public-input profile pins | `decode_canonical_post_batch`, `CheckedPublicInputProfile` | Preserved. No protocol fields or hash formulas change. |
| Complete root-update chain | `verify_root_update_chain` | Preserved. Claimed proof-window anchor is not replaced by a later resumed applied prefix. |
| Intermediate reserved type/system sender/outbound events | `verify_validated_intermediate_blocks` | Preserved for every v3 ordinary position, including full records. Preparation runs this gate before publishing a fact and the final request reruns it over cold and cached facts. |
| Terminal system target/success and candidate framing | `inspect_validated_settling_block`, `bind_effects_to_execution` | Preserved. No omission rules apply to terminal effects. |
| Inbound/outbound recursive effects, values and event coverage | `authorize_inbound_effects`, `authorize_outbound_effects` | Preserved before DA binding. |
| Native carrier, blob order/profile and physical KZG hashes | `verify_da_carrier_with_limit` | Preserved. No calldata carrier or semantic format changes. |
| Single new-network format, original logical range, environments, tx placement and full bytes | `verify_derivable_da_payload` | Added. Missing reconstruction, altered environment/count/transactions/full terminal all reject. RPC tests exercise real execution and zero attester calls. |
| Exact group and effect sidecars plus canonical system tx reconstruction | `verify_complete_effect_metadata` | Retains the previous verifier’s complete checks. Captured mixed inbound/outbound v3 requests reproduce their original signatures; six sidecar/group mutations reach this DA gate and reject with zero signing calls. Historical DA verifiers are test-only. |
| Native semantic materialization and FIFO observation claims | `verify_native_semantics_with_observations` | Preserved, including source-reverted and static cases. |
| Witness-backed observation replay | `Validator::verify_observations` | Runs after semantic binding and before the private attestable capability is constructed. A captured caught-revert request reproduces its original signature; forging both ABI and semantic outcome copies reaches `observation_replay` and rejects with zero signing calls. |
| Independently recomputed digest | `CheckedPublicInputProfile::recompute_public_inputs_hash` | Preserved; same v3 request must later yield identical cold/cached digest. Legacy and v3 digests differ because DA differs. |
| Cancellation/deadline and private attester boundary | `service/settlement_job.rs`, `service/rpc.rs`, `attest.rs` | Preserved. Negative tests count actual signing calls; `AttestablePublicInputsHash` is constructed only after observation replay and final cancellation/deadline checks. Preparation, retention and capabilities cannot construct it. |

## Current component evidence

After alignment with freshly fetched fork main `5469c88`, the complete signer
suite passes **317 tests with zero failures**; two explicit measurement/inventory
utilities are ignored and not counted. Formatting, strict Clippy and
affected-package checks also pass. See the [alignment record](main-alignment.md) and
[focused soundness review](soundness-review.md). The native request fixtures are
self-contained in
[`service/tests/derivation/captured.rs`](../../../../crates/eez-proof-signer/src/service/tests/derivation/captured.rs):

- `native_mixed_effects_and_caught_revert_reproduce_original_attestations`:
  real Stateless execution, cross-chain DA/semantic/observation checks, exact
  original digest/signature, and deterministic retries.
- `native_mixed_sidecars_and_group_boundaries_cannot_be_changed_before_signing`:
  missing/extra/reordered/changed sidecars and missing/changed group sizes;
  exact `da_payload` pipeline failure and no attester calls through RPC.
- `forged_native_lookup_outcome_reaches_observation_replay_and_never_signs`:
  mutually consistent forged outcome claims pass earlier gates but fail actual
  observation execution, before construction of an attestable digest.

The independent synthetic fixtures also cover legacy/2930/1559/4844/7702 transactions,
blob gas, fork transitions and positional/environment tampering. BPO uses the
pinned implementation's Osaka execution spec and is accepted; Amsterdam remains
outside the supported profile. `reverted_ordinary_creation_reconstructs_failed_receipt_and_nonce_change`
passes through real RPC with independently specified CREATE-revert gas, receipt
and sender-nonce expectations, and rejects omission from DA before signing.
`independent_7702_authorization_nonce_and_refund_are_reconstructed` passes
with independently encoded state/receipt leaves for successful empty-code clearing
and ignored wrong-chain authorization. It checks the additional authorization nonce
and capped refund (36,800 gas), versus no authorization nonce/refund (46,000 gas).

Cold, prepared and mixed requests reproduce the same v3 digest and signature;
captured recursive, reverted, static/lookup and sidecar/group mutations reach the
shipping v3 gates and rejected cases record zero attester calls. All 22 existing
cross-chain process scenarios pass with background preparation. Both real native
maximums use one blob, zero cache misses, a fresh terminal and clean independent
follower replay; see the [native checkpoint](native-prepared-checkpoint.md).

The evidence supports the focused change-correctness conclusion; it is not a
mathematical proof or a general security assessment. There is no timestamp
activation switch or old-network upgrade mode.
