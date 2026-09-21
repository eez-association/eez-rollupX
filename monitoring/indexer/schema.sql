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
-- Re-enrich older records once for L2 range fees, retaining known details.
UPDATE eez_indexer.details SET retry_at=now()
WHERE data->>'version' IS DISTINCT FROM '3';
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
