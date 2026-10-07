# Runbooks

Repeatable operational procedures for humans and agents. Start here before
reconstructing a process from scripts, old logs, or an earlier conversation.

| Task | Procedure | Verification status |
| --- | --- | --- |
| New Chiado rollup: minimal command sequence | [Chiado quick start](chiado-quickstart.md) | Published-image procedure; fresh-host rehearsal pending |
| Sync and run a Composer for an existing Chiado L2 | [Existing-rollup Composer](chiado-existing-rollup.md) | Source reviewed; live replay/restore rehearsal pending |
| Deploy, start, check, and stop a Chiado rollup | [Chiado deployment](chiado-deployment.md) | Source reviewed; static checks; full deployment rehearsal pending |
| Start and check a Composer on Ethereum mainnet, Hoodi or Sepolia | [Ethereum node](ethereum-node.md) | Static Compose checks, unit tests and builder acceptance probes; not yet run against a synced Ethereum node |
| Trace Composer gas for selected source hashes or an address | [Composer gas traces](composer-gas-tracing.md) | Hash-mode Chiado evidence; address-mode local coverage |
| Run L2 Blockscout, Chiado Blobscan, or both | [Optional explorers](explorers.md) | Compose/helper and isolated Blobscan source checks; full stack rehearsal pending; operated outside CI |

Use `docs/proposals/` for intended changes, `docs/audits/` for dated evidence,
and `docs/runbooks/` for the current procedure. Keep host-specific deployment
inventories and secrets in the operator's private records; link sanitized
execution evidence from an [audit](../audits/TEMPLATE.md).

When adding or updating a runbook, include:

- Scope, supported topology, source baseline, and verification status.
- Prerequisites, required inputs, resource ownership, and files to preserve.
- Ordered commands with working directory, expected results, and stop conditions.
- Separate first-time operations from repeatable start/stop and recovery.
- Health checks, common failures, rollback limits, and a handoff record.

Link the scripts and configuration that implement the procedure. Keep one
authoritative walkthrough and link it from the README and `AGENTS.md`. When
an operation reveals a missing step, update the procedure after verifying it;
record one-off observations and unverified hypotheses in the audit. Reading a
runbook does not itself authorize deployment, funding, or destructive recovery.
