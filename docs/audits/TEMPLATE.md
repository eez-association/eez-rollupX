# [Investigation or validation title]

Copy to `docs/audits/YYYY-MM-DD-<topic>/README.md`. Replace placeholders and omit
irrelevant sections. This records engineering evidence; calling it an audit does
not establish independent security review or production readiness.

| Field | Value |
| --- | --- |
| Date and status | [UTC date; investigation/validation status and unresolved work] |
| Scope | [Question or acceptance criteria evaluated; what was excluded] |
| Source | [Repository, exact commit, and identity of any uncommitted patch] |
| Protocol | [Pinned submodule commit and relevant working-tree differences] |
| Executables | [Test/runtime binary hashes or immutable image digests, when used] |
| Environment | [Toolchain, network/fork, relevant configuration and dependencies; redact secrets] |
| Related records | [Proposal, implementation/coverage record, and earlier or superseding audits] |

## Findings

State what the evidence establishes and its limits. Separate observations,
reproduced behavior, and hypotheses. A reproduced failure path does not prove
the cause of a historical incident. Keep implementation, validation, deployment,
and remaining acceptance work distinct.

## Evidence inventory

Commit small sanitized summaries and reproduction inputs when appropriate.
Reference large artifacts by retrievable location and digest, with access and
retention limits. Label local-only, missing, or expired evidence explicitly;
an ignored `artifacts/` path alone is not available to another checkout.

| ID | Contents / provenance | Location and digest | Availability / retention |
| --- | --- | --- | --- |
| E1 | [Input, executable, output, or diagnostic; producing revision/run] | [Relative path or artifact reference and hash] | [Checked in / retrievable / local only / missing; limits] |

## Reproduction and executed checks

Record the actual working directory, prerequisites, commands, sanitized settings,
exit statuses, and observations. Distinguish planned commands from executed ones.
For a fix, identify baseline failure and post-change results; if comparison was
unavailable, state why. Record selected/executed/ignored test counts where relevant.

| Requirement / question | Production path or check | Evidence type | Result and evidence |
| --- | --- | --- | --- |
| [Proposal ID or investigation question] | [Path, test, or exact command] | [Static / unit / mocked / embedded / process / live] | [Pass / fail / skipped / inconclusive, with reason and evidence ID] |

Preserve first failures and distinguish retry-only passes. State whether semantic
assertions and independent replay ran. When recording performance, include units,
workload, sample counts, configuration, and comparison baseline; identify network,
fee, or state differences that prevent causal attribution. Do not extrapolate a
bounded successful run into an untested capacity or recovery guarantee.

## Operational changes, if any

Record the existing authorization/scope, actual deployed artifacts, affected
resources, preservation steps, and executed recovery/rollback checks. State what
was retained, lost, or requires resubmission. Do not report a proposed procedure
as executed or a node restart as proof of archival recovery.

## Open work and superseding evidence

| Unmet criterion / uncertainty | Impact | Next concrete check or action |
| --- | --- | --- |
| [Remaining item, or explicitly none within the recorded scope] | [Limit on the conclusion] | [Reproduction, implementation, or validation step] |

Preserve dated results. Add dated corrections and links when later evidence
supersedes a conclusion; update the proposal's status and current operator docs
where appropriate. Results for an older executable do not validate a later one.
