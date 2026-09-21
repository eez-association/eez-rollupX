"""Real observer/Beacon fixture checks; no transactions are sent or funded."""
import copy
import json
import pathlib


def exercise(run, psql, scalar, index, write_index, index_config, control, rpc_name, report):
    fixtures=json.loads((pathlib.Path(__file__).parent/'fixtures/blob-carriers.json').read_text())
    expected_bytes=sum(item['encoded_bytes'] for item in fixtures)
    config=copy.deepcopy(index_config)
    config['deployment']='blob-measurements'
    config['l1']['contracts']=['0x'+'bb'*20]
    config['lifecycle']=[]
    control(phase='healthy',finalize=False,correlation='none',post_count=2,post_success=False,
        multi_rollup=False,protocol_logs='missing',source_unselected=False,source_receipt_missing=False,
        source_cost_missing=False,source_hash_mismatch=False,source_receipt_failure=False,
        source_receipt_delay_ms=0,blob_mode='valid')
    def scan():
        result=run(index)
        assert 'blob measurement collection failed' not in result.stdout, result.stdout
        return result
    def query(expression,view='posting_blob_totals',extra=''):
        return scalar(f"SELECT {expression} FROM eez.{view} c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='{config['deployment']}' {extra};")
    def attempts():
        return scalar(f"SELECT count(*) FROM eez.blob_attempts a JOIN eez.chain_instances c ON c.id=a.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='{config['deployment']}';")
    def requests():
        command=['docker','exec',rpc_name,'python3','-c',"import urllib.request; print(urllib.request.urlopen('http://localhost:9000/fixture/blob-requests').read().decode())"]
        return json.loads(run(command).stdout)['requests']
    write_index(config)
    scan()
    assert attempts()=='0'
    assert query('posts||\':\'||measured_posts')=='2:0'
    assert query("purchased_capacity_utilization IS NULL")=='t'
    # Existing configurations do no Beacon work, even with relevant paid blobs.
    assert requests()==0
    config['blobs']={'beacon_url':'http://rpc:9000'}
    write_index(config)
    control(blob_mode='unavailable')
    scan()
    assert query("count(*)",'posting_blob_measurements',"AND measurement_state='unavailable'")=='2'
    assert query('count(*)','posting_blob_measurements',"AND retrieval_duration_ns>0 AND retrieval_duration_ns<30000000000")=='2'
    fees=query('known_total_paid','canonical_cost_totals',"AND role='l1'")
    assert fees=='1722648'
    control(blob_mode='pruned')
    scan();scan()
    assert query('count(*)','posting_blob_measurements',"AND measurement_state='pruned'")=='2'
    assert query('known_total_paid','canonical_cost_totals',"AND role='l1'")==fees
    control(blob_mode='valid')
    scan()
    assert query('measured_posts')=='1'
    assert query('measurement_coverage')=='0.50000000000000000000'
    scan()
    assert query('posts||\':\'||reverted_posts||\':\'||measured_posts')=='2:2:2'
    assert query('measured_purchased_blobs||\':\'||measured_encoded_bytes')==f'3:{expected_bytes}'
    assert query(f'purchased_capacity_utilization={expected_bytes}::numeric/(3*126976)')=='t'
    assert query("count(*)",'posting_blob_measurements',"AND measurement_provenance='authenticated_canonical_bytes' AND NOT has_applied_settlement")=='2'
    before=requests()
    scan()
    assert requests()==before  # Process restart retains measurements; no refetch.
    control(phase='wrong_identity')
    assert run(index,check=False).returncode != 0
    assert query('count(*)','posting_blob_measurements',"AND NOT source_valid AND measurement_provenance='unknown'")=='2'
    control(phase='healthy')
    scan()
    assert query('count(*)','posting_blob_measurements',"AND source_valid AND measurement_provenance='authenticated_canonical_bytes'")=='2'
    control(phase='fork')
    scan()
    assert requests()==before
    assert query('count(*)','posting_blob_measurements')=='2'
    assert query('measured_encoded_bytes')==str(expected_bytes)
    assert query('count(*)','canonical_receipt_facts',"AND role='l1'")=='3'
    # A reader sees old measurements disappear with canonical membership. Bytes
    # remain immutable and reusable if their transaction is included again.
    outcome=run(psql,data="BEGIN; UPDATE eez.blocks b SET canonical=false FROM eez.chain_instances c JOIN eez.deployments d ON d.id=c.deployment_id WHERE b.chain_instance_id=c.id AND d.name='blob-measurements' AND c.role='l1' AND b.number=2 AND b.canonical; SELECT count(*) FROM eez.posting_blob_measurements m JOIN eez.deployments d ON d.id=m.deployment_id WHERE d.name='blob-measurements'; ROLLBACK;")
    assert '\n     0\n' in outcome.stdout, outcome.stdout
    # Independent KZG failure preserves fees and reveals partial measured-subset
    # coverage. Legacy sidecars recover only the missing post, without refetching
    # the already authenticated one.
    config['deployment']='blob-authentication'
    write_index(config);control(phase='healthy',blob_mode='corrupt')
    scan()
    assert query('count(*)','posting_blob_measurements',"AND measurement_state='authentication_failed'")=='1'
    assert query('measured_posts')=='1'
    assert query('known_total_paid','canonical_cost_totals',"AND role='l1'")==fees
    control(blob_mode='legacy')
    scan()
    assert query('measured_posts')=='2'
    assert query('count(*)','posting_blob_measurements',"AND retrieval='beacon_sidecars'")=='1'
    assert query('count(*)','posting_blob_measurements',"AND retrieval='beacon_blobs'")=='1'
    config['deployment']='blob-metadata-gap'
    write_index(config);control(blob_mode='missing_metadata')
    before=requests()
    scan()
    assert query('count(*)','posting_blob_measurements',"AND measurement_state='metadata_unavailable' AND retrieval_duration_ns IS NULL")=='2'
    assert requests()==before
    control(blob_mode='valid')
    scan();scan()
    assert query('measured_posts')=='2'
    # Byte measurement does not fabricate missing receipt charges/counts.
    config['deployment']='blob-receipt-gap'
    write_index(config);control(blob_mode='valid',source_cost_missing=True)
    scan()
    assert query('count(*)','posting_blob_measurements',"AND measurement_state='receipt_count_unknown' AND encoded_bytes IS NOT NULL")=='2'
    assert query('purchased_capacity_utilization IS NULL')=='t'
    assert query('measured_posts')=='0'
    before=requests()
    control(source_cost_missing=False)
    scan()
    assert query('measured_posts')=='2'
    assert requests()==before
    # Canonical content and derived measurements cannot be rewritten by the
    # writer; conflicting facts must never overwrite an authenticated value.
    rejected=run(psql,data="SET ROLE eez_observer_writer; UPDATE eez.blob_measurements SET encoded_bytes=encoded_bytes+1;",check=False)
    assert rejected.returncode!=0 and 'immutable' in rejected.stdout
    run(psql,data='SET ROLE eez_observer_reader; SELECT count(*) FROM eez.posting_blob_measurements; SELECT count(*) FROM eez.posting_blob_totals; SELECT count(*) FROM eez.blob_metadata;')
    control(blob_mode='disabled',source_cost_missing=False,post_count=1)
    write_index(index_config)
    report['checks'].append('authenticated_blob_bytes_bounded_optional_config_pruning_kzg_legacy_recovery_weighted_subset_reverts_receipt_gaps_restart_and_reorg_without_duplicate_fees')
