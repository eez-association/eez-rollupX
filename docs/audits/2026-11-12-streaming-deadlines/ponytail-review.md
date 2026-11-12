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
