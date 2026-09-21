# Protocol alert operations

Stage-2 alerts are opt-in through `protocol_alerts` in the stack input and require
`database_enabled: true`. The absence of this object preserves the stage-1 alert
set. Enabling the database alone does not establish that its indexer runs.

```json
{
  "database_enabled": true,
  "protocol_alerts": {
    "owner": "rollup-operations",
    "index_lag_blocks": 64,
    "gap_threshold": 1,
    "progress_warning_seconds": 120,
    "progress_critical_seconds": 300,
    "blob_collection": false,
    "follower_comparison": false,
    "lifecycle_sources": [{"service": "composer", "instance": "node-1"}]
  }
}
```

This is a configuration fragment; retain the normal private targets and alert
receivers. `owner` is required and becomes a bounded alert label. Assign its
actual on-call recipient through the existing operator webhook. All protocol
rules use Grafana's single alerting route, stable UIDs, explicit error handling,
dashboard links, a standard same-origin `runbook_url` to the authenticated
**EEZ operator runbooks** dashboard, and recovery notifications. Configure the
browser-facing `grafana_public_url` in the stack input when the default local
port is not the recipient's Grafana address. The unchanged routing policy
groups by deployment and alert name; repeated evaluations do not create new
identities. No transaction/address/epoch label is added.

## Diagnosis

Thresholds above are provisional defaults, not calibrated service objectives.
Integer thresholds must be positive and at most 86400; the critical progress age
must exceed warning age. Index lag is **blocks**, gap threshold is a **count** and
ages are **seconds**. Collection failures and coverage warnings normally hold for
two minutes, evaluated every 30 seconds. Progress/policy conditions already
measure elapsed time, so they add no second two-minute hold. The test profile
uses ten-second evaluation/holds and a shortened rate window; these settings do
not change deployed defaults.

Queries fail explicitly into Grafana's alerting error state. A dependent query
with no data is not itself paged: separate expected-source/evidence rules report
unavailable inputs. Disabling an optional capability removes its rules. Enabling
one with missing metrics warns instead of treating its values as zero.

Start with the parent source alert. Confirm private network/scrape reachability,
identity and fresh successful collection before interpreting dependent values.
Index failure suppresses stale index counts and canonical-progress symptoms;
observer failure suppresses observer-dependent rules; signer rejection symptoms
require healthy L1 and L2 collection. These gates leave raw source-error data
visible. Never clear receipt gaps, reset epochs or modify chain data to silence
an alert. Verify recovery notifications after repair and retain incident evidence.

## Canonical index

`EEZCanonicalIndexUnavailable_l1/l2` means a configured scanner is stale, failed
or missing. Inspect bounded index error reasons, chain/genesis and database
access. A finalized contradiction needs investigation, not automatic unfreezing.

`EEZCanonicalIndexLag_l1/l2` compares actual head-minus-cursor lag with
`index_lag_blocks`. Preserve configured bootstrap/selection and allow bounded
backfill. `EEZCanonicalIndexGaps_l1/l2` counts persisted open/frozen history or
receipt gaps against `gap_threshold`; absent receipts do not have zero cost.
Check the SQL coverage and receipt rows from the cross-chain/gas dashboards and
restore an archival source if history is unavailable.

## Canonical progress

`EEZCanonicalProgressStalledWarning/Critical` requires ready or in-flight work,
advancing L1 over a two-minute observation window, valid canonical outcome
evaluation and a known last canonical terminal timestamp older than the
configured threshold. The stable queue recording rule has one selected producer;
there is no implicit fallback between RPC and native populations.

This condition is deployment-aggregate because the original RPC queue is
aggregate. A direction with continuing progress can mask another blocked
direction; use the detailed dashboard and source evidence when diagnosing.
Canonical terminal handling includes verified terminal failures and is not a
successful-commit throughput claim. Successful commits remain separate data.
Reorgs can retract the timestamp; the alert uses the recomputed gauge.

If work exists but no canonical terminal history is retained,
`EEZCanonicalProgressUnknown` warns. It does not invent an epoch-zero completion
or page a fabricated stall. `EEZCanonicalOutcomeEvidenceUnavailable` reports
invalid/missing evaluation after both indexes are healthy. Inspect lifecycle
capture, native coherence, exact receipt/root/correlation anchors and unresolved
claims. Quiet work or nonadvancing L1 does not trigger a cross-chain stall page.

## Lifecycle

List expected observer sources explicitly by their configured lifecycle service
and instance (for example `composer`, not the process target's `eez-composer`).
Missing/stale collection warns even if all its gauges disappear. Each source has
separate coverage alerts for gaps, quarantine, lost/unreadable segments, stale
partial segments and trailing uncertainty. Coverage appears immediately in the
dashboard; the two-minute alert hold avoids paging transient incomplete writes.

Configured native lifecycle writers additionally expose enabled state and
cumulative loss/error counters. Writer failure or increases over five minutes
warn independently of trace sampling. Keep the drop counters intact on recovery.
Inspect immutable control records, disk/permissions, segment inventory and SQL
gap state. Preserve original bytes before an administrative replay; see
[lifecycle ingestion](../contracts/lifecycle-ingestion.md).

Native trace losses and collector losses have their existing independent
five-minute warnings. Diagnose bounded queues, receiver reachability and Tempo
health using the [trace runbook](traces.md). An expired/missing trace is not a
missing receipt, and loss counters may overlap rather than count unique spans.

## Blobs

Enable `blob_collection` only when the observer's authenticated historical blob
worker is configured. Collection failure has its own warning; otherwise
`EEZBlobMeasurementCoverageGap` warns when nonmeasured post states exceed the
gap threshold. Inspect Beacon connectivity, pruned history, authentication,
codec/version and resource-limit states separately. Recover available archive
bytes and let the bounded worker retry; never count an unmeasured stream as an
empty blob.

Historical pruning/measurement gaps do not page as current derivation failures.
The separate `required_blob_derivation` capability enables
`EEZRequiredBlobBlocksDerivation`: fresh observer-confirmed native evidence that
the same canonical posting occurrence is blocked after at least three exact
reference retries, with a 30-second hold. `required_blob_retry_threshold` can be
2–100 and requires that capability. The observer must have configured L1 posting
contracts and a node supporting the typed current-dependency diagnostic.
Success, a different failure, reorg or expired evidence retracts the page.
Missing/unsupported/stale evidence gets its own warning, never a replacement
inference from historical blob coverage. Inspect Beacon connectivity and the
exact canonical settlement reference before repairing endpoints or archive access.

## Funding

The [funding contract](../contracts/funding.md) separates explicitly configured
observer accounts from the actual runtime poster. Configure the indexer's
`funding` accounts/chain/address and complete wallet selection first. Then declare
the corresponding bounded alert capabilities, for example:

```json
{
  "funding_accounts": [{"account": "poster", "chain": "l1"}],
  "poster_funding_estimates": true
}
```

These fields belong inside `protocol_alerts`. The native estimate flag requires a
configured composer metrics target; it does not assert that its signing address
equals any observer-configured account. No alert joins or compares those balances.

`EEZFundingRunwayBelowDay_<account>` warns only on exact SQL-confirmed positive
covered burn and a known fresh account balance. Equality at 24 hours is not below
the boundary. Zero burn stays unknown without a coverage-failure alert; missing
history, mismatched canonical revision and query failures produce separate
coverage warnings. A zero actual balance with positive burn is known zero runway.

`EEZActualPosterCannotFundCandidate_<instance>` pages immediately when the runtime's
exact candidate comparison is insufficient and both the sample and native target
are fresh. An insufficient necessary blob minimum also proves inability to fund,
with `minimum_only=1`; it does not pretend a complete execution estimate exists.
An unknown/expired candidate or balance withdraws the page. When ready/in-flight
work exists, missing comparison data gets a separate warning. A future candidate
can have a different reserve; these observations do not promise future fees.

Before funding an account, verify the actual chain, destination address,
operational ownership and transfer procedure outside the dashboard. Inspect the
candidate reserve, balance anchor, burn window and exact canonical receipt spend.
Never fund an address merely because it has the display role `poster`, and never
weaken admission/gas checks to silence a funding alert. No transaction is sent by
this observer or its tests.

## Signer

With protocol alerts enabled, configured signer metrics targets get
`EEZSignerRepeatedRejections_<instance>`. At least three `invalid` or `rejected`
request outcomes in five minutes warn by outcome; the optional
`signer_rejection_threshold` overrides that count and requires a signer target.
Those finite categories come from the RPC status contract, not arbitrary error
messages. Successful TCP connection does not establish signing correctness.

Confirm both upstream chains and signer metric collection are healthy, inspect
the correlated request logs/trace and actual validation reason, then check chain
identity, settlement context and caller configuration. Do not relax validation
to restore throughput. A missing request counter gets an explicit capability
warning. A rejection rate does not prove valid work is blocked.

`EEZSignerAuthorizedAttestationFailures` is a separate critical condition. Its
producer starts only after the signer has successfully recomputed and verified
all execution, settlement, blob and observation claims and returned authorized
attestation material. It counts response failures from signing, the attestation
deadline and panics; malformed input, failed validation and caller cancellation
cannot drive this page. Three failures in five minutes with no successful
authorized response in that same window is the default threshold, configurable
through `signer_authorized_failure_threshold` (1..86400, requires a signer target).
Signer scrape and both upstream RPC sources must be healthy. A successful
authorized response suppresses the failure page until that success ages out;
old successes outside the window do not mask a new outage, and old failures age
out without resetting counters. Missing outcome counters are explicitly unknown.
This establishes failure to complete known-valid attestations, not causality for
every queued request. Inspect the attestation error/deadline and correlated
trace, preserve validation and signing boundaries, and recover the signer before
retrying the affected work.

## Pure-L2 policy

Policy-age alerts require a composer metrics target and explicit
`pure_l2_inclusion_grace_seconds` under `protocol_alerts`. Absence leaves them
disabled; no runtime grace is guessed. A nonnegative value up to 86400 enables
a warning after work remains continuously due for that grace period. Critical
uses grace plus `policy_critical_delay_seconds` (default 300). This extra delay
is rejected unless grace is explicitly configured.

Rules require enabled policy, valid evaluation no older than 45 seconds, live
native scrape, a due policy decision and pending pure-L2 transactions. The
condition begins when anchor age exceeds the active interval; the alert hold
measures how long work remains due. No pending work or invalid evaluation resets
that hold. New work after a long quiet period therefore receives its full grace,
even if the settlement anchor is hours old. The measured age is the actual
policy's candidate-target timestamp minus canonical settlement-anchor timestamp;
it is not the oldest transaction's age. Inactive/unbounded settings remain
unknown and are never replaced by numeric zero. Stale/missing policy evaluation
warns separately and suppresses overdue claims. Check active policy, target-slot
timing, pending work, submission and signer stages before changing batching.

## Follower

Enable `follower_comparison` only with the independent observer comparison
configured. `EEZFollowerStateRootMismatch` pages after 30 seconds of known,
fresh agreement=0 from exact canonical reference validation. Its matching key is
the actual settled block and provenance established by the producer, not height
alone. Preserve block/root/parent and L1 occurrence evidence, inspect reorg and
independent execution status, and escalate a reproducible discrepancy.

Disabled, stale, lagging, reused or invalid-provenance evidence is unknown, never
agreement. An enabled capability in that state gets
`EEZFollowerComparisonUnknown`; it cannot sustain the mismatch page after its
evidence expires. Recovery means a fresh exact comparison, not hiding the source.

## Acceptance and limits

`test_protocol_rules.py` evaluates the exact provisioned PromQL over deterministic
series, including missing/stale inputs, boundary values, retracted progress,
suppression and recovery. `smoke-alerts.py` exercises the actual pinned Grafana
and Prometheus with disposable synthetic metrics and records real webhook
delivery/recovery. Neither substitutes for fault injection against native
workloads or calibrated live deployment thresholds. Missing funding estimates
and uncovered burn remain unknown.
