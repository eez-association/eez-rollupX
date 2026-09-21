-- Blob bytes are independent of canonical receipt costs. Content hashes and
-- decoded-carrier metadata are immutable and reusable across reinclusions.
CREATE TABLE eez.blob_metadata (
 chain_instance_id BIGINT NOT NULL,
 transaction_hash eez.hash32 NOT NULL,
 calldata_hash eez.hash32 NOT NULL,
 blob_hashes eez.hash32[] NOT NULL CHECK(array_ndims(blob_hashes)=1 AND cardinality(blob_hashes) BETWEEN 1 AND 64 AND array_position(blob_hashes,NULL) IS NULL),
 blob_indices eez.u256[] NOT NULL CHECK(cardinality(blob_indices)<=64 AND array_position(blob_indices,NULL) IS NULL AND (cardinality(blob_indices)=0 OR array_ndims(blob_indices)=1)),
 calldata_tail_hash eez.hash32 NOT NULL,
 observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(chain_instance_id,transaction_hash),
 FOREIGN KEY(chain_instance_id,transaction_hash) REFERENCES eez.transactions(chain_instance_id,hash)
);
CREATE TRIGGER blob_metadata_guard BEFORE UPDATE ON eez.blob_metadata
 FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();
CREATE TABLE eez.blob_attempts (
 chain_instance_id BIGINT NOT NULL,
 transaction_hash eez.hash32 NOT NULL,
 last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 retrieval_duration_ns eez.u64,
 state TEXT NOT NULL CHECK(state IN ('interrupted','metadata_unavailable','invalid_metadata','unsupported_carrier','limit','unavailable','pruned','invalid_response','authentication_failed','unsupported_codec','invalid_stream','transport','http','rpc','unsupported','malformed','identity_mismatch','oversized','measured')),
 PRIMARY KEY(chain_instance_id,transaction_hash),
 FOREIGN KEY(chain_instance_id,transaction_hash) REFERENCES eez.transactions(chain_instance_id,hash)
);
CREATE TABLE eez.blob_measurements (
 chain_instance_id BIGINT NOT NULL,
 transaction_hash eez.hash32 NOT NULL,
 codec_version SMALLINT NOT NULL CHECK(codec_version=0),
 encoded_bytes eez.u64 NOT NULL,
 purchased_blobs INTEGER NOT NULL CHECK(purchased_blobs BETWEEN 1 AND 64),
 usable_bytes_per_blob INTEGER NOT NULL CHECK(usable_bytes_per_blob=126976),
 retrieval TEXT NOT NULL CHECK(retrieval IN ('beacon_blobs','beacon_sidecars')),
 measured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(chain_instance_id,transaction_hash),
 FOREIGN KEY(chain_instance_id,transaction_hash) REFERENCES eez.blob_metadata(chain_instance_id,transaction_hash),
 CHECK(encoded_bytes BETWEEN 2 AND purchased_blobs::bigint*usable_bytes_per_blob)
);
CREATE TRIGGER blob_measurement_guard BEFORE UPDATE ON eez.blob_measurements
 FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();
CREATE VIEW eez.posting_blob_measurements AS
 SELECT post.*,metadata.blob_hashes,metadata.blob_indices,metadata.calldata_tail_hash,
   measurement.codec_version,measurement.encoded_bytes,measurement.usable_bytes_per_blob,
   measurement.retrieval,measurement.measured_at,attempt.last_attempt_at,attempt.retrieval_duration_ns,
   CASE WHEN measurement.transaction_hash IS NOT NULL AND post.purchased_blobs=measurement.purchased_blobs
          THEN 'measured'
        WHEN measurement.transaction_hash IS NOT NULL THEN 'receipt_count_unknown'
        ELSE COALESCE(attempt.state,'not_measured') END AS measurement_state,
   CASE WHEN measurement.transaction_hash IS NOT NULL AND post.source_valid THEN 'authenticated_canonical_bytes' ELSE 'unknown' END AS measurement_provenance,
   EXISTS(SELECT FROM eez.canonical_settled_l2_blocks settlement WHERE settlement.l1_chain_id=post.chain_instance_id
     AND settlement.settlement_transaction_hash=post.transaction_hash AND settlement.settlement_block_hash=post.block_hash
     AND settlement.source_valid) AS has_applied_settlement
 FROM eez.posting_receipts post
 LEFT JOIN eez.blob_metadata metadata USING(chain_instance_id,transaction_hash)
 LEFT JOIN eez.blob_measurements measurement USING(chain_instance_id,transaction_hash)
 LEFT JOIN eez.blob_attempts attempt USING(chain_instance_id,transaction_hash)
 WHERE post.transaction_type=3;
CREATE VIEW eez.posting_blob_totals AS
 SELECT chain_instance_id,deployment_id,role,source_valid,
   count(*) AS posts,count(*) FILTER(WHERE NOT success) AS reverted_posts,
   count(*) FILTER(WHERE has_applied_settlement) AS posts_with_applied_settlement,
   count(*) FILTER(WHERE measurement_state='measured') AS measured_posts,
   sum(purchased_blobs) AS known_purchased_blobs,
   sum(purchased_blobs) FILTER(WHERE measurement_state='measured') AS measured_purchased_blobs,
   sum(encoded_bytes) FILTER(WHERE measurement_state='measured') AS measured_encoded_bytes,
   sum(encoded_bytes) FILTER(WHERE measurement_state='measured')/
     NULLIF(sum(purchased_blobs*usable_bytes_per_blob) FILTER(WHERE measurement_state='measured'),0) AS purchased_capacity_utilization,
   count(*) FILTER(WHERE measurement_state='measured')::numeric/NULLIF(count(*),0) AS measurement_coverage
 FROM eez.posting_blob_measurements
 GROUP BY chain_instance_id,deployment_id,role,source_valid;
GRANT SELECT,INSERT,UPDATE ON eez.blob_metadata,eez.blob_measurements,eez.blob_attempts TO eez_observer_writer;
GRANT SELECT ON eez.blob_metadata,eez.blob_measurements,eez.posting_blob_measurements,eez.posting_blob_totals TO eez_observer_reader,eez_observer_writer;
