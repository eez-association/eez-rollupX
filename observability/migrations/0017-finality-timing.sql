-- Preserve the first actual observer finalization time. Already-finalized rows
-- deliberately stay NULL: migration time is not historical finality evidence.
ALTER TABLE eez.blocks ADD COLUMN finalized_observed_at TIMESTAMPTZ;
CREATE FUNCTION eez.capture_finalization_time() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' THEN
   NEW.finalized_observed_at=CASE WHEN NEW.finalized THEN statement_timestamp() END;
 ELSIF OLD.finalized_observed_at IS NOT NULL THEN
   NEW.finalized_observed_at=OLD.finalized_observed_at;
 ELSIF NEW.finalized AND NOT OLD.finalized THEN
   NEW.finalized_observed_at=statement_timestamp();
 ELSE NEW.finalized_observed_at=NULL;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER capture_finalization_time BEFORE INSERT OR UPDATE ON eez.blocks
 FOR EACH ROW EXECUTE FUNCTION eez.capture_finalization_time();

-- Hash/number membership is mandatory. A same-height historical prefix/root
-- that no longer has an exact current canonical range cannot acquire finality.
CREATE VIEW eez.lifecycle_finality_dependencies AS
 SELECT a.source_id,a.epoch,a.sequence,a.context_l1,a.context_l2,
   dependency.number,dependency.hash,
   (dependency.number=0 AND dependency.hash=l2.genesis_hash) AS genesis,
   proof.number IS NOT NULL AS finalized,
   proof.observed_at
 FROM eez.lifecycle_outcome_anchors a
 JOIN eez.chain_instances l2 ON l2.id=a.context_l2
 CROSS JOIN LATERAL (
   SELECT a.l2_snapshot_number AS number,a.l2_snapshot_hash AS hash
   UNION SELECT a.applied_endpoint_number,a.applied_endpoint_hash WHERE a.applied_endpoint_hash IS NOT NULL
   UNION SELECT a.source_block_number,a.source_block_hash WHERE a.direction='outbound' AND a.source_block_hash IS NOT NULL
 ) dependency
 LEFT JOIN LATERAL (
   SELECT f.number,f.observed_at
   FROM eez.settlement_finality f
   JOIN eez.verified_l2_finality current ON current.l2_chain_id=f.l2_chain_id
     AND current.number=f.number AND current.hash=f.hash
   JOIN eez.settlements s ON s.id=f.settlement_id AND s.l1_chain_id=a.context_l1
   WHERE f.l2_chain_id=a.context_l2 AND f.number=dependency.number AND f.hash=dependency.hash
 ) proof ON true;

CREATE VIEW eez.lifecycle_finality_claims AS
 SELECT a.source_id,a.epoch,a.sequence,a.context_l1,a.context_l2,
   a.admission_epoch,a.admission_sequence,a.canonical_state,
   a.observed_at_ms AS terminal_observed_at_ms,a.elapsed_ns,
   CASE WHEN a.canonical_state<>'verified' THEN 'canonical_unknown'
     WHEN NOT snapshot.finalized THEN 'awaiting_l1_finality'
     WHEN NOT dependencies.complete THEN 'awaiting_exact_l2_finality'
     WHEN snapshot.finalized_observed_at IS NULL THEN 'historical_finality_time_unknown'
     ELSE 'finalized' END AS finality_state,
   CASE WHEN snapshot.finalized AND dependencies.complete AND snapshot.finalized_observed_at IS NOT NULL
     THEN GREATEST(snapshot.finalized_observed_at,dependencies.observed_at) END AS finality_observed_at
 FROM eez.lifecycle_outcome_anchors a
 JOIN eez.blocks snapshot ON snapshot.chain_instance_id=a.context_l1 AND snapshot.hash=a.l1_snapshot_hash
   AND snapshot.number=a.l1_snapshot_number
 CROSS JOIN LATERAL (
   SELECT bool_and(d.genesis OR d.finalized) AS complete,
     max(d.observed_at) FILTER(WHERE NOT d.genesis) AS observed_at
   FROM eez.lifecycle_finality_dependencies d WHERE d.source_id=a.source_id AND d.epoch=a.epoch
     AND d.sequence=a.sequence AND d.context_l1=a.context_l1 AND d.context_l2=a.context_l2
 ) dependencies;

-- Only an actual Admitted event defines the UTC start. Native monotonic
-- completion durations remain separate. This is observed finality, including
-- indexing delay, with an explicit synchronized-UTC assumption, not consensus
-- finality time or a sub-second cross-host clock guarantee.
CREATE VIEW eez.lifecycle_admission_finality AS
 WITH timings AS (
 SELECT admission.source_id,admission.deployment_id,admission.service,admission.instance,
   admission.admission_epoch,admission.admission_sequence,admission.source,
   admission.observed_at_ms AS admitted_at_ms,admission.first_seen_at,admission.source_valid,
   admission.status AS admission_status,admission.outcome,admission.evidence_class,
   canonical.context_l1,canonical.context_l2,
   claims.finality_state,claims.finality_observed_at,claims.clock_consistent
 FROM eez.lifecycle_admission_status admission
 LEFT JOIN eez.lifecycle_canonical_outcomes canonical ON canonical.source_id=admission.source_id
   AND canonical.admission_epoch=admission.admission_epoch AND canonical.admission_sequence=admission.admission_sequence
   AND canonical.evidence_class='node_reconciled' AND admission.status='canonical'
 CROSS JOIN LATERAL (
   SELECT CASE WHEN count(*)=0 THEN 'canonical_unknown'
     WHEN bool_and(claim.finality_state='finalized') THEN 'finalized'
     ELSE min(claim.finality_state) FILTER(WHERE claim.finality_state<>'finalized') END AS finality_state,
     max(claim.finality_observed_at) AS finality_observed_at,
     bool_and(claim.terminal_observed_at_ms>=admission.observed_at_ms
       AND abs((claim.terminal_observed_at_ms-admission.observed_at_ms)/1000.0-claim.elapsed_ns/1e9)<=5)
       AS clock_consistent
   FROM eez.lifecycle_finality_claims claim WHERE claim.source_id=admission.source_id
     AND claim.admission_epoch=admission.admission_epoch AND claim.admission_sequence=admission.admission_sequence
     AND claim.context_l1=canonical.context_l1 AND claim.context_l2=canonical.context_l2
     AND claim.canonical_state='verified'
 ) claims
 ), classified AS (
 SELECT timings.*,CASE
   WHEN NOT COALESCE(source_valid,false) THEN 'stale_lifecycle'
   WHEN admission_status='local_policy' THEN 'local_policy_not_applicable'
   WHEN admission_status<>'canonical' THEN 'canonical_unknown'
   WHEN finality_state<>'finalized' THEN finality_state
   WHEN admitted_at_ms>extract(epoch FROM statement_timestamp())*1000+5000
     OR finality_observed_at>statement_timestamp()+interval '5 seconds'
     OR admitted_at_ms>extract(epoch FROM first_seen_at)*1000+5000
     OR NOT COALESCE(clock_consistent,false) THEN 'clock_inconsistent'
   WHEN finality_observed_at<to_timestamp(admitted_at_ms/1000.0) THEN 'finality_observed_before_admission'
   ELSE 'observed_finalized' END AS timing_state
 FROM timings
 )
 SELECT classified.*,
   CASE WHEN timing_state='observed_finalized' THEN
     extract(epoch FROM finality_observed_at)-admitted_at_ms/1000.0 END AS accepted_to_observed_finality_seconds,
   'synchronized_utc_assumed_not_verified'::text AS clock_basis
 FROM classified;

GRANT SELECT ON eez.lifecycle_finality_dependencies,eez.lifecycle_finality_claims,
 eez.lifecycle_admission_finality TO eez_observer_writer,eez_observer_reader;
