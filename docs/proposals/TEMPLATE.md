# [Proposal title]

Copy to `docs/proposals/<topic>.md`. Replace placeholders and omit sections that
do not apply. Keep the proposal proportional to the change; an existing proposal
can be extended. Template text and normative wording do not establish acceptance.

| Field | Value |
| --- | --- |
| Design status | [Draft / accepted / superseded; identify the actual decision or superseding proposal] |
| Date | [YYYY-MM-DD] |
| Source baseline | [Repository, branch, exact commit, and relevant uncommitted changes] |
| Protocol baseline | [Submodule commit, if relevant] |
| Related work | [Issue/PR, existing specifications, implementation record, and audits] |

## Problem and scope

Describe the concrete problem, affected callers, and evidence. State the desired
behavior, what changes, and the non-goals. Link an existing investigation rather
than copying its history. Identify assumptions and unresolved decisions.

## Requirements and acceptance

Give important requirements stable IDs so implementation and audits can refer
back to them. Include failure, recovery, ordering, and resource bounds where
relevant. Define observable success before choosing implementation details.

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | [Requirement] | [Setup, operation, expected result, and necessary evidence] |

## Design and alternatives

Describe the proposed flow, lifecycle/state ownership, affected components,
interfaces, and configuration. Explain the alternatives and material tradeoffs.
Identify protocol constraints and the specification revision they come from.

## Compatibility and recovery

When affected, cover stored data and historical replay, supported networks/forks,
mixed component versions, configuration defaults, upgrade order, and required
restarts or migrations. State failure handling, rollback limits, and what happens
to accepted but unsettled work. Do not assume a downgrade or database restore
recovers volatile requests.

## Validation plan

Map acceptance checks to appropriate unit, compatibility, process, or live tests.
Identify baselines, prerequisites, and checks expected from CI. For performance
work, define the workload, configuration, metrics, and comparison method before
measuring. A plan is not evidence that its checks ran.

## Implementation and follow-up

Link implementation/coverage records and dated audits as they become available.
Report implementation, validation, and deployment status separately. Record
unmet criteria, blockers, and the next concrete action. Update current operator
documentation when the behavior ships.
