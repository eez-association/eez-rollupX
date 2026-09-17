-- Keep configured receipt selection distinct from bounded lifecycle dependencies.
-- Existing transactions were all selected by the original scanner.
CREATE TABLE eez.selected_transactions (
  chain_instance_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  PRIMARY KEY(chain_instance_id,transaction_hash),
  FOREIGN KEY(chain_instance_id,transaction_hash) REFERENCES eez.transactions(chain_instance_id,hash)
);
INSERT INTO eez.selected_transactions SELECT chain_instance_id,hash FROM eez.transactions;
CREATE TRIGGER selected_transaction_guard BEFORE UPDATE ON eez.selected_transactions
  FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();

-- One immutable billing fact per canonical inclusion, including dependencies.
CREATE VIEW eez.canonical_receipt_facts AS
  SELECT r.*, b.number AS block_number, b.timestamp_seconds, b.finalized,
         t.sender, t.recipient, t.transaction_type, t.method_selector,
         c.deployment_id, c.role, c.chain_id, c.genesis_hash, c.rollup_id,
         c.bootstrap_number, c.frozen_reason, c.last_success_at, c.collection_error,
         (c.frozen_reason IS NULL AND c.collection_error IS NULL AND c.last_success_at IS NOT NULL
          AND c.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') AS source_valid
  FROM eez.receipt_observations r
  JOIN eez.blocks b ON b.chain_instance_id=r.chain_instance_id AND b.hash=r.block_hash
  JOIN eez.transactions t ON t.chain_instance_id=r.chain_instance_id AND t.hash=r.transaction_hash
  JOIN eez.chain_instances c ON c.id=r.chain_instance_id
  WHERE r.canonical AND b.canonical;
CREATE OR REPLACE VIEW eez.canonical_receipts AS
  SELECT r.* FROM eez.canonical_receipt_facts r
  WHERE EXISTS(SELECT FROM eez.selected_transactions s
    WHERE s.chain_instance_id=r.chain_instance_id AND s.transaction_hash=r.transaction_hash);

CREATE TABLE eez.lifecycle_source_links (
  source_id BIGINT NOT NULL,
  epoch eez.epoch16 NOT NULL,
  sequence eez.u64 NOT NULL,
  context_l1 BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  context_l2 BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  source JSONB NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('linked','identity_mismatch')),
  chain_instance_id BIGINT REFERENCES eez.chain_instances(id),
  transaction_hash eez.hash32 NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(source_id,epoch,sequence,context_l1,context_l2,source),
  FOREIGN KEY(source_id,epoch,sequence) REFERENCES eez.lifecycle_events(source_id,epoch,sequence),
  CHECK((state='linked')=(chain_instance_id IS NOT NULL))
);
CREATE INDEX lifecycle_source_transaction ON eez.lifecycle_source_links(chain_instance_id,transaction_hash);
CREATE TRIGGER lifecycle_source_link_guard BEFORE UPDATE ON eez.lifecycle_source_links
  FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();
CREATE INDEX lifecycle_admission_projection ON eez.lifecycle_events(source_id,epoch,sequence)
  WHERE state='complete' AND payload->'event'->>'event'='admitted';

CREATE TABLE eez.source_receipt_requests (
  chain_instance_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  transaction_hash eez.hash32 NOT NULL,
  last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
  last_success_at TIMESTAMPTZ,
  observation_status TEXT NOT NULL DEFAULT 'pending' CHECK(observation_status IN ('pending','not_observed','outside_indexed_interval','transaction_unavailable','source_block_not_indexed','observed','accounting_gap','failed')),
  collection_error VARCHAR(64),
  PRIMARY KEY(chain_instance_id,transaction_hash),
  CHECK((observation_status IN ('failed','accounting_gap'))=(collection_error IS NOT NULL))
);
CREATE INDEX source_receipt_attempt_order ON eez.source_receipt_requests(chain_instance_id,last_attempt_at,transaction_hash);

-- A later transport conflict retracts the relationship, without deleting receipt
-- facts independently fetched for the named transaction.
CREATE VIEW eez.lifecycle_source_admissions AS
 SELECT DISTINCT link.source_id,link.epoch,link.sequence,link.chain_instance_id,link.transaction_hash,
   event.payload->'event'->'source' AS source,event.payload->>'observed_at_unix_ms' AS observed_at_ms
 FROM eez.lifecycle_source_links link
 JOIN eez.lifecycle_events event USING(source_id,epoch,sequence)
 WHERE link.state='linked' AND event.state='complete' AND link.source=event.payload->'event'->'source';

CREATE VIEW eez.lifecycle_source_link_status AS
 SELECT link.source_id,link.epoch,link.sequence,link.context_l1,link.context_l2,link.state,link.chain_instance_id,link.transaction_hash
 FROM eez.lifecycle_source_links link JOIN eez.lifecycle_events event USING(source_id,epoch,sequence)
 WHERE event.state='complete' AND link.source=event.payload->'event'->'source';

-- EXISTS deduplicates repeated admissions and multiple native producers. These
-- are source receipt costs, never companion allocation or cross-chain success.
CREATE VIEW eez.source_receipts AS
 SELECT r.*,
   CASE WHEN r.source_valid THEN 'receipt_verified' ELSE 'unknown' END AS evidence_class,
   CASE WHEN r.role='l2' AND r.source_valid AND EXISTS(SELECT FROM eez.canonical_settled_l2_blocks applied
     WHERE applied.l2_chain_id=r.chain_instance_id AND applied.hash=r.block_hash AND applied.number=r.block_number AND applied.source_valid)
     THEN true END AS l2_settlement_verified,
   CASE WHEN r.role='l2' AND r.source_valid AND EXISTS(SELECT FROM eez.verified_l2_finality finality
     WHERE finality.l2_chain_id=r.chain_instance_id AND finality.hash=r.block_hash AND finality.number=r.block_number)
     THEN true END AS l2_finality_verified
 FROM eez.canonical_receipt_facts r
 WHERE EXISTS(SELECT FROM eez.lifecycle_source_admissions a
   WHERE a.chain_instance_id=r.chain_instance_id AND a.transaction_hash=r.transaction_hash);
CREATE VIEW eez.source_receipt_cost_totals AS
 SELECT chain_instance_id,deployment_id,role,chain_id,genesis_hash,rollup_id,
   bootstrap_number,frozen_reason,last_success_at,collection_error,source_valid,
   count(*) AS observed_receipts,
   count(*) FILTER(WHERE accounting_gap IS NULL) AS accounted_receipts,
   count(*) FILTER(WHERE accounting_gap IS NOT NULL) AS incomplete_receipts,
   count(*) FILTER(WHERE NOT success) AS reverted_receipts,
   sum(execution_paid) AS known_execution_paid,sum(blob_paid) AS known_blob_paid,
   sum(execution_paid+blob_paid) AS known_total_paid
 FROM eez.source_receipts
 GROUP BY chain_instance_id,deployment_id,role,chain_id,genesis_hash,rollup_id,
   bootstrap_number,frozen_reason,last_success_at,collection_error,source_valid;
CREATE VIEW eez.source_receipt_coverage AS
 SELECT c.id AS chain_instance_id,c.deployment_id,c.role,
   count(request.transaction_hash) AS known_sources,
   count(request.transaction_hash) FILTER(WHERE fact.transaction_hash IS NULL) AS missing_receipts,
   count(request.transaction_hash) FILTER(WHERE fact.accounting_gap IS NOT NULL) AS accounting_gaps,
   count(request.transaction_hash) FILTER(WHERE request.observation_status='failed') AS lookup_errors,
   count(request.transaction_hash) FILTER(WHERE request.observation_status='not_observed') AS not_observed,
   count(request.transaction_hash) FILTER(WHERE request.observation_status='outside_indexed_interval') AS outside_interval,
   count(request.transaction_hash) FILTER(WHERE request.observation_status='transaction_unavailable') AS unavailable_transactions,
   count(request.transaction_hash) FILTER(WHERE request.observation_status='source_block_not_indexed') AS unindexed_blocks,
   max(request.last_success_at) AS last_lookup_success_at
 FROM eez.chain_instances c
 LEFT JOIN eez.source_receipt_requests request ON request.chain_instance_id=c.id
   AND EXISTS(SELECT FROM eez.lifecycle_source_admissions a WHERE a.chain_instance_id=c.id AND a.transaction_hash=request.transaction_hash)
 LEFT JOIN eez.source_receipts fact ON fact.chain_instance_id=c.id AND fact.transaction_hash=request.transaction_hash
 GROUP BY c.id;

CREATE VIEW eez.source_receipt_lookup_status AS
 SELECT request.* FROM eez.source_receipt_requests request
 WHERE EXISTS(SELECT FROM eez.lifecycle_source_admissions a
   WHERE a.chain_instance_id=request.chain_instance_id AND a.transaction_hash=request.transaction_hash);

GRANT SELECT,INSERT ON eez.selected_transactions,eez.lifecycle_source_links TO eez_observer_writer;
GRANT SELECT,INSERT,UPDATE ON eez.source_receipt_requests TO eez_observer_writer;
GRANT SELECT ON eez.canonical_receipt_facts,eez.lifecycle_source_admissions,eez.lifecycle_source_link_status,
  eez.source_receipts,eez.source_receipt_cost_totals,eez.source_receipt_coverage,eez.source_receipt_lookup_status
  TO eez_observer_writer,eez_observer_reader;
