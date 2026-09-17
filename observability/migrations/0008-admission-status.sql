-- These views derive from currently complete events. A late transport conflict
-- therefore retracts evidence immediately, without a stale materialized decision.
CREATE INDEX lifecycle_terminal_admission ON eez.lifecycle_events(
  source_id,(decode(payload#>>'{event,member,admission,epoch}','hex')),
  ((payload#>>'{event,member,admission,sequence}')::numeric))
 WHERE state='complete' AND payload->'event'->>'event'='terminal';

CREATE VIEW eez.lifecycle_admission_observations AS
 SELECT e.source_id,s.deployment_id,s.service,s.instance,e.epoch AS admission_epoch,
   e.sequence AS admission_sequence,e.payload->'event'->'source' AS source,
   e.payload->'event'->'previous' AS previous_admission,
   e.payload->'event'->'replaced' AS replaced_admission,
   (e.payload->>'observed_at_unix_ms')::eez.u64 AS observed_at_ms,e.first_seen_at,
   (s.collection_error IS NULL AND s.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') AS source_valid,
   epoch.liveness,epoch.last_probe_at,epoch.ended_at,epoch.fully_scanned
 FROM eez.lifecycle_events e JOIN eez.lifecycle_sources s ON s.id=e.source_id
 JOIN eez.lifecycle_epoch_status epoch ON epoch.source_id=e.source_id AND epoch.epoch=e.epoch
 WHERE e.state='complete' AND e.payload->'event'->>'event'='admitted';

CREATE VIEW eez.lifecycle_terminal_observations AS
 SELECT e.source_id,e.epoch,e.sequence,e.payload->'event'->'member' AS member,
   e.payload#>'{event,member,source}' AS source,
   decode(e.payload#>>'{event,member,admission,epoch}','hex') AS admission_epoch,
   (e.payload#>>'{event,member,admission,sequence}')::eez.u64 AS admission_sequence,
   e.payload->'event'->'attempt' AS attempt,
   e.payload->'event'->>'outcome' AS claimed_outcome,
   e.payload->'event'->>'assurance' AS claimed_assurance,
   e.payload->'event'->'anchor' AS anchor,
   e.payload->'event'->'reconciliation' AS reconciliation,
   (e.payload->'event'->>'elapsed_ns')::eez.u64 AS elapsed_ns,
   (e.payload->>'observed_at_unix_ms')::eez.u64 AS observed_at_ms,e.first_seen_at
 FROM eez.lifecycle_events e
 WHERE e.state='complete' AND e.payload->'event'->>'event'='terminal';

CREATE VIEW eez.lifecycle_local_policy_evidence AS
 SELECT t.*,
   CASE
     WHEN a.source_id IS NULL THEN 'missing_admission'
     WHEN a.source IS DISTINCT FROM t.source THEN 'source_mismatch'
     WHEN t.epoch<>t.admission_epoch OR t.sequence<=t.admission_sequence THEN 'invalid_admission_order'
     WHEN COALESCE(t.attempt,'null'::jsonb)<>'null'::jsonb AND (t.attempt->>'epoch') IS DISTINCT FROM encode(t.epoch,'hex') THEN 'invalid_ownership_scope'
     WHEN t.claimed_assurance IS DISTINCT FROM 'local_policy' OR t.claimed_outcome IS NULL OR t.claimed_outcome NOT IN ('attempts_exhausted','nonce_gap','pool_discarded','replaced') THEN 'requires_canonical_validation'
     WHEN COALESCE(t.anchor,'null'::jsonb)<>'null'::jsonb THEN 'unexpected_anchor'
     WHEN COALESCE(t.reconciliation,'null'::jsonb)<>'null'::jsonb AND NOT EXISTS(
       SELECT FROM eez.lifecycle_events parent
       CROSS JOIN LATERAL (
         SELECT count(*) AS members,bool_and(
           resolution->'member'=t.member AND resolution->'owner'=t.attempt
           AND (resolution->>'elapsed_ns')::eez.u64=t.elapsed_ns
           AND resolution->'disposition'=jsonb_build_object('state','terminal','outcome',t.claimed_outcome,'assurance','local_policy')
         ) AS matches
         FROM jsonb_array_elements(parent.payload#>'{event,details,resolutions}') resolution
         WHERE resolution#>'{member,admission}'=t.member->'admission'
       ) partition
       WHERE parent.source_id=t.source_id AND parent.state='complete'
         AND parent.payload->'event'->>'event'='reconciled'
         AND parent.epoch=decode(t.reconciliation->>'epoch','hex')
         AND parent.sequence=(t.reconciliation->>'sequence')::eez.u64
         AND parent.epoch=t.epoch AND parent.sequence<t.sequence AND parent.sequence>t.admission_sequence
         AND partition.members=1 AND partition.matches
     ) THEN 'unresolved_reconciliation'
     ELSE 'local_policy'
   END AS evidence_state
 FROM eez.lifecycle_terminal_observations t
 LEFT JOIN eez.lifecycle_admission_observations a USING(source_id,admission_epoch,admission_sequence);

-- A single verified policy outcome can have duplicate observations. Other or
-- contradictory terminal claims keep the admission unresolved until evaluated;
-- do not choose a winner by wall clock or by random process-epoch ordering.
CREATE VIEW eez.lifecycle_admission_status AS
 SELECT admission.*,terminal.claims AS terminal_claims,
   terminal.policy_claims,terminal.outcomes AS distinct_claimed_outcomes,terminal.decisions AS distinct_claimed_decisions,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN 'local_policy' WHEN admission.liveness='ended' THEN 'unresolved_restart' ELSE 'unresolved' END AS status,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN terminal.outcome END AS outcome,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN 'local_policy' ELSE 'unknown' END AS evidence_class
 FROM eez.lifecycle_admission_observations admission
 CROSS JOIN LATERAL (
   SELECT count(*) AS claims,count(*) FILTER(WHERE evidence_state='local_policy') AS policy_claims,
     count(DISTINCT claimed_outcome) AS outcomes,
     count(DISTINCT (claimed_outcome,attempt,reconciliation,elapsed_ns)) AS decisions,
     min(claimed_outcome) AS outcome
   FROM eez.lifecycle_local_policy_evidence t
   WHERE t.source_id=admission.source_id AND t.admission_epoch=admission.admission_epoch
     AND t.admission_sequence=admission.admission_sequence
 ) terminal;
GRANT SELECT ON eez.lifecycle_admission_observations,eez.lifecycle_terminal_observations,
 eez.lifecycle_local_policy_evidence,eez.lifecycle_admission_status TO eez_observer_writer,eez_observer_reader;
