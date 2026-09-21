-- Applied transactionally by `eez-observer migrate`, never by indexer startup.
CREATE SCHEMA eez;
REVOKE ALL ON SCHEMA eez FROM PUBLIC;

CREATE DOMAIN eez.hash32 AS BYTEA CHECK (octet_length(VALUE) = 32);
CREATE DOMAIN eez.address20 AS BYTEA CHECK (octet_length(VALUE) = 20);
CREATE DOMAIN eez.u64 AS NUMERIC(20,0)
  CHECK (VALUE >= 0 AND VALUE <= 18446744073709551615);
CREATE DOMAIN eez.u256 AS NUMERIC(78,0)
  CHECK (VALUE >= 0 AND VALUE <= 115792089237316195423570985008687907853269984665640564039457584007913129639935);
CREATE DOMAIN eez.product AS NUMERIC(156,0)
  CHECK (VALUE >= 0 AND VALUE <= 13407807929942597099574024998205846127479365820592393377723561443721764030073546976801874298166903427690031858186486050853753882811946569946433649006084095);

CREATE TABLE eez.schema_migrations (
  version INTEGER PRIMARY KEY CHECK (version > 0),
  checksum eez.hash32 NOT NULL,
  applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE eez.deployments (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  environment VARCHAR(64) NOT NULL CHECK (environment ~ '^[A-Za-z0-9_.-]+$'),
  name VARCHAR(64) NOT NULL CHECK (name ~ '^[A-Za-z0-9_.-]+$'),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (environment, name)
);

CREATE TABLE eez.chain_instances (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  deployment_id BIGINT NOT NULL REFERENCES eez.deployments(id),
  role TEXT NOT NULL CHECK (role IN ('l1', 'l2')),
  chain_id eez.u256 NOT NULL,
  genesis_hash eez.hash32 NOT NULL,
  rollup_id eez.u64 NOT NULL,
  bootstrap_number eez.u64 NOT NULL,
  selection JSONB NOT NULL DEFAULT '{}',
  last_success_at TIMESTAMPTZ,
  collection_error VARCHAR(64),
  frozen_reason TEXT CHECK (frozen_reason IN ('finalized_contradiction', 'conflicting_fact')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (deployment_id, role, chain_id, genesis_hash, rollup_id)
);

CREATE TABLE eez.blocks (
  chain_instance_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  hash eez.hash32 NOT NULL,
  number eez.u64 NOT NULL,
  parent_hash eez.hash32 NOT NULL,
  timestamp_seconds eez.u64 NOT NULL,
  canonical BOOLEAN NOT NULL DEFAULT false,
  finalized BOOLEAN NOT NULL DEFAULT false,
  receipt_coverage TEXT NOT NULL CHECK (receipt_coverage IN ('pending', 'complete', 'gap')),
  last_receipt_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (chain_instance_id, hash),
  CHECK (NOT finalized OR canonical)
);
CREATE UNIQUE INDEX canonical_height ON eez.blocks(chain_instance_id, number) WHERE canonical;
CREATE INDEX block_height_history ON eez.blocks(chain_instance_id, number);

CREATE TABLE eez.scan_cursors (
  chain_instance_id BIGINT PRIMARY KEY REFERENCES eez.chain_instances(id),
  block_hash eez.hash32,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY (chain_instance_id, block_hash) REFERENCES eez.blocks(chain_instance_id, hash)
);

CREATE TABLE eez.transactions (
  chain_instance_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  hash eez.hash32 NOT NULL,
  sender eez.address20 NOT NULL,
  recipient eez.address20,
  nonce eez.u256 NOT NULL,
  transaction_type SMALLINT NOT NULL CHECK (transaction_type BETWEEN 0 AND 255),
  method_selector BYTEA CHECK (octet_length(method_selector) = 4),
  PRIMARY KEY (chain_instance_id, hash)
);

CREATE TABLE eez.receipt_observations (
  chain_instance_id BIGINT NOT NULL,
  transaction_hash eez.hash32 NOT NULL,
  block_hash eez.hash32 NOT NULL,
  transaction_index eez.u64 NOT NULL,
  success BOOLEAN NOT NULL,
  -- Maintained from the parent block by triggers; never an independent decision.
  canonical BOOLEAN NOT NULL,
  gas_used eez.u256,
  effective_gas_price eez.u256,
  blob_gas_used eez.u256,
  blob_gas_price eez.u256,
  execution_paid eez.product,
  blob_paid eez.product,
  purchased_blobs eez.u256,
  accounting_gap VARCHAR(64),
  observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (chain_instance_id, transaction_hash, block_hash),
  UNIQUE (chain_instance_id, block_hash, transaction_index),
  FOREIGN KEY (chain_instance_id, transaction_hash) REFERENCES eez.transactions(chain_instance_id, hash),
  FOREIGN KEY (chain_instance_id, block_hash) REFERENCES eez.blocks(chain_instance_id, hash),
  CHECK ((accounting_gap IS NULL AND gas_used IS NOT NULL AND effective_gas_price IS NOT NULL
          AND blob_gas_used IS NOT NULL AND blob_gas_price IS NOT NULL AND execution_paid IS NOT NULL
          AND blob_paid IS NOT NULL AND purchased_blobs IS NOT NULL
          AND execution_paid = gas_used * effective_gas_price AND blob_paid = blob_gas_used * blob_gas_price
          AND mod(blob_gas_used, 131072) = 0 AND purchased_blobs = blob_gas_used / 131072)
    OR (accounting_gap IS NOT NULL AND accounting_gap ~ '^[a-z_]+$'
        AND gas_used IS NULL AND effective_gas_price IS NULL AND blob_gas_used IS NULL
        AND blob_gas_price IS NULL AND execution_paid IS NULL AND blob_paid IS NULL AND purchased_blobs IS NULL))
);
CREATE UNIQUE INDEX canonical_transaction_once ON eez.receipt_observations(chain_instance_id, transaction_hash) WHERE canonical;

CREATE FUNCTION eez.receipt_canonical_from_block() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  SELECT canonical INTO STRICT NEW.canonical FROM eez.blocks
    WHERE chain_instance_id = NEW.chain_instance_id AND hash = NEW.block_hash FOR SHARE;
  RETURN NEW;
END $$;
CREATE TRIGGER receipt_canonical BEFORE INSERT OR UPDATE ON eez.receipt_observations
  FOR EACH ROW EXECUTE FUNCTION eez.receipt_canonical_from_block();

CREATE FUNCTION eez.guard_receipt_fact() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF ROW(NEW.chain_instance_id, NEW.transaction_hash, NEW.block_hash, NEW.transaction_index, NEW.success)
     IS DISTINCT FROM ROW(OLD.chain_instance_id, OLD.transaction_hash, OLD.block_hash, OLD.transaction_index, OLD.success) THEN
    RAISE EXCEPTION 'receipt inclusion identity is immutable' USING ERRCODE = '23000';
  END IF;
  IF OLD.accounting_gap IS NULL AND
     ROW(NEW.gas_used, NEW.effective_gas_price, NEW.blob_gas_used, NEW.blob_gas_price,
         NEW.execution_paid, NEW.blob_paid, NEW.purchased_blobs, NEW.accounting_gap)
     IS DISTINCT FROM ROW(OLD.gas_used, OLD.effective_gas_price, OLD.blob_gas_used, OLD.blob_gas_price,
         OLD.execution_paid, OLD.blob_paid, OLD.purchased_blobs, OLD.accounting_gap) THEN
    RAISE EXCEPTION 'complete receipt charges are immutable' USING ERRCODE = '23000';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER receipt_guard BEFORE UPDATE ON eez.receipt_observations
  FOR EACH ROW EXECUTE FUNCTION eez.guard_receipt_fact();

CREATE FUNCTION eez.guard_transaction_fact() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF NEW IS DISTINCT FROM OLD THEN
    RAISE EXCEPTION 'logical transaction identity is immutable' USING ERRCODE = '23000';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER transaction_guard BEFORE UPDATE ON eez.transactions
  FOR EACH ROW EXECUTE FUNCTION eez.guard_transaction_fact();

CREATE FUNCTION eez.guard_finalized_block() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  IF OLD.finalized AND (NOT NEW.canonical OR NOT NEW.finalized) THEN
    RAISE EXCEPTION 'finalized block cannot be retracted' USING ERRCODE = '23000';
  END IF;
  IF NEW.hash <> OLD.hash OR NEW.number <> OLD.number OR NEW.parent_hash <> OLD.parent_hash
     OR NEW.timestamp_seconds <> OLD.timestamp_seconds OR NEW.chain_instance_id <> OLD.chain_instance_id THEN
    RAISE EXCEPTION 'block identity is immutable' USING ERRCODE = '23000';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER guard_block BEFORE UPDATE ON eez.blocks
  FOR EACH ROW EXECUTE FUNCTION eez.guard_finalized_block();

CREATE FUNCTION eez.propagate_block_canonical() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  UPDATE eez.receipt_observations SET canonical = NEW.canonical
    WHERE chain_instance_id = NEW.chain_instance_id AND block_hash = NEW.hash;
  RETURN NEW;
END $$;
CREATE TRIGGER block_canonical AFTER UPDATE OF canonical ON eez.blocks
  FOR EACH ROW WHEN (OLD.canonical IS DISTINCT FROM NEW.canonical)
  EXECUTE FUNCTION eez.propagate_block_canonical();

CREATE TABLE eez.collection_gaps (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  chain_instance_id BIGINT NOT NULL REFERENCES eez.chain_instances(id),
  source TEXT NOT NULL CHECK (source IN ('chain', 'receipt', 'blob', 'correlation')),
  reference_key VARCHAR(160) NOT NULL,
  from_number eez.u64,
  to_number eez.u64,
  reason VARCHAR(64) NOT NULL CHECK (reason ~ '^[a-z_]+$'),
  state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'resolved', 'frozen', 'orphaned')),
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (chain_instance_id, source, reference_key, reason),
  CHECK (to_number IS NULL OR from_number IS NULL OR from_number <= to_number)
);

-- No attempt/source joins here: one row per canonical receipt is the billing base.
CREATE VIEW eez.canonical_receipts AS
  SELECT r.*, b.number AS block_number, b.timestamp_seconds, b.finalized,
         t.sender, t.recipient, t.transaction_type, t.method_selector,
         c.deployment_id, c.role, c.chain_id, c.genesis_hash, c.rollup_id,
         c.bootstrap_number, c.frozen_reason, c.last_success_at, c.collection_error,
         (c.frozen_reason IS NULL AND c.collection_error IS NULL AND c.last_success_at IS NOT NULL
          AND c.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') AS source_valid
  FROM eez.receipt_observations r
  JOIN eez.blocks b ON b.chain_instance_id = r.chain_instance_id AND b.hash = r.block_hash
  JOIN eez.transactions t ON t.chain_instance_id = r.chain_instance_id AND t.hash = r.transaction_hash
  JOIN eez.chain_instances c ON c.id = r.chain_instance_id
  WHERE r.canonical AND b.canonical;

CREATE VIEW eez.canonical_cost_totals AS
  SELECT chain_instance_id, deployment_id, role, chain_id, genesis_hash, rollup_id, bootstrap_number,
         frozen_reason, last_success_at, collection_error, source_valid, count(*) AS observed_receipts,
         count(*) FILTER (WHERE accounting_gap IS NULL) AS accounted_receipts,
         count(*) FILTER (WHERE accounting_gap IS NOT NULL) AS incomplete_receipts,
         sum(execution_paid) AS known_execution_paid, sum(blob_paid) AS known_blob_paid,
         sum(execution_paid + blob_paid) AS known_total_paid,
         sum(purchased_blobs) AS known_purchased_blobs,
         count(*) FILTER (WHERE NOT success) AS reverted_receipts
  FROM eez.canonical_receipts
  GROUP BY chain_instance_id, deployment_id, role, chain_id, genesis_hash, rollup_id, bootstrap_number, frozen_reason,last_success_at,collection_error,source_valid;

CREATE VIEW eez.index_coverage AS
  SELECT c.id AS chain_instance_id, c.deployment_id, c.role, c.chain_id, c.genesis_hash, c.rollup_id,
         c.bootstrap_number, c.frozen_reason, c.selection, c.last_success_at, c.collection_error, cursor.block_hash AS cursor_hash, cursor.updated_at,
         count(b.hash) AS indexed_blocks,
         count(b.hash) FILTER (WHERE b.receipt_coverage <> 'complete') AS incomplete_blocks,
         min(b.number) AS indexed_from, max(b.number) AS indexed_through
  FROM eez.chain_instances c
  LEFT JOIN eez.scan_cursors cursor ON cursor.chain_instance_id = c.id
  LEFT JOIN eez.blocks b ON b.chain_instance_id = c.id AND b.canonical
  GROUP BY c.id, cursor.block_hash, cursor.updated_at;

-- Cluster roles can be provisioned with login credentials separately. Migration
-- creates absent roles without login; it never stores or guesses passwords.
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'eez_observer_writer') THEN CREATE ROLE eez_observer_writer NOLOGIN; END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'eez_observer_reader') THEN CREATE ROLE eez_observer_reader NOLOGIN; END IF;
  IF EXISTS (SELECT FROM pg_roles WHERE rolname IN ('eez_observer_writer', 'eez_observer_reader')
      AND (rolsuper OR rolcreatedb OR rolcreaterole OR rolbypassrls))
      OR pg_has_role('eez_observer_reader', 'eez_observer_writer', 'MEMBER') THEN
    RAISE EXCEPTION 'observer roles must be unprivileged and reader must not inherit writer';
  END IF;
END $$;
GRANT USAGE ON SCHEMA eez TO eez_observer_writer, eez_observer_reader;
GRANT SELECT ON ALL TABLES IN SCHEMA eez TO eez_observer_writer;
GRANT INSERT, UPDATE ON eez.deployments, eez.chain_instances, eez.blocks, eez.scan_cursors,
  eez.transactions, eez.receipt_observations, eez.collection_gaps TO eez_observer_writer;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA eez TO eez_observer_writer;
GRANT SELECT ON eez.canonical_receipts, eez.canonical_cost_totals, eez.index_coverage TO eez_observer_reader;
