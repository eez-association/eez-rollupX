This is the initial documentation-only review at `d9b974c1`. Subsequent
measurement-driver work and its validation are tracked in [the follow-up](followup.md).

What this change does: It records the defined #148 workload, exact input sizes
and a resource preflight that prevents defensible timing on this host. It leaves
runtime validation and actual preparation/streaming measurements explicitly pending.

Reviewed the complete audit and JSON evidence against the pinned schema, fixture
READMEs, production preparation/client/session/retry paths, owning specifications
and related issue/PR scopes. Applied the requested
[Ponytail](https://github.com/DietrichGebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail/SKILL.md)
and [review instructions](https://github.com/DietrichGebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md)
without delegating to another agent.

Confirmed and corrected before publication:

1. The draft described nearest-rank p95 for 20 samples as the maximum.
   It is the 19th ordered sample. The method now states the correct rank;
   no actual latency samples or percentiles exist to correct.
2. The process-name filter in the rejected preflight omitted `clippy-driver`.
   Its presence remains visible in the retained middle top-process snapshot.
   The record explicitly includes it in the future gate and requires process
   checks at every sampling interval. The preflight is still ineligible from
   its disk and other observed build activity; it never admitted timing.

No runtime code, wire/profile, signing, request ownership, retry policy, cutoff,
fixture expectations, operator configuration or dependency was changed.
There is no evidence-backed functional fix to review. No new parser or
measurement framework was introduced merely to package these blocked results.

Verdict: Ship as a **draft partial-investigation PR**. Keep #148 open and its
measurement criterion pending. This is not a completed performance characterization.

Not checked: runtime package/conformance/process correctness, independent fresh
follower replay, actual preparation/streaming latency, production workload
representativeness or the original review-input provenance. Build/storage and
quiet-window prerequisites must be resolved before those results can be claimed.


## Complete driver and evidence follow-up

What this change does: It adds a twelve-case driver for three captured native workloads through the production client and real, network-denied signer. A corrected observer permits reported timing only after sustained shared-host eligibility and retains failed admission or contamination. No production runtime defect or performance improvement is claimed.

This review follows [Ponytail](https://github.com/DietrichGebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail/SKILL.md) and its [review instructions](https://github.com/DietrichGebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md), confirmed current on 2026-11-12. No agent was delegated work. Scope is the complete diff from association baseline `601ebfa9`, the actual client/session/worker callers, fixture construction, test proxy consumers, native CI selection, resource observer, and published evidence. Expected load is one sequential experiment on this shared host.

Confirmed findings corrected before measurements:

1. The new test originally sat outside the published native-suite module inventory. Nesting it under `streaming` and checking that inventory ensures CI selects the ordinary correctness test.
2. Observation originally lacked an aggregate trace bound. The proxy now rejects beyond 4,096 events or 4 MiB, and the observed upstream response limit is 64 KiB. The existing mutation/capture paths remain unobserved by default.
3. An exited harness could leave its SIGTERM-ignoring descendant running. The controller now finishes cleanup of that newly created process group; the controlled regression also proves a separate owned control group remains alive.
4. A partial JSON line after interruption crashed report generation. The raw file now remains intact, with an explicit ineligible diagnostic summary.
5. Real kernel process names contain spaces. The observer now splits only PID and parent fields. Linux-truncated Rust names and every published native vector/blob-verifier executable are also screened.
6. Contamination previously caused a second signal to an old process-group ID after the 120-second post interval. Cleanup now occurs once, immediately before post-observation. The first post resource delta also starts at the final during snapshot, preserving adjacent ten-second intervals.
7. The utility now bounds each attempt by the smaller of three minutes and the remaining fifteen-minute group budget. These experiment limits remain distinct from the production ten-minute exchange/operation deadlines.

8. Fable 5.2 found that the quiet baseline began at the prior bad snapshot. The corrected observer starts at the current first-clean snapshot and resets on rejection. Both an initially failed sequence and a good/bad/good sequence fail at the old head; deleting the reset fails the new control. The accompanying prose now includes the pre-proof signer-log read inside complete_attempt_ms for all four cases.

The retained regressions fail against their preceding implementations and pass after correction. First setup failures are separately identified: the nested build's Rustup selection, sandbox socket denial, missing owned linker libraries, and the transient orchestration selector for the signer's binary test target. They are not runtime deadline reproductions. The strict lint failure was fixed with `Vec::len`, without an allow attribute.

No production runtime source, wire/profile, signing, retry/cutoff, queue ownership, protocol pin, fixture expectation or package version changed within #148. Reviewed main integration adds separate accepted tests, harness changes and a runtime comment; the whole crates tree must not be described as identical to the earlier build. Cargo.lock adds two existing test dependency edges. The local identity helper derives its expectation from independent local configuration, never from the capability response. Existing proxy consumers and request-worker cancellation paths were traced. The loopback proxy overhead, cached filesystem pages, fresh signer per attempt, fixed group order and readiness polling are explicit limits.

Validation: final affected package tests 406 pass (five generation utilities excluded); logical Composer cutoff/retry tests 22 pass; complete tooling 553 pass/seven opt-in exclusions; corrected observer checks twelve pass and exact scanner regressions two pass; strict workspace Clippy, signer all-target check, Rustdoc and format pass. Five source process tests pass, including independent follower replay and the twelve-case ordinary workload check. The frozen shipping release build, guarded native archive and sealing pass; all 70 producer tests pass without skips. The ordinary release correctness test passes all twelve inner combinations. These executions provide correctness evidence and are not workload latency measurements. The corrected gate observed 178 snapshots across 30 minutes but admitted no workload; CPU idle and swap traffic prevented a sustained 120-second clean baseline. Zero warmups, attempts or eligible timing samples exist. The resource blocker is explicitly retained.

Verdict: Keep PR #165 draft while actual eligible timing evidence and the final evidence-review/exact-head CI gates remain pending. No remaining confirmed logic finding in the reviewed driver is being left open. This partial evidence review does not satisfy the missing timing criterion or authorize merging.

Not checked: Actual quiet-window distributions, production traffic capacity, durable queue recovery or the original review-input provenance. Captured inputs do not exercise fresh Composer witness capture, midflight lost ACK, client-channel reconnection or signer accepted-quota counters. Independent expected identity uses the shared identity library; Finalize equivalence is by digest. Optional SIGTERM/SIGHUP cleanup and operator-asserted profile caveats remain explicit; separate sealed artifacts and source/hash checks bind the actual local release bytes. CI owns the complete release/Kurtosis matrix; this task does not establish deployment of any revision.

Bounded Fable 5.2 evidence review: **OK to publish this partial update; merge not
approved**. It used eleven Read calls, no fallback/delegation and no commands or
independent hashing. Six wording comments were corrected in the follow-up without
functional changes. Its final completed-timing review gate remains pending.
The initial sandbox DNS failure and approved retry are retained separately.


## Completed bounded measurement record

What this change does: It records the actual twelve-case campaign from immutable
same-run hosted release artifacts, with twenty measured repetitions per case,
separate startup/restart/capability intervals, observed replay charges and full
resource history. It preserves every failed/partial local window and the original
N14 provenance limitation. No runtime optimization, deadline/configuration change,
SLA, live-L1 or cross-host improvement is claimed.

Reviewed the complete final evidence mapping against the actual raw counts,
source/config/input and pre/post binary hashes, native assertions, timing bounds,
all aggregate distributions and all 97 resource snapshots. The existing CI
producer/verifier/consumer is reused; the single optional job preserves every
normal validation lane and promotion guard. No additional framework or dependency
was introduced for later uses.

Confirmed record findings were fixed: CPU PSI spiked during sampling despite
passing declared gates; I/O PSI was sustained through most of the baseline;
capability and restart medians needed explicit columns and stage-band descriptions.
The corrected record discloses these observations without a new threshold or
cause. Bounded Claude Fable 5.2 review and correction verification passed, with
verified model/no fallback/no delegation; its read-only and attestation limits are
retained in [the completed record](timing-results.json).

Verdict: The bounded characterization and evidence are ready for final normal CI.
Main merge remains conditional on all thirteen required fresh exact-head checks,
expected-head/main compatibility, and no review blocker. Final-main CI and tested
image promotion/public verification remain required afterwards.

Not checked: An independent crypto/native rerun by the record reviewers, larger
or production/V1 workloads, live posting, saturation/durable recovery guarantees,
or the original review-input provenance. Optional signal-cleanup/static-profile
caveats and the declared-gate/shared-host pressure limits remain explicit.
