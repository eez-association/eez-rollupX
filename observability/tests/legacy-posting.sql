-- Synthetic v1 history matching the RPC fixture. Method evidence is intentionally
-- absent so the forward migration and ordinary scanner must recover it.
INSERT INTO eez.deployments(environment,name) VALUES('fixture','upgrade') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,selection)
  VALUES(:deployment,'l1',1,decode(lpad(to_hex(100),64,'0'),'hex'),0,2,
  '{"wallets":["0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"contracts":["0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],"include_all_transactions":false}') RETURNING id AS chain \gset
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
  VALUES(:chain,decode(lpad(to_hex(102),64,'0'),'hex'),2,decode(lpad(to_hex(101),64,'0'),'hex'),110,true,'complete');
INSERT INTO eez.scan_cursors(chain_instance_id,block_hash) VALUES(:chain,decode(lpad(to_hex(102),64,'0'),'hex'));
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type,method_selector)
  VALUES(:chain,decode(lpad(to_hex(1002),64,'0'),'hex'),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),1,3,decode('POST_SELECTOR','hex'));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,
  gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
  VALUES(:chain,decode(lpad(to_hex(1002),64,'0'),'hex'),decode(lpad(to_hex(102),64,'0'),'hex'),0,false,true,22000,9,131072,3,198000,393216,1);
