# Independently funded mixed-direction batches

| Field | Value |
| --- | --- |
| Status | Implementation complete; process, live-network and CI qualification in progress |
| Base | `c1dafa6113425fce74bdba8c32ae8b0d7a7ba5ae`, unmerged PR #101 |
| Candidate | `1eff1be2468de0781e42efb10a986f68c24f7c34`, `codex/mixed-direction-fees`, [PR #102](https://github.com/eez-association/eez-rollupX/pull/102) |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and unchanged |
| Worktree | `artifacts/mixed-direction-fees-20261026`, initially clean; the original checkout and PR #101 worktree were preserved |
| Toolchain | Rust 1.95.0, LLVM 22, Foundry 1.7.1; locked dependencies |
| Contract | [Accepted requirements M1–M5](../../proposals/cross-chain-fee-recovery.md#mixed-direction-batching-authorized-on-2026-10-26), [fee policy](../../configurable-settlement-policy.md) |

## Change and review boundaries

The direction guard caused an eligible inbound source to be deferred whenever
an outbound source was present with either fee route enabled. FIFO prefix
rebuilding could therefore reduce alternating traffic to single-source posts.
The change removes that guard without changing FIFO, whole groups, canonical
outbound-first execution, or capacity/deadline admission.

The final L2 tariff for a mixed post uses the existing native estimate of posting
the accepted outbound prefix separately. It includes retained ordinary history
and physical blobs. Reuse requires identical ordered source hashes and the full
L1 fee quote, including parent, target, prices and caps. The reference changes
only after a prefix passes its funding check. Failed additions retain the last
accepted reference; candidate reconstruction starts with a new reference.

The complete signed mixed bundle still passes `PostingRuntime::finalize` before
the fee gate. Missing or stale reference evidence follows the existing unsent
`Excluded { charge_attempt: false }` path. Outbound-only batches keep their final
receipt-based whole-post floor. Source gas, clipped tips, excluded senders,
network/beneficiary identity, refund selection and canonical reconciliation are
unchanged. An L1 refund cannot finance the L2 check. Gross whole-post/refund
observations are not a net per-direction profit ledger.

This is Composer policy. There are no protocol, genesis, transaction encoding,
signer RPC, dependency, database or configuration migrations. Rollback restores
direction separation; accepted in-memory work must be drained or accounted for.
The [operator procedure](../../runbooks/chiado-deployment.md#fee-configuration)
includes the new mixed-post observations.

## Executed checks

| Check | Evidence / result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | 413 passed; one existing manual timing profile ignored. New tests cover independent L2 coverage, absent/stale/mismatched reference evidence and the outbound-only final-cost floor. |
| `node sdk/l2-fees.test.mjs` | Seven passed; quote/payment interfaces are unchanged. |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | Passed. |
| `bash -n testing/kurtosis/scripts/verify-state-chaining.sh` | Passed; syntax alone does not validate the live procedure. |
| `forge build --root contracts` | Passed with existing Foundry lint warnings. No Solidity changes. |
| Default role features, local development build | Node, follower and signer built with `--locked`, without test/all-feature overrides. |
| Workspace Clippy with warnings denied | Passed in the candidate's CI Lint job. |
| Initial CI | [Run 40299062811](https://github.com/eez-association/eez-rollupX/actions/runs/40299062811): Lint passed; superseded by the process-fixture correction below. Require the complete matrix on the final PR head. |

Initial compilation rejected a non-serializable diagnostic value; the corrected
diagnostic uses the validated target fields. The first unit run then exposed a
test-fixture mistake: the poster and both users shared an address, correctly
excluding all tips. Distinct poster identity fixes the fixture, not the exclusion
rule. Both failures are retained locally.

The first E2E harness build was killed by the host OOM killer while compiling
the node library (12.4 GiB resident anonymous memory). It did not execute tests.
The next build uses one Cargo job and
`--config 'profile.dev.package.eez-node.opt-level=0'` for the harness library.
The out-of-process Composer/follower binaries retain their original default
development build and recorded hashes. This is not a release-performance test.

The first process case reached native validation and signing, then the fixture's
RPC rejected its L1 source: 1,500,000 gas at a 2,000 gwei cap reserves 3 ETH,
above that RPC's 1 ETH transaction-fee limit. It admitted only the post, and the
Composer correctly reconciled partial consumption. The remaining cases were
interrupted after diagnosis. The corrected fixture reserves 400,000 gas
(0.8 ETH maximum), still above its measured execution, with unchanged payment
and assertions. No runtime cap or validation was disabled.

## Live comparison and retained evidence

The owned enclave is `eez-mixed-fees-20261026`, using chain IDs 7331/696990 and
the existing fee-enabled private PBS profile. Explorers/UI are disabled. No
operational Chiado service, key, database or public ingress was changed.

The baseline uses tested source `792198328f0e0efdcfd7c5a42e75ecdb281cebf5`
(tree-identical to the base), node image
`sha256:a4a0e4ec383b1ee9344a08f66ef713e5904fd497f10e0779f585506a0f9e947e`.
The first state-chaining invocation timed out before sending its burst while
the PBS route had no canonical settlement. A second invocation omitted
`KURTOSIS_ARGS_FILE`, so the harness bid one wei and correctly failed L1 tariff
admission. These are unsuccessful preparations, not reproductions of the mixed
direction defect. The corrected run exports both enclave and args file and starts
after confirmed canonical L2 height 198. Its six directional state-ordering cases
and poison-source recovery passed. The mixed-direction case then failed at the
intended assertion: inbound delivery at L2 block 498 and outbound source at 504.

After confirming an empty queue, the owned Composer was stopped, its `/data`
copied to `artifacts/baseline-stopped-data`, and its image updated through
`kurtosis service update --image`. The safe block 504 hash and state root were
identical before/after. Its running executable SHA-256 is
`a0adba4f2b7e4bb967ab033a9564a64488173db942831da722663af56ebcb28d`, matching
the locally built candidate. The development image ID is
`sha256:4dd7340f550efe679f70ed595957549470df71d70efe67c4e1da2a1e34b72426`.
The independently built baseline signer and deployed contracts remain in use.
All nine candidate state-chaining scenarios passed, including the previously
failing mixed case: both directions share Sync height 864. Repeated inbound calls
also settled atomically at height 900. Container update changed the published
RPC ports, which were rediscovered through `ports.sh`.

The mixed post is
`0x7e1cd877cc8288250cdaea155adee3b27842419b10ba1177010c1f89b8ea3650`, L1
block 145. Its canonical receipt independently confirms 247,852 gas and total
posting cost 247,852,002,113,888 wei. The outbound-only reference is 191,839 gas
and 191,839,001,665,784 wei; the additional mixed cost is
56,013,000,448,104 wei. The measured L2 tips exceed the independently required
outbound contribution. This live example is overfunded; the unit regression
also checks the boundary where L2 covers its reference but not the whole post.

Both existing live fee scripts passed. L1 coverage includes legacy/type-2
nested calls and Safe creation, underpayment rejection, largest-payer refunds
and two four-user nested batches. Both batches settle despite refund shortfalls
(279,976,801,978,401 versus 587,739,004,245,245 wei, then
252,736,801,852,861 versus 360,126,002,651,954 wei). L2 coverage rejects an
underpaid source without ownership/execution and settles independently funded
nested sources together with correct fees, target state and canonical disposition.

Local-only logs, runtime hashes, args, image identities and retained test state
are in the isolated worktree's ignored `artifacts/` directory. They are not
available from a fresh clone. No performance improvement is established by
startup, receipts alone or the local development build; compare equivalent
workloads and build profiles before making latency claims.

Remaining gates: corrected source-based mixed and existing fee regressions,
final-head CI and independent follower replay. No production promotion is
recorded here.
