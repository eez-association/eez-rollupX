-- Node-reported applied ranges are separate evidence from paid posting calls.
CREATE TABLE eez.correlation_sources (
  l1_chain_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  l2_chain_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  last_success_at TIMESTAMPTZ,
  collection_error VARCHAR(64),
  frozen_reason TEXT CHECK (frozen_reason IN ('l2_finalized_contradiction')),
  PRIMARY KEY(l1_chain_id,l2_chain_id)
);
CREATE TABLE eez.correlation_runs (
  l1_chain_id BIGINT NOT NULL,
  l2_chain_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  block_hash eez.hash32 NOT NULL,
  last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_success_at TIMESTAMPTZ,
  collection_error VARCHAR(64),
  range_observed BOOLEAN NOT NULL DEFAULT false,
  PRIMARY KEY(l1_chain_id,l2_chain_id,transaction_hash,block_hash),
  FOREIGN KEY(l1_chain_id,l2_chain_id) REFERENCES eez.correlation_sources(l1_chain_id,l2_chain_id),
  FOREIGN KEY(l1_chain_id,transaction_hash,block_hash) REFERENCES eez.receipt_observations(chain_instance_id,transaction_hash,block_hash)
);
CREATE TABLE eez.settlements (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  l1_chain_id BIGINT NOT NULL,
  l2_chain_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  block_hash eez.hash32 NOT NULL,
  transaction_index eez.u64 NOT NULL,
  block_count INTEGER NOT NULL CHECK(block_count BETWEEN 1 AND 4096),
  evidence_digest eez.hash32 NOT NULL,
  reported_canonical_l2 BOOLEAN NOT NULL,
  evidence_class TEXT NOT NULL DEFAULT 'node_reconciled' CHECK(evidence_class='node_reconciled'),
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  range_verified_at TIMESTAMPTZ,
  last_validation_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_finality_attempt_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
  finality_done BOOLEAN NOT NULL DEFAULT false,
  UNIQUE(l1_chain_id,l2_chain_id,transaction_hash,block_hash,evidence_digest),
  FOREIGN KEY(l1_chain_id,l2_chain_id,transaction_hash,block_hash)
    REFERENCES eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash)
);
CREATE INDEX pending_settlement_validation ON eez.settlements(l1_chain_id,l2_chain_id,last_validation_at,id) WHERE range_verified_at IS NULL;
CREATE INDEX pending_settlement_finality ON eez.settlements(l1_chain_id,l2_chain_id,last_finality_attempt_at,id) WHERE range_verified_at IS NOT NULL AND NOT finality_done;
CREATE TABLE eez.settlement_blocks (
  settlement_id BIGINT NOT NULL REFERENCES eez.settlements(id),
  ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4095),
  number eez.u64 NOT NULL,
  hash eez.hash32 NOT NULL,
  PRIMARY KEY(settlement_id,ordinal),
  UNIQUE(settlement_id,number),
  UNIQUE(settlement_id,hash)
);
CREATE FUNCTION eez.guard_settlement() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF ROW(NEW.id,NEW.l1_chain_id,NEW.l2_chain_id,NEW.transaction_hash,NEW.block_hash,NEW.transaction_index,
         NEW.block_count,NEW.evidence_digest,NEW.reported_canonical_l2,NEW.evidence_class,NEW.observed_at)
     IS DISTINCT FROM ROW(OLD.id,OLD.l1_chain_id,OLD.l2_chain_id,OLD.transaction_hash,OLD.block_hash,OLD.transaction_index,
         OLD.block_count,OLD.evidence_digest,OLD.reported_canonical_l2,OLD.evidence_class,OLD.observed_at)
     OR (OLD.range_verified_at IS NOT NULL AND NEW.range_verified_at IS DISTINCT FROM OLD.range_verified_at)
     OR (OLD.finality_done AND NOT NEW.finality_done) THEN
    RAISE EXCEPTION 'settlement observation is immutable' USING ERRCODE='23000';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER settlement_guard BEFORE UPDATE ON eez.settlements FOR EACH ROW EXECUTE FUNCTION eez.guard_settlement();
CREATE TRIGGER settlement_block_guard BEFORE UPDATE ON eez.settlement_blocks FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();

-- Multiple different claims for one inclusion invalidate the stronger claim;
-- all versions remain stored. Receipt facts and fees remain independently usable.
CREATE VIEW eez.settlement_evidence AS
 SELECT s.*,p.block_number AS l1_block_number,p.finalized AS l1_finalized,
   (SELECT count(*) FROM eez.settlements other WHERE other.l1_chain_id=s.l1_chain_id AND other.l2_chain_id=s.l2_chain_id
     AND other.transaction_hash=s.transaction_hash AND other.block_hash=s.block_hash) AS claim_count,
   (p.source_valid AND c2.frozen_reason IS NULL AND c2.collection_error IS NULL
      AND c2.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds'
      AND source.frozen_reason IS NULL AND source.collection_error IS NULL
      AND source.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') AS source_valid
 FROM eez.settlements s
 JOIN eez.posting_receipts p ON p.chain_instance_id=s.l1_chain_id AND p.transaction_hash=s.transaction_hash AND p.block_hash=s.block_hash
 JOIN eez.chain_instances c2 ON c2.id=s.l2_chain_id
 JOIN eez.correlation_sources source USING(l1_chain_id,l2_chain_id)
 WHERE p.success AND p.transaction_index=s.transaction_index;
CREATE VIEW eez.canonical_settled_l2_blocks AS
 SELECT s.id AS settlement_id,s.l1_chain_id,s.l2_chain_id,s.transaction_hash AS settlement_transaction_hash,
   s.block_hash AS settlement_block_hash,s.l1_block_number,s.l1_finalized,s.evidence_class,
   applied.number,applied.hash,s.source_valid
 FROM eez.settlement_evidence s
 JOIN eez.settlement_blocks applied ON applied.settlement_id=s.id
 JOIN eez.blocks b ON b.chain_instance_id=s.l2_chain_id AND b.number=applied.number AND b.hash=applied.hash AND b.canonical
 WHERE s.range_verified_at IS NOT NULL AND s.claim_count=1;

-- Retain the first observed finalized application proof. L2 receipt indexing can
-- still reconcile unsafe data; a contradiction freezes this finality signal.
CREATE TABLE eez.settlement_finality (
  settlement_id BIGINT NOT NULL REFERENCES eez.settlements(id),
  l2_chain_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  number eez.u64 NOT NULL,
  hash eez.hash32 NOT NULL,
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(l2_chain_id,number)
);
CREATE TRIGGER settlement_finality_guard BEFORE UPDATE ON eez.settlement_finality FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();
CREATE VIEW eez.verified_l2_finality AS
 SELECT DISTINCT f.l2_chain_id,f.number,f.hash FROM eez.settlement_finality f
 JOIN eez.canonical_settled_l2_blocks b ON b.l2_chain_id=f.l2_chain_id AND b.hash=f.hash AND b.number=f.number
 WHERE b.source_valid AND b.l1_finalized;
CREATE VIEW eez.settlement_coverage AS
 SELECT source.l1_chain_id,source.l2_chain_id,source.last_success_at,source.collection_error,source.frozen_reason,
   count(p.transaction_hash) AS candidate_posts,
   count(p.transaction_hash) FILTER(WHERE run.range_observed IS NOT TRUE) AS posts_without_observed_range,
   count(p.transaction_hash) FILTER(WHERE run.collection_error IS NOT NULL) AS collection_errors,
   count(p.transaction_hash) FILTER(WHERE NOT EXISTS(SELECT FROM eez.settlement_evidence evidence
     WHERE evidence.l1_chain_id=source.l1_chain_id AND evidence.l2_chain_id=source.l2_chain_id
       AND evidence.transaction_hash=p.transaction_hash AND evidence.block_hash=p.block_hash
       AND evidence.range_verified_at IS NOT NULL AND evidence.claim_count=1)) AS posts_without_verified_range,
   count(p.transaction_hash) FILTER(WHERE run.collection_error IS NOT NULL OR NOT EXISTS(SELECT FROM eez.settlement_evidence evidence
     WHERE evidence.l1_chain_id=source.l1_chain_id AND evidence.l2_chain_id=source.l2_chain_id
       AND evidence.transaction_hash=p.transaction_hash AND evidence.block_hash=p.block_hash
       AND evidence.range_verified_at IS NOT NULL AND evidence.claim_count=1)) AS posts_with_gaps,
   (SELECT count(*) FROM eez.settlements s JOIN eez.blocks b ON b.chain_instance_id=s.l1_chain_id AND b.hash=s.block_hash
      WHERE s.l1_chain_id=source.l1_chain_id AND s.l2_chain_id=source.l2_chain_id
        AND s.range_verified_at IS NOT NULL AND NOT s.finality_done AND b.canonical AND b.finalized) AS finality_pending_ranges
 FROM eez.correlation_sources source
 LEFT JOIN eez.posting_receipts p ON p.chain_instance_id=source.l1_chain_id AND p.success
 LEFT JOIN eez.correlation_runs run ON run.l1_chain_id=source.l1_chain_id AND run.l2_chain_id=source.l2_chain_id
   AND run.transaction_hash=p.transaction_hash AND run.block_hash=p.block_hash
 GROUP BY source.l1_chain_id,source.l2_chain_id;

GRANT SELECT,INSERT,UPDATE ON eez.correlation_sources,eez.correlation_runs,eez.settlements,eez.settlement_blocks TO eez_observer_writer;
GRANT SELECT,INSERT ON eez.settlement_finality TO eez_observer_writer;
GRANT USAGE,SELECT ON SEQUENCE eez.settlements_id_seq TO eez_observer_writer;
GRANT SELECT ON eez.settlement_evidence,eez.canonical_settled_l2_blocks,eez.verified_l2_finality,eez.settlement_coverage
 TO eez_observer_writer,eez_observer_reader;
