# RPC exporter metric contract, v1

Producer: `eez-observer exporter`. These signals are implemented and all are
gauges. The exporter renders bounded Prometheus text directly and does not
install a global metrics recorder. This keeps its database-free process
independent of Reth's singleton and of future native histogram integration.

| Metric | Labels | Meaning / missing data |
| --- | --- | --- |
| `eez_observer_collection_success` | `source=l1/l2/queue/reference_l1` | 1 only for a successful sample no older than 90 seconds; 0 before success or after failure/staleness. The unconfigured reference has no series |
| `eez_observer_collection_last_success_timestamp_seconds` | `source` | Unix time of last successful identity-verified collection. Preserved after failure; absent before first success |
| `eez_observer_collection_error` | `source`, `reason` | Current failure, value 1. Reasons: transport, http, rpc, unsupported, malformed, identity_mismatch, oversized. Old error disappears after recovery |
| `eez_observer_chain_head_number` | `source=l1/l2/reference_l1` | Latest observed head height. May decrease after reorg; never a finalized-head claim |
| `eez_observer_chain_head_timestamp_seconds` | `source` | Chain timestamp of that head; compare with local time/reference rather than assuming every reachable RPC is advancing |
| `eez_observer_queue_transactions` | `state=queued_ready/queued_blocked/queued_unknown/in_flight` | Disjoint source-reported counts. Queued total is sum of queued states; active total adds in_flight. Readiness assertions require evaluation-valid gating |
| `eez_observer_queue_evaluation_valid` | none | 1 when sourceState contains both block identities and evaluatedAtMs is at most 60 seconds old and no more than 5 seconds in the future; 0 when unknown/stale |
| `eez_observer_queue_evaluated_timestamp_seconds` | none | Source's evaluatedAtMs, converted to seconds; absent if unknown |
| `eez_observer_queue_oldest_age_seconds` | `kind=pending/serviceable` | Source-reported wall-clock age at collection; absent if null. These sampled maxima are neither live counters nor latency distributions |

Failure removes the source's data series immediately. The last-success timestamp
is never overwritten with a failure time or a fabricated zero. Data older than
90 seconds is omitted on scrape; large backwards wall-clock jumps also invalidate
the sample. Queue evaluation validity is recomputed at scrape time and remains
independent of successful RPC collection. Data is an RPC observation, not proof
of canonical finality or independently replayed cross-chain effects.

Identity is checked for each collection. Individual RPC reads are sequential,
not an atomic multi-call chain snapshot; reorg/settlement claims therefore stay
out of this exporter. Queue source-state hashes are validated structurally,
not independently compared with two canonical chains. That belongs to the later
canonical observer. The runtime's committed-only completion timestamp is not
exported as general completion progress.

Target labels own `environment`, `deployment`, `service`, `instance` and `job`.
Applications emit only the bounded intrinsic labels above. Recording rules are
the stable dashboard interface. Queue rules gate on both collection success
and evaluation validity; if evaluation is unknown, those rule outputs are
absent even though raw membership counts may be available.

The full lifecycle event schema, native durations, per-direction metrics,
canonical cost metrics and alert rules are not implemented by this contract.
They retain their proposal acceptance gates; absence must not be shown as zero.
