# Deployment record and acceptance

For the current single-machine test milestone, use [local testing](local-testing.md).
The owner/host/receiver/off-host requirements in this production template are
future rollout inputs; they are not prerequisites for trying the Compose stack.

Status: **prepared template; no live rollout performed**. Copy this record into
the operator's private deployment directory and replace each `pending` value.
It collects the WP7 inputs and evidence; it is not a new application configuration
format. The executable inputs remain the existing stack, indexer and node-agent
configurations. Keep authenticated URLs, private hostnames, backup keys and
contact details out of the public repository.

## Deployment manifest

| Input | Selected value / evidence |
| --- | --- |
| Service owner and backup operator | pending |
| Alert recipient, acknowledgement/escalation path | pending |
| Environment, deployment, stable service/instance identities | pending |
| Node host and separate monitoring host | pending; document shared failure domain if colocated |
| Independent heartbeat receiver host, owner and delivery path | pending; must remain functional when monitoring host is unavailable |
| Release commit, frozen protocol revision, verified runtime manifest | pending; use the tested candidate, not a mutable tag |
| Node/signer image digests and observer executable checksum | pending; exact source-to-artifact verification required |
| Monitoring image lock/configuration revision | pending; preserve `images.lock.json` and rendered configuration checksums |
| L1/L2 chain IDs, genesis hashes, rollup/contract identity | pending; verify against private read-only RPCs |
| Bootstrap blocks and immutable receipt selections | pending; record archival reachability and irrecoverable ranges |
| Private RPC, Beacon/archive, follower and reference endpoints | pending; mark intentionally disabled optional capabilities explicitly |
| Metrics listeners/targets, node-agent forwarding and database tunnel | pending; verify reachability from the consuming host |
| Grafana operator URL and access recovery | pending; verify notification links from the operator's browser |
| Native telemetry files, approved diagnostic directory and UID/GID | pending; observer/Alloy read access, no signing-key access |
| Lifecycle physical filesystem and quota | pending; separate 3 GiB initial hard cap, including open-unlinked files |
| Monitoring filesystems, free-space floors and capacity headroom | pending; retention does not enforce a physical quota |
| Host CPU/RAM and selected service limits | pending; use Compose limits as initial guards, not proven deployment capacity |
| Trace sampling, source selection and enabled capability profiles | pending; retain measured configuration and explicit unknown states |
| Backup owner, schedule, retention and authenticated off-host destination | pending |
| Backup encryption/access recovery and clean restore host | pending |
| Required RPO/RTO and measured full-dataset results | pending; local fixture timings are not production objectives |

Prepare the private [stack input](../stack.example.json),
[indexer input](../indexer.example.json) and, when enabled,
[node agent input](node-agent.md). Preserve the deployed chain/node configuration
and existing public routes. The indexer's bootstrap/selection cannot be widened
in place: use a new namespace and explicit bounded rescan when coverage changes.
Settled-transaction unit costs require the documented complete prefix; unavailable
history must remain a coverage gap.

The [Compose services](../compose.yaml) define initial memory/CPU limits and
persistent volumes. Defaults retain metrics for at most 30 days/2 GB of blocks,
logs for 14 days and traces for 168 hours; allow WAL, compaction and filesystem
headroom. Canonical accounting is retained for the deployment lifetime with
backups. Choose actual host disk bounds before starting ingestion.

## Ordered rollout

1. Verify the candidate's requirement ledger, release artifacts and configuration
   identities. Fill the manifest and confirm the private network/access paths.
   Keep a recoverable configuration/volume backup before changing an existing
   monitoring installation. A repository test report does not authorize replacing
   a running node or signer.
2. Prepare the runtime using [the stack procedure](../README.md). Inspect rendered
   Compose and provisioning, then start only the selected monitoring project.
   Verify operator access and a real alert's firing/recovery links. Establish the
   independently hosted heartbeat receiver and test missed-heartbeat notification.
3. Enable read-only exporter/client/host probes. Compare configured chain/genesis
   identities and RPC results with panels. Missing, disabled and stale inputs must
   display unknown; quiet cross-chain traffic must display idle/unexercised.
4. Follow [database setup](database.md) for explicit migrations and separate
   administrator/writer/reader identities. Start [the observer](observer-host.md)
   with the selected immutable bootstrap and capped batch size. Record source
   availability, cursor/hash progress, RPC load and reader latency during backfill.
   Reduce collection load if it affects the node; never advance cursors manually.
5. Enable validated native instrumentation through the existing runtime deployment
   workflow. Provision the [physical lifecycle volume](lifecycle-storage.md) and
   collector read permissions first. Verify writer health, observer checkpoints
   and visible gaps. Installing the observer does not enable durable node queues.
6. Enable the selected [trace profile](traces.md) and
   [protocol alerts](protocol-alerts.md) only for configured producers. Check actual
   notification delivery and request → attempt → receipt → trace/log navigation.
   Expired traces must leave canonical records accessible. A continuous funded
   Chiado canary is outside this rollout; passive reachability is not execution
   proof during quiet periods.
7. Run the operator-owned [restore procedure](restore.md) on a separate clean host,
   including off-host transfer and access/key recovery. Check exact accounting,
   checkpoints, role permissions and working contact points after catch-up. Record
   observed RPO/RTO and any lifecycle data that cannot be recovered.

Record each step's time, responsible operator, configuration hashes and evidence
location. Keep failures and corrections alongside successful results. Do not mark
deployment acceptance complete while any manifest or required gate is pending.

## Seven-day baseline and handover

Record at least seven consecutive days including quiet periods and representative
traffic. Keep daily measurements and the configuration revision for each interval:

| Measurement | Evidence required |
| --- | --- |
| Signal coverage | Source success/freshness, scrape failures, queue validity, observer lag, lifecycle gaps and trace drops |
| Protocol behavior | Successful canonical completions, pending/serviceable ages, retries, errors, target misses and explicitly unexercised intervals |
| Resource cost | Active series, event/trace bytes per day, process/backend CPU and memory, physical disk use/growth, spool quota and ingestion/backfill load |
| Operator experience | Dashboard/SQL query latency, actual alert delivery/recovery, false positives, missed incidents and usable runbook links |
| Recovery | Successful scheduled backups, off-host availability, restored checkpoints and measured RPO/RTO |

Select final retention, sampling, capacity and provisional alert thresholds from
these observations; repeat affected checks after changing them. Define service
objectives with explicit windows and denominators, retaining both all-accepted
user experience and separately labelled serviceable-work diagnostics. Do not hide
blocked work or missing observations to improve a reported objective.

Handover requires an operator to diagnose a representative alert, find a request's
canonical receipt costs, distinguish unknown data, and restore monitoring without
altering chain state. Preserve the completed manifest, baseline, contracts,
dashboard inventory and remaining limitations with the deployment record.

Rollback follows [operations](operations.md): disable optional collection or
restore a validated compatible runtime independently of protocol state. Preserve
monitoring volumes and evidence. Never reverse SQL migrations blindly or delete
chain data to repair monitoring.
