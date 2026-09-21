-- Derived native coherence is deliberately separate from canonical assurance.
-- Exact transport digests fence cached checks against late conflicts/replays.
CREATE VIEW eez.lifecycle_native_records AS
 SELECT e.source_id,e.epoch,e.sequence,e.payload,
   chunk.record->>'digest' AS digest
 FROM eez.lifecycle_events e JOIN eez.lifecycle_chunks chunk
 ON chunk.source_id=e.source_id AND chunk.epoch=e.epoch AND chunk.sequence=e.sequence
   AND chunk.chunk_index=e.chunk_count-1
 WHERE e.state='complete' AND chunk.record->>'complete'='true'
   AND chunk.record->>'digest' ~ '^[0-9a-f]{64}$';

CREATE INDEX lifecycle_attempt_generation ON eez.lifecycle_events
 (source_id,epoch,(COALESCE(payload#>>'{event,attempt,generation}',payload#>>'{event,replacement,generation}')),sequence)
 WHERE state='complete' AND payload->'event'->>'event' IN ('attempt_prepared','attempt_repriced');
CREATE VIEW eez.lifecycle_native_lineage AS
 SELECT record.*,COALESCE(payload#>>'{event,attempt,generation}',payload#>>'{event,replacement,generation}') AS generation
 FROM eez.lifecycle_native_records record
 WHERE payload->'event'->>'event' IN ('attempt_prepared','attempt_repriced');

CREATE TABLE eez.lifecycle_native_checks (
 source_id BIGINT NOT NULL,
 epoch eez.epoch16 NOT NULL,
 sequence eez.u64 NOT NULL,
 context_l1 BIGINT NOT NULL REFERENCES eez.chain_instances(id),
 context_l2 BIGINT NOT NULL REFERENCES eez.chain_instances(id),
 last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 state TEXT NOT NULL CHECK(state IN ('coherent','unsupported_claim','missing_record','missing_parent','identity',
   'resolution','missing_membership','lineage','source','references','limit')),
 record_refs JSONB NOT NULL CHECK(jsonb_typeof(record_refs)='array' AND jsonb_array_length(record_refs)<=130),
 parent_sequence eez.u64,
 generation eez.u64,
 decision JSONB,
 PRIMARY KEY(source_id,epoch,sequence,context_l1,context_l2),
 FOREIGN KEY(source_id,epoch,sequence) REFERENCES eez.lifecycle_events(source_id,epoch,sequence),
 CHECK((state='coherent')=(decision IS NOT NULL)),
 CHECK(state<>'coherent' OR (parent_sequence IS NOT NULL AND generation IS NOT NULL
   AND jsonb_array_length(record_refs)>=3))
);

-- A new complete preparation/repricing record can change the previously checked
-- lineage, even when all old records still exist. Invalidate that cache as well.
CREATE VIEW eez.lifecycle_native_outcomes AS
 SELECT check_result.*,
   CASE WHEN state<>'coherent' THEN state
     WHEN EXISTS(SELECT FROM jsonb_array_elements(record_refs) ref WHERE NOT EXISTS(
       SELECT FROM eez.lifecycle_native_records record
       WHERE record.source_id=check_result.source_id
         AND record.epoch=decode(ref->>'epoch','hex')
         AND record.sequence=(ref->>'sequence')::eez.u64 AND record.digest=ref->>'digest'))
       THEN 'changed_evidence'
     WHEN EXISTS(SELECT FROM eez.lifecycle_native_lineage record
       WHERE record.source_id=check_result.source_id AND record.epoch=check_result.epoch
         AND record.sequence<check_result.parent_sequence AND record.generation=check_result.generation::text
         AND NOT EXISTS(SELECT FROM jsonb_array_elements(record_refs) ref
           WHERE record.epoch=decode(ref->>'epoch','hex') AND record.sequence=(ref->>'sequence')::eez.u64
             AND record.digest=ref->>'digest')) THEN 'changed_lineage'
     ELSE 'coherent' END AS native_state,
   'unknown'::text AS evidence_class
 FROM eez.lifecycle_native_checks check_result;
GRANT SELECT,INSERT,UPDATE ON eez.lifecycle_native_checks TO eez_observer_writer;
GRANT SELECT ON eez.lifecycle_native_records,eez.lifecycle_native_lineage,eez.lifecycle_native_outcomes
 TO eez_observer_writer,eez_observer_reader;
