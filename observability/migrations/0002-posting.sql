-- Classification is separate from immutable transaction identity and accounting.
-- Existing canonical charges stay available while old calldata is backfilled.
CREATE TABLE eez.posting_analysis (
  chain_instance_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'other', 'decoded', 'invalid', 'unverified')),
  calldata_hash eez.hash32,
  last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  observed_at TIMESTAMPTZ,
  collection_error VARCHAR(64) CHECK (collection_error ~ '^[a-z_]+$'),
  PRIMARY KEY (chain_instance_id, transaction_hash),
  FOREIGN KEY (chain_instance_id, transaction_hash) REFERENCES eez.transactions(chain_instance_id, hash),
  CHECK ((state IN ('pending','other') AND calldata_hash IS NULL)
      OR (state IN ('decoded','invalid','unverified') AND calldata_hash IS NOT NULL)),
  CHECK ((state = 'pending') = (observed_at IS NULL))
);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state)
  SELECT chain_instance_id,hash,'pending' FROM eez.transactions;
CREATE INDEX pending_posting_analysis ON eez.posting_analysis(chain_instance_id,last_attempt_at) WHERE state='pending';

CREATE FUNCTION eez.guard_posting_analysis() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF ROW(NEW.chain_instance_id,NEW.transaction_hash) IS DISTINCT FROM ROW(OLD.chain_instance_id,OLD.transaction_hash)
     OR (OLD.state <> 'pending' AND ROW(NEW.state,NEW.calldata_hash) IS DISTINCT FROM ROW(OLD.state,OLD.calldata_hash)) THEN
    RAISE EXCEPTION 'decoded transaction classification is immutable' USING ERRCODE='23000';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER posting_analysis_guard BEFORE UPDATE ON eez.posting_analysis
  FOR EACH ROW EXECUTE FUNCTION eez.guard_posting_analysis();

-- EXISTS prevents duplicate configuration entries from multiplying a receipt.
CREATE VIEW eez.wallet_receipts AS
  SELECT r.* FROM eez.canonical_receipts r
  JOIN eez.chain_instances c ON c.id=r.chain_instance_id
  WHERE EXISTS (SELECT FROM jsonb_array_elements_text(COALESCE(c.selection->'wallets','[]')) w(address)
                WHERE lower(w.address)='0x'||encode(r.sender,'hex'));
CREATE VIEW eez.wallet_cost_totals AS
  SELECT chain_instance_id,deployment_id,role,chain_id,genesis_hash,rollup_id,sender,
    bootstrap_number,frozen_reason,last_success_at,collection_error,source_valid,
    count(*) AS observed_receipts,
    count(*) FILTER (WHERE accounting_gap IS NULL) AS accounted_receipts,
    count(*) FILTER (WHERE accounting_gap IS NOT NULL) AS incomplete_receipts,
    sum(execution_paid) AS known_execution_paid,sum(blob_paid) AS known_blob_paid,
    sum(execution_paid+blob_paid) AS known_total_paid,
    count(*) FILTER (WHERE NOT success) AS reverted_receipts
  FROM eez.wallet_receipts
  GROUP BY chain_instance_id,deployment_id,role,chain_id,genesis_hash,rollup_id,sender,
    bootstrap_number,frozen_reason,last_success_at,collection_error,source_valid;

-- These are paid calls to configured contracts with an ABI-decoded posting
-- method. Reverted calls still count. Successful application is a separate claim.
CREATE VIEW eez.posting_receipts AS
  SELECT r.*,a.calldata_hash FROM eez.canonical_receipts r
  JOIN eez.chain_instances c ON c.id=r.chain_instance_id
  JOIN eez.posting_analysis a ON a.chain_instance_id=r.chain_instance_id AND a.transaction_hash=r.transaction_hash
  WHERE r.role='l1' AND a.state='decoded'
    AND EXISTS (SELECT FROM jsonb_array_elements_text(COALESCE(c.selection->'contracts','[]')) contract(address)
                WHERE lower(contract.address)='0x'||encode(r.recipient,'hex'));
CREATE VIEW eez.posting_coverage AS
  SELECT r.chain_instance_id,r.deployment_id,r.role,r.source_valid,
    count(*) AS tracked_contract_receipts,
    count(*) FILTER (WHERE a.state='decoded') AS decoded_post_receipts,
    count(*) FILTER (WHERE a.state='invalid') AS invalid_post_calldata_receipts,
    count(*) FILTER (WHERE a.state='other') AS other_method_receipts,
    count(*) FILTER (WHERE a.state IS NULL OR a.state IN ('pending','unverified')) AS unknown_method_receipts,
    count(*) FILTER (WHERE a.collection_error IS NOT NULL) AS method_collection_errors,
    max(a.last_attempt_at) AS last_method_attempt_at
  FROM eez.canonical_receipts r
  JOIN eez.chain_instances c ON c.id=r.chain_instance_id
  LEFT JOIN eez.posting_analysis a ON a.chain_instance_id=r.chain_instance_id AND a.transaction_hash=r.transaction_hash
  WHERE r.role='l1'
    AND EXISTS (SELECT FROM jsonb_array_elements_text(COALESCE(c.selection->'contracts','[]')) contract(address)
                WHERE lower(contract.address)='0x'||encode(r.recipient,'hex'))
  GROUP BY r.chain_instance_id,r.deployment_id,r.role,r.source_valid;

GRANT SELECT,INSERT,UPDATE ON eez.posting_analysis TO eez_observer_writer;
GRANT SELECT ON eez.wallet_receipts,eez.wallet_cost_totals,eez.posting_receipts,eez.posting_coverage
  TO eez_observer_writer,eez_observer_reader;
