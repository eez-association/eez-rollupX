# Sparse proof timing and admission width

| Field | Value |
| --- | --- |
| Date and status | 2026-10-25 UTC; controlled reproduction, correction and final-artifact process validation pass |
| Scope | The delayed mixed-wave packing assertion, proof timing prediction, and unchanged admission/deadline boundaries |
| Source | `1109dbab7f12fc735a6bf6986e7d7e7f987495d7` plus the focused patch identified in [admission.json](admission.json) |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Executables | Before/after Composer unit-test hashes retained in [admission.json](admission.json); no new process runtime executed locally |
| Environment | Rust 1.95.0, locked dependencies, one Cargo job, local memory guard |
| Related records | [First failure and exact-runtime diagnostics](../2026-10-24-extended-validation/dynamic-admission.md); [accepted timing policy](../../configurable-settlement-policy.md) |

## Controlled counterexample

The original CI failure's warmup and large-candidate timing summaries were not
retained. They remain unavailable. The following inputs are synthetic, explicit
reproduction inputs; they are **not** recovered measurements from that failure.

The former `strict-fifo-7` predictor retained a sparse proof's extrapolation term
unless a wider observation took at least as long. Consequently, a slightly
slower one-group warmup could remain a per-group cost even after faster proofs
had validated eleven groups. A request for eleven groups used the covering
sample directly; twelve groups returned to the configured 500 ms seed. Repeating
the eleven-group proof did not change this comparison until the sparse sample
left the 32-observation history.

The controlled admission test uses these inputs:

| Input | Value |
| --- | --- |
| Usable opportunity | 977 ms |
| Prior completed preparation / finishing | 258 / 242 ms for eleven groups |
| Physical blob preparation | 237 ms for one blob |
| Finishing variation allowance | 97.7 ms, the existing 10% rule |
| Sparse proof | One group, one blob, ten witnesses, 40 ms |
| Wider proof | Eleven groups, one blob, five witnesses, 31 ms |
| Requested witnesses | Five |
| Observed finalization | Eleven groups, one blob, five witnesses, 152 ms |
| Controlled execution | One ms per complete group |

At the twelfth group's start, 966 ms remain. The old estimate reserves 1 ms
for the next group, 339.7 ms for finishing/KZG, 500 ms for proof and 150 ms for
post-proof work: **990.7 ms** total. It refuses that group. The first eleven
groups instead use a 38.75 ms covering proof estimate and fit, including their
215 ms observed post-proof reserve. This is a timing-derived width boundary,
not a configured transaction-count or protocol limit.

The mechanism is consistent with the preserved eleven-group CI observations.
It does not prove that these exact inputs caused that historical failure.
Existing complete diagnostic logs independently show sparse-sample scaling and
FIFO reconstruction costs; their later passing results do not erase the first
failure.

## Correction and limits

For extrapolation, a wider observation supersedes a sparse scaling term when it
supplies at least as much of every requested dimension, strictly more of one.
Elapsed ordering is removed from this comparison. All observations still
contribute to the slowest-duration floor, including the superseded sparse one.
Thus the twelve-group prediction above is 50 ms: the old 40 ms observation plus
25% headroom, greater than the wider sample's scaled estimate.

The independent blob and witness dimensions, request-relative clipping, bounded
history, seed cap, headroom, zero/overflow behavior and covering-sample path are
retained. A wider group observation with insufficient history or blob coverage
cannot supersede that dimension of an earlier sample. No execution result,
signature, canonical completion or queue eviction is authorized by this estimate.

Group/blob/witness counts do not characterize arbitrary calldata, gas, proof
shape, transport stalls or machine load. This correction does not establish a
worst-case duration bound. It trades a repeated fixed-overhead extrapolation for
the measured wider workload, while retaining the slow-duration floor. Proof
execution and all rebuilds remain under the original deadline; final native
validation, FIFO repair, source ownership and retry accounting remain required.
On a heavily contended machine, the unchanged process test can still miss its
packing threshold for other reasons.

The change is labeled `strict-fifo-8` in existing admission events. The
[compatibility procedure](../../configurable-settlement-policy.md#fifo-policy-compatibility)
requires only a Composer upgrade. Signer/follower wire data, historical replay,
protocol, genesis, dependencies, event schema and storage remain unchanged.
Rollback restores version 7's extrapolation; timing samples and queue restart
limits remain in memory. Existing collectors accept the version string.

## Executed validation

All commands ran from the isolated worktree identified in the JSON. Cargo used
Rust 1.95.0, locked offline dependencies and the existing one-job memory guard.
Neither invocation was cancelled by that guard.

| Check | Result |
| --- | --- |
| Before: `cargo +1.95.0 test --locked --offline -p eez-composer --lib faster_wider -- --nocapture` | **Two failures**, zero passes, 409 outside the filter. The predictor returns 500 ms instead of 50 ms and the full admission gate refuses group twelve. 393.10 s including the dependency-feature rebuild; tests took under 0.01 s. |
| After: `cargo +1.95.0 test --locked --offline -p eez-composer --lib` | **410 passed**, zero failures, one pre-existing ignored manual timing profile, zero filtered out. Both new regressions pass. 19.01 s including incremental build; test execution 0.35 s. |
| `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | **Pass**, 106.03 s. Also checks the concurrently prepared MissingProxy process-test changes; it does not execute those processes. |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | **Pass**. |

The sole ignored unit case is
`held_pool::tests::profile_blocked_backlog_discovery`, an existing manual
component profile that records timings without timing assertions. No test,
threshold, cutoff or protocol check was removed or weakened.

| Requirement | Regression evidence |
| --- | --- |
| Faster wider evidence releases the artificial measured-width boundary | `faster_wider_proofs_do_not_lock_admission_to_the_last_measured_width` drives the real group-start and pre-append gates through thirty groups under the same 977 ms opportunity. |
| Retain the slower observation's duration floor | `faster_wider_proof_supersedes_sparse_scaling_but_preserves_its_slow_floor` requires 50 ms; `expensive_sparse_proof_retains_the_slowest_observation_floor` retains the existing 750 ms assertion. |
| Missing dimensions remain conservative | The new predictor regression retains the 500 ms seed when blob or history coverage is insufficient, including unknown (`usize::MAX`) witness count; existing coverage/zero/overflow/history-bound tests pass. |
| Resource and deadline boundaries remain binding | The admission regression rejects extra physical blobs and work at the original cutoff; existing FIFO repair, source ownership, timeout and retry tests pass in the full Composer unit suite. |

Small inputs, exact commands, outcomes, source-patch hashes and executable/log
digests are checked in as [admission.json](admission.json). Bulk output logs and
patch copies are **local-only**, under ignored
`artifacts/admission-controlled-20261025/` and the guard's
`artifacts/extended-validation/checks/admission-*-20261025/` directories. Another
checkout does not automatically have those files. The before-test executable
was overwritten by the corrected build; its hash and reconstructible source
identity were recorded first.

The original `dynamic_admission::ethereum_delayed_start_mixed_wave_after_small_post_replays`
process test is unchanged, including **accepted > 12**, receipt/state/nonce
checks and independent follower convergence. It passes in **62.270 seconds**
on the final release artifacts in [CI run 40195871216](https://github.com/eez-association/eez-rollupX/actions/runs/40195871216),
job `121187111352`. Its Gnosis counterpart also passes in **95.870 seconds**,
job `121187111297`. Both new controlled regressions pass in the same run's
complete workspace unit lane. The exact runtime head is `b1881cf737cdc64ee30a9767807f53d21a3daa86`;
the tested merge has an identical tree, as recorded in [CI evidence](ci.md).

These results establish the controlled mechanism, correction and selected
final-runtime process behavior. They do not prove the precise cause of the
missing-log historical failure or throughput parity; the separate
[paired live comparison](latency.md) retains those limits.
