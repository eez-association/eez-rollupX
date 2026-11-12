# Ponytail review

What this change does: Adds receiver-closure characterization tests at the real
committer and capture-task boundaries, and records a diagnostic recommendation
for #141. It changes no production behavior and does not claim a permanent
settlement stall.

Review followed [Ponytail](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail/SKILL.md)
and [Ponytail review](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md).
This was a same-agent review, not an independent reviewer or a vulnerability scan.

Checked the full diff and connected production paths: both committer methods;
Sequencer, Deriver replay/replacement and Composer callers; `run_capture`,
`NodeWitnessSource`, parent persistence and proof construction; Reth critical-task
reporting; preparation errors and held-pool restoration. Expected load is one
notification per committed local block, with serial capture and optional bounded
prevalidation. Added tests have bounded waits and use local channels and temporary
stores; production cost, queue limits and wire/storage formats are unchanged.

Review corrections before completion:

- The assessment initially grouped all Deriver calls with replay suppression.
  The stale-history replacement caller passes `true`; the report now distinguishes
  that newly executed sibling from ordinary replay.
- Replaced an immediate `is_finished` task-manager snapshot with a named sentinel
  panic. The next reported task identity now checks normal-return/cancellation
  behavior without relying on scheduling delay.
- Corrected the test to use Reth's public panic-error display instead of its
  private task-name field; the named-task assertion remains.
- Kept synthetic witness-store results separate from full witness validation,
  real-state reconstruction, and live settlement. Kept #86's historical results
  separate from fresh tests and receiver-close coverage.

No remaining correctness, security, load or unnecessary-abstraction finding in
the scoped diff. Validation results and residual limits are in [the assessment](README.md).

Verdict: Ship.

Not checked: real-state on-demand reconstruction after receiver loss, a live
accepted-work/settlement experiment, mid-capture blocking-worker cancellation,
production performance, or the original review's input provenance.
