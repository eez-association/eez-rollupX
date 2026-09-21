-- Header/body observations are independent of configured receipt selection.
-- Existing history has unknown metadata; never backfill zeros from absent rows.
ALTER TABLE eez.blocks
 ADD COLUMN transaction_count eez.u64,
 ADD COLUMN gas_used eez.u64,
 ADD COLUMN gas_limit eez.u64,
 ADD COLUMN blob_gas_used eez.u64,
 ADD COLUMN excess_blob_gas eez.u64,
 ADD CONSTRAINT block_gas_bounds CHECK (gas_used IS NULL OR gas_limit IS NULL OR gas_used <= gas_limit),
 ADD CONSTRAINT block_blob_gas_units CHECK (blob_gas_used IS NULL OR mod(blob_gas_used,131072)=0);

CREATE FUNCTION eez.guard_block_activity() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF (OLD.transaction_count IS NOT NULL AND NEW.transaction_count IS DISTINCT FROM OLD.transaction_count)
 OR (OLD.gas_used IS NOT NULL AND NEW.gas_used IS DISTINCT FROM OLD.gas_used)
 OR (OLD.gas_limit IS NOT NULL AND NEW.gas_limit IS DISTINCT FROM OLD.gas_limit)
 OR (OLD.blob_gas_used IS NOT NULL AND NEW.blob_gas_used IS DISTINCT FROM OLD.blob_gas_used)
 OR (OLD.excess_blob_gas IS NOT NULL AND NEW.excess_blob_gas IS DISTINCT FROM OLD.excess_blob_gas) THEN
  RAISE EXCEPTION 'known block activity is immutable' USING ERRCODE='23000';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER block_activity_guard BEFORE UPDATE ON eez.blocks
 FOR EACH ROW EXECUTE FUNCTION eez.guard_block_activity();

CREATE VIEW eez.canonical_block_activity AS
 SELECT c.deployment_id,b.chain_instance_id,c.role AS chain,b.hash AS block_hash,
   b.number AS block_number,b.timestamp_seconds,b.transaction_count,b.gas_used,b.gas_limit,
   b.blob_gas_used,b.excess_blob_gas,
   CASE WHEN parent.timestamp_seconds<=b.timestamp_seconds
     THEN b.timestamp_seconds-parent.timestamp_seconds END AS interval_seconds,
   (c.frozen_reason IS NULL AND c.collection_error IS NULL
    AND c.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') IS TRUE AS source_valid
 FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id
 LEFT JOIN eez.blocks parent ON parent.chain_instance_id=b.chain_instance_id
   AND parent.hash=b.parent_hash AND parent.number+1=b.number AND parent.canonical
 WHERE b.canonical;
GRANT SELECT ON eez.canonical_block_activity TO eez_observer_writer,eez_observer_reader;
