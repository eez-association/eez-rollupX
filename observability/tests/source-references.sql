-- SQL extraction is deliberately independent of outcome interpretation. Runtime
-- integration and identity/receipt assertions live in smoke-database.py.
BEGIN;
SET LOCAL ROLE eez_observer_reader;
DO $$
DECLARE
 source JSONB := '{"chain_id":1,"genesis_hash":"0101010101010101010101010101010101010101010101010101010101010101","rollup_id":0,"transaction_hash":"0202020202020202020202020202020202020202020202020202020202020202","direction":"inbound"}';
 member JSONB;
 event JSONB;
 actual JSONB;
 count_sources BIGINT;
BEGIN
 member:=jsonb_build_object('source',source,'admission',jsonb_build_object('epoch',repeat('01',16),'sequence',1));
 FOR event IN SELECT value FROM jsonb_array_elements(jsonb_build_array(
   jsonb_build_object('event','admitted','source',source),
   jsonb_build_object('event','readiness','member',member),
   jsonb_build_object('event','terminal','member',member),
   jsonb_build_object('event','preparation_started','members',jsonb_build_array(member,member)),
   jsonb_build_object('event','attempt_prepared','members',jsonb_build_array(member,member)),
   jsonb_build_object('event','reconciled','details',jsonb_build_object('resolutions',jsonb_build_array(jsonb_build_object('member',member),jsonb_build_object('member',member))))
 )) LOOP
   SELECT count(*),jsonb_agg(value)->0 INTO count_sources,actual
   FROM eez.lifecycle_event_sources(jsonb_build_object('event',event)) value;
   IF count_sources<>1 OR actual IS DISTINCT FROM source THEN RAISE EXCEPTION 'source extraction/dedup failed: %',event; END IF;
 END LOOP;
 FOR event IN SELECT value FROM jsonb_array_elements(jsonb_build_array(
   jsonb_build_object('event','reconciled'),
   jsonb_build_object('event','reconciled','details',NULL),
   jsonb_build_object('event','reconciled','details',jsonb_build_object('sources',jsonb_build_array(jsonb_build_object('source_hash',repeat('02',32),'direction','inbound')))),
   jsonb_build_object('event','attempt_prepared','members',jsonb_build_array()),
   jsonb_build_object('event','dispatched','transaction_hash',repeat('02',32)),
   jsonb_build_object('event','unsupported','source',source)
 )) LOOP
   SELECT count(*) INTO count_sources FROM eez.lifecycle_event_sources(jsonb_build_object('event',event));
   IF count_sources<>0 THEN RAISE EXCEPTION 'inferred absent source: %',event; END IF;
 END LOOP;
END $$;
ROLLBACK;
SELECT 'source reference extraction invariants passed';
