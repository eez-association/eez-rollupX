-- Log evidence is independent of billing facts. Retain at most two distinct
-- complete claims per inclusion: a second claim permanently prevents promotion.
CREATE TABLE eez.protocol_log_claims (
  chain_instance_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  block_hash eez.hash32 NOT NULL,
  digest eez.hash32 NOT NULL,
  events JSONB NOT NULL CHECK(jsonb_typeof(events)='array' AND jsonb_array_length(events)<=4096),
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(chain_instance_id,transaction_hash,block_hash,digest),
  FOREIGN KEY(chain_instance_id,transaction_hash,block_hash)
    REFERENCES eez.receipt_observations(chain_instance_id,transaction_hash,block_hash)
);
CREATE TABLE eez.protocol_log_attempts (
  chain_instance_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  block_hash eez.hash32 NOT NULL,
  last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_success_at TIMESTAMPTZ,
  collection_error VARCHAR(64),
  PRIMARY KEY(chain_instance_id,transaction_hash,block_hash),
  FOREIGN KEY(chain_instance_id,transaction_hash,block_hash)
    REFERENCES eez.receipt_observations(chain_instance_id,transaction_hash,block_hash)
);
CREATE VIEW eez.protocol_log_coverage AS
 SELECT r.chain_instance_id,r.transaction_hash,r.block_hash,r.block_number,r.transaction_index,
   r.source_valid,r.finalized,a.last_attempt_at,a.last_success_at,a.collection_error,
   CASE WHEN claim.count>1 THEN 'conflict' WHEN claim.count=1 THEN 'complete'
        WHEN a.collection_error IS NOT NULL THEN 'gap' ELSE 'pending' END AS state
 FROM eez.canonical_receipt_facts r
 LEFT JOIN eez.protocol_log_attempts a USING(chain_instance_id,transaction_hash,block_hash)
 CROSS JOIN LATERAL (SELECT count(*) FROM eez.protocol_log_claims c
   WHERE c.chain_instance_id=r.chain_instance_id AND c.transaction_hash=r.transaction_hash AND c.block_hash=r.block_hash) claim
 WHERE r.role='l1';
CREATE VIEW eez.canonical_protocol_log_sets AS
 SELECT coverage.*,claim.events,claim.digest,claim.observed_at
 FROM eez.protocol_log_coverage coverage
 JOIN eez.protocol_log_claims claim USING(chain_instance_id,transaction_hash,block_hash)
 WHERE coverage.state='complete';
-- Readers must filter the exact configured EEZ emitter and rollup before using
-- these decoded observations as protocol evidence. Empty sets are meaningful.
CREATE VIEW eez.canonical_protocol_logs AS
 SELECT logs.chain_instance_id,logs.transaction_hash,logs.block_hash,logs.block_number,
   logs.transaction_index,logs.source_valid,logs.finalized,
   decode(event->>'address','hex') AS address,(event->>'index')::eez.u64 AS log_index,
   (event->>'rollup_id')::eez.u64 AS rollup_id,event->>'kind' AS kind,
   decode(event->>'root','hex') AS root,decode(event->>'call_hash','hex') AS call_hash,
   (event->>'queue_index')::NUMERIC(78,0) AS queue_index
 FROM eez.canonical_protocol_log_sets logs CROSS JOIN LATERAL jsonb_array_elements(logs.events) event;
GRANT SELECT,INSERT ON eez.protocol_log_claims TO eez_observer_writer;
GRANT SELECT,INSERT,UPDATE ON eez.protocol_log_attempts TO eez_observer_writer;
GRANT SELECT ON eez.protocol_log_coverage,eez.canonical_protocol_log_sets,eez.canonical_protocol_logs
 TO eez_observer_writer,eez_observer_reader;
-- Diagnostic access includes orphaned/conflicting observations; these raw tables
-- make no canonical or outcome assurance claim.
GRANT SELECT ON eez.protocol_log_claims,eez.protocol_log_attempts TO eez_observer_reader;
