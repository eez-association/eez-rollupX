# Current required blob dependency

This signal is separate from historical blob measurement and archive coverage.
An unavailable/pruned historical audit response cannot create a derivation page.
The producer is the native operation that is currently trying to obtain data for
an applied L1 settlement, including live Watcher hydration before it emits an
event to the Deriver.

`eez-l1` classifies only two concrete failure sites: unavailable Beacon body
responses and an absent expected transaction versioned hash after retrieval.
Classification requires a nonempty applied settlement and carries exact L1
block number/hash, transaction hash/index and rollup ID. Missing transaction or
execution-block data, changing block metadata, generic transport errors,
malformed/KZG-invalid data and unapplied posts retain their existing error types.
No error text is parsed. The typed error preserves the original displayed
`SourceIncomplete` message and retry classification.

A singleton process-local diagnostic stores that reference, closed reason,
monotonic attempt age and saturating u32 consecutive-attempt count. Changing the
exact reference resets the count. Deriver catch-up and live Watcher operations
share it. An unrelated failure or successful completion clears the reporting
owner's dependency; an idle sibling cannot erase an actively blocked backfill.
A completed derivation scan clears dependencies only through its verified scan
height, so an older concurrent completion cannot erase a newer failure. Reorg
and correlation-suffix invalidation clear the state. No state is persisted and
no diagnostic controls native retries, signing, replay or canonical decisions.

The additive `eez_getDerivationEvidence` response includes `requiredBlob`, either
null or the structured current dependency, alongside process epoch, explicit
role/scope and scan generation. Both launchers advertise their L1 chain ID, EEZ
contract and rollup. The existing correlation methods and persistent format are
unchanged. Hashes and transaction identities are diagnostic RPC fields, never
metric labels; arbitrary Beacon messages and endpoint credentials are omitted.

The observer's separate bounded worker is enabled for an indexer with configured
L1 posting contracts. It uses the existing primary L2 endpoint; no Beacon URL or
additional configuration is needed for this native signal. It validates both
chain IDs/genesis hashes every pass and checks process scope, fresh native age,
exact successful receipt inclusion/index/recipient, the matching type-3 blob
transaction and canonical L1 anchor. It rereads the diagnostic and anchor before
publishing. A completed scan already covering the reported failure makes that
dependency superseded/unknown. Epoch, generation, reference or owner changes
invalidate an in-flight observation. A null dependency is healthy only when the
node is ready and its completed scan matches the current canonical L1 head.
Older node APIs, missing data and stale or conflicting facts remain unknown.

Metrics have only normal scrape identity labels; unknown has a finite reason:

| Metric | Meaning |
| --- | --- |
| `eez_observer_required_blob_evaluation_valid` | 1 for fresh independently anchored collection; otherwise 0 |
| `eez_observer_required_blob_blocked` | 1 for the confirmed current dependency; 0 for a current complete scan with none; absent when unknown |
| `eez_observer_required_blob_retries` | Consecutive actual attempts for the exact native reference; 0 after confirmed recovery |
| `eez_observer_required_blob_last_attempt_timestamp_seconds` | Observer collection time |
| `eez_observer_required_blob_unknown{reason}` | Closed unknown reason |

The alert requires valid=1, blocked=1, at least three attempts, fresh observer
scraping and the configured hold. Historical coverage retains warning semantics.
Collection uses the shared four-request semaphore, five-second HTTP timeout,
bounded bodies, 30-second pass deadline, 15/30/60-second failure backoff and
90-second sample/native-evidence freshness. It needs no database migration and
cannot delay canonical SQL receipt accounting.
