-- Append-only cursor progress preserves historical sample validity. Orphaning
-- or deleting canonical history invalidates every cached sample of that chain.
ALTER TABLE eez.chain_instances ADD COLUMN canonical_revision BIGINT NOT NULL DEFAULT 0
  CHECK (canonical_revision >= 0);

CREATE FUNCTION eez.advance_canonical_revision() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  -- Normal scanner paths already lock chain -> cursor -> blocks. Administrative
  -- writers must take the same chain lock before editing/deleting block rows.
  -- UPDATE serializes concurrent increments; rollback rolls back this increment.
  -- Both triggers filter canonical loss before invoking this function; DELETE
  -- must not dereference an unassigned NEW record.
  UPDATE eez.chain_instances SET canonical_revision=canonical_revision+1
    WHERE id=OLD.chain_instance_id;
  IF TG_OP='DELETE' THEN RETURN OLD; END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER canonical_revision_on_orphan AFTER UPDATE OF canonical ON eez.blocks
  FOR EACH ROW WHEN (OLD.canonical AND NOT NEW.canonical)
  EXECUTE FUNCTION eez.advance_canonical_revision();
CREATE TRIGGER canonical_revision_on_delete BEFORE DELETE ON eez.blocks
  FOR EACH ROW WHEN (OLD.canonical)
  EXECUTE FUNCTION eez.advance_canonical_revision();
