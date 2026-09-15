# Cross-chain queue liveness implementation and validation

Status: implemented, validated and deployed to Chiado on 2026-09-15. This audit
supplements the [proposal](../../proposals/cross-chain-queue-liveness.md) and
[initial Chiado investigation](../2026-09-15-chiado-nonce-gap/README.md).

## Implementation

`HeldPool` remains the sole queue and ownership authority. Admission reserves
from the first missing nonce, recovery inserts after same-chain predecessors,
and explicit validated readmission detaches only a locally terminal old owner.
An unresolved or canonical owner produces a retryable reconciliation error.
Old callbacks keep their exact attempt generation and post hash.

Enabled-policy composition claims from a finite queue snapshot, validates each
source against pinned state plus the accepted prefix, and skips successors of
a blocked sender/direction. Accepted complete groups consume the configured or
automatic limit. A source wait does not charge an attempt or reduce automatic
capacity. Resource and infrastructure failures retain their existing handling.
Posting remains governed by `SettlementPolicy`.

The monitor reads bounded aggregate queue observations through the normal L2
RPC. HTTP and WebSocket use the same registered module. Source waits, unknown
readiness, active settlement, and stalled executable work have distinct states.
Service age excludes time spent waiting for source conditions and time when the
network was idle. This telemetry never authorizes execution.

## Requirement coverage

Test names below refer to the existing crate's unit or native test module.
Execution results and deployment evidence are recorded separately; the presence
of a test is not a claim that it passed.

| Requirement | Implementation | Automated coverage |
| --- | --- | --- |
| First unreserved nonce, lower rollback observations, explicit overflow | `held_pool.rs::next_expected_nonce` | `rollback_admission_repairs_the_first_hole_in_order`; `sparse_maximum_nonce_does_not_prevent_repair_and_full_chain_overflows`; existing maximum-nonce tests |
| One insertion rule; preserve independent order and directional chains | `insert_queued`, `push_front_recovered`, replacement admission | `recovery_across_generations_keeps_lower_nonce_first`; 1,440 recovery permutations; directional replacement/recovery tests |
| Recover an orphaned confirmed source while its successor has an unresolved attempt | Existing ledger invalidation/reconciliation with ordered pool restoration | `queue_reorg.rs::native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order`; both composer and follower must observe the real L1 reorg, restore N/N+1/N+2 exactly once and converge |
| Explicit readmission/replacement survives old callbacks | `retire_local_owner_for_admission` under the pool mutex | `explicit_resubmission_survives_old_reconciliation`; `explicit_replacement_retires_its_old_terminal_owner`; finality tests |
| Canonical/unresolved owners cannot be reset by stale ingress | `AdmissionError::ReconciliationPending`; ingress error mapping | `stale_ingress_cannot_reset_canonical_or_unresolved_ownership`; `unresolved_ledger_owner_cannot_be_replaced_by_a_new_pool_reservation` |
| Exact active duplicate and both 10% replacement fee checks retained | Existing admission checks before ownership transfer | HeldPool replacement, hash deduplication, in-flight and node ingress tests |
| Guard internal restoration against terminal or transferred ownership | Existing `requeue_composing` used by setup, deferral and outer errors | `compose_error_restores_only_remaining_owners_without_undoing_classification`; `legacy_outer_restore_can_leave_only_untracked_successor` |
| Classify source nonce, fee and balance without parsing strings | `SourceCondition`; existing REVM validation match in `local/client.rs` | Enabled/disabled source validation tests; six native source recovery cases in `settlement_policy.rs` |
| Retain unfunded withdrawals, including zero physical L1 backing | `L1TargetSession::execute` funding guard; pinned `read_rollup_escrow` and exact per-rollup gate | Both `queue_liveness.rs` escrow recovery profiles, independent sender, later inbound deposit, exact balance/nonce and follower convergence |
| Failed forks do not mutate the accepted prefix | Existing fork/commit boundaries; source-condition arms discard failed candidate | Native cumulative-balance cases; cross-chain failed-frame and partial-consumption suites |
| Provider/proof/canonical failures remain global and conservative | Existing transient handling and guarded outer recovery | Settlement-policy outage/prover cases; partial-consumption, dynamic-admission exclusions |
| Reach independent work at limit one; skip blocked suffixes | `composer/selection.rs`; `HeldPool::candidates` and `claim` | 10,000-successor test; native fixed-one and automatic fee/balance tests |
| Revalidate cached status; reject stale snapshot replacements | Claimed hash/nonce/owner validation; pinned simulation | Snapshot replacement and recovered-predecessor tests; same-height telemetry invalidation |
| Preserve outbound-before-inbound order and fair resumption | `Selection::resume_chain`; pool discovery cursor | Phase-boundary and deadline unit tests; native fee-blocked inbound with independent outbound |
| No failed attempt/backoff/posting urgency for source-only waits | `Attempt::source_deferred`, admission completion, historical predecessor cancellation | Admission controller source-wait test; native attempt counts remain zero and blocked-only post counts |
| Preserve gas/blob/whole-group/proof/target constraints | Existing resource admission and final bundle checks | Dynamic-admission, settlement-policy gas/blob tests; cross-chain and partial-consumption native suites |
| Preserve pure/interval/general posting and restart semantics | Existing policy; no new startup options | Existing settlement-policy always, interval, idle consolidation, restart and backlog tests |
| Bounded aggregate and per-hash state; exact hashes/generations | `held_pool/status.rs`; `queue_rpc.rs` | Identity/replacement/reorg status test; duplicate completion test; RPC startup/invalid-hash test |
| No raw signed envelopes or secret material in status | Explicit aggregate/per-identity fields | Status serialization assertion; RPC response inspection |
| Local terminal/replacement/recovery transitions | Existing ledger views plus structured transition events | Ownership and replacement regression tests; native logs |
| Expected source waits do not flood error logs | Structured pool transitions; inspector/dispatch error logging | Both native escrow tests require one unchanged blocking transition and no source-blocked error messages |
| Distinguish source waits, stalls, missing telemetry and inactivity | Adjacent observatory `cross_chain_health` and queue panel | Python health/collector tests; both JavaScript suites, including live feed transitions |
| New executable work gets a fresh service window | `oldestServiceAgeMs`; monitor bounds time since completion by service age | `service_age_excludes_source_waiting_and_survives_claiming`; monitor idle/blocked-age regression |
| New regressions run in existing CI | `scripts/ci/native-tests.py` archives and cross-chain suite | Native artifact inventory/filter tests; existing relay tests cover controlled single-post mode and hold/drop barriers |

## Interpretations and discovered paths

- When inbound deferral exposes outbound work after that phase ended, the next
  opportunity starts discovery with that source chain. The execution phases
  remain unchanged. The same cursor prevents a preparation deadline from
  repeatedly selecting a costly source's next higher nonce.
- Native escrow validation found that missing physical funds in the L1 manager
  can fail a synthetic value frame before the exact per-rollup escrow gate is
  reached. Enabled policy now reports this as an escrow source wait using the
  current fork's physical balance. Successful compositions still pass the
  authoritative per-rollup escrow check. No funds or execution outcomes are
  synthesized to make a candidate appear executable.
- Readiness is observational and invalidated by exact source hashes. The
  service timer starts only once a group was executable; source waiting is not
  service time. Restart still loses in-memory requests and telemetry.
- The original Chiado initiating sequence remains unproven because its logs
  rotated. Reproducing a faulty path does not establish which path triggered
  that historical incident.
- The native reorg fixture extends `CrossChainWorld`, its existing relay
  barriers and Beacon captures. Controlled consensus starts before protocol
  deployment and uses the embedded client's real Engine API/payload builder.
  It prepares a competing payload before settlement arrives, then selects that
  fork while the composer and its in-memory ledger remain alive. This avoids
  relying on a local miner that restores its own head or on a nonstandard FCU
  rewind. The test relay explicitly rejects multi-transaction bundles in this
  mode; normal native bundle handling is unchanged.
- An orphaned post may be included again and restore the same L2 execution
  hash. The regression requires a different canonical L1 inclusion reference,
  actual reorg/retreat observations, ordered source effects and follower
  convergence. Requiring a different L2 hash would incorrectly reject valid
  recovery.

## Temporary recovery of the running network

At **2026-09-15 08:03:10 UTC**, the existing Chiado composer was restarted with
the same image, configuration and databases. The pre-restart confirmed L2 block
**121,985** retained its exact hash. At 08:10 UTC the confirmed cursor had
advanced to **122,405**, the affected sender's latest nonce had advanced from
**14,514 to 14,690**, and the last queue sample contained **5** requests. No
nonce-gap errors had recurred in the captured post-restart interval.

This clears the volatile queue and restores operation; it does not deploy the
permanent correction or preserve unsettled requests. Their resubmission was
already authorized. Evidence is retained in
`artifacts/queue-liveness-20260915/recovery/`.

## Validation records

Local executable identities, source hashes, commands and logs are under
`artifacts/queue-liveness-20260915/`. The native runner freezes production
binaries separately from Cargo's test feature unification, verifies their
hashes, requires one named test to run, records every exit status, and retains
failed scenario diagnostics. Failed preliminary runs are retained and are not
counted as final acceptance.

The checked-in [validation inventory](validation.json) records **89/89** native
scenarios against the frozen composer, follower and signer. This includes the
settlement-policy, dynamic-admission, partial-consumption, cross-chain,
chained-interstate, gas/cost, resumption and reorg cases. The first two escrow
assertions accidentally matched any error message; they were corrected to
detect source-blocked errors specifically and both scenarios passed again.
No production execution change was needed for that assertion correction.

Additional checks passed:

- `cargo +1.94.0 fmt --all -- --check` and `git diff --check`.
- `cargo +1.94.0 clippy -p eez-composer -p eez-node -p eez-protocol -p eez-evm-inspector -p eez-testkit -p eez-node-e2e --all-targets --offline --locked -j 3 -- -D warnings`.
- Affected Rust library tests: **384 passed** across composer, protocol, node,
  inspector and testkit. Runtime and affected native targets built successfully.
- `python3 -m unittest discover -s scripts/ci/tests`: **219 tests**, four
  environment-gated skips, no failures. Includes all 12 relay control tests.
- Observatory Python tests: **50 passed** in the existing dependency image.
  Both `test_app.mjs` and `test_live_app.mjs` passed.

## Chiado deployment and live validation

The composer, signer and monitor images were replaced at **08:33 UTC**. The
pre-upgrade confirmed block **123,805** retained its exact hash. The deployment
changed only image references: databases, contract addresses, keys, automatic
admission, the 500-gwei fee ceiling, `always` pure-L2 mode and the 1,800-block
general consolidation setting were preserved.

Between **08:36:49 and 08:38:28 UTC**, two fresh source accounts exercised each
direction. A's nonce 0 settled; A's nonce 1 waited for its depleted balance. B's
independent nonce 0 settled while A remained queued. A top-up then allowed A's
original signed nonce-1 request to settle. All **six** source receipts succeeded;
inbound receipts contained canonical consumption events. Final nonces were
A=2/B=1 on each source chain and the queue was empty.

An isolated follower copied from the previously verified L2 block 87,700
replayed L1 evidence and matched the final block **124,115**:

```text
hash:      0x49778323e61abcd7f84790ed4f229a8274fcb6fc0ef3eec6530078705e5e53ec
stateRoot: 0xe52092edcf12d8ab7d23f7cede193717ea3988f0e33110330acf93a028ccb60d
```

The [deployment and receipt evidence](chiado.json) includes executable/image
identities, source hashes, blocking observations and follower agreement.
The public monitor returned the new queue panel and snapshots. Both public
chain WebSockets delivered heads; the L2 socket served the new queue RPC, and
the monitor WebSocket pushed queue status with increasing sequence numbers.
The probe supplied the same Origin header as a browser; the existing Origin
restriction was preserved.

No remaining normative queue-liveness requirement is knowingly unimplemented
or untested. Restart still discards pending in-memory requests by design. The
live exercise is bounded; it does not establish an unlimited-load throughput
guarantee or recover the original incident's missing logs.
