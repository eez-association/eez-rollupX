# Independently funded mixed-direction batches

| Field | Value |
| --- | --- |
| Status | Implementation, source process and live-network validation passed; final-head CI pending |
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

## Requirement coverage

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| M1 | Remove the direction-only deferral in `composer.rs`; retain FIFO and final execution. | Same state-chaining script fails on the baseline and passes all nine scenarios on the candidate; mixed Sync 864 independently replayed. |
| M2 | `fees::check_tariff` uses the independently measured outbound cost for mixed posts. | `mixed_tariff_requires_outbound_coverage_without_charging_inbound_cost`, live L2 underpayment rejection and canonical fee receipts. |
| M3 | Candidate-local `StandaloneCost` binds ordered source hashes and the complete `FeeQuote`. | `mixed_tariff_rejects_missing_stale_and_mismatched_reference` covers missing evidence and twelve invalid reference mutations. |
| M4 | `PostingRuntime::finalize` still precedes the fee gate; outbound-only checks actual final receipts. | `outbound_only_tariff_retains_complete_final_receipt_cost`, source process tests and real-PBS fee/state suites. |
| M5 | Existing refund selection, queue ownership and reconciliation remain authoritative. | Live largest-payer refunds and refund-shortfall batches; mixed process cases check full or partial canonical consumption and independent replay. |

## Executed checks

| Check | Evidence / result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | 413 passed; one existing manual timing profile ignored. New tests cover independent L2 coverage, absent/stale/mismatched reference evidence and the outbound-only final-cost floor. |
| `node sdk/l2-fees.test.mjs` | Seven passed; quote/payment interfaces are unchanged. |
| Source-built `eez-node-e2e`, `fee_quotes::mixed::` | Four passed, zero failed/ignored; Ethereum and Gnosis mixed preparation, partial consumption and replay; L2-only full mixed consumption and replay. |
| Existing fee quote process regressions | Three passed, zero failed/ignored: enforced nested underpayment rejection/replay, 5% legacy payment and 1% type-2 payment below the 20% advisory quote. |
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

The next four cases failed the intended-success assertion: the development L1
mempool adapter sorts different senders by priority, so the 1,000 gwei inbound
source executes before its prerequisite post (poster cap: 100 gwei).
A diagnostic reproduction confirmed the inbound receipt at transaction index
zero, status failure and 36,700 gas used, below its 400,000 limit. This
fixture limitation is already documented in `eez-node/src/bundle_rpc.rs` and the
existing L1 refund process tests. The revised first Ethereum case passed. The remainder of that invocation was
interrupted while waiting for Cargo, then all four cases were rerun in one
consistent harness environment to avoid cross-invocation cache rebuilds. No
failed case was classified as a flaky pass. The revised refund-enabled cases verify mixed
native preparation, source-before-post rejection, exact partial consumption and
independent replay. The L2-only case uses the existing explicit ordering aid for
its unpriced inbound source and requires full mixed consumption and replay.
Full mixed success with both fee routes and actual L1 payouts are covered by
the separate real-PBS Kurtosis state-chaining and fee suites. The dev mempool
adapter establishes neither bundle ordering nor refund payment.

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

A fresh candidate follower with an empty database then replayed L1 and beacon
DA without a Composer RPC or copied state. All blocks 1–1,224 matched the
Composer's block hash, state root, transaction root, receipt root, gas used and
ordered transaction hashes. At mixed height 864, all three receipts also matched
status, transaction index, gas, cumulative gas, effective price and logs. The
comparison ended at safe hash
`0x059235c622976073aa464efa07d2c43f4df2c276de35322b2655cd7058270cc1`.
The follower executable SHA-256 is
`0eaa898c649dc3e7b27557ef9ba45460abc09a7b83e800cd3f45b6aa881a8ae7`.
Small sanitized [evidence](evidence.json) records identities, costs, replay and
local log hashes; raw logs and databases remain local-only.

## Final source review and outstanding qualification

A fresh source review traced the retained reference through candidate selection,
FIFO rebuilding, final native execution, fee validation and unsent-attempt
recovery. It checked whole-group/source ordering, quote freshness, checked cost
arithmetic, excluded senders, outbound-only behavior and unchanged refund and
ownership paths. No additional correction was required in the production change.
The process fixture corrections do not change production validation or tariffs.

Final local process runs used the source-built harness
`target/debug/deps/node_e2e-34e064174f25e8ae`, with the three prebuilt runtime
selector variables unset, `RUSTUP_TOOLCHAIN=1.95.0`, task-owned `TMPDIR`, and
retained `EEZ_TEST_LOG_DIR`/`EEZ_TEST_DATADIR_DIR`. Exact selections:

```sh
target/debug/deps/node_e2e-34e064174f25e8ae fee_quotes::mixed:: --test-threads=1
target/debug/deps/node_e2e-34e064174f25e8ae \
  fee_quotes::enforced_nested_quote_rejects_underpayment_and_replays \
  fee_quotes::simplified_five_percent_legacy_payment_settles_below_twenty_percent_quote \
  fee_quotes::simplified_one_percent_eip1559_payment_settles_below_twenty_percent_quote \
  --exact --test-threads=1
```

The first invocation passed four tests (183 filtered); the second passed three
(184 filtered). These are source process checks, not the release-artifact matrix.
The second CI invocation at `58b512b...` also passed Lint before being superseded
by the final test corrections. Require the complete matrix on the final PR head.
No production promotion is recorded here; the candidate enclave remains running.
The temporary cold follower was shut down gracefully after verification, retaining
its database. Larger sustained mixed bursts and release-build latency comparisons
remain unmeasured by this change; no throughput claim is made.
