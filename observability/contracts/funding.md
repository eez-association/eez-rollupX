# Funding observations

Funding monitoring is optional and read-only. Native candidate affordability and
observer-configured wallet runway are separate populations. An operator-provided
`poster` address is **not** assumed to equal the actual runtime signing account.
There are no address, transaction, block-hash or epoch metric labels.

## Observer account configuration

The indexer accepts an optional `funding` object; omission/null disables its worker:

```json
{
  "funding": {
    "burn_window_seconds": 86400,
    "accounts": [
      {"role": "poster", "chain": "l1", "address": "0x1111111111111111111111111111111111111111"},
      {"role": "system", "chain": "l2", "address": "0x2222222222222222222222222222222222222222"}
    ]
  }
}
```

These are example public addresses, not deployment defaults. Configure one or
two nonzero accounts with unique `poster`/`system` roles. `poster` must use L1;
the system account's chain is explicit. Each account must already be in that
chain's configured wallet selection, or the scanner must explicitly include all
transactions. Recipient-contract selection alone cannot establish complete sender
spend and is rejected. The burn window defaults to 86400 seconds, with bounds
60–604800 seconds. Unknown fields, unsupported roles and invalid combinations fail
before runtime. Older binaries reject the new config field; remove it before
rolling back. Schema 15 adds the canonical revision used to fence cached history;
use the normal stop/backup/explicit-migrate/start upgrade procedure.

The two account polls run concurrently, independently from receipt/lifecycle
workers. Each has a deadline of 30 seconds and uses the existing four-call shared RPC
semaphore, five-second request bounds and finite response limits. Failures retain
their own bounded reason and do not turn the other account into an error.
Successful polls repeat every 15 seconds; failed polls use existing 15/30/60-second
backoff. Observer startup/cancellation retains its existing task ownership.

## Balance and burn assurance

Every poll verifies the configured chain ID/genesis. A fresh validated canonical
cursor supplies the common balance/burn block, avoiding a latest-head query that
perpetually outruns a fast-chain indexer. If no cursor with a block no older than
90 seconds is available, a canonical latest-block balance can still be collected;
that does not imply complete burn history. The balance RPC uses EIP-1898
`blockHash` plus `requireCanonical: true`, never an unpinned implicit `latest`.
The worker rechecks the block and chain identity after SQL/RPC work. Unsupported
hash-selected balances fail explicitly; there is no less-assured fallback.

One SQL statement/snapshot checks the exact balance block is a retained canonical
anchor at or before the validated cursor, with its matching timestamp,
healthy/unfrozen source, bootstrap coverage preceding the entire window,
complete canonical block/receipt coverage, no unresolved index gaps and no
missing cost fields. It sums each canonical account receipt exactly once,
including reverted charges. It uses `(anchor_time - window, anchor_time]`, so
receipts exactly on the lower boundary are excluded. No attempt/member joins,
L1/L2 currency addition or inferred companion ownership enter this calculation.

Only complete **positive** spend yields a burn rate and runway. Zero observed
spend, insufficient history, source errors and missing costs remain unknown;
they do not become infinite runway or a zero-cost success. SQL NUMERIC computes
the 24-hour warning decision exactly, including zero balances and equality at the
threshold. Float values are display/monitoring approximations, not the decision
boundary. Original exact receipt costs remain in SQL.

Scraping removes balance data after a failed/stale poll (90 seconds, maximum
five-second future-clock allowance). Runway additionally requires a fresh healthy
indexer and the same persisted canonical revision. Ordinary append-only cursor
advancement preserves a still-canonical historical window. Orphaning or deleting
any canonical block increments the chain revision transactionally, invalidating
cached samples even across concurrent observer writers. Rollback rolls back the
increment; bigint exhaustion fails explicitly instead of wrapping. The revision
is never a metric label.

Normal scanner writes already lock chain, then cursor, then blocks. The revision
trigger updates the already-owned chain row. Administrative block changes must
take that same chain lock first; bypassing this order can conflict with a scanner
and fail through PostgreSQL deadlock handling. Stop the observer for reviewed
repairs, preserve evidence and never reset `canonical_revision` manually.

All observer metrics below have only `account=poster/system,chain=l1/l2` in
addition to normal target labels:

| Metric suffix after `eez_observer_funding_` | Contract |
| --- | --- |
| `source_valid` | Fresh verified balance available (1/0) |
| `last_attempt_timestamp_seconds` | Last account poll observation |
| `balance_native_units` | Balance at the stated canonical block; absent on failure/staleness |
| `balance_block_timestamp_seconds` | Canonical balance anchor age, distinct from collection time |
| `burn_valid` | Complete positive canonical burn with a still-matching cursor (1/0) |
| `burn_native_units_per_second` | Covered spend/window, absent if unknown |
| `runway_seconds` | Balance divided by covered positive burn, absent if unknown |
| `runway_below_day` | Exact SQL decision `balance * window < spend * 86400` |
| `error{reason}` | Bounded balance collection error |
| `burn_unknown{reason}` | Explicit absence cause, including `zero_burn`, `incomplete_history`, `canonical_changed`, `stale_head` |

Native units use 18 decimal places for these Ethereum-family chains (xDAI on
Chiado). No fiat conversion is performed.

## Actual native poster candidate

The composer uses its existing balance read and existing gas/fee arithmetic.
No telemetry RPC is added and no signing key, signed bytes, fee decision or
protocol outcome changes. In the legacy posting path, metrics-enabled diagnosis
pins the pre-existing optional balance query to the canonical fee-sample block;
metrics-disabled behavior retains its prior query. A failed diagnostic read
continues to leave protocol behavior unchanged.

The network-quoted posting path records its actual validated final gas reserve
and already-read sample-block balance. If even the necessary blob-only reserve
cannot be funded, it records that proven lower-bound failure separately using
`minimum_only=1`; it does not invent an execution-gas estimate before simulation.
Unknown/failed preflight work remains unknown. The next attempt invalidates the
previous candidate before fee/balance work begins. A process-local generation
token fences overlapping preparation and repricing: older completions cannot
republish validity or erase a newer completed sample. Publication uses a
nonblocking telemetry-local lock and checks generation before/after metric writes.
Contention and superseded/exhausted tokens are counted under the fixed reasons
of `eez_poster_funding_sample_drops_total`; no protocol owner is locked or delayed.

`eez_poster_can_fund_candidate` compares full-width 256-bit values exactly. The
helper accepts fixed-width bytes and never receives an address or transaction.
Companion costs and the observer's configured accounts do not enter this actual
signer comparison. Approximate required/balance native-unit gauges are diagnostic;
their rounding cannot change the affordability boolean.

Composer's existing prefix produces `reth_eez_poster_funding_estimate_valid`,
`balance_valid`, `estimate_timestamp_seconds`, `required_native_units`,
`balance_native_units` and `minimum_only`, plus
`reth_eez_poster_can_fund_candidate`. Missing balances publish invalid/NaN,
never zero. Alerts require both validity flags, native scrape health and an
estimate no older than 90 seconds. These are recent candidate observations, not a
promise about a future batch or current wallet identity configured elsewhere.

[Funding alerts](../runbooks/protocol-alerts.md#funding) opt into each population
explicitly. [Validation](../validation/funding.md) records exact boundaries,
reorg/coverage behavior and process tests.
