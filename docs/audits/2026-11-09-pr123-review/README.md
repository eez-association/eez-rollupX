# PR #123: independent deep review

| Field | Value |
| --- | --- |
| Date | 2026-11-09 UTC |
| PR | [#123 — Support all pinned protocol E2E scenarios with independent replay](https://github.com/eez-association/eez-rollupX/pull/123) |
| Reviewed head | `8b5cafd3c591af0602d8d835b88e06667a13ada1` |
| Original base / merge base | `c702f90da19cc149890d0a17a4ef6c200dcb6a21` |
| Current integration target | `ff7a5cdc81ce02ad30c6c267c4554de77cecb753` |
| Protocol at head and target | `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Local root | Dirty `feat/l2-fee-recovery` at `fa916b13254a36423cad130080dde367bb18a7d6`; existing changes preserved |
| Method | Primary source review and two focused sub-reviews, plus independent parallel Claude review; targeted executable reproductions and existing package tests |

The subsequent [correction record](../2026-11-09-pr123-fixes/README.md) records
owner-authorized implementation and newer validation. The findings and decision
below describe the original reviewed head and are preserved as dated evidence.

## Decision

**Request changes. Do not merge this head yet.** The reconciled review has a
P1 lookup-callback security concern supported by Rust and pinned-Solidity
characterizations, a reproduced P1 historical replay incompatibility, and two
reproduced P2 construction/accounting defects. The latter two fail closed; that
assessment does not apply to the lookup concern. A complete malicious signed
settlement or live exploit has not been executed. Existing passing scenarios do
not cover these combinations.

This is an engineering review, not a proof of correctness or an external security
audit. No PR source, branch, status, comments, deployment or operator state was
changed. Review-only Rust tests were added to disposable source snapshots and
are retained here as reproducible evidence.

## F0 — P1: successful non-applying lookups retain authenticated L1 effects

**Location:** [`execute_lookup_call`, materialize.rs:1147–1160](https://github.com/eez-association/eez-rollupX/blob/8b5cafd3c591af0602d8d835b88e06667a13ada1/crates/eez-protocol/src/entries/materialize.rs#L1147),
with the widened lookup classifier in `settlement.rs:41–59`.

A successful L1→L2 root inside a source frame predicted to revert becomes a
lookup: no canonical L2 delivery, unchanged root, zero ether delta. The PR now
retains executable L2→L1 callbacks in that successful L1 entry, without forced
rollback markers. The pinned L1 contract executes those callbacks as the L2
proxy and returns successfully. It does not enforce the source caller's later
rollback. Consequently the L1 callback can survive if that caller does not revert
in the actual environment, despite the L2 transition having been discarded.

**Evidence:** the added Rust characterization passes and confirms no target
delivery, lookup classification, `success == true`, one callback and
`revertNextNCalls == 0`. The existing signer test
`source_reverted_lookup_binds_complete_callback_table_before_replay` passed in
the 338-test signer suite: its native semantic binding accepts the shape for
independent observation replay. That test alone is not a complete signing test.

The added Foundry characterization runs the exact pinned `EEZ` with the
repository's **mock proof-system fixture**. It confirms unchanged rollup root
while a successful lookup increments L1 state with `msg.sender` equal to the L2
proxy. The failed-root control rolls the callback back. The final variant uses
the same source wrapper and entry: under `prevrandao == 0` the wrapper reverts
and effects disappear; under a different `prevrandao` the wrapper returns and
the authenticated callback persists. This establishes the contract primitive
and source-context sensitivity, not production proof acceptance or a live theft.

Composer uses predicted environment fields (`posting.rs:168–181`,
`estimate.rs:483–490`), and `EffectInspector::verify` compares calls/outcomes,
not enclosing frame disposition. Actual proposer context can differ. Claude's
specific EIP-4788 exploitation route and an ERC-20 release exploit remain
unexecuted. Source inspection and the two independent component characterizations
are enough to require resolution before merge; they are not a claim that all
end-to-end authorization checks were bypassed.

**Required correction:** establish an enforceable rollback/authority rule for
this profile. Restore rejection of successful non-applying lookups with mutable
callbacks unless safe semantics are implemented and independently validated.
Do not rely on the external caller reproducing simulated rollback. Test the full
Composer/signer/L1 path with changed source execution context and an authenticated
L1 application before enabling it.

Evidence: [Rust shape test](pr123_lookup_review.rs),
[pinned Solidity persistence/control tests](PR123LookupReview.t.sol).
Copy the Solidity test into the initialized pinned core's `test/` directory and
run `forge test --match-path test/PR123LookupReview.t.sol -vv` with Foundry 1.7.1.
The Rust test runs via `cargo +1.95.0 test --locked -p eez-protocol
--test pr123_lookup_review`. Both require isolated review checkouts.

## F1 — P2: committed-value projection counts descendants of reverted frames

**Location:** [`entry_flat_value_sum`, lines 230–238](https://github.com/eez-association/eez-rollupX/blob/8b5cafd3c591af0602d8d835b88e06667a13ada1/crates/eez-protocol/src/entries/mod.rs#L230).

The helper counts each successful reentrant row independently. A successful row
can be inside another row/call that subsequently fails. Its transfers must then
disappear, even though its own `success` flag is true. Natural ancestor failure
does not produce forced rollback markers for every descendant array: the new
materializer relies on the enclosing EVM revert.

Two minimal alternating call forests reproduce the discrepancy. The root starts
on L1 and succeeds after catching its first callback's failure. All transfers are
zero except the final descendant's 7 wei:

| Forest | Committed transfer | Composer helper projection | Independent traversal |
| --- | --- | --- | --- |
| L1 → L2 → L1 (fails) → L2 → L1 | 0 | −7 wei | 0 |
| L1 → L2 → L1 (fails) → L2 → L1 → L2 | 0 | +7 wei | 0 |

The tables materialize successfully. `verify_nested_group` accepts their exact
traces and returns zero `ether_delta`; `entry_flat_value_sum` returns the wrong
directional totals. Both correctness assertions fail at the reviewed head.

**Production consequence:** Composer uses these projections for outbound deltas
at `composer.rs:6629–6648` and inbound deltas at `6713–6717`. The signer independently
compares its contextual result with the claim at `settlement/inbound.rs:573–580`
and `settlement/outbound.rs:657–664`, so the wrong claim is rejected. This caller
trace establishes the expected settlement failure; no full live settlement was
executed for these synthetic fixtures. Current main's new deposit-limit check
also uses the helper (`composer.rs:83–103`), so a sufficiently large rolled-back
descendant deposit can be incorrectly counted toward its 0.1 ETH cap. That cap
interaction is source-derived, not a separate process-test result.

**Baseline comparison:** the original base rejects both forests early with
`unsuccessful cross-chain calls with descendants are not supported`. #123 removes
that guard and claims support, exposing the incomplete accounting rule. This is
a defect in the new supported profile, not evidence that these previously
rejected transactions worked on the base.

**Required correction:** calculate surviving directional value with enclosing
frame/rollback ownership, using a shared protocol-level traversal or the validated
semantic forest. Preserve the independent signer's calculation and mismatch
rejection. Add nonzero-value failed-ancestor cases in both directions, plus a
Composer/signing-boundary regression and a current-main deposit-cap regression.

Evidence: [two failing value-projection tests](pr123_value_review.rs).

## F2 — P2: retrying a failed L2 root appends callbacks to its reusable source slot

**Location:** [root-slot reuse, lines 204–219](https://github.com/eez-association/eez-rollupX/blob/8b5cafd3c591af0602d8d835b88e06667a13ada1/crates/eez-protocol/src/entries/materialize.rs#L204).

A failed L2→L1 root restores the L2 source cursor. An identical retry should use
the same source row and execute its one callback program again. The materializer
finds the previous slot, compares only the cached outcome, reinstalls its context,
and appends the retry's callback and trace into the existing arrays.

Reproduction: L2 calls L1; L1 calls back to L2, then the L1 root fails. L2 catches
that failure and repeats the identical call with the identical callback/result.
The generated source table has one slot with **two** callbacks. The host correctly
contains two separate attempts. Independent verification rejects the first
attempt with:

```text
nested pair did not consume the complete 1 table:
cursor=0, seen_rows=[], call_cursors=[1], call_lengths=[2]
```

A no-retry control passes. A second control keeps the same two-attempt host but
substitutes the exact source program materialized from one invocation; it passes,
with one callback, zero surviving outbound events and zero value transfer. This
falsifies the alternative explanation that an identical retry is inherently
unrepresentable. It does not exercise the core's unsupported case of changing
local dependencies and asking the same mutable row for a different result.

**Production consequence:** the malformed constructed program cannot pass the
independent table gate. It can cause valid newly admitted retry/callback work to
fail composition/replay/settlement; no invalid-attestation acceptance was shown.

**Baseline distinction:** naturally failed calls with descendants were explicitly
unsupported at the base. The related successful-root/enclosing-rollback retry
also fails on the base; that variant is recorded as a pre-existing defect and
is not presented as a regression introduced by #123. Existing PR retry tests
use callback-free roots, so they miss the combined case.

**Required correction:** build/check each invocation against one reusable root
program rather than extending the old program. Compare complete callback, row,
trace and outcome semantics, similarly to nested-row reuse. Keep first-match
cursor rules and reject incompatible retries. Retain both retry variants and
the controls as regressions.

Evidence: [retry reproductions and controls](pr123_entry_review.rs).

## F3 — P1: new materialization rejects baseline-supported historical tables

**Location:** [`materialize.rs:954–959`](https://github.com/eez-association/eez-rollupX/blob/8b5cafd3c591af0602d8d835b88e06667a13ada1/crates/eez-protocol/src/entries/materialize.rs#L954),
with exact binding at `blob_semantics/reconstruction.rs:362–370`.

For an L2 mutable root with two identical terminal static callbacks, the original
base produces two L1 static rows; this head reuses one. Both revisions verify
and bind their own tables. But head reconstruction of the **frozen baseline ABI**
fails with `Settlement("mutable table mismatch")`. This was executed sequentially
against refreshed base and head snapshots, not inferred solely from a diff.

The deriver uses this binding at `deriver.rs:972` and `2334`. Historical batches
containing that previously accepted shape therefore fail reconstruction after
upgrade. No deployed occurrence has been established. The proposal's claim that
previously supported data retains its decoding/execution rules is not satisfied
by the reproduced case; upgrading every binary together does not repair old data.

**Required correction:** preserve validated reconstruction of baseline history
and add frozen historical replay fixtures. Do not remove exact comparison or
blindly accept unused rows. Any intentionally incompatible activation needs an
explicit historical-replay/migration design and corrected operator guidance.

Evidence: [base/head differential test](pr123_historical_review.rs). Run once with
`REVIEW_PHASE=base` at the original base (adapt `revert_spans` to `revert_span`),
then at head; the test writes its frozen ABI under `/tmp/rollupx-pr123-review/`.

## F4 — P2: accepted signer contract and new mixed-static implementation disagree

`crates/eez-proof-signer/SPEC.md:903–907` says a mixed-static multi-root host MUST
NOT contain reverted mutable roots or root revert spans. The new verifier and
acceptance tests permit them. This normative disagreement must be resolved in
the owning specification or implementation; implementation tests alone do not
approve extending the signer's narrower contract. The validation-version guidance
in `AGENTS.md` also needs alignment with RPC 8 / signing domain 7.

## Reviewed boundaries and integration

- Pinned execution/static/rollback requirements and corresponding Solidity:
  local arrays belong to frames; natural reverts restore their descendants;
  static candidates must be distinguishable; mutable cursor reuse does not
  authorize a changed cached answer.
- Materializer forest validation, root/nested cursor restoration, trace
  finalization, reusable static candidate selection and independent inverse
  verification. Contextual value facts restore on failed ancestors; the flat
  projection above does not.
- Lookup-only observation authorization: native semantics derive scratch tables;
  authenticated EEZL2 execution must succeed separately from the actual target
  outcome; missing witness/code and forged outputs reject before signing.
  Scratch writes and rollback journals remain separate from canonical execution.
- Signer service ordering: validated execution → native DA/table/effect binding →
  observation replay → cancellation/deadline checks → signing authority.
  No demonstrated bypass was found in these inspected paths.
- Whole-group source counting still precedes full binding/verification; it is not
  itself authorization. Deriver/follower checks and source identities remain
  mandatory after the new partition selection.
- Current-main #126 changes retained-history startup handling separately from the
  new reconstruction partitioning; no direct conflict was found. This is static
  integration evidence, not a fresh combined process qualification.
- RPC validation version 8 versus unchanged signing domain 7 is an explicit
  compatibility decision. Composer/signer/followers must upgrade together; old
  witnesses/sessions may need rebuilding and downgrade after new shapes enter
  canonical history is unsupported. External Nethermind follower interoperability
  is not established by Rust/Kurtosis tests in this repository.
- The PR still has a `.gitleaks.toml` merge conflict. Current-main identity prose
  also needs its older “version remains 7” statement qualified historically when
  integrating version 8. Neither issue replaces the correctness fixes above.

## Executed validation

Rust 1.95.0; locked dependencies; LLVM prefix `/usr/lib/llvm-22`. The exact-head
existing worktree was clean before testing, including its initialized protocol.
Independent review snapshots came from `git archive`, with the exact protocol
source separately exported. Tests used the existing Cargo target cache; no
cached executable was invoked as a substitute for a Cargo-selected test target.

| Check | Fresh result |
| --- | --- |
| Protocol/codec existing unit and integration tests | 306 passed, including 17 call-pattern tests, KZG compatibility, rolling-hash and public-input vectors |
| Proof-signer binary target | 338 passed, 5 ignored explicit fixture generation/inventory utilities |
| Control RPC | 7 passed |
| Prover client | 34 passed |
| Composer | 421 passed, 1 ignored utility |
| Driver | 74 passed |
| Added head repros | 2 controls passed, 4 assertions failed across the two defects/related variants |
| Adapted base repros | 1 control passed; new-profile shapes reject as unsupported; existing enclosing-rollback variant reproduces its earlier defect |

Existing tests total **1,180 passing**, with six ignored utilities. Added failing
reproductions mean the review test run is **not green**.

Representative commands, from the relevant exact source root:

```bash
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 cargo +1.95.0 test --locked \
  -p eez-protocol -p eez-blob-codec -p eez-payload-codec --tests --no-fail-fast \
  -- --test-threads=1
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 cargo +1.95.0 test --locked -p eez-proof-signer
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 cargo +1.95.0 test --locked \
  -p eez-control-rpc -p eez-prover-client --lib
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 cargo +1.95.0 test --locked \
  -p eez-composer -p eez-driver --lib
```

For the reproductions, copy the adjacent Rust files into `crates/eez-protocol/tests/`
of an isolated exact-head checkout and select `--test pr123_entry_review` and
`--test pr123_value_review`. The base API uses `revert_span: Option<u32>` in place
of `revert_spans: Vec<u32>`; only that representation was adapted for comparison.

First failures are retained. The first RPC/client run failed because sandbox
rules denied local test-server sockets; rerunning with loopback access passed.
The signer has no library test target, so the actual binary target was explicitly
run rather than counting the earlier multi-package `--lib` command as coverage.
The first base reproduction build reused incompatible newer cached library
metadata because archive files retained historical mtimes. Refreshing snapshot
source mtimes forced Cargo to rebuild the base; the corrected baseline results
above come from that rebuild. Head snapshots were likewise refreshed before the
final complete protocol run. These are harness/environment corrections, not
retry-only product passes.

[Check summary](check-summary.json) retains commands' test-result lines and local
log digests. Bulk logs under `/tmp/rollupx-pr123-review/` are local-only and may
expire; the small retained reproductions do not depend on those logs. The retained
Rust files were formatted with Rustfmt 1.95.0; their formatting check and local
audit-link checks pass. GitHub was rechecked before completion: the PR remains
open at the reviewed head and `main` remains at the target recorded above.

Additional reconciliation tests: base historical fixture 1 passed; head replay
regression 1 failed; lookup Rust characterization 1 passed; pinned Solidity
lookup persistence and failed-entry control 2 passed (mock proof system). These
are separate from the 1,180 existing Rust tests. The first lookup Rust harness
had a `u16`/`U256` assertion type error; corrected before execution, with its
compiler log retained.

Not executed: full workspace Clippy/build matrix, full core Solidity suite,
protocol Anvil runner, source-based node process suites, live Kurtosis 64-scenario
qualification, independent cold follower replay, live deployments or performance
comparisons. Existing PR claims for those lanes remain historical evidence, not
fresh evidence from this review. Current-main integration CI and focused process
tests are required after fixes; no merge should follow from the existing green
historical head alone.

## Claude review

Independent parallel review requested with Claude Code 2.1.319,
`--model claude-fable-5-1 --effort max`, limited to Read/Glob/Grep over the same
head/main/protocol snapshots and diffs. Startup confirmed `claude-fable-5-1`.
Anthropic's [model configuration documentation](https://code.claude.com/docs/en/model-config)
identifies Fable 5.1 as its most capable current Claude Code model. The process
completed successfully after 123 turns with no permission denials. Its
[unedited final response](claude-review.md) is retained separately; its claims
are not all endorsed as reproduced findings.

Reconciliation:

- Claude identified root-slot callback duplication independently (its 2a), matching
  F2. Its value finding matches F1, but it disclosed that grep surfaced three lines
  of our new value test; this part was not completely blinded.
- Its highest-severity concern prompted the additional Rust and Solidity
  characterizations in F0. End-to-end signing/exploit acceptance remains open.
- Its historical replay concern prompted the successful base / failing head
  differential in F3. Its normative mismatch was source-confirmed as F4.
- Its opaque-retry and missing-proxy/lookalike concerns are retained as follow-up
  leads, not established additional bugs here. Source confirms the indicated
  retain/requeue branches, but this review did not execute the full bounded retry
  lifecycle or prove indefinite starvation.
- Its complexity concerns lack measured adversarial work bounds in this review;
  no performance/DoS finding is promoted from them alone.
- Its expectation of no textual merge conflicts is superseded by our concrete
  `.gitleaks.toml` conflict check. Its reading of some REVM behavior from memory
  is a stated limitation, not independent pinned-source evidence.

The final merge recommendation remains **request changes**. Fix the confirmed
issues, resolve the lookup authority concern with full-path adversarial tests,
align the normative contract, and qualify the integrated main revision before
reconsidering merge.
