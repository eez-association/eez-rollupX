# Backport handoff format

Use two JSON files and Python 3 (standard library only). The validator checks
structure; it does not establish technical correctness or approval to publish.
Copy the [manifest](examples/backport-handoff/manifest.json) and
[handoff](examples/backport-handoff/handoff.json) examples as a format reference.
They illustrate the format, not real work to publish.

## Freeze inputs, then draft

1. Record the review's full source/target commits, protocol pins, merge base and
   scope in `manifest.json`. Inventory **every finding**, including minor gaps and
   deferred decisions, using stable IDs and `origin: source` or `target`.
   When converting an existing review, preserve its IDs and revisions. Resolve
   omissions against that review before freezing the manifest.
2. Keep the manifest as a separate input. Run `init` once; it records a canonical
   SHA-256 digest and creates an intentionally incomplete handoff. Record that digest
   with the source review so the next agent can compare it. Do not edit the
   manifest or regenerate its digest merely to make validation pass. A deliberate
   change of review scope needs a separately identified manifest.
3. Fill `handoff.json`, following [Writing issues](issue-guidelines.md). Run `check`,
   fix reported errors, and repeat until it succeeds. Render the Markdown from the
   same validated files; do not rewrite it into a second, divergent report. If a
   requirement cannot be met, report the blocker rather than inventing evidence
   or dropping a finding to pass.

```bash
python3 scripts/backport-handoff.py init /tmp/review/manifest.json /tmp/review/handoff.json
python3 scripts/backport-handoff.py check /tmp/review/manifest.json /tmp/review/handoff.json
python3 scripts/backport-handoff.py render /tmp/review/manifest.json /tmp/review/handoff.json > /tmp/review/handoff.md
```

Use a task-owned scratch directory; the paths above are examples. `init` refuses
to overwrite a file. `check` and `render` only read their inputs and never call
GitHub. On failure they exit nonzero; rendering emits no Markdown. Return the
manifest, handoff JSON, and generated Markdown as accessible attachments/artifacts,
including the recorded digest and validation result. Keep these files outside
tracked paths for a read-only task. Publication still requires authorization.

## Fields

Unknown fields and duplicate JSON keys are rejected. Baselines live only in the
manifest; the renderer takes them from there, avoiding a second set of revisions.

- `resolutions`: exactly one entry per manifest finding. Use
  `{"finding":"F1","action":"drafts","drafts":["N1"]}` to route to one or
  more drafts. Alternatively use `action: existing_issue` with `issue`, `reason`
  and `evidence` (the issue must belong to the target repository), or `action: no_action` with `reason` and `evidence`. No finding
  may disappear; drafts without a finding are rejected.
- `drafts`: each has a unique `id`, `kind` (`implementation`, `investigation`, or
  `decision`), `title`, `summary`, `evidence`, `dependencies`, `priority`, `effort`,
  and `issue` (a target-repository GitHub issue URL for an update; otherwise `null`).
- New drafts require nonempty `done_when`; new issue drafts also require
  `issue_type` (`Bug`, `Task`, or `Feature`). Investigations additionally require
  nonempty `evidence_needed`. Decisions render separately from implementation.
- Updates **cannot** supply `done_when` or a replacement body. They require
  `proposed_changes` (an empty list when none). Put any requested scope or
  acceptance change there; it renders as requiring separate approval. The summary
  is an additive comment. Dependencies render as coordination, not new closure
  gates. Do not smuggle replacement criteria into prose; this remains a human check.
- `evidence`: nonempty `{label, url}` entries with full-SHA GitHub blob URLs and
  line anchors where applicable. Each resolution/draft needs frozen target
  evidence and, for source-origin findings, frozen source evidence. Additional
  historical/current pinned links are allowed. Link accessibility, line accuracy,
  and whether the source supports the claim must still be checked by the agent.
- `dependencies`: draft IDs or GitHub issue URLs, or `[]`. Unknown references,
  self-dependencies, cycles and multiple update drafts for one issue are rejected.
- `priority` and `effort`: `{value, reason}`. Values are `P0`–`P3` and
  `S`, `M`, `L`, `Unknown`. All are suggestions for human confirmation.

The checksum detects a changed manifest against a previously initialized handoff;
it cannot protect against rewriting both. Validation cannot discover findings
omitted from the input inventory, verify commit ancestry or issue freshness, or
prove semantic scope preservation. Those remain part of the review, not a reason
to claim that passing structural checks validates the code.
