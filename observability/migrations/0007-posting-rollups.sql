-- Preserve the verification table of an exactly decoded posting call. NULL is
-- historical/missing evidence; [] is a decoded empty table, never unknown.
ALTER TABLE eez.posting_analysis ADD COLUMN verification_rollups eez.u64[]
  CHECK(verification_rollups IS NULL OR (state='decoded'
    AND COALESCE(array_ndims(verification_rollups),1)=1
    AND COALESCE(array_lower(verification_rollups,1),1)=1
    AND array_position(verification_rollups,NULL) IS NULL));
CREATE INDEX pending_posting_rollups ON eez.posting_analysis(chain_instance_id,last_attempt_at)
  WHERE state='decoded' AND verification_rollups IS NULL;
CREATE OR REPLACE FUNCTION eez.guard_posting_analysis() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF ROW(NEW.chain_instance_id,NEW.transaction_hash) IS DISTINCT FROM ROW(OLD.chain_instance_id,OLD.transaction_hash)
     OR (OLD.state <> 'pending' AND ROW(NEW.state,NEW.calldata_hash) IS DISTINCT FROM ROW(OLD.state,OLD.calldata_hash))
     OR (OLD.verification_rollups IS NOT NULL AND NEW.verification_rollups IS DISTINCT FROM OLD.verification_rollups) THEN
    RAISE EXCEPTION 'decoded transaction classification is immutable' USING ERRCODE='23000';
  END IF;
  RETURN NEW;
END $$;

CREATE VIEW eez.posting_rollup_evidence AS
 SELECT p.*,a.verification_rollups,a.last_attempt_at AS rollup_last_attempt_at,
   a.collection_error AS rollup_collection_error
 FROM eez.posting_receipts p JOIN eez.posting_analysis a
 ON a.chain_instance_id=p.chain_instance_id AND a.transaction_hash=p.transaction_hash;
-- The decoded table alone proves neither proof validity nor applied effects.
-- Queue ownership consumers additionally require exact contract, canonical
-- inclusion, successful receipt, and complete membership/occurrence evidence.
CREATE VIEW eez.posting_rollup_membership AS
 SELECT p.chain_instance_id,p.transaction_hash,p.block_hash,p.block_number,p.transaction_index,
   p.recipient,p.success,p.source_valid,p.calldata_hash,
   entry.id AS verification_rollup_id,entry.ordinality AS table_position
 FROM eez.posting_rollup_evidence p
 CROSS JOIN LATERAL unnest(p.verification_rollups) WITH ORDINALITY entry(id,ordinality);
GRANT SELECT ON eez.posting_rollup_evidence,eez.posting_rollup_membership TO eez_observer_writer,eez_observer_reader;
