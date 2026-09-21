-- Lifecycle observations are unsampled best-effort evidence, not queue durability.
CREATE DOMAIN eez.epoch16 AS BYTEA CHECK(octet_length(VALUE)=16);
CREATE TABLE eez.lifecycle_sources (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  deployment_id BIGINT NOT NULL REFERENCES eez.deployments(id),
  service VARCHAR(64) NOT NULL CHECK(service ~ '^[A-Za-z0-9_.-]+$'),
  instance VARCHAR(64) NOT NULL CHECK(instance ~ '^[A-Za-z0-9_.-]+$'),
  directory TEXT NOT NULL,
  inventory_generation BIGINT NOT NULL DEFAULT 0 CHECK(inventory_generation>=0),
  last_success_at TIMESTAMPTZ,
  collection_error VARCHAR(64),
  UNIQUE(deployment_id,service,instance)
);
CREATE TABLE eez.lifecycle_epochs (
  source_id BIGINT NOT NULL REFERENCES eez.lifecycle_sources(id),
  epoch eez.epoch16 NOT NULL,
  started_at_ms eez.u64,
  process_id BIGINT,
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  liveness TEXT NOT NULL DEFAULT 'unknown' CHECK(liveness IN ('unknown','live','ended')),
  last_probe_at TIMESTAMPTZ,
  ended_at TIMESTAMPTZ,
  fully_scanned BOOLEAN NOT NULL DEFAULT false,
  PRIMARY KEY(source_id,epoch)
);
CREATE TABLE eez.lifecycle_segments (
  source_id BIGINT NOT NULL,
  epoch eez.epoch16 NOT NULL,
  segment_id eez.epoch16 NOT NULL,
  header_digest eez.hash32,
  offset_bytes BIGINT NOT NULL DEFAULT 0 CHECK(offset_bytes>=0),
  snapshot_bytes BIGINT NOT NULL DEFAULT 0 CHECK(snapshot_bytes>=0),
  present BOOLEAN NOT NULL DEFAULT true,
  scan_complete BOOLEAN NOT NULL DEFAULT false,
  last_scan_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  missing_since TIMESTAMPTZ,
  partial_since TIMESTAMPTZ,
  read_error VARCHAR(64),
  PRIMARY KEY(source_id,epoch,segment_id),
  FOREIGN KEY(source_id,epoch) REFERENCES eez.lifecycle_epochs(source_id,epoch)
);
CREATE INDEX lifecycle_scan_order ON eez.lifecycle_segments(source_id,last_scan_at,epoch,segment_id);
-- Raw quarantine bytes are writer/admin-only, never available to Grafana.
CREATE TABLE eez.lifecycle_records (
  source_id BIGINT NOT NULL,
  epoch eez.epoch16 NOT NULL,
  segment_id eez.epoch16 NOT NULL,
  offset_bytes BIGINT NOT NULL CHECK(offset_bytes>=0),
  digest eez.hash32 NOT NULL,
  raw BYTEA NOT NULL CHECK(octet_length(raw)<=16385),
  reason VARCHAR(64),
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(source_id,epoch,segment_id,offset_bytes,digest),
  FOREIGN KEY(source_id,epoch,segment_id) REFERENCES eez.lifecycle_segments(source_id,epoch,segment_id)
);
CREATE TABLE eez.lifecycle_events (
  source_id BIGINT NOT NULL,
  epoch eez.epoch16 NOT NULL,
  sequence eez.u64 NOT NULL CHECK(sequence>0),
  chunk_count INTEGER NOT NULL CHECK(chunk_count BETWEEN 1 AND 1366),
  state TEXT NOT NULL DEFAULT 'incomplete' CHECK(state IN ('incomplete','complete','invalid','conflict')),
  payload JSONB,
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_assembly_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
  PRIMARY KEY(source_id,epoch,sequence),
  FOREIGN KEY(source_id,epoch) REFERENCES eez.lifecycle_epochs(source_id,epoch),
  CHECK((state='complete') = (payload IS NOT NULL))
);
CREATE INDEX lifecycle_assembly_queue ON eez.lifecycle_events(source_id,last_assembly_at,epoch,sequence) WHERE state='incomplete';
CREATE TABLE eez.lifecycle_chunks (
  source_id BIGINT NOT NULL,
  epoch eez.epoch16 NOT NULL,
  sequence eez.u64 NOT NULL,
  chunk_index INTEGER NOT NULL CHECK(chunk_index BETWEEN 0 AND 1365),
  segment_id eez.epoch16 NOT NULL,
  offset_bytes BIGINT NOT NULL,
  record JSONB NOT NULL,
  raw_digest eez.hash32 NOT NULL,
  PRIMARY KEY(source_id,epoch,sequence,chunk_index),
  FOREIGN KEY(source_id,epoch,sequence) REFERENCES eez.lifecycle_events(source_id,epoch,sequence),
  FOREIGN KEY(source_id,epoch,segment_id,offset_bytes,raw_digest) REFERENCES eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE eez.lifecycle_controls (
  source_id BIGINT NOT NULL,
  epoch eez.epoch16 NOT NULL,
  control_sequence eez.u64 NOT NULL CHECK(control_sequence>0),
  last_assigned_sequence eez.u64 NOT NULL,
  observed_at_ms eez.u64 NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('started','active','segment_closed','stopped')),
  stats JSONB NOT NULL,
  raw_digest eez.hash32 NOT NULL,
  conflicted BOOLEAN NOT NULL DEFAULT false,
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(source_id,epoch,control_sequence),
  FOREIGN KEY(source_id,epoch) REFERENCES eez.lifecycle_epochs(source_id,epoch)
);
CREATE FUNCTION eez.guard_lifecycle_record() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF ROW(NEW.source_id,NEW.epoch,NEW.segment_id,NEW.offset_bytes,NEW.digest,NEW.raw,NEW.observed_at)
   IS DISTINCT FROM ROW(OLD.source_id,OLD.epoch,OLD.segment_id,OLD.offset_bytes,OLD.digest,OLD.raw,OLD.observed_at)
   OR (OLD.reason IS NOT NULL AND NEW.reason IS DISTINCT FROM OLD.reason) THEN
   RAISE EXCEPTION 'lifecycle raw evidence is immutable' USING ERRCODE='23000';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER lifecycle_record_guard BEFORE UPDATE ON eez.lifecycle_records FOR EACH ROW EXECUTE FUNCTION eez.guard_lifecycle_record();
CREATE TRIGGER lifecycle_chunk_guard BEFORE UPDATE ON eez.lifecycle_chunks FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();

CREATE VIEW eez.lifecycle_event_history AS
 SELECT s.deployment_id,s.service,s.instance,e.epoch,e.sequence,e.payload,e.first_seen_at,
   (s.collection_error IS NULL AND s.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') AS source_valid,
   CASE WHEN epoch.liveness='live' AND epoch.last_probe_at<now()-interval '90 seconds' THEN 'unknown' ELSE epoch.liveness END AS liveness,epoch.last_probe_at,epoch.ended_at
 FROM eez.lifecycle_events e JOIN eez.lifecycle_sources s ON s.id=e.source_id
 JOIN eez.lifecycle_epochs epoch ON epoch.source_id=e.source_id AND epoch.epoch=e.epoch
 WHERE e.state='complete';

-- Intervals, never generate_series over emitter-controlled sequence numbers.
-- The next observation establishes when a hole first became observable. A late
-- complete event removes its hole; invalid/conflicted evidence stays explicit.
CREATE VIEW eez.lifecycle_gaps AS
 WITH high_water AS (
   SELECT source_id,epoch,sequence,first_seen_at FROM eez.lifecycle_events
   UNION ALL
   SELECT source_id,epoch,last_assigned_sequence,observed_at FROM eez.lifecycle_controls WHERE NOT conflicted
 ), bounds AS (
   SELECT source_id,epoch,max(sequence) AS highest FROM high_water GROUP BY source_id,epoch
 ), points AS (
   SELECT source_id,epoch,sequence,first_seen_at AS seen_at FROM eez.lifecycle_events
   UNION ALL
   SELECT b.source_id,b.epoch,b.highest+1,min(h.first_seen_at) FROM bounds b
     JOIN high_water h ON h.source_id=b.source_id AND h.epoch=b.epoch AND h.sequence=b.highest
     GROUP BY b.source_id,b.epoch,b.highest
 ), ordered AS (
   SELECT *,lag(sequence,1,0::numeric) OVER(PARTITION BY source_id,epoch ORDER BY sequence) AS previous FROM points
 ), gaps AS (
   SELECT source_id,epoch,previous+1 AS first_sequence,sequence-1 AS last_sequence,seen_at AS detected_at,'sequence_hole'::text AS reason
     FROM ordered WHERE sequence>previous+1
   UNION ALL
   SELECT source_id,epoch,sequence,sequence,first_seen_at,state FROM eez.lifecycle_events event WHERE state IN ('invalid','conflict') OR (state='incomplete' AND chunk_count<>(SELECT count(*) FROM eez.lifecycle_chunks c WHERE c.source_id=event.source_id AND c.epoch=event.epoch AND c.sequence=event.sequence))
 )
 SELECT g.*,CASE WHEN g.reason IN ('invalid','conflict') OR EXISTS(SELECT FROM eez.lifecycle_records r WHERE r.source_id=g.source_id AND r.epoch=g.epoch AND r.reason IN ('invalid_or_unknown_record','invalid_or_unknown_header','conflicting_epoch_header','changed_segment_header')) THEN 'quarantined'
   WHEN e.liveness='ended' AND e.fully_scanned THEN 'unrecoverable_from_spool'
   WHEN g.detected_at<=now()-interval '60 seconds' THEN 'suspected_missing' ELSE 'provisional' END AS status
 FROM gaps g JOIN eez.lifecycle_epochs e USING(source_id,epoch);

-- Preserve cumulative drop uncertainty as reported, not as an exact lost range.
CREATE VIEW eez.lifecycle_epoch_status AS
 SELECT e.source_id,s.deployment_id,s.service,s.instance,e.epoch,e.started_at_ms,e.process_id,e.first_seen_at,
   CASE WHEN e.liveness='live' AND e.last_probe_at<now()-interval '90 seconds' THEN 'unknown' ELSE e.liveness END AS liveness,
   e.last_probe_at,e.ended_at,e.fully_scanned
 FROM eez.lifecycle_epochs e JOIN eez.lifecycle_sources s ON s.id=e.source_id;

CREATE VIEW eez.lifecycle_writer_status AS
 SELECT DISTINCT ON(c.source_id,c.epoch) c.source_id,s.deployment_id,s.service,s.instance,c.epoch,
   c.control_sequence,c.observed_at_ms,c.observed_at,c.conflicted,
   CASE WHEN NOT c.conflicted THEN c.last_assigned_sequence END AS last_assigned_sequence,
   CASE WHEN NOT c.conflicted THEN c.status END AS writer_status,
   CASE WHEN NOT c.conflicted THEN c.stats END AS reported_stats
 FROM eez.lifecycle_controls c JOIN eez.lifecycle_sources s ON s.id=c.source_id
 ORDER BY c.source_id,c.epoch,c.control_sequence DESC;

CREATE VIEW eez.lifecycle_coverage AS
 SELECT s.id AS source_id,s.deployment_id,s.service,s.instance,s.last_success_at,s.collection_error,
   (SELECT count(*) FROM eez.lifecycle_events e WHERE e.source_id=s.id AND state='complete') AS complete_events,
   (SELECT count(*) FROM eez.lifecycle_gaps g WHERE g.source_id=s.id) AS gap_intervals,
   (SELECT count(*) FROM eez.lifecycle_records r WHERE r.source_id=s.id AND reason IS NOT NULL) AS quarantined_records,
   (SELECT count(*) FROM eez.lifecycle_segments seg WHERE seg.source_id=s.id AND NOT present AND NOT scan_complete) AS lost_segments,
   (SELECT count(*) FROM eez.lifecycle_segments seg WHERE seg.source_id=s.id AND partial_since<=now()-interval '60 seconds') AS stale_partial_segments,
   (SELECT count(*) FROM eez.lifecycle_epochs e WHERE e.source_id=s.id AND e.liveness='ended' AND NOT EXISTS(
      SELECT FROM eez.lifecycle_controls c WHERE c.source_id=e.source_id AND c.epoch=e.epoch AND c.status='stopped' AND NOT c.conflicted AND c.control_sequence=(SELECT max(last.control_sequence) FROM eez.lifecycle_controls last WHERE last.source_id=e.source_id AND last.epoch=e.epoch))) AS unknown_trailing_epochs,
   (SELECT count(*) FROM eez.lifecycle_segments seg WHERE seg.source_id=s.id AND read_error IS NOT NULL) AS unreadable_segments,
   (SELECT count(*) FROM eez.lifecycle_events e WHERE e.source_id=s.id AND state='incomplete') AS pending_events,
   (SELECT count(*) FROM eez.lifecycle_epochs e WHERE e.source_id=s.id AND (e.liveness='unknown' OR (e.liveness='live' AND e.last_probe_at<now()-interval '90 seconds'))) AS unknown_liveness_epochs,
   (SELECT count(*) FROM eez.lifecycle_segments seg WHERE seg.source_id=s.id AND seg.present AND NOT seg.scan_complete) AS pending_segments
 FROM eez.lifecycle_sources s;

GRANT SELECT,INSERT,UPDATE ON eez.lifecycle_sources,eez.lifecycle_epochs,eez.lifecycle_segments,eez.lifecycle_records,
  eez.lifecycle_events,eez.lifecycle_chunks,eez.lifecycle_controls TO eez_observer_writer;
GRANT USAGE,SELECT ON SEQUENCE eez.lifecycle_sources_id_seq TO eez_observer_writer;
GRANT SELECT ON eez.lifecycle_event_history,eez.lifecycle_gaps,eez.lifecycle_coverage,eez.lifecycle_writer_status,eez.lifecycle_epoch_status TO eez_observer_writer,eez_observer_reader;
