# Observability review dispositions

2026-09-16. This records Codex's assessment of the [Claude review](network-observability-claude-review.md) and changes incorporated into the [implementation proposal](network-observability.md). Claude reviewed the document only. The source checks below were performed locally by Codex; no live deployment or tests were run for this document review.

The first network attempt failed with DNS resolution; automatic approval review rejected a subsequent source-access invocation because it could transmit repository source. The successful narrowed invocation transmitted only the user-requested proposal, with every Claude tool disabled and no repository source-file access.

| Finding | Disposition | Concrete specification change |
| --- | --- | --- |
| C01: exporter assigned to the wrong stage | Accepted | Split WP3a database-free exporter from WP3b canonical indexing; WP2 live acceptance depends on WP3a |
| C02: lifecycle delivery and Loki ingestion | Accepted with qualification | Dedicated bounded non-blocking JSONL writer, local observer tail with transactional checkpoints, Loki copy for search; colocate observer with files, not on a remote host expecting a filesystem mount. No intentional sampling does not promise losslessness |
| C03: recorder naming/buckets/startup | Accepted; locally corroborated | Use the pinned Reth singleton's supported builder hook if integration proves safe; configure buckets before install; RPC/process-wide fallback means stage 1 can start without upstream changes |
| C04: completion evidence undefined | Accepted | Add a per-direction/outcome evidence matrix and explicit receipt-verified/node-reconciled/local-policy/unknown classes. V1 does not claim independent replay verification of every source |
| C05: overlapping queue states | Accepted | Export disjoint queued-ready/blocked/unknown/in-flight states; specify both queued and active total equations |
| C06: receipt versus transaction identity | Accepted | Separate logical transaction key from receipt inclusion key, retaining block hash and orphan history |
| C07: submissions, fee bumps and attempts | Accepted in part | Store dispatch occurrences, reuse native generation+signed-hash attempt IDs, and link retries. Reject globally unique tx hash in attempt membership: repeated dispatches/attempt relations must not imply duplicate receipts |
| C08: blob length replayability | Accepted with capability gate | Prefer authenticated canonical bytes and exact versioned parsing; measure coverage and provenance; unsupported exact-length recovery remains unknown rather than blocking receipt indexing |
| C09: sequence assignment | Accepted with correction | Assign at the owned transition before enqueue; random per-process epoch; explicitly handle out-of-order publication. An atomic sequence does not itself enforce asynchronous delivery order |
| C10: restart semantics | Accepted with correction | `unresolved_restart` is observer history state, not a new protocol outcome; exclude from current queue ages, preserve links to subsequent readmissions |
| C11: alerts need stages/producers | Accepted | Alert table now assigns stages and producers, with a concrete small stage-1 set |
| C12: metric-source handoff | Accepted | Distinct exporter/native namespaces and stable recording rules selecting exactly one producer, switched after comparison |
| C13: stage-1 scope too broad | Accepted | Minimal identity/endpoint foundation; transition metrics go to WP4, trace export to WP5; PostgreSQL and Tempo are disabled profiles in stage 1 |
| C14: label ownership | Accepted | Deployment/environment/service/instance have one configuration source; metric target labels and log/OTel identity use matching values; intrinsic labels come from applications |
| C15: L2 finality | Accepted with correction | Require exact canonical hash membership and applied settlement range, with finalized L1 anchor. Height comparison alone and successful post receipts are insufficient |
| C16: backfill availability | Accepted | Verify identity/source availability, record coverage across the entire indexed range, distinguish pending/pruned/unsupported blob measurements |
| C17: shutdown flushing | Accepted | Explicit lesser-of-two-seconds-or-25%-remaining-budget flush limit; unreachable collector during shutdown added to failure drills |
| C18: optional scope | Accepted | New public status view deferred; alternative-platform evaluation closed for v1 |

Additional recommendations corrected rather than copied:

- `NUMERIC(78,0)` fits an individual U256 quantity but not every product of two U256 values. Use checked `NUMERIC(156,0)` for individual full-width products and exact unconstrained `NUMERIC` sums for lifetime totals.
- Hashes and addresses are necessary non-indexed diagnostic/SQL fields. Ban them from unbounded metric/Loki indexed labels, not from every JSON record. Secrets and raw signed pending transactions remain excluded from all exported telemetry.
- Compare protocol invariants and outcomes in equivalent controlled workloads. Two independent runs with different block timing need not have identical block bytes to demonstrate instrumentation correctness.
- Post-restart canonical source status can be known even when cross-chain ownership/effect history is unavailable. Preserve both facts rather than upgrading source success into cross-chain completion.
- Treat vendor-specific assertions about Loki limits/deduplication as unverified; the chosen v1 file-ingestion path removes Loki query behavior as an ingestion dependency.

Local verification supporting the changes:

| Evidence | Finding |
| --- | --- |
| Pinned Reth `fd59fd2`, `crates/node/metrics/src/recorder.rs` in the local Cargo checkout | Process-global singleton, `reth` prefix, default builder, and supported custom-builder initialization |
| Locked `metrics-exporter-prometheus` 0.18.3, `src/exporter/builder.rs` | Default summary exposition; bucket matcher configuration required for classic histograms |
| [Queue telemetry](../../crates/eez-composer/src/held_pool/status.rs) | Current ages use wall-clock timestamps; completion timestamp covers `Committed` only |
| [Attempt ownership](../../crates/eez-composer/src/optimistic.rs) | Existing generation+postBatch-hash identity and terminal outcome taxonomy |
| [Correlation records](../../crates/eez-deriver/src/correlation.rs) and [RPC](../../crates/eez-node-common/src/correlation_rpc.rs) | Batch-to-L2-block association without full per-source ownership |
| [Canonical cost reporting](../../crates/eez-composer/src/composer/costs.rs) | Checked receipt costs, companion filtering, apply/retract events and canonical-state revalidation already exist |
| [Settlement manifest](../../crates/eez-protocol/src/settlement_manifest.rs) | Per-source ownership metadata is local and explicitly not a consensus/DA format |
| [Blob codec](../../crates/eez-blob-codec/src/lib.rs) | Decode/unpack APIs exist; exact-length recovery still needs version-specific fixtures |

Readiness: WP0, WP3a and the WP2 stack can begin with the revised fixed first-release boundary. Native startup integration remains evidence-gated with an RPC fallback. WP3b/WP4 require implementation fixtures for the now-specified evidence/event schema and reorg/retention behavior. This review establishes a defined design; it does not establish runtime correctness or production readiness.

The [second Claude review](network-observability-claude-followup.md) explicitly concluded that WP0/WP3a/WP2 can start and identified five remaining later-stage details. These were resolved in the final revision:

| Finding | Final disposition |
| --- | --- |
| B1: drop/gap classification | Added writer control records bypassing the data channel, cumulative drops and bounded sequence uncertainty, a 60-second suspected-gap window, late-event recovery, and explicit retention/abandoned-epoch criteria. Rejected the suggestion that min/max drop bounds or a later sequence alone prove all intervening events are lost |
| B2: chunk identity | One sequence per logical event; `(epoch, sequence, chunk_index)` within deployment/service/instance scope identifies raw chunks; explicit chunk count, completion marker and digest gate assembly; conflicting duplicates are quarantined |
| B3: wallet spend versus companion attribution | Configured sender identity legitimately defines wallet spend. Settlement classification uses contract/method evidence. WP3b companion audit acceptance uses explicit fixture membership; live incomplete companion classification is visible, and per-source allocation belongs to WP4 |
| B4: terminal-progress alert producer | Observer canonical outcome views provide the authoritative retractable progress gauge; native counters are optional cross-checks and histogram integration cannot block this alert |
| B5: partial line/writer liveness | Added process/epoch headers and instance/epoch writer locks, with non-writing observer probes. Timeouts cause stale/unknown status, not fabricated proof of a crashed writer; partial records from ended epochs become explicit gaps |

Minor follow-up corrections also landed: segment IDs survive `.active`/`.closed` renames; physical volume limits include open-unlinked files; lifecycle uses a dedicated channel API outside `RUST_LOG`; L1 stale-head thresholds account for poll intervals; restart detection does not require a new request event; lifecycle files have explicit access roles; receipt facts survive disputed cross-chain claims and reorgs retract anchor-dependent outcome views.

The second review was followed by these targeted specification edits and document checks, not a third Claude pass. All identified design blockers now have explicit decisions; source-integration, replay, failure-mode and performance tests remain the planned acceptance gates.
