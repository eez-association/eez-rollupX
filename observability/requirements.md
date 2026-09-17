# Implementation and acceptance ledger

Source of truth: [proposal](../docs/proposals/network-observability.md), including
its final implementation decisions and [review dispositions](../docs/proposals/network-observability-review-disposition.md).
`Verified` below means the named evidence exists, not production acceptance.
An open row is a release gate, not an implicitly deferred requirement.

| ID | Requirement / invariant | Implementation and configuration | Tests / evidence / documentation | Status |
| --- | --- | --- | --- | --- |
| R01 | One identity source; bounded labels; no secrets or signed pending envelopes | Exporter config, stack prepare including native_processes, typed lifecycle schema | Observer/native config tests, prepare tests; contracts/metrics.md and native-startup.md | Partial: log/trace adapters pending |
| R02 | Existing configurations and protocol behavior unchanged by disabled telemetry | Optional EEZ_TELEMETRY_CONFIG, entrypoint wiring, CLI precedence | Native startup tests; source-built bidirectional enabled/disabled canaries and legacy RPC regression in validation/native-lifecycle.md | Partial: admission/readiness comparison passed; later instrumentation and failure modes still need comparison |
| R03 | One Reth recorder/subscriber; verified prefix, histogram buckets, embedded attribution | Pinned recorder builder hook; standalone signer exporter; original subscribers | Bucket/prefix and singleton tests; actual devnet metrics sample pending | Partial |
| R04 | Read-only exporter; chain/genesis checked each poll; bounded calls/backoff | crates/eez-observer, config.example.json | Nine Rust tests, smoke-exporter.py; validation/initial-slice.md | Verified |
| R05 | Disjoint queue counts; stale/failed/null means unknown; one recording-rule producer | Observer model/metrics; prometheus/recording-rules.yaml | Rust freshness/partition tests; rules.test.yaml, smoke-stack.py | Verified |
| R06 | Private pinned self-hosted stack, resource/retention budgets, persistent stores | compose.yaml, images.lock.json, prepare.py | Isolated stack run; validation/backend-stack.md | Partial: node-host agent packaging and trace profile pending; private PostgreSQL profile tested |
| R07 | First two dashboards, explicit no-data/error alerts, recovery and independent heartbeat | Grafana provisioning and network/infrastructure dashboards | smoke-stack.py exercises rules and local receivers | Partial: actual independent host/receivers pending |
| R08 | Canonical scanner identity, bootstrap, parent-linked consistent fetch, atomic cursor | Observer indexer/ledger; explicit immutable selection and bootstrap | validation/database.md: restart, killed-before-commit, concurrent cursors, mid-fetch fork, history-gap recovery | Verified for receipt scanner; large/deep backfill performance remains R27/R30 |
| R09 | Preserve orphan/reinclusion history; finalized contradiction freezes affected views | SQL blocks/receipts/cursor model; L1 anchor validation; source_valid gating | Real same-height fork, reinclusion, finality freeze tests in smoke-database.py | Implemented: exact applied L2 hashes plus finalized L1 anchors; schema-3 fixture verification recorded in database evidence |
| R10 | Exact gas/blob charges, canonical reverts, missing fields gaps; no join multiplication | Observer accounting parser and canonical SQL receipt/total/coverage views | validation/receipt-accounting.md: full saved audit, overflow/missing/revert fixtures | Partial: durable canonical selection verified; settlement/source joins pending |
| R11 | Wallet spend, settlement/companion attribution and L2 fees distinct | Wallet views and current-ABI decoded posting view; method coverage and bounded backfill | SQL/RPC wallet/post/revert/invalid/unverified separation; nested ABI and allocation-bound tests | Partial: applied-range evidence implemented; companion/source allocation pending |
| R12 | Authenticated/versioned blob length; measured-subset weighted utilization and coverage | Codec exact boundary, bounded retrieval, measurement states | Terminator/trailing-zero/authentication/pruning/revert fixtures | Open |
| R13 | Explicit migrations, schema compatibility, private writer/read-only viewer roles | migrate subcommand, append-only migration history, private opt-in PostgreSQL profile and generated role credentials | Real schema/checksum/downgrade refusal, no implicit DDL, reader authentication/permissions, persistence | Partial: populated forward upgrade and old-binary refusal tested; clean restore pending |
| R14 | Typed versioned events; epoch/sequence at owned transition; many-to-many membership | eez-telemetry schema; contracts/lifecycle-v1.md and native-lifecycle.md | Chunk/schema fixtures; Composer component regressions; validation/native-attempts.md and native-preparation.md: source-built legacy/policy/disabled canaries | Partial: admission/readiness, preparation identity/membership/cancellation and attempt/repricing/dispatch verified in their stated scopes; canonical outcomes pending |
| R15 | Bounded nonblocking lifecycle writer independent of logging/sampling | eez-telemetry spool; 4096-record channel, 16 KiB records, 64 MiB segments, 24h/2 GiB retention | validation/lifecycle-transport.md: overflow, ENOSPC, rotation, shutdown, concurrency; validation/native-lifecycle.md: real native capture | Partial: basic native workload passed; hard-volume/performance gates remain |
| R16 | Chunk completion+digest, immutable segment IDs, independent control/drop records | eez-telemetry wire/spool | Missing/reordered/conflicting chunks, bypassed-full-channel control tests | Verified at transport/SQL layer; native capture still R14 |
| R17 | Instance/epoch fencing and headers; explicit ended/unknown liveness | OS advisory locks; read-only probe_epoch | Collision and restart-without-traffic tests | Verified in isolated writer-crash/probe/SQL fixtures; native restart workload remains R19/R27 |
| R18 | Transactional file offsets, dedup/quarantine, provisional/suspected/recoverable gaps | Independent configured source workers, schema-4 chunk/control/raw evidence and atomic checkpoints | Real importer-kill rollback, writer crash, late chunks, quarantine, rotation, retention, concurrent replay and full-u64 interval tests | Implemented and fixture-verified; workload capacity remains R27/R30 |
| R19 | Restart admissions unresolved; exact assurance and reorg-retractable outcomes | Native transitions, observer evidence/anchor validation | Outcome matrix, source-only receipt vs cross-chain claim, readmission tests | Open |
| R20 | Monotonic native ages/stages; direction and outcome counts, pending distributions | Held-pool/composer/signer instrumentation | Queue/attempt regressions, histogram output; completed vs pending tests | Open |
| R21 | Four protocol dashboards with coverage, links, policy/restart annotations | Cross-chain/settlement/blob/gas dashboards and SQL views | All dashboard queries on audit fixtures; missing/expired trace links | Open |
| R22 | Stage-2 progress/age/funding/blob/signer/index-gap alerts | Validated native/observer producers, Grafana rules/runbooks | Failure/recovery and upstream suppression fixtures | Open |
| R23 | Finite OTel spans; W3C gRPC context; batch/retry links; no signed-wire change | Existing subscriber layers, prover client/server metadata | Real connected composer/signer trace, invalid context, unchanged wire tests | Open |
| R24 | Bounded tracing, explicit sampling/drops and post-drain flush budget | Alloy/Tempo profile (7d), exporter queues/timeouts | Collector outage/overload/shutdown, receipt persistence after expiry | Open |
| R25 | Optional follower compares exact settled block/root; disabled/lag unknown | Independent follower observer capability | Agreement/disagreement/lag/reorg tests | Open |
| R26 | Devnet bidirectional canaries check semantic effects; quiet is unexercised | Existing devnet/Kurtosis infrastructure; shared enabled/disabled cross_chain canary | Both directions, destination values, safe-state convergence, recomputed proof signature and native lifecycle identity passed | Partial: mixed fault workloads and dashboard quiet/unexercised presentation remain |
| R27 | Telemetry failures never block protocol; bounded physical spool storage | Native queue API, separate 3 GiB volume, bounded reader descriptors | Collector/disk-full and lagging-reader drills; overhead <5% provisional | Open |
| R28 | CI validates affected crates/features and backend queries | .github/workflows/observability.yml | Observer/telemetry tests, lint, stack regression and database recovery run locally; database suite added to CI | Partial |
| R29 | Clean-host restore including SQL/config/contact points; measured RPO/RTO | Backup/upgrade/rollback tooling and operations runbook | Separate-instance restore and rescan; container persistence already tested | Partial |
| R30 | Real rollout manifest, owner, endpoints, bootstrap, private network, capped backfill | Deployment inputs pending; preserve chain data/public routes | Seven-day baseline, workload/quiet, cardinality/latency/storage/alert calibration | Open |

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
- Existing entrypoint drains do not expose a remaining shutdown budget. The
  lifecycle startup foundation signals shutdown without waiting after those
  drains; tracing integration still needs a measured budget-aware flush hook.
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
