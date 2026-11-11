# PR #123 review corrections

Status: corrections implemented and locally validated; current-head hosted CI and
full-network qualification remain pending. No merge or deployment performed.

## Source identity

- Reviewed PR head: `8b5cafd3c591af0602d8d835b88e06667a13ada1`.
- Historical baseline: `c702f90da19cc149890d0a17a4ef6c200dcb6a21`.
- Integrated main: `ff7a5cdc81ce02ad30c6c267c4554de77cecb753` (includes #88,
  deposit-cap #125 and retained-history #126).
- Corrective commits: `19a86d8b`, merge `53095ac2`, compatibility/process tests
  `d620a6df`; Claude follow-up corrections `7bb9ee2a`; final quote alignment
  and policy documentation `62d6a1ed`.
- Unchanged protocol: `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Worktree: `artifacts/full-protocol-e2e-20261030`, branch
  `codex/full-protocol-e2e-20261030`. The operator's dirty root checkout was preserved.

## Corrections and boundaries

1. Successful non-applying L1 roots with callbacks reject during materialization
   and lookup classification. Failed-root callbacks remain supported: their L1
   entry enforces rollback. Independent native-semantic binding tests assert the
   precise rejection, and a source process regression submits a caught source
   rollback with an L1 callback. The original security concern was demonstrated
   with component characterizations, not a complete malicious production signature.
2. Settlement, escrow admission and quotes use paired rollback-aware traversal
   rather than summing isolated table rows. The old flat-value API was removed.
   Planning finalizes a copy with a placeholder root; it does not authorize
   signing, which still validates real roots and execution. Main's deposit cap
   uses gross surviving deposits, without netting withdrawals. Inbound paired
   validation runs on every chain; only the per-source-transaction cap is mainnet
   specific. Escrow admission separately requires gross executed withdrawals,
   including reverted calls, and debits only surviving withdrawals. This is a
   conservative funding bound, not a claim of minimal peak liquidity.
3. Reused root programs are rebuilt and compared in full. Identical callbacks do
   not accumulate; changed or newly added callbacks reject. Natural failure and
   enclosing-source-rollback retries have regression controls.
4. Historical static observations retain their per-observation rows. Runtime
   first-match selection remains unchanged; each observed read can cover one
   byte-identical validated row. Extra or differing unobserved rows reject.
   Terminal failed roots similarly preserve baseline serialized slots separately
   from the restored VM cursor. Root scopes restore both allocation and runtime
   cursors. Callback retries always rebuild the runtime-selected slot; terminal
   allocation cannot overwrite a different later same-hash program. There is no
   selectable legacy decoder or execution mode.
5. The signer specification now matches mixed-static failed-root/span validation,
   and AGENTS/version guidance distinguishes RPC validation 8 from signing domain 7.
   The operator upgrade guidance names the corrected acceptance boundary.

The implementation uses the existing materializer, verifier, FIFO schedule and
native binding. The pin, production contracts, genesis and wire schema are unchanged.
The only Solidity edit is an integration-test actor for the rejection regression.

## Baseline and regression evidence

The [original review](../2026-11-09-pr123-review/README.md) retains failing value
and callback-retry reproductions; disposable checkouts/logs remain local-only in
`/tmp/rollupx-pr123-review/`. They were executed against the exact reviewed head
before these changes. Current regressions assert zero surviving failed-ancestor
transfers and one complete callback program per reusable slot.

Frozen fixtures under
[`call_patterns/fixtures`](../../../crates/eez-protocol/tests/call_patterns/fixtures)
were generated from the original base, not from the corrected implementation:

- Repeated terminal static callbacks: base pair verification and settlement
  binding passed; original PR head rejected the frozen L1 batch with
  `mutable table mismatch`; corrected reconstruction accepts it.
- Repeated terminal failed roots: base pair verification and binding passed;
  corrected serialization retains the exact base source table. An extra duplicate
  source slot is rejected without a corresponding invocation.
- Repeated failed nested rows were also probed, but base verification already
  rejected them for incomplete row coverage. That case does not establish a
  historical compatibility requirement. The failed baseline probe is retained.

A temporary [terminal-root matrix](retry_matrix.rs) enumerated 1–4 roots with
two call identities, success/failure outcomes, and an optional leading enclosing rollback scope of 1–4 roots.
The original base verifier accepted 860 cases. Of these, 558 are accepted by the
corrected implementation with exactly identical source and host ABI bytes.
The other 302 require a restored runtime cursor to return a different outcome
for the same first-matching call; all reject with the specific outcome-reuse
error. This matches the pinned protocol's documented mutable retry limitation,
not a new historical execution mode. The corrected verifier accepts 822 total
matrix cases. These are synthetic table checks, not on-chain or full-signature
proofs; baseline verifier acceptance alone does not prove executability.

The matrix was temporarily copied to `crates/eez-protocol/tests/retry_matrix.rs`
and run with:

```bash
MATRIX_OUTPUT=/tmp/retry-matrix.tsv cargo +1.95.0 test --locked \
  -p eez-protocol --test retry_matrix -- --nocapture
```
At the base only, map the newer `revert_spans` vector to the old optional
`revert_span` field. The temporary test target was removed afterwards; this
retained source is diagnostic evidence, not a new CI target.

The regressions are modules of `call_patterns`, already included in CI's verified
release archive and producer suite. No new CI selector or execution mode was added.
No assertion is made that a deployed chain contains these historical shapes.

## Validation

Rust 1.95.0, locked dependencies, `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`;
Foundry 1.7.1 with the pinned compiler settings.

The package run at `7bb9ee2a` passed 1,242
tests: Composer 425, RPC 7, deriver 49, driver 74, signer 338, prover client 34,
protocol 275 and blob/payload codecs 40 (including independent KZG and commitment
vectors). Six ignored package tests are explicit fixture-generation/inventory
utilities (five signer) and a manual component timing profile (one Composer),
not passing coverage. Workspace all-target Clippy and signer rustdoc with warnings
as errors passed. Integration Solidity tests passed 153/153.
Node unit tests passed 98/98;
SDK tests passed 7/7. The final quote alignment passed all 425 Composer tests and
workspace Clippy again. The pinned core suite passed 778 tests with three
fork-dependent tests skipped because their RPC environment was absent. The
mandated `script/*` and `test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental`
exclusions were retained, not counted as coverage. All 64 pinned Anvil scenarios
passed with semantic assertions on isolated ports 28545–28672; these test the
pinned contracts, not the full node/prover/follower network.

Bulk logs and process datadirs are local-only;
small fixture bytes and test source are retained in the repository.

Commands include:

```bash
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec --tests
cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client \
  -p eez-composer -p eez-driver -p eez-deriver --lib --bins
forge fmt --check --root contracts
forge test --root contracts
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/pr123-fixes/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/pr123-fixes/datadirs" \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- cross_chain:: --test-threads=1
```

The [final process selection](process-selection.txt) contains 28 tests selected
from call patterns, overlay rollback, lookup rejection, grouped interstate work
and nested/refund/mixed fee quotes. All **28 passed**, with zero failures or skips,
from code commit `62d6a1ed` after the broad cross-chain pass. These include semantic
assertions and independent follower replay. Both invocations cleared prebuilt-binary
overrides and ran serially. Separate diagnostic directories retain both runs.
The broader pass passed 56 tests with one ignored live-Kurtosis capture case.
Its runtime source was `d620a6df` plus the [Clippy-only cleanup](initial-clippy.patch);
the test harness had been built before that style-only cleanup. It is not presented
as coverage of the subsequent Claude fixes. Executable hashes and local log digests
are retained in [the check summary](check-summary.json).

Final source-process invocation (after the broad suite finished):

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/pr123-fixes-reviewed/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/pr123-fixes-reviewed/datadirs" \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  'chained_interstate::' 'cross_chain::call_patterns::' \
  'cross_chain::inbound_successful_lookup_with_callback_is_rejected' \
  'cross_chain::overlay_' 'cross_chain::outbound_reverting_destination_' \
  'fee_quotes::exact_quote_with_caught_nested_revert_' \
  'fee_quotes::unsigned_nested_quote_' 'fee_quotes::l1_refund_quote_' \
  'fee_quotes::compact_nested_' 'fee_quotes::mixed::both_fee_routes_' \
  'fee_quotes::mixed::gnosis_both_fee_routes_' 'fee_quotes::replay_validation::' \
  --test-threads=1
```

Formatting, whitespace, relative documentation links and the recorded shell
examples were checked. CI's checksum-verified Gitleaks 8.30.1 found no leaks in
the corrective commit range (`8b5cafd3..62d6a1ed -m`) or either added audit
directory, with the repository configuration and no inline-ignore bypass.

Initial development runs exposed stale test expectations after preserving static
rows, a missed repeated-candidate coverage traversal, temporary harness compilation
errors and a Clippy style warning. These were corrected; the first-failure logs
remain under `/tmp/pr123-fix-*`. They are not hidden retries or claimed passing
checks. No new dependency upgrade was introduced to bypass a failure; the merged
main lockfile includes its separately reviewed documentation-parser security update.

`cargo deny check advisories bans sources` passed under the repository policy.
`cargo audit` exited successfully with 20 allowed unmaintained/unsound warnings;
their IDs and affected crates are retained in the check summary. Those packages
are unchanged by this PR correction; the only merged lockfile version change is
main's `pulldown-cmark` 0.13.5 → 0.13.6 security update. No warning suppression or
incidental dependency upgrade was added. These results do not establish a
warning-free dependency set.

Full live Kurtosis qualification, the release artifact matrix and deployment
promotion remain CI/prepared-environment work. No deployment, funding, service
restart or operator-state cleanup was performed. Local process tests use their
own networks and retain diagnostic state.

## Independent review

Claude Code was invoked with `claude-fable-5-1 --effort max` and read-only
Read/Glob/Grep tools against the corrective diff and pinned checkout. The
[original response](claude-review.md) is retained unchanged. Reconciliation:

1. Both mixed retry sequences reproduced failures. Allocation now distinguishes
   runtime reuse from historical terminal slots; the same regressions pass.
2. Paired inbound validation now runs on every chain before accepting work.
   Malformed-composition admission tests cover mainnet, Gnosis and Chiado. The
   original early-return behavior was established by source inspection, not a
   live repeated-slot reproduction.
3. Gross executed withdrawals are retained independently of rollback-restored
   settlement effects. A failed value-bearing root and a failed ancestor with
   a successful withdrawal descendant require funding while settling zero.
   Synthetic simulation still does not itself establish real manager liquidity.
4. The mainnet cap is explicitly per inbound source transaction, as in main #125.
   Extending policy to L2-origin callbacks is outside this correction.
5. Callback-free zero-value lookup outcomes remain within the supported profile.
   The specification now explicitly says they do not prove canonical L2 effects
   or enforce a predicted enclosing L1 rollback. No production exploit was
   established by the reviewer; this remains an application compatibility boundary.
6. Stale static deduplication/mutable-host language and a misleading test name
   were corrected.
7. Selected source encoding is hoisted out of the scan. Static exact-byte
   coverage still scans bounded tables; no hash cache or new algorithm was added.
   This review does not establish an asymptotic performance improvement.

The [targeted maximum-effort follow-up](claude-followup.md) found no concrete bug
in the corrected logic and identified three low-severity residuals. Its quote
funding mismatch was fixed by using the same executed-value bound as admission;
the two `etherDelta` sentences now explicitly exclude reverted descendant values.
The builder's append-order retry limitation is documented as unsupported; no
reordering allocator was added. This preserves a bounded implementation and
explicit rejection rather than claiming every possible prearranged core table
can be synthesized. Claude's review was static; the test evidence above is ours.
