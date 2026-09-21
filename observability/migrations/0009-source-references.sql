-- A retained member can recover receipt facts even when its admission record
-- was lost. Extract only explicit scoped Source objects, never infer identity
-- from a bare hash or fabricate an admission/runtime terminal decision.
CREATE FUNCTION eez.lifecycle_event_sources(payload JSONB) RETURNS SETOF JSONB
 LANGUAGE SQL IMMUTABLE STRICT PARALLEL SAFE AS $$
 SELECT DISTINCT source FROM jsonb_path_query(payload,
   CASE payload->'event'->>'event'
     WHEN 'admitted' THEN '$.event.source'::jsonpath
     WHEN 'readiness' THEN '$.event.member.source'::jsonpath
     WHEN 'terminal' THEN '$.event.member.source'::jsonpath
     WHEN 'preparation_started' THEN '$.event.members[*].source'::jsonpath
     WHEN 'attempt_prepared' THEN '$.event.members[*].source'::jsonpath
     WHEN 'reconciled' THEN '$.event.details.resolutions[*].member.source'::jsonpath
     ELSE '$.absent_source_reference'::jsonpath
   END) source WHERE jsonb_typeof(source)='object'
$$;
REVOKE ALL ON FUNCTION eez.lifecycle_event_sources(JSONB) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION eez.lifecycle_event_sources(JSONB) TO eez_observer_writer,eez_observer_reader;
CREATE INDEX lifecycle_source_reference_projection ON eez.lifecycle_events(source_id,epoch,sequence)
 WHERE state='complete' AND payload->'event'->>'event' IN
 ('admitted','readiness','terminal','preparation_started','attempt_prepared','reconciled');
CREATE VIEW eez.lifecycle_declared_sources AS
 SELECT e.source_id,e.epoch,e.sequence,s.source,e.payload->'event'->>'event' AS event_kind,
   e.payload->>'observed_at_unix_ms' AS observed_at_ms
 FROM eez.lifecycle_events e CROSS JOIN LATERAL eez.lifecycle_event_sources(e.payload) s(source)
 WHERE e.state='complete' AND e.payload->'event'->>'event' IN
 ('admitted','readiness','terminal','preparation_started','attempt_prepared','reconciled');
CREATE VIEW eez.lifecycle_source_references AS
 SELECT DISTINCT link.source_id,link.epoch,link.sequence,link.chain_instance_id,link.transaction_hash,
   declared.source,declared.observed_at_ms,declared.event_kind
 FROM eez.lifecycle_source_links link
 JOIN eez.lifecycle_declared_sources declared USING(source_id,epoch,sequence,source)
 WHERE link.state='linked';
CREATE OR REPLACE VIEW eez.lifecycle_source_admissions AS
 SELECT source_id,epoch,sequence,chain_instance_id,transaction_hash,source,observed_at_ms
 FROM eez.lifecycle_source_references WHERE event_kind='admitted';
CREATE OR REPLACE VIEW eez.lifecycle_source_link_status AS
 SELECT link.source_id,link.epoch,link.sequence,link.context_l1,link.context_l2,link.state,link.chain_instance_id,link.transaction_hash
 FROM eez.lifecycle_source_links link
 JOIN eez.lifecycle_declared_sources declared USING(source_id,epoch,sequence,source);

CREATE OR REPLACE VIEW eez.source_receipts AS
 SELECT r.*,
   CASE WHEN r.source_valid THEN 'receipt_verified' ELSE 'unknown' END AS evidence_class,
   CASE WHEN r.role='l2' AND r.source_valid AND EXISTS(SELECT FROM eez.canonical_settled_l2_blocks applied
     WHERE applied.l2_chain_id=r.chain_instance_id AND applied.hash=r.block_hash AND applied.number=r.block_number AND applied.source_valid)
     THEN true END AS l2_settlement_verified,
   CASE WHEN r.role='l2' AND r.source_valid AND EXISTS(SELECT FROM eez.verified_l2_finality finality
     WHERE finality.l2_chain_id=r.chain_instance_id AND finality.hash=r.block_hash AND finality.number=r.block_number)
     THEN true END AS l2_finality_verified
 FROM eez.canonical_receipt_facts r
 WHERE EXISTS(SELECT FROM eez.lifecycle_source_references a
   WHERE a.chain_instance_id=r.chain_instance_id AND a.transaction_hash=r.transaction_hash);
CREATE OR REPLACE VIEW eez.source_receipt_coverage AS
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
   AND EXISTS(SELECT FROM eez.lifecycle_source_references a WHERE a.chain_instance_id=c.id AND a.transaction_hash=request.transaction_hash)
 LEFT JOIN eez.source_receipts fact ON fact.chain_instance_id=c.id AND fact.transaction_hash=request.transaction_hash
 GROUP BY c.id;

CREATE OR REPLACE VIEW eez.source_receipt_lookup_status AS
 SELECT request.* FROM eez.source_receipt_requests request
 WHERE EXISTS(SELECT FROM eez.lifecycle_source_references a
   WHERE a.chain_instance_id=request.chain_instance_id AND a.transaction_hash=request.transaction_hash);


GRANT SELECT ON eez.lifecycle_declared_sources,eez.lifecycle_source_references TO eez_observer_writer,eez_observer_reader;
