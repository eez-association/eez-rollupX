-- PR26 tag 3/profile 1 permits 65,536 logical blocks including the terminal.
-- This is a forward observer schema upgrade only: preserve all historical facts,
-- receipt costs, provenance/finality predicates and outer blob codec version 0.
ALTER TABLE eez.settlements DROP CONSTRAINT settlements_block_count_check;
ALTER TABLE eez.settlements ADD CONSTRAINT settlements_block_count_check
  CHECK(block_count BETWEEN 1 AND 65536);
ALTER TABLE eez.settlement_blocks DROP CONSTRAINT settlement_blocks_ordinal_check;
ALTER TABLE eez.settlement_blocks ADD CONSTRAINT settlement_blocks_ordinal_check
  CHECK(ordinal BETWEEN 0 AND 65535);
