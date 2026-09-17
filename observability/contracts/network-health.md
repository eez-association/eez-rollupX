# Network and infrastructure observations

The optional exporter `network` object enables independently collected safe-head
and explorer observations. In an indexer configuration it belongs inside
`exporter`. Its absence preserves the existing RPC call pattern. For example:

```json
{
  "network": {
    "l2_safe_head": true,
    "explorers": [
      {
        "chain": "l2",
        "profile": "blockscout_v2",
        "url": "http://private-explorer:4000"
      }
    ]
  }
}
```

This fragment supplements the required L1/L2 endpoint and expected genesis
configuration. An explicit empty capability object is invalid. There can be at
most two explorers, one per chain; unknown fields/profiles/chains, duplicate
chains and explorer base URLs with a query or fragment fail validation before
workers start. URLs and hashes never become metric labels or error messages.

Each enabled source has an independent worker. It shares the existing RPC
transport's four-request semaphore, five-second request timeout, redirect
refusal and one-MiB JSON response limit. A complete pass is bounded to 60 seconds.
Successful passes wait 15 seconds; failures back off to 30 and then 60 seconds.
The application owns and cancels these tasks with its other collectors.

The node-safe worker reads the L2 `safe` block tag and checks the exact hash
against canonical block-by-number, before and after the remaining observations.
It checks chain ID/genesis around the pass and rejects a height ahead of the
observed latest head. Unsupported tags, unavailable blocks, identity changes
and forks yield unknown. **Node-reported safe is not independent finality.**

The explorer profile uses Blockscout's [v2 blocks API](https://github.com/blockscout/agent-skills/blob/main/blockscout-analysis/references/blockscout-api/blocks.md):
check genesis block zero, request the first canonical block page, validate its
bounded descending entries, and compare the highest entry's exact hash/height
with the configured chain RPC. Its lag is the observed latest RPC height minus
that entry's height. This verifies the highest observed indexed block, not
gap-free historical indexing. Blobscan's slot cursor is not converted into a
block height; it remains an inspection link without a fabricated lag.

| Metric | Meaning |
| --- | --- |
| `eez_observer_network_enabled` | Configured capability, including explicit zero for disabled capabilities |
| `eez_observer_network_evaluation_valid` | A successful, unfailed observation no older than 90 seconds, with at most five seconds of future clock tolerance |
| `eez_observer_network_state` | One finite reason for configured/disabled, pending, failed or stale state |
| `eez_observer_network_last_success_timestamp_seconds` | Last actual success; failures do not overwrite it |
| `eez_observer_network_height`, `eez_observer_network_lag_blocks` | Present only with valid evidence; missing data is not zero lag |

Intrinsic labels are only `chain=l1/l2`, `profile=node_safe/blockscout_v2` and
the finite `reason` where applicable. Standard scrape identity labels remain
deployment-owned. The three capability combinations are fixed; no URL or source
hash increases cardinality. One failed optional source cannot disable chain
heads, queue observations or another network worker.

The existing indexer additionally exposes
`eez_observer_index_finalized_timestamp_seconds` from the same exact finalized
block as `eez_observer_index_finalized_number`. Missing/future timestamps,
failed/stale collection and frozen finality suppress it. The dashboard subtracts
this block timestamp from current time; it never uses last collection time as
finality age. Exact settled L2 finality keeps the existing applied-hash/L1-anchor
checks. No SQL migration is required.

Network panels retain the observer instance selector. Infrastructure adds a
separate client/host instance selector because those processes have different
identities. Its optional node-exporter panels cover CPU, per-interface traffic,
per-device mean I/O latency and busy time. Lighthouse panels show reported sync,
peers, head slot and finalized epoch without a slots-per-epoch assumption.
Embedded Reth peer metrics are labelled process-wide because the pinned builds
cannot safely separate their L1/L2 values. Process-start changes are an observed
restart lower bound, not a container orchestrator restart counter.

Disabled or missing producers remain unknown. Old observers reject the new
configuration field; upgrade the observer before enabling it, or omit it when
rolling back. Other configuration defaults and the protocol/SQL formats are
unchanged.

The observer uses bounded polling and cursor/backfill recovery. The proposal's
subscription wakeups are not implemented: existing configured endpoints are
HTTP-only, and adding a parallel subscription connection is unnecessary for
canonical recovery. This is an explicit implementation deviation; observations
and latency measurements include the documented polling delay.
