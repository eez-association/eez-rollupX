# Defensive prover review follow-up

Status: both reported defects corrected, local regression validation and fresh
diff review complete; the new head still requires the full CI matrix. This record distinguishes
the supplied static review from locally reproduced behavior; it is not a new
independent security audit or a claim of complete prover soundness.

| Field | Value |
| --- | --- |
| Supplied review | RollupX `1109dbab7f12fc735a6bf6986e7d7e7f987495d7`; RX-PROVER-01 and RX-PROVER-02 |
| Fix baseline | `79bf3ac91be970b8bdcf64e914011df00fe33e0b` on `codex/l1-history-offline-proving` |
| Initial Git status | Clean in the dedicated worktree before adding these regressions |
| PR base | `a50c326d99f7cd1c21406eedbf370a344015a6b5` |
| Protocol | Clean `84a5b5207880f192a6a5896cbfe0bac5249db47f`; no pin or Solidity change |
| Isolation | Dedicated `artifacts/l1-history-offline-proving` worktree; original checkout and operator networks untouched |
| Toolchain | Rust 1.95.0, locked dependencies; Foundry 1.7.1 for the integration build |

The five production files relevant to these findings (`validate/stateless.rs`,
`settlement/outbound.rs`, `settlement/effect_binding.rs`,
`settlement/semantics.rs`, and protocol `entries/materialize.rs`) are byte-for-byte
unchanged between the supplied review revision and the fix baseline. Baseline
tests below added only regression support before changing those implementations.

## RX-PROVER-01: observation replay position

The defect is confirmed dynamically. A first-position history import was
correctly included in the ordinary-prefix commitment but incorrectly treated
as the replay boundary. The prior backend rejected the correct observation and
accepted the pre-history result. A complete v2 request reached the real attester
and returned a signature over that incorrect result; the regression recovered
the configured public test signer from the signature before failing its
zero-signing-calls assertion.

The reproduction uses a permitted schedule with history imports at Sync. It
does not imply that the default history offset produces this block shape.
The fixture reads both history storage and a probe balance changed by an
ordinary signed transaction. Thus it distinguishes pre-history, post-history /
pre-ordinary, and complete ordinary-prefix states without relying solely on
block hashes or successful receipts.

The fix uses the backend's validated `system_sender_flags`, the same boundary
used to derive `ordinary_prefix_hash`. History imports remain in the ordinary
prefix. Prefix checkpoints and terminal-block handling retain their existing
validation. No caller-supplied replay index is trusted.

The standalone process comparison is also complete. The corrected binary
validated all six blocks in each request, rejected all four earlier-state
observations at `observation_replay`, and signed the two correct digests with
outbound networking denied. Running the exact same regression against the
retained pre-fix binary produced a cryptographically valid proof for
`static-before-history.v2.protobuf`, causing the intended test failure.
This older binary has recorded source provenance at `d46f0a14` plus the unrelated
driver stabilization patch; the signer/service implementations and lockfile are
unchanged between that source baseline and `79bf3ac9`. It supplements the
in-worktree before-fix reproduction, rather than being mislabeled as a build of
the auditor's exact revision.

## RX-PROVER-02: root slots versus event counts

The lower-level outbound verifier defect is confirmed dynamically: one nested
source root produces two events; a following flat root produces one. The old
verifier selected one event for the nested host and returned
`ObservationCountMismatch { expected: 2, actual: 1 }`.

The fix verifies each host's recursive source table first, then consumes that
verification's committed outbound-event count. The final host owns all remaining
events for the user transaction, preserving exact surplus rejection. Consumed
events cannot be reused, and the final global unconsumed-observation check
remains. Missing events, malformed encoding, order/hash mismatches, nonzero
`callGas` and value accounting remain mandatory rejection paths.

The tests also cover a zero-event reverted host followed by a successful host:
the first host must not consume the second host's event. The semantic regression
checks the same nested-plus-flat source group with split hosts (rejected at
`L1EntryCount`) and the canonical single host (accepted).

There was also a specification disagreement. Since commit `4d32bf133`, including
the PR base, the production materializer uses **one L1 host per L2 user
transaction**, with supported callbacks inline. The full signer re-materializes
native semantics and requires the same host layout. Therefore the split-host
case is representable by the lower-level effect verifier but is not a currently
accepted complete proving request. The old signer/RPC prose promising separate
callback hosts was inaccurate. This fix corrects that prose and preserves the
canonical producer/semantic gate; it does not introduce a second host layout or
claim a shipping Composer reproduction of the split form.

## Compatibility and dependencies

The prover RPC remains v2. There is no ABI, native transaction, block hash,
genesis, database, L1/L2 RPC, or protocol-pin change. Fixing replay to enforce the
existing pinned-state invariant does not define a new normative validation
contract; the version and portable identity vectors remain unchanged. Likewise,
the host clarification describes the already enforced native semantic layout.
Unpatched signers are not safe at the observation boundary. Validation identity
is not attestation of the running implementation; deploy the corrected signer
artifact and discard its prior in-memory sessions when restarting it.

`reth-evm` becomes a direct **dev-dependency** of the signer to build real EVM
witness fixtures. It was already pinned transitively. The complete lockfile diff
adds only that package edge: no dependency version, source, feature request, or
checksum changes. The local dependency-review guard changes to
`834a018a7e81af4b1cd56320cdd8e16c71ead92815c2692a9fbac1a8a76689d7`;
this digest does not enter portable wire identity.

## Validation evidence

Commands run from the dedicated worktree. Build/test commands use the existing
task-local resource guard with one Cargo job and LLVM 22; it retains the command,
exit status, log and memory observations. Logs under `artifacts/` are local-only
and are not available in another checkout. The committed deterministic
[history fixtures](../../../crates/eez-proof-signer/tests/fixtures/native-history-observations/README.md)
and test sources are the reproducible inputs.

[Evidence metadata](evidence.json) retains commands, first failures, log hashes,
executable identities and small sanitized outcome records. The pre-fix unit-test
executable was overwritten by Cargo; its first failure logs and source identities
remain. The pre-fix standalone executable is retained locally with its previously
recorded SHA-256. Both process binaries and the identical process-test executable
are identified in the evidence record.

| Check | Result so far |
| --- | --- |
| Before: `cargo +1.95.0 test --locked -p eez-proof-signer scheduled_history_observations -- --nocapture` | Both selected tests failed: correct result rejected, pre-history result accepted |
| Before: `cargo +1.95.0 test --locked -p eez-proof-signer scheduled_history_earlier_state_observations_never_sign -- --nocapture` | Selected test failed: cryptographically verified incorrect-result proof; signing calls 1 instead of 0 |
| Before: `cargo +1.95.0 test --locked -p eez-proof-signer split_nested_hosts_consume_events_by_verified_call_count -- --nocapture` | Selected test failed with expected 2 / actual 1 event mismatch |
| After: `cargo +1.95.0 test --locked -p eez-proof-signer scheduled_history -- --nocapture` | 4 passed; static and mutable lookup, both earlier states, optional native suffix, full signing gate |
| After: `cargo +1.95.0 test --locked -p eez-proof-signer settlement::tests::outbound -- --nocapture` | 28 passed, including nested-plus-flat positive and ten invalid-event mutations |
| `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -- --test-threads=2` | 328 signer + 7 RPC + 34 client tests passed; 3 ignored inventory/generation utilities, not skipped behavior tests |
| After: `cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- cross_chain::history_observations:: --test-threads=1 --nocapture` | 1 process test passed: 2 correct signatures, 4 observation-replay rejections, 36 block ACKs; no outbound network access |
| Before: same process test with the retained pre-fix signer | Selected test failed as expected after verifying an incorrect-result signature |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed after fixing two style findings in the new tests; first lint failure retained |
| `cargo +1.95.0 check --locked -p eez-proof-signer --all-targets` | Passed |
| `cargo +1.95.0 clippy --locked -p eez-proof-signer --all-targets --all-features -- -D warnings` | Passed |
| `RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked -p eez-proof-signer --no-deps` | Passed |
| `cargo deny check advisories bans sources`; `cargo audit` | Passed with existing approved exceptions; audit reports 9 allowed warnings, no new ignores |
| `cargo +1.95.0 fmt --all --check`; `git diff --check`; conformance manifest check | Passed; all 29 frozen vectors intact |
| `forge build --root contracts` | Passed using existing outputs; existing lint/remapping warnings remain. No Solidity changes or new contract-test coverage claimed |

Initial test-harness failures (incorrect `--lib` selection, fixture compile errors
and a rejected zero genesis base fee) are retained separately. They are not
defect reproductions or product regressions. The final fixture uses the required
genesis base fee and actual per-block fee calculation.

## Coverage and fresh review

| Requirement | Implementation | Regression |
| --- | --- | --- |
| Hash and observation use the same transaction prefix | [Stateless replay](../../../crates/eez-proof-signer/src/validate/stateless.rs) consumes the already checked sender flags | `scheduled_history_observations_include_history_and_ordinary_state`; `scheduled_history_observations_reject_both_earlier_states` |
| Wrong earlier-state outcomes cannot authorize a signature | Full streaming, native DA, semantic and observation gates remain required | [Service tests](../../../crates/eez-proof-signer/src/service/tests/derivation/history_observations.rs) plus [standalone process regression](../../../crates/eez-node-e2e/tests/cross_chain/history_observations.rs) |
| Event counts follow verified call trees | [Outbound authorization](../../../crates/eez-proof-signer/src/settlement/outbound.rs) consumes independently verified counts | `split_nested_hosts_consume_events_by_verified_call_count`; `split_nested_hosts_reject_invalid_event_partitions`; reverted-root boundary |
| Lower-level authorization does not widen the complete native profile | Unchanged native semantic reconstruction and canonical host layout; corrected signer/RPC prose | `nested_outbound_semantics_require_the_canonical_single_host` |

The fresh review traced both fixes through their production callers and the final
attester. Sender-flag length and event ordering are bound to exact decoded blocks
before use; no request-provided replay position is accepted. Event-consumption
flags are local to one authorization attempt, and every failure discards the
attempt. Missing/extra events, canonical decoding, reserved sources, value
accounting, DA, context and public-input checks remain in force. No new shared
state, concurrency mechanism, fallback or alternate encoding was introduced.
This is a fresh self-review, not a second independent audit.

## Limits and remaining validation

- The full CI/release/Kurtosis matrix must run on the new commit. Previous evidence
  for `b1881cf7` does not validate these new signer changes. The full workspace
  runtime suite, core Solidity/Anvil suites, load and live-chain campaigns were
  not rerun locally for this signer-only production change.
- The history-plus-ordinary-plus-native-suffix case has real backend execution
  coverage. The full request/process fixture deliberately uses a complete
  ordinary terminal block; it does not establish a separate full settlement
  proof for the synthetic empty-load suffix.
- No Composer-generated process test of split nested hosts is claimed: that
  layout is rejected by the full native semantic contract. Canonical coalescing
  is retained and tested, rather than adding a competing producer layout.
- The supplied review's remaining soundness assessments are static conclusions.
  These regressions close the two reported mechanisms; they do not establish
  absence of other prover defects or replace independent review of the new head.
- The previously approved `crypto-bigint` advisory exception is unchanged and
  remains an unresolved dependency risk.
