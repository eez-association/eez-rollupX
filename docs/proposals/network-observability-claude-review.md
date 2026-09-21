Review provenance: requested by the user and run through Claude Code 2.1.273 on 2026-09-16. The successful invocation had all tools disabled and received only the proposal text. It did not receive repository source files or inspect a deployment. CLI-reported models: claude-haiku-4-5-20251001, claude-fable-5-1. Reviewed input SHA-256: `1a867f23f022274dd053e5343c6c1ca1dde354307b7849a32facb1aac4042edf`. The review below is preserved verbatim; its recommendations are not all accepted. See [dispositions](network-observability-review-disposition.md) and the [revised proposal](network-observability.md).

---

# Review: EEZ network observability proposal (draft 2026-09-16)

Scope of this review: the supplied document only. I have not inspected the repository, the pinned Reth/Lighthouse builds, Grafana/Loki/Tempo/Alloy behavior, or any deployment. Where a finding depends on code or vendor behavior, it is marked **hypothesis** and must be validated in WP0; findings marked **definite** are internal inconsistencies or gaps in the document itself.

---

## 1. Implementation-readiness verdict

**Overall:** the architecture and accounting rules are sound and unusually careful about unknown/stale semantics. The document is ready to start **WP0 and the stage-1 stack (WP2)** now. It is **not** ready to start WP3/WP4 as written, because the lifecycle-event delivery path, the canonical-completion evidence model, and the observer's stage placement are internally inconsistent or undefined.

**Can start now**
- WP0 (integration investigation, contracts), with the additional decisions in §3 folded into its deliverables.
- WP2 stack layout, provisioning, dead-man check, external probe, overview/infrastructure dashboards.
- An RPC-exporter-only observer for queue/head/freshness metrics (currently mis-placed in stage-2 WP3 — see C01).
- The `eez-telemetry` crate limited to identity conventions, a private Prometheus endpoint per process, and freshness gauges.

**Needs decisions first**
- Lifecycle event transport and its guarantee statement (C02).
- Metrics recorder ownership, final metric names, histogram bucket ownership (C03).
- Per-direction canonical completion evidence derivable from chain data (C04).
- Splitting WP3 into stage-1 exporter and stage-2 ledger (C01), and shrinking stage-1 WP1 (C13).

**Estimate:** 12–19 engineer-days is optimistic for seven instrumented crates, a reorg-safe indexer, six dashboards, and ten fault drills. The document already says to re-baseline after WP0; I recommend that re-baseline be a hard gate and that stage 1 be frozen to the MVP in §4.

---

## 2. Prioritized findings

### C01 — WP3 stage placement contradicts its own stage-1 role
- **Severity:** High · **Definite**
- **Where:** Delivery plan table, WP table, WP3 text.
- **Failure scenario:** Stage 1 lists a "queue/RPC exporter" deliverable and WP2 acceptance requires "unavailable queue data displays unknown," yet the only component that exports queue-RPC metrics is WP3, placed in stage 2 with a dependency on WP2 PostgreSQL. A sequential implementer either builds an ad-hoc exporter outside any work package or cannot pass WP2 acceptance.
- **Smallest correction:** Split into **WP3a** (stage 1, no database: `eez-observer` in exporter-only mode polling queue/correlation/head RPCs, exposing freshness) and **WP3b** (stage 2: ledger, migrations, reorg reconciliation). WP2 live acceptance depends on WP3a, not WP1.
- **Blocks:** Stage 1.

### C02 — Lifecycle stream guarantee is self-contradictory and routed through the lossiest path
- **Severity:** High · **Definite** (contradiction) + **hypothesis** (vendor behavior)
- **Where:** WP4; Architecture (Alloy "bounded buffers"); requirement 3; WP2 (Docker `local` driver).
- **Failure scenario:** WP4 requires "unsampled" events and "never apply diagnostic log dropping," while the architecture mandates Alloy with bounded buffers and resource limits, Loki has finite retention and ingestion limits, and the log path may traverse Docker's `local` driver (which is not file-tailable and requires Docker API/socket privileges to read). Under Loki 429s or Alloy queue overflow, lifecycle events are silently dropped; the observer's Loki query with an overlap window then sees no gap unless every event carries a sequence number and the observer checks contiguity. Additionally, Loki as a replay source has per-query entry limits, stream-level dedup of identical (timestamp, line) pairs, and ordering constraints the document defers to "verify" — a long chain of vendor behaviors for the one stream that accounting depends on.
- **Smallest correction:**
  1. Restate the guarantee: *no intentional sampling or filtering; involuntary loss is possible and is detected via (deployment, process epoch, sequence) gaps; loss beyond local retention is a recorded `collection_gaps` row.*
  2. Make the **default transport a dedicated local JSON-lines file** written by a non-blocking appender with a drop counter (`eez_telemetry_dropped_events_total{signal="lifecycle"}`), size/time-rotated, bind-mounted, and tailed **directly by the observer** with a persisted file/offset checkpoint. Send the same file to Loki via Alloy for search only.
  3. Keep the Loki-query adapter as fallback only if the observer cannot reach the node host's filesystem.
- **Blocks:** Stage 2 (WP4). Also resolves the WP2 diagnostic-log path: application writes JSON to files, Alloy tails files; no Docker socket.

### C03 — Recorder ownership determines prefix, bucket configuration, and cross-process name consistency
- **Severity:** High · **Hypothesis** (requires code validation)
- **Where:** Requirement 2, 5; WP1 metric table ("WP0 must resolve the effective Reth prefix").
- **Failure scenario:** If the node process uses Reth's already-installed global recorder, then (a) EEZ metrics may be exported with Reth's prefix while the signer/observer processes, which install their own recorder, export unprefixed names — the same logical metric has two names and dashboards need per-process queries; (b) histogram bucket configuration is typically set on the exporter *builder* before install, so EEZ code cannot register `eez_stage_duration_seconds` buckets after Reth has installed the recorder; the metric may be exposed as summary quantiles, which cannot be aggregated across instances (contradicting requirement 5's "aggregatable histograms").
- **Smallest correction:** Make WP0 produce a binary decision with evidence: **(A)** EEZ installs the Prometheus recorder first, configured with bucket sets for both `reth_*` and `eez_*` histograms, and Reth's install path tolerates a pre-installed recorder; or **(B)** Reth's recorder is used as-is, EEZ metric names include the observed prefix, the contract publishes per-process names, and stage-duration histograms are deferred until (A) is achieved or a separate EEZ endpoint is added. Do not let WP1 start until this is chosen. Prometheus `metric_relabel_configs` may normalize names but not bucket semantics.
- **Blocks:** Stage 1 (endpoint and names); histogram part blocks stage 2.

### C04 — Canonical completion evidence per direction is undefined
- **Severity:** High · **Definite** (gap) + **hypothesis** (protocol capability)
- **Where:** Accounting rules; WP3 ("reconcile canonical outcomes from chain data"); WP4 ("reconcile known requests against available RPC/chain evidence").
- **Failure scenario:** The document promises that historical accepted requests without a durable terminal observation become unresolved rather than silently completed, and that chain indexing can backfill canonical data. But it never states what on-chain evidence constitutes "completed" for an L1→L2 request, an L2→L1 request, a source-only completion, or an expected revert. If the only completion signal is the node's in-memory `eez_getCrossChainTransaction`, then after a node restart every in-flight request stays unresolved forever even when the chain shows it completed — or, worse, the observer pulls completion from a source that forgot the request and marks it unknown while the operator's dashboard shows canonical activity.
- **Smallest correction:** WP0 contract gains a table: *direction × outcome → canonical evidence (contract event / receipt field / correlation RPC) × whether it is derivable without node memory × provenance tag stored in `lifecycle_events`.* Any outcome not derivable from chain data is labeled "node-attested only" in dashboards.
- **Blocks:** Stage 2 (WP3b/WP4 acceptance "unknown outcomes stay unresolved" cannot be tested without this).

### C05 — `eez_queue_transactions{state}` uses overlapping subsets
- **Severity:** Medium · **Definite**
- **Where:** WP1 metric table.
- **Failure scenario:** The note says ready/blocked/unknown are subsets of queued. Any panel or alert using `sum by (direction)(eez_queue_transactions)` — the natural PromQL — overcounts, and a stage-1 "total queued" panel disagrees with the RPC's own total.
- **Smallest correction:** Make `state` a disjoint partition (e.g. `queued_ready`, `queued_blocked`, `queued_unknown`, `in_flight`) and document that totals are sums. If the RPC exposes only overlapping counts, export them under distinct metric names (`eez_queue_ready_transactions`), not one label.
- **Blocks:** Stage 1.

### C06 — Receipt identity rule conflicts between accounting rule 1 and WP3 tables
- **Severity:** Medium · **Definite**
- **Where:** Accounting rule 1 ("index each receipt once using chain identity and transaction hash") vs WP3 (`receipt_observations`: "separate receipt per inclusion block").
- **Failure scenario:** Implementer chooses the rule-1 key, a reorg reincludes a transaction in a new block, and the upsert overwrites the orphaned receipt history — violating WP3's "preserve old inclusion observations."
- **Smallest correction:** State both keys explicitly: logical transaction = `(chain_instance_id, tx_hash)`; receipt observation = `(chain_instance_id, tx_hash, block_hash)`; canonical cost view joins observations to blocks where `canonical = true`, which yields at most one row per transaction.
- **Blocks:** Stage 2.

### C07 — No attempt-to-submission mapping for fee bumps/replacements
- **Severity:** Medium · **Definite** (gap)
- **Where:** WP3 tables; accounting rule 1.
- **Failure scenario:** One settlement attempt produces several L1 transaction hashes (replacement/fee bump). `settlements` references "settlement receipt reference" singularly; if the first-submitted hash is stored and a replacement is what lands, the attempt shows no canonical settlement while cost appears under an unlinked transaction, and "attempts versus canonical batches" panels drift.
- **Smallest correction:** Add `attempt_submissions (attempt_id, chain_instance_id, tx_hash, nonce, submitted_at, evidence_source)` with unique `(chain_instance_id, tx_hash)`; canonical settlement = the submission whose receipt observation is canonical.
- **Blocks:** Stage 2.

### C08 — Blob byte measurement depends on a best-effort stream when a replayable source exists
- **Severity:** Medium · **Definite** (design choice) + **hypothesis** (codec decodability)
- **Where:** Accounting rules 5–6; `blob_measurements` ("known encoded bytes and measurement coverage").
- **Failure scenario:** Encoded stream bytes come only from composer lifecycle events. Any lifecycle gap (C02) becomes a permanent utilization coverage gap even though the canonical blob content is retrievable and deterministic. Utilization coverage then tracks log-transport health, not data availability.
- **Smallest correction:** Primary byte measurement = decode the canonical blob(s) with the versioned codec at index time (within the Beacon blob retention window, else from the archive); lifecycle-reported bytes are a cross-check that flags codec/version disagreement. This also makes "purchased blobs for those same posts" trivially the same population as the measured bytes.
- **Blocks:** Stage 2. Note the Beacon retention constraint (see §5): observer downtime longer than the blob retention window makes the archive the only source.

### C09 — Sequence/epoch assignment point is unspecified
- **Severity:** Medium · **Definite** (gap)
- **Where:** Requirement 3; WP1 ("capture bounded event fields, then publish asynchronously").
- **Failure scenario:** Sequence numbers assigned in the asynchronous publisher after lock release can be assigned out of transition order; the observer's gap detection produces false gaps, and timeline reconstruction orders a "completed" before its "admitted."
- **Smallest correction:** Assign `seq` from an atomic counter **at the transition, while the existing state ownership rule holds** (an atomic increment is the only work done under the lock); the async publisher must not reassign. Define `epoch` (recommend UUIDv7 or process-start UTC nanos) once per process and emit it in `process_start_time`-style metrics so the observer can correlate epochs across signals.
- **Blocks:** Stage 2.

### C10 — Node-restart rule for open admissions is implied but not specified
- **Severity:** Medium · **Definite** (gap) + **hypothesis**
- **Where:** "Queue state is currently in memory"; WP4 ("an absent queue entry is insufficient evidence of completion").
- **Failure scenario:** After a node restart the observer sees a new epoch; without a stated rule, open admissions from the prior epoch are either left `pending` indefinitely (inflating pending-age percentiles forever) or auto-closed.
- **Smallest correction:** Rule: on observing a new node epoch, every prior-epoch admission without terminal disposition transitions to `unresolved_restart` with the restart time; it may later transition to a canonical outcome via C04 evidence or to `readmitted` if the same hash is admitted again. Whether the node itself re-derives inbound requests from L1 on restart is a hypothesis WP0 must answer, because it changes whether `readmitted` is expected or anomalous.
- **Blocks:** Stage 2.

### C11 — Alert table is not stage-tagged; several rules need stage-2 producers
- **Severity:** Medium · **Definite**
- **Where:** Initial alerts table vs stage-1 deliverable "a small alert set."
- **Failure scenario:** Stage-1 acceptance "stopping a dependency produces the configured alert" is tested against an unspecified subset; rules like "ready/in-flight work exists and no canonical progress," runway, blob retrieval, and follower disagreement require WP3b/WP4/WP6 producers. Implementer either fakes them from unavailable data (which requirement 1 forbids) or leaves the stage-1 alert set undefined.
- **Smallest correction:** Add a `stage` and `producer` column to the alert table. Stage 1 = external RPC probe, L1 head stale vs reference, queue RPC unreachable (unknown), container down/restart loop, disk projection, telemetry missing, dead-man.
- **Blocks:** Stage 1.

### C12 — Metric ownership handoff by "one configuration change" is underspecified
- **Severity:** Medium · **Definite**
- **Where:** WP1 ("transfer ownership to native metrics in one configuration change").
- **Failure scenario:** The stage-1 exporter and stage-2 native code emit the same metric name from different targets; during the handoff window both are scraped, panels double, or the exporter is dropped before the native metric is verified and panels go blank.
- **Smallest correction:** Exporter metrics use a distinct namespace (`eez_observer_queue_*`); dashboards and alerts reference **recording rules** (`eez:queue_transactions:current`) whose expression is the only thing changed at handoff.
- **Blocks:** Stage 1 (naming must be set before dashboards are written).

### C13 — Stage-1 WP1 scope contradicts the stage-1 estimate
- **Severity:** Medium · **Definite**
- **Where:** WP1 (seven crates, transitions, spans) placed in stage 1 (4–6 days including WP0's 1–2 days and the full WP2 stack).
- **Failure scenario:** Either stage 1 slips substantially or instrumentation is merged without the WP1 acceptance tests (mutex hold time, duplicate global init, unbounded labels).
- **Smallest correction:** Stage-1 WP1 = crate skeleton with identity/context types, per-process private endpoint, `eez_collection_last_success_timestamp_seconds`, `eez_chain_head_number`, and build/version info. Queue transitions, attempt counters, stage durations move to WP4; spans to WP5. The "start spans, event identity and metrics together" intent is met by fixing identities in the WP0 contract, not by shipping spans in stage 1.
- **Blocks:** Stage 1.

### C14 — Shared context labels: who emits `deployment`, `chain`, `rollup`, `instance`?
- **Severity:** Medium · **Definite** (gap)
- **Where:** "Define shared context now"; "Use bounded metric labels: environment, chain, rollup, instance…".
- **Failure scenario:** The application emits `deployment`/`environment` as metric labels *and* Prometheus adds them as target labels → label conflicts (`exported_environment`), or logs/traces carry resource attributes with different values than metrics, breaking cross-store navigation.
- **Smallest correction:** Application emits only signal-intrinsic labels (`chain`, `rollup`, `direction`, `stage`, `outcome`, `reason`, `state`, `kind`, `source`). `deployment`, `environment`, `service`, `instance` are set by Prometheus target labels, by Alloy static labels for Loki, and as OTel resource attributes at process start — from one config file so values match byte-for-byte.
- **Blocks:** Stage 1.

### C15 — L2 finality and "finalized-chain contradiction" are not defined for the settled chain
- **Severity:** Low · **Definite** (gap)
- **Where:** Requirement 4; WP3 ("finality status comes from validated chain observations").
- **Failure scenario:** Observer marks L2 blocks finalized from the node's `finalized` tag; after an L1 reorg that removes a settlement, the node's L2 view rolls back and the observer raises a "finalized contradiction" critical alert for expected behavior — or never finalizes L2 at all.
- **Smallest correction:** Define: L2 block finalized ⇔ its canonical settlement receipt is in an L1 block at or below the L1 finalized head. L2 "contradiction" is only critical when the L1 block containing the settlement was itself finalized.
- **Blocks:** Stage 2.

### C16 — Backfill boundary vs data availability of the RPC sources
- **Severity:** Low · **Definite** (gap) + **hypothesis**
- **Where:** WP3 ("replayable from the configured deployment boundary while the source retains the data"); WP7 ("record which historical ranges can actually be rebuilt").
- **Failure scenario:** Bootstrap block set at deployment genesis, but the L1 RPC used for accounting has pruned receipts/logs before some height, and Beacon blob content is only retained for a bounded window. Backfill silently produces partial totals labeled as complete.
- **Smallest correction:** WP3b startup verifies receipt/log availability at the bootstrap block and records `collection_gaps` rows for any unavailable range before the first aggregate view is published; blob byte coverage explicitly distinguishes "not yet measured" from "source pruned."
- **Blocks:** Stage 2.

### C17 — Exporter shutdown flush vs node shutdown deadline
- **Severity:** Low · **Definite** (gap)
- **Where:** WP1 ("shutdown within its existing deadline"); WP5 ("bounded shutdown flushing").
- **Failure scenario:** OTLP batch exporter flush timeout defaults exceed the node's shutdown deadline while the collector is unreachable; shutdown is delayed or killed mid-flush.
- **Smallest correction:** Require exporter flush timeout ≤ 25% of the node shutdown deadline, set explicitly in config, and include "collector unreachable during shutdown" in the WP6 drill table.
- **Blocks:** Stage 2 (WP5).

### C18 — Scope items with no stage-1/2 benefit
- **Severity:** Low · **Definite**
- **Where:** "limited public status view"; SigNoz alternative paragraph; Grafana public/viewer roles.
- **Failure scenario:** Public dashboards on a PostgreSQL datasource expose query surface and require a hardening pass that competes with the accounting work.
- **Smallest correction:** Move the public status view to a post-v1 extension alongside Chiado canaries; delete the SigNoz paragraph from the implementation plan (keep it in a decision log).
- **Blocks:** None (stage 3 scope reduction).

---

## 3. Decisions the spec should make now (with recommended defaults)

| ID | Decision | Recommended default |
|---|---|---|
| D1 | Lifecycle event transport | Dedicated local JSON-lines file, non-blocking appender with drop counter, rotated; observer tails directly with offset checkpoint; Loki receives a copy for search only (C02) |
| D2 | Guarantee statement | "No intentional sampling; involuntary loss detected by (deployment, epoch, seq) gaps; loss beyond local rotation is a recorded gap" |
| D3 | Epoch and sequence | `epoch` = UUIDv7 per process start; `seq` = atomic counter incremented at the transition under existing ownership (C09) |
| D4 | Recorder ownership | WP0 chooses A (EEZ-installed recorder with buckets) or B (Reth's recorder, prefixed names, no aggregatable histograms in v1); dashboards use the published names only (C03) |
| D5 | Stage-1 exporter namespace | `eez_observer_*`; dashboards/alerts read recording rules `eez:*:current` (C12) |
| D6 | Label ownership | App: intrinsic labels only; `deployment/environment/service/instance` from scrape config, Alloy static labels, OTel resource — one source file (C14) |
| D7 | Queue state partition | Disjoint `state` values; totals by sum (C05) |
| D8 | SQL identities and types | tx `(chain_instance_id, tx_hash)`; receipt `(…, block_hash)`; `attempt_submissions`; `NUMERIC(78,0)` for wei/gas and products; `BYTEA` for hashes; all foreign keys enforced (C06, C07) |
| D9 | L2 finality | Settled in an L1 block ≤ L1 finalized head (C15) |
| D10 | Canonical completion evidence | Per-direction table produced in WP0; outcomes not derivable from chain are tagged "node-attested" (C04) |
| D11 | Node restart handling | Prior-epoch open admissions → `unresolved_restart`; may resolve via chain evidence or `readmitted` (C10) |
| D12 | Blob bytes source | Decode canonical blob with versioned codec at index time; lifecycle bytes as cross-check (C08) |
| D13 | RPC sources | Accounting from the deployment's own L1/L2 RPC over private network; one external Chiado RPC for reference head only; observer verifies chainId + genesis hash at startup and refuses to write on mismatch |
| D14 | Migrations | `eez-observer migrate` administrative subcommand; service refuses to start on schema version mismatch; forward-only migrations |
| D15 | Stage-1 alert set | External RPC probe, L1 head stale vs reference, queue RPC unknown, container down, disk projection, telemetry missing, dead-man (C11) |
| D16 | Placement | Grafana, Prometheus, Loki, PostgreSQL, observer on the monitoring host; node host exposes metrics/RPC/log files over a private network or tunnel |
| D17 | Purchased-blob population | Blobs of canonically included settlement transactions regardless of revert status (blob gas is paid on revert); orphaned attempts excluded |
| D18 | Exporter timeouts | OTLP flush ≤ 25% of node shutdown deadline; Prometheus scrape timeout < interval (C17) |

---

## 4. Minimum viable first release and acceptance tests

**Contents (stage 1, frozen):**
- WP0 outputs: capability matrix, `contracts/metrics.md` with final names per process, per-direction completion-evidence table (D10), recorder decision (D4).
- WP2 stack: Grafana, Prometheus, Loki, Alloy (file tailing), PostgreSQL provisioned but unused by dashboards; no Tempo profile enabled.
- WP3a exporter-only observer: queue RPC aggregates, correlation/head RPCs, `eez_observer_collection_last_success_timestamp_seconds`, identity check.
- WP1-minimal: private Prometheus endpoint per process (node, signer), build info, head gauges, JSON log files.
- Two dashboards: overview, infrastructure. Aggregate queue panels labeled "aggregate (RPC)".
- D15 alert set, one Grafana Alerting route, independent dead-man receiver, external RPC probe.

**Acceptance tests (devnet):**
1. Clean host: documented commands bring up the stack; provisioned dashboards and data sources load; all panels show either data or **unknown** (no zeros from absent series).
2. Stop the signer container → infrastructure alert fires within 2 evaluation intervals; restart → recovery notification received.
3. Stop the node RPC → queue panels show **unknown** with last-success timestamp; `eez_observer_collection_last_success_timestamp_seconds{source="queue_rpc"}` stops advancing; "queue RPC unknown" alert fires.
4. Stop the entire monitoring stack → dead-man alert arrives on the independent receiver within its configured window.
5. Block the external probe's path to the public RPC → availability alert fires with probe context.
6. Restart Grafana, Prometheus, Loki, PostgreSQL containers → dashboards, alert state history, and retained metrics survive.
7. Scrape both node and signer endpoints: metric names for shared logical metrics are identical across processes, or the contract explicitly documents the per-process difference (D4).
8. `sum(eez_observer_queue_transactions)` equals the RPC's reported total in a controlled workload (C05).
9. Node exports metrics and logs with telemetry endpoints unreachable; settlement results for a fixed devnet workload are byte-identical to a telemetry-disabled run.
10. No exported label or JSON field contains a transaction hash, address, key material, or raw signed transaction (grep against the sanitized capture).

Everything else — ledger, lifecycle history, protocol dashboards, traces, follower checks — is stage 2/3 and must not be partially represented in stage-1 panels.

---

## 5. Explicit unverified assumptions

These are stated or implied by the proposal and require code or vendor validation; none should be treated as established.

1. Reth's global metrics recorder can be pre-empted or configured by EEZ, and its install path is idempotent or tolerant of a pre-installed recorder (C03).
2. The pinned exporter emits Prometheus histograms (not summaries) for `metrics` histograms, and bucket sets can be configured for EEZ metrics (C03).
3. Embedded L1 and L2 Reth metrics cannot be separated by label; the proposal's fallback (process-wide display) is assumed acceptable to operators.
4. Cross-chain completion for each direction has on-chain evidence (events/receipts) that the observer can read without node memory (C04). If not, post-restart reconciliation is impossible for some outcomes.
5. The node does or does not re-admit inbound requests from L1 after restart (C10).
6. The codec used for blob encoding is available to the observer as a library and deterministic decode recovers encoded stream length (C08). The 126,976-byte usable capacity applies to the pinned codec version.
7. The accounting RPC retains receipts and logs back to the chosen bootstrap block; Beacon blob retention covers the observer's expected downtime, otherwise the archive (Blobscan) is the sole source and its own freshness becomes an accounting dependency (C16).
8. `tracing-appender`-style non-blocking file writing with a drop counter is compatible with the existing Reth subscriber layering (D1).
9. Composer-to-signer gRPC metadata can carry W3C trace context without touching signed payloads or validation logic.
10. Loki query pagination, timestamp-tie behavior, and out-of-order handling for the pinned version — only relevant if D1's file-tail default is rejected.
11. The `eez-rollup-ui` collector has reusable canonical reconciliation logic; the proposal correctly does not depend on it, but the estimate impact of "not reusable" is unmeasured.
12. Devnet overhead measurements (≤5% throughput/p95 regression) are representative of Chiado load profiles.
13. Chiado L1 reorg depth stays within the finalized-head distance so that "finalized contradiction" is a genuine anomaly rather than an operational event.
14. Grafana Alerting on a single instance provides sufficient delivery reliability for paging when paired with the independent dead-man check; no Alertmanager HA is assumed.
