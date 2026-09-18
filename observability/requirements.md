# Implementation and acceptance ledger

Source of truth: [proposal](../docs/proposals/network-observability.md), including
its final implementation decisions and [review dispositions](../docs/proposals/network-observability-review-disposition.md).
`Verified` below means the named evidence exists, not production acceptance.
An open row is a release gate, not an implicitly deferred requirement.

| ID | Requirement / invariant | Implementation and configuration | Tests / evidence / documentation | Status |
| --- | --- | --- | --- | --- |
| R01 | One identity source; bounded labels; no secrets or signed pending envelopes | Exporter/native identity config, generated target/log labels, typed lifecycle schema and reviewed finite trace fields | Config/prepare tests; contracts/metrics.md, native-startup.md and tracing.md; actual JSON trace ID and Alloy/Loki/Tempo navigation fixtures | Implemented and fixture-verified for configured producers; deployment client-log availability and workload cardinality remain R06/R27/R30 |
| R02 | Existing configurations and protocol behavior unchanged by disabled telemetry | Optional EEZ_TELEMETRY_CONFIG, entrypoint wiring, CLI precedence | Native startup tests; source-built bidirectional enabled/disabled canaries and legacy RPC regression in validation/native-lifecycle.md | Enabled legacy/policy and disabled native canaries passed on the final local runtime (validation/coverage-followup.md); release/performance and deployment gates remain R27/R28/R30 |
| R03 | One Reth recorder/subscriber; verified prefix, histogram buckets, embedded attribution | Pinned recorder builder hook; standalone signer exporter; filtered OTel layer alongside original subscribers; embedded upstream metrics remain process-wide | Singleton/bucket tests; validation/native-metrics.md: 2/2 integrated native scrape canaries with real reth-prefixed counters/histograms; tracing startup and connected RPC tests | Implemented for the pinned capability profile: native EEZ prefix/histogram/subscriber integration verified; pinned Lighthouse endpoint/log inventory and safe node-host wiring verified in validation/client-capabilities.md; expanded embedded metric inventory passed both final native canaries (880/886 sample names; validation/embedded-client-metrics.json); unlabelled client gauges remain mixed/unattributable, never inferred per-chain |
| R04 | Read-only exporter; chain/genesis checked each poll; bounded calls/backoff | crates/eez-observer, config.example.json | Baseline exporter tests and smoke-exporter.py; optional network identity/fork/freshness fixtures and real backend failure/recovery (contracts/network-health.md; validation/coverage-followup.json) | Verified in stated local scopes; optional network probes preserve absent-config behavior |
| R05 | Disjoint queue counts; stale/failed/null means unknown; one recording-rule producer | Observer model/metrics; prometheus/recording-rules.yaml | Rust freshness/partition tests; rules.test.yaml, smoke-stack.py | Verified |
| R06 | Private pinned self-hosted stack, resource/retention budgets, persistent stores | compose.yaml, images.lock.json, prepare.py; optional private PostgreSQL and Alloy/Tempo profiles with persistent volumes | validation/backend-stack.md, tempo-backend.md and restore.md: ingestion, navigation, effective retention configuration, container replacement and fresh-instance restore | Partial: packaged profiles fixture-verified; node-host agent packaging/private owner and group readers are fixture-verified (validation/client-capabilities.md); 3 GiB physical lifecycle quota drill is verified separately (validation/spool-quota.md); native protocol progress under unavailable storage/collector and dynamic disk exhaustion is verified separately (native-telemetry-failure.md, native-enospc.md, native-loss-alerts.md); deployment retention/capacity and comparative overhead remain open |
| R07 | First two dashboards, explicit no-data/error alerts, recovery and independent heartbeat | Grafana provisioning and network/infrastructure dashboards | smoke-stack.py exercises rules and local receivers | Partial: actual independent host/receivers pending |
| R08 | Canonical scanner identity, bootstrap, parent-linked consistent fetch, atomic cursor | Observer indexer/ledger; explicit immutable selection and bootstrap | validation/database.md: restart, killed-before-commit, concurrent cursors, mid-fetch fork, history-gap recovery | Verified for receipt scanner; large/deep backfill performance remains R27/R30 |
| R09 | Preserve orphan/reinclusion history; finalized contradiction freezes affected views | SQL blocks/receipts/cursor model; L1 anchor validation; source_valid gating | Real same-height fork, reinclusion, finality freeze tests in smoke-database.py | Implemented: exact applied L2 hashes plus finalized L1 anchors; schema-3 fixture verification recorded in database evidence |
| R10 | Exact gas/blob charges, canonical reverts, missing fields gaps; no join multiplication | Observer accounting parser, selected receipt views and schema-5/9 source receipt/reference views | validation/receipt-accounting.md and source-receipts.md: exact sums, missing/revert/reinclusion and deduplication fixtures | Partial: selected/source/companion receipt totals and integer shared estimates fixture-verified without join multiplication; mixed live native legacy/policy, partial-restart and reorg accounting passed with exact source/post fees and paid populations (validation/native-observer.md) |
| R11 | Wallet spend, settlement/companion attribution and L2 fees distinct | Wallet/posting views, applied ranges and separate lifecycle-referenced source receipt worker (admissions and later explicit members) | SQL/RPC wallet/post/source separation, 30-second lookup budget, chain isolation, identity context fence; ABI bounds tests | Partial: schema-14 companion attribution and equal_share_v1 source overhead estimates implemented; validation/cost-allocation.md records paid requeues, duplicates/conflicts/reorgs and saved-v13 upgrade; schema-18 settled-L2-transaction unit costs now pass 16 focused SQL assertions; combined schema20 backend64-group gate, 88 real Grafana queries and mixed live native paid-population/source-fee audit pass; deployment calibration remains R30 |
| R12 | Authenticated/versioned blob length; measured-subset weighted utilization and coverage | Count-only shared codec boundary, physical KZG authentication, optional bounded Beacon worker, schema-12 immutable metadata/measurement views | validation/blob-measurements.md: codec/KZG/mixed-traffic regressions and real RPC/Beacon weighted-subset/pruning/revert/recovery fixtures | Implemented and fixture-verified: final schema-13 duration/freshness/reader process checks passed; deployment archive coverage/capacity remain R27/R30 |
| R13 | Explicit migrations, schema compatibility, private writer/read-only viewer roles | migrate subcommand, append-only migration history through schema21, private PostgreSQL profile and separate role credentials/settings | Schema/checksum/downgrade refusal, no implicit DDL, populated upgrades; validation/restore.md: fresh-instance exact migration identity, reader/writer authentication, privileges and reader deadlines | Implemented and fixture-verified: prior saved1/4/5/6/8/13/14 upgrades are retained; the schema21 release gate passes58 groups including saved-v20 upgrade/downgrade refusal, and fresh-instance restore verifies all21 checksums (validation/observer-capacity.md). Deployment recovery targets remain R29/R30 |
| R14 | Typed versioned events; epoch/sequence at owned transition; many-to-many membership | eez-telemetry schema; contracts/lifecycle-v1.md and native-lifecycle.md | Composer/component and source-built capture evidence in validation/native-attempts.md, native-preparation.md, native-reconciliation.md and native-local-dispositions.md; native-outcomes.md and canonical-outcomes.md add observer checks | Partial: owned admission/readiness, preparation/cancellation, attempt/repricing/dispatch, reconciliation/reorg and local dispositions verified in stated scopes; captured native decisions pass observer coherence, with separate canonical SQL fixtures; integrated native legacy/policy, both partial-restart modes and actual outbound reorg pass (validation/native-observer.md); overhead remains R27 |
| R15 | Bounded nonblocking lifecycle writer independent of logging/sampling | eez-telemetry spool; 4096-record channel, 16 KiB records, 64 MiB segments, 24h/2 GiB retention | validation/lifecycle-transport.md: overflow, ENOSPC, rotation, shutdown, concurrency; validation/native-lifecycle.md: real native capture | Partial: native lifecycle capture and real 3 GiB disk-full/recovery gate passed (validation/native-enospc.md); comparative overhead and actual deployment quota provisioning remain |
| R16 | Chunk completion+digest, immutable segment IDs, independent control/drop records | eez-telemetry wire/spool | Missing/reordered/conflicting chunks, bypassed-full-channel control tests | Verified at transport/SQL layer and through assembled native lifecycle evidence in R14/R19; workload overhead remains R27 |
| R17 | Instance/epoch fencing and headers; explicit ended/unknown liveness | OS advisory locks; read-only probe_epoch | Collision and restart-without-traffic tests | Verified in isolated writer-crash/probe/SQL fixtures; live Composer-restart tests additionally prove original unresolved_restart and separate new-epoch admission after actual resubmission in legacy and policy modes |
| R18 | Transactional file offsets, dedup/quarantine, provisional/suspected/recoverable gaps | Independent configured source workers, schema-4 chunk/control/raw evidence and atomic checkpoints | Real importer-kill rollback, writer crash, late chunks, quarantine, rotation, retention, concurrent replay and full-u64 interval tests | Implemented and fixture-verified; workload capacity remains R27/R30 |
| R19 | Restart admissions unresolved; exact assurance and reorg-retractable outcomes | Native reconciliation/reorg transitions; digest-bound native coherence; schema-11 receipt/root/correlation anchors, canonical selection/progress; schema-17 exact observed finality and clock states | Native capture/RPC comparisons; validation/canonical-outcomes.md: 40 anchor and 7 admission assertions plus importer/database process checks | Implemented and locally verified: native coherence, canonical anchor/selection views and progress gauges implemented and fixture-verified; schema-17 exact finality timing SQL fixtures pass with explicit unknown history/clock semantics; real finality dashboard/count/clock-state queries pass in the 88-query gate; live native→observer legacy/policy restart and actual outbound reorg acceptance pass (validation/native-observer.md), including exact source/post receipts, paid populations, separate admission epochs, retraction, and actual canonical metric scrapes |
| R20 | Monotonic native ages/stages; direction and outcome counts, pending distributions | Native snapshots/terminal partitions; bounded monotonic blocked/serviceable/unknown clocks with missing-clock coverage; Composer/client/signer stage guards; policy/cut/slot signals; explicit current-source rules | validation/native-metrics.md: finite timers, monotonic partitions, 3 source-switch tests, 9 real promtool cases and expanded native canaries passed; validation/native-trace-pilot.md adds final signer/client/telemetry tests | Partial: metrics, explicit source switch, integrated stage/policy/client assertions and mixed live native cases verified; comparative overhead, final shipping-artifact and deployment acceptance pending |
| R21 | Four protocol dashboards with coverage, links, policy/restart annotations | Four protocol JSON dashboards, profile-selected provisioning, bounded reader pool, exact string rendering, optional explorer/Tempo links and scoped release annotations | 96 real Grafana queries through schema21, including readiness partition/coverage panels, plus actual Chromium request→timeline/attempt/receipt navigation with selected-source data; real expired-trace UI with durable SQL retained; validation/protocol-dashboards.md and trace-retention.md | Partial: actual browser selectors/links/expiry and release annotation fixtures passed; mixed native receipt/population/reorg reconciliation passes separately in validation/native-observer.md; deployment acceptance remains open |
| R22 | Stage-2 progress/age/funding/blob/signer/index-gap alerts | Validated native/observer producers, Grafana rules/runbooks | validation/protocol-alerts.md/json and protocol-alert-rules.json: actual pinned Grafana notification/recovery and suppression, typed rule fixtures | Fixture-verified in documented synthetic/native producer scopes; actual delivered notification origin/private runbook correction passed real browser navigation (validation/alert-links.md); broader native fault/deployment calibration remains explicit |
| R23 | Finite OTel spans; W3C gRPC context; batch/retry links; no signed-wire change | Optional native trace layer; finite preparation/proving/retry/client/signer spans; bounded source links, lifecycle trace IDs and explicit reviewed successful-attestation JSON field; W3C metadata only | validation/native-trace-pilot.md/json: real native bidirectional batch/retry canary passed 1/1 with exact ancestry/resources/admission links, semantics and independently checked signature; actual node-agent Loki projection; trace-retention.md: real expired-trace browser navigation preserves accounting | Partial: connected native batch/retry/log and backend expiry/navigation gates passed; workload sampling/overhead, shipping-artifact and deployment acceptance remain open |
| R24 | Bounded tracing, explicit sampling/drops and post-drain flush budget | Bounded native queue/SDK/export limits and sampler/drop counters; private Alloy/Tempo 168h profile; Reth min(2s, remaining/4) and explicit deadline-free signer 2s post-drain allowance | validation/signer-trace-shutdown.md: actual reachable/stalled HTTP shutdown in 43 telemetry tests; 287 signer / 15 client / 4 control regressions; existing queue/outage/overflow/remaining-budget tests; trace-retention.md: physical deletion with SQL preserved | Partial: exporter bounds, Reth budget and explicit signer allowance verified in stated scopes; actual native loss-alert firing/recovery and semantic execution now pass (validation/native-loss-alerts.md); corrected notification links also pass; performance and deployment acceptance remain open |
| R25 | Optional follower compares exact settled block/root; disabled/lag unknown | Boot-local actual STF evidence, explicit role/mode/scope RPC, optional exact-reference observer worker and known-gated dashboards | validation/follower-comparison.md: typed/HTTP/SQL checks, fresh follower and persisted-history restart plus existing reorg/overlay/backfill canaries; 73-query dashboard gate | Implemented and fixture-verified in stated scopes; combined local build/Clippy passed; shipping artifacts and actual independent deployment remain R28/R30 gates |
| R26 | Devnet bidirectional canaries check semantic effects; quiet is unexercised | Existing devnet/Kurtosis infrastructure; shared enabled/disabled cross_chain canary; explicit idle activity beside retained canonical success history | Both directions, destination values, safe-state convergence, recomputed proof signature and native lifecycle identity; actual Chromium idle presentation plus fresh/unknown/stale datasource checks in validation/protocol-dashboards.md | Quiet presentation fixture-verified without synthetic health or a continuous-canary claim; final mixed-fault acceptance is tracked with native observer and R27 validation |
| R27 | Telemetry failures never block protocol; bounded physical spool storage | Native queue API, fixed 3 GiB ext4 fixture, bounded native reader and Alloy discovery | validation/spool-quota.md/json: six physical ENOSPC/open-unlinked/rotation/descriptor/recovery groups passed; explicit loss controls; no OOM | Partial: isolated disk/reader/API behavior and native unavailable-storage/stalled-collector progress verified (validation/native-telemetry-failure.md, 1/1 semantic canary); dynamic ENOSPC with an initially healthy real writer, stopped Alloy reader, both-direction semantic/signature progress while full, and exact fresh post-release persisted admissions/commits now pass (validation/native-enospc.md/json); actual HeldPool probe and fixed-candidate comparative harness are implemented and unit-checked (validation/overhead-harness.md); controller-owned cleanup has configuration, subprocess and adapter regressions plus five passing native cases covering success, forced termination, early deadline and original fixture compatibility (validation/release-runtime.md); provisional <5% overhead, live probe calibration and deployment quota provisioning remain open |
| R28 | CI validates affected crates/features and backend queries | Observability workflow, runtime Docker compile-input allowlist/COPY, existing CI contract tests | validation/runtime-packaging.md: real Docker context regression, tooling checks plus 27/27 actual tiny-workspace producer/relocated compilerless consumer tests passed, including observer artifact and required physical-quota archived handoff; codec/Tempo/dashboard gates wired; observer now ships in frozen runtime archive/node image and real tiny-workspace producer/consumer fixture passes | Partial: sealed45e5c015 application artifacts,1194 archived unit tests, release database/dashboard checks and the full28-scenario local network lane passed. The original embedded and cross-chain lanes each retain one retry-only failure (cross-chain:73 passes,1 failure,2 standard skips). The corrected burst assertion passed baseline/candidate checks on Ethereum and Gnosis; the independent sealed-runtime physical-quota follow-up also passed. The fitting40 admission split remains under investigation, and all five real controller-cleanup/fixture-compatibility cases passed. Remote Actions acceptance is still required |
| R29 | Clean-host restore including SQL/config/contact points; measured RPO/RTO | Standard SQL dump/restore and cold configuration/diagnostic archives; smoke-restore.py; runbooks/restore.md | validation/restore.md and restore.json: 6 groups passed in fresh projects/volumes, exact costs/cursors/checksums/roles, idempotent catch-up, retained history and actual alert routing; schema21 follow-up verifies all21 migration checksums and restored operator runbook contents; 32.77s backup, 68.91s restore on the schema21 standalone observer, explicit one-record lifecycle loss | Partial: application-level separate-instance recovery measured on the same Docker host; physical-host loss, off-host transfer/key recovery, full-size data and production schedule/RPO/RTO remain deployment gates |
| R30 | Real rollout manifest, owner, endpoints, bootstrap, private network, capped backfill | runbooks/deployment.md provides the private manifest template, existing config mapping and ordered rollout; actual deployment inputs pending | Seven-day baseline, workload/quiet, cardinality/latency/storage/alert calibration; operator clean-host recovery and handover | Open: prepared template is not a live manifest or rollout result |

## Interpretation and findings

- The earlier proposal-only Claude review is complete. Its unaccepted recommendations
  (including inferred completion from height and numeric width for products) do not
  override the final proposal decisions.
- Native attempt ownership already uses generation plus signed post hash. Telemetry
  wraps that identity with the boot epoch; it must not introduce a second owner.
- `OptimisticallyIncluded::update` publishes a cloned state only after validation.
  Emitting from a fallible mutation closure could report a transition that never
  committed. Capture after successful publication and allocate sequence while its
  existing ownership is held; serialize and perform I/O after lock release.
- Native dispatch targets are future block number/timestamp pairs, without a
  known block hash. The event contract preserves those fields instead of demanding
  or fabricating a canonical target hash.
- Composer/follower derive a conservative remaining budget from the pinned
  Reth runner's default grace duration and a launcher-drop timestamp preceding
  its drain timer. After drain/teardown, trace flushing waits at most
  `min(2 seconds, remaining / 4)`; errors, no launch or an exhausted deadline
  permit zero wait. Deterministic boundary tests and the combined native startup
  canaries cover this wrapper. Lifecycle shutdown still signals without waiting.
  The signer preserves `wait_until_idle`; because its application has no finite
  total shutdown deadline, it then uses an explicit bounded two-second telemetry
  allowance. Finite-deadline callers retain the existing remaining-budget rule.
  Actual reachable/stalled HTTP shutdown tests cover this interpretation in
  [signer shutdown evidence](validation/signer-trace-shutdown.md) and
  [the trace contract](contracts/tracing.md).
- The transport shutdown review found a send admitted before stop could arrive
  after the writer's final empty-queue check. In-flight send reservations now fence
  the final drain; a deterministic regression covers that interleaving. See
  [native startup evidence](validation/native-startup.md) for the complete checks.
- The existing queue RPC ages use wall time and committed-only progress. Preserve
  that compatible API; new native durations use monotonic time, and canonical
  terminal progress requires the observer's validated evidence.
- Filesystem retention is not a physical quota. The deployment must provision the
  separate hard volume budget and test readers that lag across unlink/rotation.
- Real host allocation, operator/independent heartbeat receiver config paths, and
  seven days of baseline observations are still needed for R30. Isolated tests do
  not substitute for this production evidence.

## Scope boundaries

No public dashboard, continuous funded Chiado canary, zero-loss admission journal,
independent replay verifier, protocol encoding change, or queue durability change.
These exclusions are in the proposal; they do not defer any row above.

- Canonical scanner selection is persisted alongside bootstrap. Changing either in
  place is rejected, because a wider selection without rescan would silently
  understate historical coverage. A new namespace is the conservative rescan path.
- Concurrent scanner processes may each win a different chain; both may report a
  retryable cursor mismatch. Tests assert no duplicate charge and successful
  recovery, not that a particular whole process wins both independent workers.
- The PostgreSQL adapter currently requires Unix sockets or explicit private TCP
  transport; the packaged cross-host path is an authenticated tunnel. Requested
  TLS modes fail explicitly instead of being ignored. See runbooks/database.md.

- Response byte limits alone do not bound nested ABI decoder allocations. The
  observer now checks contiguous token layouts and array bounds using the existing
  generated ABI token types before decoding; noncanonical/unsupported layouts
  retain paid costs with unverified method coverage. No signing/protocol decoder
  behavior changed. Review allocation amplification in other raw ABI consumers as
  a separate defensive follow-up.
- The signer captured-anchor fixture intentionally uses the old `0xcafef125`
  selector. The current protocol uses `0xe4a480e4`; method coverage must state its
  supported ABI era instead of silently reinterpreting old calldata.

- Main alignment is a continuing user requirement. On 2026-09-16, refreshed
  public origin/main over HTTPS (the session's SSH key was unavailable) and merged
  `5469c88860408daf776ee812156dd280c1a9e262` into this branch as `5cbdd02`.
  There were no conflicts, uncommitted work was byte-for-byte preserved, and the
  protocol gitlink stayed `3c0cdf4955941a5c23639346932b55b33babebbd`. The user
  subsequently confirmed main will remain unchanged and explicitly requested no
  periodic checks. Keep `5469c88` as the baseline without polling. The new root
  AGENTS.md applies; subsequent builds/checks use Rust 1.94.0. Earlier
  evidence remains tied to its recorded revision/toolchain.

- Lifecycle segment enumeration is deterministic, not causal. Random filename
  order and writer UTC timestamps cannot establish event publication order or
  restart causality. The importer must use retained epoch evidence/lock liveness,
  persist offsets atomically with chunks, and rotate work fairly across known
  segments. An incomplete inventory never proves retention loss.

- Unknown lifecycle schema evidence is quarantined, not declared irrecoverably
  lost merely because its writer ended. It may explain an apparent sequence hole.
  File enumeration errors do not prove retention loss, and missing locks do not
  prove writer death. The schema-4 process suite exercises these distinctions.
- Lifecycle event history is complete transport only. Native assurance/outcome
  fields remain claims until R19 checks their specific canonical anchors and
  membership. A complete event is not a completed cross-chain request.

- Native reconciliation retains `UserLocation::Terminal.canonical` as the checked
  L1 snapshot (`as_of`), not necessarily the posting receipt's inclusion block.
  Outcome capture must preserve that distinction and use the actual source/post
  receipt references and applied endpoint available in `reconcile_attempt`.
  Copying the snapshot into a settlement-anchor field would overstate evidence.
- `replace_prepared_post_batch` can change the signed hash within the same native
  generation before dispatch. Attempt capture now records explicit variant links
  with membership retained through the original prepared event. A changed candidate
  is not another paid receipt; submission observations still make no settled-source
  claim. Canonical reconciliation needs its separate receipt/endpoint evidence.

- R19 evidence prerequisite: [protocol receipt logs](contracts/protocol-logs.md)
  retain exact root/consumption occurrences with bounded capture, immutable claims,
  explicit conflicts and canonical retraction. This does not complete R19 outcome
  materialization or source/companion attribution.

- R11/R19 association prerequisite: [posting verification tables](contracts/posting-rollups.md)
  retain the declared rollups in exact wire order, separately from receipt success.
  A successful later batch replaces queues only for its listed rollups; an unrelated
  post is not automatically a boundary for a source's applied effects.

- R19 local history: [admission status views](contracts/admission-status.md) distinguish
  positively ended epochs from unknown liveness, preserve actual readmission IDs,
  and validate local-policy parent/child evidence. These local-history checks alone
  leave canonical terminal claims unresolved; schema 11 adds the separate
  [canonical anchor checks](contracts/canonical-outcomes.md).

- R10/R11/R19 source recovery: schema 9 extracts explicit scoped source objects
  from retained readiness, terminal, preparation, attempt and reconciliation
  members as well as admissions. The receipt worker projects 64 distinct
  event/source references per producer/pass; actual admission history remains
  separate. See [the source receipt contract](contracts/source-receipts.md) for
  coverage, compatibility and limits. SQL extraction/migration checks, the real
  importer/lookup process case and saved-v8 upgrade lane passed in
  [the recovery validation](validation/source-references.md). This does not establish companion
  allocation or canonical cross-chain assurance.

- R19 native coherence: [native outcome projection](contracts/native-outcomes.md) binds checks
  to complete transport digests and invalidates late conflicting/extra lineage evidence.
  The separate [schema-11 anchor consumer](contracts/canonical-outcomes.md) now
  validates available chain evidence and selects canonical outcomes/progress.
  Native coherence alone still has `unknown` canonical assurance.

## Acceptance coverage follow-up

The checkpoint review found missing network/infrastructure panels and separate
blocked/serviceable timing. The [network contract](contracts/network-health.md)
now specifies optional bounded safe-head/explorer workers, exact finality age,
verified client scopes and explicit unsupported capabilities. Infrastructure
panels cover CPU, network, I/O latency, peers, consensus sync and observed process
restarts; the separate client/host selector preserves observer identity.
[Native metrics](contracts/native-metrics.md#blocked-serviceable-and-unknown-time)
now partition observed waiting time without inventing missing history or altering
queue RPC/lifecycle formats. The updated [coverage record](validation/coverage-followup.json)
keeps local checks, earlier failures and remaining acceptance gates separate.
Polling-only collection is an explicit architecture deviation described in the
network contract; subscriptions are not claimed as implemented.

The [release validation record](validation/release-runtime.md) adds default-feature
release binaries, 1,194 archived unit tests, 64 PostgreSQL groups, full Kurtosis
coverage and physical ENOSPC evidence. It retains the original native timeout and
retry-only failures, their diagnoses, and the status of corrected test oracles.
These results do not close R27 overhead or the deployment/operations gates.

- The failed release load pilot exposed observer capacity gaps. The
  [capacity follow-up](validation/observer-capacity.md) records schema-21 query
  improvements, representative native PostgreSQL storage, bounded backlog
  scheduling and failure capture. Its synthetic SQL regression, 58 database check groups, returned-error cleanup
  regression and bidirectional native observer canary pass. Release load and
  comparative overhead acceptance remain open and are tracked there.
