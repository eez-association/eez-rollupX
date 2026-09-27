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

-- Additive candidate history. File checkpoints and both event families commit
-- together; keeping this table on rollback allows old collectors to coexist.
CREATE TABLE IF NOT EXISTS eez_indexer.admission_events (
    candidate_id text NOT NULL, sequence integer NOT NULL CHECK(sequence >= 0),
    data jsonb NOT NULL, imported_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(candidate_id,sequence)
);
CREATE INDEX IF NOT EXISTS admission_events_time ON eez_indexer.admission_events
    (((data->>'timestamp_ms')::bigint));
CREATE INDEX IF NOT EXISTS posting_candidate ON eez_indexer.posting_events
    ((data->'detail'->>'candidate_id')) WHERE data->>'stage'='prepared';
-- Separate from settlement transactions: sources without registry logs must
-- not change settlement cost totals. Canonical posting deletion invalidates
-- enrichment and permits collection again if the same source is re-included.
CREATE TABLE IF NOT EXISTS eez_indexer.admission_source_receipts (
    block_hash text NOT NULL, postbatch_hash text NOT NULL, source_hash text NOT NULL,
    coverage text NOT NULL CHECK(coverage IN ('available','absent','before_postbatch')),
    gas_used numeric(78,0), success boolean,
    checked_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(block_hash,postbatch_hash,source_hash),
    FOREIGN KEY(block_hash,postbatch_hash) REFERENCES eez_indexer.transactions(block_hash,transaction_hash) ON DELETE CASCADE,
    CHECK((coverage='available' AND gas_used IS NOT NULL AND gas_used >= 0 AND success IS NOT NULL)
       OR (coverage<>'available' AND gas_used IS NULL AND success IS NULL))
);
CREATE OR REPLACE VIEW eez_indexer.admission_candidates AS
WITH events AS (
    SELECT candidate_id, min((data->>'timestamp_ms')::bigint) AS first_ms,
        (array_agg(data ORDER BY sequence))[1] AS context,
        (array_agg(data->'detail' ORDER BY sequence) FILTER(WHERE data->>'kind'='started'))[1] AS started,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='summary'))[1] AS summary,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='full_projection'))[1] AS projection,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='proof_requested'))[1] AS preparation,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='proof_received'))[1] AS proof,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='preflight'))[1] AS preflight,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind' IN ('proof_failed','preflight_failed')))[1] AS failure,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='dispatch'))[1] AS dispatch,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='delivery'))[1] AS delivery,
        count(*)=max(sequence)+1 AND min(sequence)=0 AS contiguous,
        bool_or(data->>'kind'='finished') AS finished
    FROM eez_indexer.admission_events GROUP BY candidate_id
)
SELECT e.candidate_id, to_timestamp(e.first_ms/1000.0) AS time,
    e.context->>'rebuild_of' AS rebuild_of, e.context->>'l2_parent' AS l2_parent,
    (e.context->>'target_block')::bigint AS target_block,
    to_timestamp((e.context->>'target_timestamp')::bigint) AS target_time,
    e.context->>'policy_version' AS policy_version,e.context->>'estimator_version' AS estimator_version,
    (e.started->>'queued')::bigint AS queued, (e.started->>'selected')::bigint AS selected,
    (e.summary->>'accepted')::bigint AS accepted,(e.summary->>'deferred')::bigint AS deferred,
    coalesce(e.failure->>'reason',e.summary->>'reason') AS reason,
    (e.summary->>'projected_gas')::numeric AS admission_gas,
    (e.projection->>'projected_gas')::numeric AS full_projection_gas,
    (e.summary->>'planning_cap')::numeric AS planning_cap,
    (e.preflight->'preflight_gas_used'->>0)::numeric AS preflight_gas_used,
    (e.preflight->'preflight_gas_required'->>0)::numeric AS preflight_gas_required,
    (e.preflight->'signed_gas_limits'->>0)::numeric AS signed_gas_limit,
    (e.preparation->>'preparation_ms')::double precision AS preparation_ms,
    (e.preparation->>'nominal_preparation_ms')::double precision AS nominal_preparation_ms,
    (e.proof->>'proof_ms')::double precision AS proof_ms,
    (e.preflight->>'finalization_ms')::double precision AS finalization_ms,
    (e.dispatch->>'remaining_ms')::double precision AS dispatch_remaining_ms,
    (e.delivery->>'delivery_ms')::double precision AS delivery_ms,
    e.delivery->>'acknowledged' AS relay_acknowledged,
    p.attempt_id,p.transaction_hash,p.canonical_l1_block,p.chain_status,
    t.gas_used AS canonical_receipt_gas,
    CASE WHEN NOT e.contiguous THEN 'Missing events' WHEN e.finished THEN 'Complete sequence'
         ELSE 'Open sequence' END AS history,
    e.preflight AS preflight_detail,
    (e.started->>'selection_limit')::numeric AS selection_limit,
    e.started->>'selection_limit_reason' AS selection_limit_reason,
    e.summary->>'reason' AS admission_stop_reason,
    p.canonical_l1_hash,
    e.summary->'capacity_deferrals' AS capacity_deferrals,
    (e.summary->>'fifo_bypassed')::boolean AS fifo_bypassed,
    (e.summary->>'minimum_check_count')::numeric AS minimum_check_count,
    (e.summary->>'simulation_count')::numeric AS simulation_count
FROM events e
LEFT JOIN eez_indexer.posting_events link ON link.data->>'stage'='prepared'
    AND link.data->'detail'->>'candidate_id'=e.candidate_id
LEFT JOIN eez_indexer.posting_attempts p ON p.attempt_id=link.attempt_id
LEFT JOIN eez_indexer.transactions t ON t.transaction_hash=p.transaction_hash AND t.block_hash=p.canonical_l1_hash;

CREATE OR REPLACE VIEW eez_indexer.admission_sources AS
WITH sources AS (
    SELECT candidate_id,data->'detail'->>'source_hash' AS source_hash,
        (array_agg(data->'detail' ORDER BY sequence) FILTER(WHERE data->>'kind'='source_selected'))[1] AS selected,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='source_estimate'))[1] AS estimate,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='source_accepted'))[1] AS accepted,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='source_outcome'))[1] AS outcome,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='source_preflight'))[1] AS preflight,
        (array_agg(data->'detail' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='source_capacity_check'))[1] AS capacity
    FROM eez_indexer.admission_events WHERE data->>'kind' LIKE 'source_%'
    GROUP BY candidate_id,data->'detail'->>'source_hash'
)
SELECT s.candidate_id,c.time,s.source_hash,coalesce(s.estimate->>'direction',s.selected->>'direction') AS direction,
    (s.selected->>'position')::bigint AS position,
    CASE WHEN s.accepted IS NOT NULL THEN 'Accepted'
         WHEN s.capacity IS NOT NULL THEN 'Capacity deferred before simulation'
         WHEN s.estimate->>'gas_gate_accepted'='false' THEN 'Gas deferred'
         WHEN s.estimate IS NOT NULL THEN 'Not accepted after gas check'
         ELSE coalesce(s.outcome->>'reason','No gas estimate recorded') END AS decision,
    coalesce(s.estimate->>'constraint',s.capacity->>'constraint') AS gas_constraint,
    (s.estimate->>'source_sim_gas')::numeric AS source_simulation_gas,
    (s.estimate->>'l1_target_sim_gas')::numeric AS l1_target_simulation_gas,
    (s.estimate->>'entry_count')::numeric AS entry_count,
    (s.estimate->>'entry_pin_gas')::numeric AS entry_pin_gas,
    (s.estimate->>'entry_discount')::numeric AS entry_discount,
    (s.estimate->>'calldata_gas')::numeric AS calldata_gas,
    coalesce(s.estimate->>'companion_declared_gas',s.capacity->>'companion_declared_gas')::numeric AS companion_declared_gas,
    (s.estimate->>'companion_sim_gas')::numeric AS companion_simulation_gas,
    coalesce(s.estimate->>'projected_before',s.capacity->>'projected_before')::numeric AS projected_before,
    (s.estimate->>'projected_after')::numeric AS projected_after,
    CASE WHEN s.accepted IS NOT NULL THEN (s.estimate->>'projected_after')::numeric-(s.estimate->>'projected_before')::numeric END AS accepted_delta_gas,
    c.transaction_hash AS postbatch_hash,c.canonical_l1_block,c.reason,c.history,
    (s.preflight->>'preflight_used_gas')::numeric AS source_preflight_gas,
    (s.preflight->>'preflight_required_gas')::numeric AS source_required_gas,
    (CASE WHEN s.accepted IS NOT NULL THEN coalesce(r.gas_used,t.gas_used) END)::numeric(78,0) AS canonical_source_receipt_gas,
    (s.estimate->>'entry_reserve_gas')::numeric AS entry_reserve_gas,
    (s.estimate->>'l1_call_count')::numeric AS l1_call_count,
    (s.estimate->>'nested_result_count')::numeric AS nested_result_count,
    s.estimate->'estimate_reasons' AS estimate_reasons,
    (s.estimate->>'batch_discount_revoked')::boolean AS batch_discount_revoked,
    r.coverage AS source_receipt_coverage,
    r.success AS source_receipt_success,
    (s.estimate->>'companion_estimate_gas')::numeric AS companion_estimate_gas,
    (s.estimate->>'companion_protocol_reserve_gas')::numeric AS companion_protocol_reserve_gas,
    s.estimate->>'companion_estimate_basis' AS companion_estimate_basis,
    (s.estimate->>'companion_target_sim_gas')::numeric AS companion_target_simulation_gas,
    s.outcome->>'reason' AS source_outcome_reason,
    s.capacity->>'basis' AS capacity_check_basis,
    (s.capacity->>'minimum_entry_gas')::numeric AS minimum_entry_gas,
    (s.capacity->>'companion_prefix_gas')::numeric AS companion_prefix_gas,
    (s.capacity->>'companion_required_gas')::numeric AS companion_required_gas,
    (s.capacity->>'postbatch_planning_cap')::numeric AS capacity_postbatch_cap,
    (s.capacity->>'whole_block_cap')::numeric AS capacity_block_cap,
    (s.capacity->>'l2_used_gas')::numeric AS capacity_l2_used_gas,
    (s.capacity->>'minimum_l2_gas')::numeric AS minimum_l2_gas,
    (s.capacity->>'l2_block_cap')::numeric AS capacity_l2_block_cap
FROM sources s JOIN eez_indexer.admission_candidates c USING(candidate_id)
LEFT JOIN eez_indexer.transactions post ON post.transaction_hash=c.transaction_hash AND post.block_hash=c.canonical_l1_hash
LEFT JOIN eez_indexer.transactions t ON t.transaction_hash=s.source_hash
    AND t.block_hash=c.canonical_l1_hash AND t.transaction_index>post.transaction_index
    AND coalesce(s.estimate->>'direction',s.selected->>'direction')='inbound'
LEFT JOIN eez_indexer.admission_source_receipts r ON r.block_hash=c.canonical_l1_hash
    AND r.postbatch_hash=c.transaction_hash AND r.source_hash=s.source_hash
    AND s.accepted IS NOT NULL AND coalesce(s.estimate->>'direction',s.selected->>'direction')='inbound';
