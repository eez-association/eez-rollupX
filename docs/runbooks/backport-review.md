# Backport review and implementation

Use this procedure whenever reviewing or implementing backports into rollupX,
including changes from rollup0. It applies to humans and agents on any machine.
This is a standing procedure, not evidence that a particular backport is validated.

## 1. Define the review

Track code-specific backport reviews in rollupX. For a read-only task without an
issue, use the task brief. If scope or decisions are private, provide an authorized
summary in the reviewing agent's task and flag inaccessible evidence. Do not copy
confidential context into public issues.

The review issue or task records source and target repos, exact revisions and protocol
pins, comparison range or candidate list, relevant scope, known exclusions, and
expected output. Resolve moving branch names to SHAs. If the range is unspecified,
state the range you examined and any coverage gaps; do not claim a complete audit.

Use separate checkouts with Git and access to both repos and their relevant
issues/PRs. Preserve local edits. Follow [AGENTS.md](../../AGENTS.md#establish-the-baseline-and-read-the-applicable-contract)
to establish the baselines and read applicable specifications and accepted decisions.
Missing source, protocol, or decision context is a reported limitation, not permission
to guess. A review-only task does not authorize implementation or issue creation.

## 2. Assess candidates

1. Build an inventory from the scoped source commits, PRs, and issues. Include
   follow-up fixes and reversions. Search open and closed rollupX issues and PRs,
   including rejected proposals, and prior audits first.
2. For each candidate, trace the behavior through current rollupX callers and tests.
   Different commit hashes or code alone do not prove a missing backport; an equivalent
   implementation may already exist.
3. Check relevance to rollupX's accepted contract and pinned protocol. Do not restore
   deliberately removed behavior or treat an upstream feature as an accepted requirement.
4. Record one disposition: **already covered**, **partially covered**, **missing and
   relevant**, **intentionally different**, or **uncertain**. Cite evidence; intentional
   differences need an accepted proposal, AGENTS.md invariant, or explicit maintainer
   decision in a linked issue/PR. Otherwise use uncertain and state whether technical
   evidence or a human decision is missing.

Hand off a short, self-contained summary and a traceable inventory: examined range,
source links, dispositions, rollupX evidence, remaining work, and existing issues or
next steps. Related commits may be grouped, but account for every scoped commit
and issue. Include all referenced findings; link detailed evidence when needed.
Evidence links must be accessible to the recipient.
Distinguish missing behavior, missing validation, and decisions requiring human input.
Separate source inspection from tests actually run. If unfinished, mark the review
partial and state what remains unexamined. Review completion does not mean the
findings are implemented.

Post the report on the review issue; for a read-only task, return it for a maintainer
to post.

## 3. Track and implement selected work

Link existing issues. When issue creation is authorized, create one rollupX issue per
actionable gap with impact, source evidence, dependencies, and completion criteria;
use [Writing issues](../issue-guidelines.md). Propose Priority and Effort for human
confirmation under [CONTRIBUTING.md](../../CONTRIBUTING.md#issues-and-triage).

For implementation authorized by the task:

- Recheck the target revision and whether the gap still exists. Adapt to this fork;
  preserve its accepted invariants and avoid unrelated dependency or protocol changes.
- Keep source PR/commit references in the resulting PR, including adaptations and
  omitted parts. A clean cherry-pick is not proof of compatibility.
- Follow [the validation checks](../../AGENTS.md#build-and-validate) and
  [evidence requirements](../../AGENTS.md#evidence-and-completion).
- Close the implementation issue only when its completion criteria are verified.
