CREATE SCHEMA IF NOT EXISTS eez_indexer;
CREATE TABLE IF NOT EXISTS eez_indexer.source (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    schema_version integer NOT NULL CHECK(schema_version = 1),
    chain_id text NOT NULL, genesis_hash text NOT NULL, registry text NOT NULL,
    rollup_id text NOT NULL, start_block bigint NOT NULL CHECK(start_block > 0),
    checked_at timestamptz, observed_head bigint
);
CREATE TABLE IF NOT EXISTS eez_indexer.blocks (
    height bigint PRIMARY KEY CHECK(height >= 0), hash text UNIQUE NOT NULL,
    parent_hash text NOT NULL, timestamp bigint NOT NULL CHECK(timestamp >= 0)
);
CREATE TABLE IF NOT EXISTS eez_indexer.transactions (
    block_hash text NOT NULL REFERENCES eez_indexer.blocks(hash) ON DELETE CASCADE,
    transaction_hash text NOT NULL, transaction_index bigint NOT NULL,
    gas_used numeric(78,0) NOT NULL, gas_price_wei numeric(78,0) NOT NULL,
    execution_fee_wei numeric(78,0) NOT NULL,
    blob_gas_used numeric(78,0) NOT NULL, blob_gas_price_wei numeric(78,0) NOT NULL,
    blob_fee_wei numeric(78,0) NOT NULL,
    PRIMARY KEY(block_hash, transaction_hash), UNIQUE(block_hash, transaction_index)
);
-- Posting observations know the transaction hash before they know its canonical
-- block. The primary key starts with block_hash and cannot serve this lookup.
CREATE INDEX IF NOT EXISTS transactions_by_hash ON eez_indexer.transactions
    (transaction_hash) INCLUDE (block_hash);
CREATE TABLE IF NOT EXISTS eez_indexer.settlements (
    block_hash text NOT NULL, transaction_hash text NOT NULL,
    log_index bigint NOT NULL, l2_block_hash text NOT NULL,
    PRIMARY KEY(block_hash, log_index),
    FOREIGN KEY(block_hash, transaction_hash) REFERENCES eez_indexer.transactions ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS eez_indexer.details_source (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    chain_id text NOT NULL, genesis_hash text NOT NULL
);
-- Optional enrichment never owns the canonical checkpoint. Reorg deletion of
-- the receipt also removes its details, including a concurrent stale write.
CREATE TABLE IF NOT EXISTS eez_indexer.details (
    block_hash text NOT NULL, transaction_hash text NOT NULL,
    data jsonb NOT NULL, checked_at timestamptz NOT NULL DEFAULT now(),
    retry_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(block_hash,transaction_hash),
    FOREIGN KEY(block_hash,transaction_hash) REFERENCES eez_indexer.transactions ON DELETE CASCADE
);
-- Re-enrich older records once for exact cross-chain source transaction counts, retaining known details.
UPDATE eez_indexer.details SET retry_at=now()
WHERE data->>'version' IS DISTINCT FROM '5';
CREATE OR REPLACE VIEW eez_indexer.settlement_history AS
SELECT to_timestamp(b.timestamp) AS time, b.height AS l1_block, b.hash AS l1_block_hash,
    t.transaction_hash, t.transaction_index, s.log_index, s.l2_block_hash
FROM eez_indexer.settlements s JOIN eez_indexer.blocks b ON b.hash=s.block_hash
JOIN eez_indexer.transactions t USING(block_hash, transaction_hash);
CREATE OR REPLACE VIEW eez_indexer.transaction_costs AS
SELECT to_timestamp(b.timestamp) AS time, b.height AS l1_block, t.transaction_hash,
    t.gas_used, t.gas_price_wei / 1000000000 AS gas_price_gwei,
    t.execution_fee_wei / 1000000000000000000 AS execution_fee,
    t.blob_fee_wei / 1000000000000000000 AS blob_fee,
    (t.execution_fee_wei + t.blob_fee_wei) / 1000000000000000000 AS total_fee
FROM eez_indexer.transactions t JOIN eez_indexer.blocks b ON b.hash=t.block_hash;

-- Only the latest observation per chain; stale values are hidden by Grafana.
CREATE TABLE IF NOT EXISTS eez_indexer.network_heads (
    chain text PRIMARY KEY CHECK(chain IN ('l1','l2')),
    data jsonb NOT NULL,
    observed_at timestamptz NOT NULL
);

-- Off-chain observations survive chain reorgs; canonical status is joined, not
-- copied into an irreversible local "success" flag.
CREATE TABLE IF NOT EXISTS eez_indexer.posting_events (
    attempt_id text NOT NULL, sequence integer NOT NULL CHECK(sequence >= 0),
    data jsonb NOT NULL, imported_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(attempt_id,sequence)
);
CREATE INDEX IF NOT EXISTS posting_events_time ON eez_indexer.posting_events
    (((data->>'timestamp_ms')::bigint));
CREATE TABLE IF NOT EXISTS eez_indexer.posting_files (
    file_id text PRIMARY KEY, byte_offset bigint NOT NULL CHECK(byte_offset >= 0),
    anchor text NOT NULL
);
CREATE TABLE IF NOT EXISTS eez_indexer.posting_collection (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    checked_at timestamptz NOT NULL, error text
);
CREATE OR REPLACE VIEW eez_indexer.posting_attempts AS
WITH attempts AS (
    SELECT attempt_id,
        (array_agg(data ORDER BY sequence DESC))[1] AS latest,
        (array_agg((data->>'timestamp_ms')::bigint ORDER BY sequence))[1] AS first_ms,
        bool_or(data->>'stage'='prepared') AS prepared,
        bool_or(data->>'stage'='repriced') AS repriced,
        bool_or(data->>'stage'='dispatched') AS dispatched,
        min(sequence)=0 AND count(*)=max(sequence)+1 AS contiguous,
        max((data->>'elapsed_ms')::double precision) FILTER(WHERE data->>'stage'='dispatched') AS preparation_ms,
        max((data->>'dispatch_elapsed_ms')::double precision) FILTER(WHERE data->>'stage' IN ('included','excluded','uncertain')) AS observation_ms
    FROM eez_indexer.posting_events GROUP BY attempt_id
)
SELECT a.attempt_id, to_timestamp(a.first_ms / 1000.0) AS time,
    a.latest->>'posting_id' AS posting_id,
    (a.latest->>'sync_height')::bigint AS l2_block,
    a.latest->>'transaction_hash' AS transaction_hash,
    a.latest->>'stage' AS stage, a.latest->'trigger_reasons' AS trigger_reasons,
    a.latest->'detail' AS detail, a.preparation_ms, a.observation_ms,
    CASE WHEN a.latest->>'stage'='preparation_failed' AND a.contiguous THEN 'Observed sequence intact'
         WHEN a.contiguous AND a.prepared
        AND (a.latest->>'stage' NOT IN ('dispatched','included') OR (a.repriced AND a.dispatched))
        THEN 'Observed sequence intact' ELSE 'Missing events' END AS history,
    b.height AS canonical_l1_block, b.hash AS canonical_l1_hash,
    CASE WHEN b.hash IS NOT NULL THEN 'Canonical commitment'
         WHEN a.latest->>'stage'='included' THEN 'Not in indexed canonical history'
         ELSE 'No indexed commitment' END AS chain_status
FROM attempts a LEFT JOIN LATERAL (
    SELECT t.block_hash FROM eez_indexer.transactions t
    WHERE t.transaction_hash=a.latest->>'transaction_hash'
      AND EXISTS(SELECT 1 FROM eez_indexer.settlements s
                 WHERE s.block_hash=t.block_hash AND s.transaction_hash=t.transaction_hash)
    LIMIT 1
) t ON true LEFT JOIN eez_indexer.blocks b ON b.hash=t.block_hash;

-- Separate view keeps earlier indexer binaries free to recreate posting_attempts
-- on restart. All timing is derived from existing retained events and canonical blocks.
CREATE OR REPLACE VIEW eez_indexer.posting_timing AS
WITH timing AS (
    SELECT attempt_id,
        max(coalesce(data->>'target_block',data->'detail'->>'target_block')::bigint) FILTER(WHERE data ? 'target_block' OR data->>'stage'='dispatched') AS target_l1_block,
        -- Older/incomplete logs can lack target timing. Do not invent it from the outcome.
        max(CASE WHEN coalesce(data->>'target_timestamp',data->'detail'->>'target_timestamp') ~ '^[0-9]{1,12}$'
                 THEN CASE WHEN (coalesce(data->>'target_timestamp',data->'detail'->>'target_timestamp'))::bigint <= 253402300799
                           THEN (coalesce(data->>'target_timestamp',data->'detail'->>'target_timestamp'))::bigint END END)
            FILTER(WHERE data ? 'target_timestamp' OR data->>'stage'='dispatched') AS target_timestamp,
        max((data->>'timestamp_ms')::bigint) FILTER(WHERE data->>'stage'='dispatched') AS dispatched_ms,
        max((data->>'timestamp_ms')::bigint) FILTER(WHERE data->>'stage'='included') AS receipt_ms
    FROM eez_indexer.posting_events GROUP BY attempt_id
)
SELECT p.*,
    a.target_l1_block, to_timestamp(a.target_timestamp) AS expected_l1_time,
    to_timestamp(b.timestamp) AS included_l1_time,
    to_timestamp(a.receipt_ms / 1000.0) AS receipt_observed_time,
    b.height - a.target_l1_block AS inclusion_block_delta,
    a.dispatched_ms / 1000.0 - a.target_timestamp AS dispatch_vs_target_seconds,
    (b.timestamp - a.target_timestamp)::double precision AS inclusion_vs_target_seconds,
    -- A receipt from an orphaned block must not measure observation of its replacement.
    CASE WHEN b.hash = p.detail->>'l1_block_hash'
         THEN a.receipt_ms / 1000.0 - a.target_timestamp END AS receipt_vs_target_seconds,
    CASE WHEN b.hash = p.detail->>'l1_block_hash'
         THEN a.receipt_ms / 1000.0 - b.timestamp END AS receipt_observation_seconds,
    CASE WHEN a.target_l1_block IS NULL THEN 'Target unavailable'
         WHEN b.height = a.target_l1_block THEN 'Expected block'
         WHEN b.height > a.target_l1_block THEN 'Later block'
         WHEN b.height < a.target_l1_block THEN 'Earlier block'
         WHEN p.stage='excluded' THEN 'Excluded'
         ELSE 'Not indexed' END AS target_result
FROM eez_indexer.posting_attempts p JOIN timing a USING(attempt_id)
LEFT JOIN eez_indexer.blocks b ON b.hash=p.canonical_l1_hash;

CREATE INDEX IF NOT EXISTS blocks_timestamp ON eez_indexer.blocks(timestamp);
-- Header evidence is recomputed against the canonical index after reorgs. Off-chain
-- observations remain observations; absent headers never prove a skipped slot.
CREATE OR REPLACE VIEW eez_indexer.posting_diagnostics AS
SELECT p.*,
    CASE WHEN p.stage='preparation_failed' THEN 'Preparation failed'
         WHEN p.stage='preflight_failed' THEN 'Deferred before dispatch'
         WHEN p.canonical_l1_block IS NOT NULL AND p.included_l1_time=p.expected_l1_time THEN 'Included at target'
         WHEN p.canonical_l1_block IS NOT NULL THEN 'Included at another target'
         WHEN p.stage IN ('dispatched','included','excluded','uncertain') AND before.hash=after.parent_hash AND before.height+1=after.height THEN 'Network slot skipped'
         WHEN exact.hash IS NOT NULL AND p.stage='excluded' THEN 'Block produced without inclusion'
         WHEN p.stage='included' THEN 'Receipt observed; awaiting canonical match'
         WHEN p.stage='uncertain' THEN 'Observation uncertain'
         WHEN p.stage='excluded' THEN 'Awaiting canonical evidence'
         ELSE 'In progress' END AS outcome,
    CASE
      WHEN p.canonical_l1_block IS NOT NULL THEN 'Transaction matches an indexed canonical commitment'
      WHEN before.hash=after.parent_hash AND before.height+1=after.height
           AND p.stage IN ('dispatched','included','excluded','uncertain')
        THEN 'Consecutive canonical headers bracket the missing target timestamp'
      WHEN p.stage='excluded' AND exact.hash IS NOT NULL
        THEN 'Target block exists; Composer observed no inclusion. Builder/proposer reason unknown'
      WHEN p.stage='preparation_failed' THEN CASE p.detail->>'reason'
        WHEN 'preparation_deadline' THEN 'Preparation exceeded the proof-start deadline'
        WHEN 'prover_deadline' THEN 'Prover deadline or retry budget exhausted'
        WHEN 'prover_unavailable' THEN 'Prover unavailable after retries'
        WHEN 'prover_aborted' THEN 'Prover aborted after retries'
        WHEN 'prover_cache_miss' THEN 'Prover witness cache miss'
        WHEN 'prover_rejected_candidate' THEN 'Prover returned a candidate rejection; inspect local validation result'
        ELSE 'Prover backend failed; inspect correlated local logs' END
      WHEN p.stage='preflight_failed' THEN CASE p.detail->>'reason'
        WHEN 'target_parent_timestamp_mismatch' THEN 'Canonical parent timestamp differs from the required target parent'
        WHEN 'target_parent_unavailable' THEN 'Required parent was unavailable before the submission deadline'
        WHEN 'submission_window_expired' THEN 'Submission deadline expired before final pricing'
        WHEN 'target_changed' THEN 'Target no longer eligible; precise subtype unavailable'
        WHEN 'ownership_changed' THEN 'Prepared attempt no longer owns this submission'
        WHEN 'target_unavailable' THEN 'Target RPC unavailable; inspect correlated local logs'
        WHEN 'final_pricing_failed' THEN 'Final pricing or simulation failed; inspect correlated local logs'
        WHEN 'invalid_envelope' THEN 'Final transaction envelope is invalid'
        WHEN 'ledger_update_failed' THEN 'Final transaction could not update the attempt ledger'
        ELSE 'Unclassified local deferral; inspect correlated local logs' END
      WHEN p.stage='uncertain' THEN 'Receipt observation failed; no conclusive verdict'
      ELSE 'Waiting for the next lifecycle event or canonical evidence' END AS explanation,
    exact.height AS target_canonical_block,
    before.height AS preceding_l1_block, after.height AS following_l1_block
FROM eez_indexer.posting_timing p
LEFT JOIN LATERAL (SELECT * FROM eez_indexer.blocks WHERE timestamp=extract(epoch FROM p.expected_l1_time)::bigint LIMIT 1) exact ON true
LEFT JOIN LATERAL (SELECT * FROM eez_indexer.blocks WHERE timestamp<extract(epoch FROM p.expected_l1_time)::bigint ORDER BY timestamp DESC LIMIT 1) before ON true
LEFT JOIN LATERAL (SELECT * FROM eez_indexer.blocks WHERE timestamp>extract(epoch FROM p.expected_l1_time)::bigint ORDER BY timestamp LIMIT 1) after ON exact.hash IS NULL;
