"""Real importer/projection checks; synthetic claims never imply chain assurance."""
import copy
import json


def exercise(run, psql, scalar, index, write_index, source_config, spool, header, chunks, report):
    epoch = '91' * 16
    directory = spool / 'native-outcomes'
    directory.mkdir(mode=0o755)
    config = copy.deepcopy(source_config)
    config['deployment'] = 'native-outcomes'
    config['lifecycle'] = [{'service': 'composer', 'instance': 'outcomes-node', 'directory': '/lifecycle/native-outcomes'}]
    write_index(config)

    def scan():
        result = run(index)
        assert 'native outcome projection failed' not in result.stdout, result.stdout
        assert 'native cost membership projection failed' not in result.stdout, result.stdout
        return result
    hash32 = lambda number: f'{number:064x}'
    block = lambda number, value: {'number': number, 'hash': hash32(value)}
    scopes = [{'chain_id': config['exporter'][role]['chain_id'],
               'genesis_hash': config['exporter'][role]['genesis_hash'][2:],
               'rollup_id': 0 if role == 'l1' else config['rollup_id']} for role in ('l1', 'l2')]
    member = {'source': {**scopes[0], 'transaction_hash': hash32(1002), 'direction': 'inbound'},
              'admission': {'epoch': epoch, 'sequence': 1}}
    owner = {'epoch': epoch, 'generation': 1, 'post_batch_hash': hash32(5)}
    action = {'call_hash': hash32(8), 'source_index': 0}
    plan = {'sender': '03' * 20, 'nonce': 3, 'charged_attempts': 0, 'outbound_group': None,
            'effects_start': 0, 'effects_end': 1, 'source_actions': [action]}
    prepared = {'event': 'attempt_prepared', 'attempt': owner, 'sync_height': 3, 'members': [member],
                'encoded_bytes': 42, 'blob_count': 1, 'blob_allowance': 1,
                'details': {'preparation': None, 'l2_chain_id': scopes[1]['chain_id'],
                            'l2_genesis_hash': scopes[1]['genesis_hash'], 'rollup_id': scopes[1]['rollup_id'],
                            'sync_hash': hash32(32), 'nonce': 0, 'plans': [plan], 'effects': [hash32(32)]}}
    parent = {'event': 'reconciled', 'attempt': owner, 'anchor': block(12, 12),
              'details': {'l1': scopes[0], 'l2': scopes[1], 'l2_snapshot': block(3, 33),
                          'post': {'block': block(11, 11), 'transaction_index': 0, 'success': True},
                          'applied': {'block': block(11, 11), 'transaction_hash': owner['post_batch_hash'],
                                      'transaction_index': 0, 'calldata_hash': hash32(10), 'effects_start': 0,
                                      'effects_end': 1, 'anchor_applied': True, 'endpoint': block(3, 32),
                                      'outbound_groups_start': 0, 'outbound_groups_end': 0,
                                      'inbound_effects_start': 0, 'inbound_effects_end': 1},
                          'sources': [{'transaction_hash': member['source']['transaction_hash'], 'direction': 'inbound',
                                       'observed_nonce': 4, 'source_block': block(11, 11), 'basis': 'receipt',
                                       'receipt': {'block': block(11, 11), 'transaction_index': 1, 'success': True},
                                       'consumed': [action]}],
                          'resolutions': [{'member': member, 'owner': owner, 'disposition': {
                              'state': 'terminal', 'outcome': 'committed', 'assurance': 'node_reconciled'}, 'elapsed_ns': 100}]}}
    terminal = {'event': 'terminal', 'member': member, 'attempt': owner, 'outcome': 'committed',
                'assurance': 'node_reconciled', 'anchor': None, 'elapsed_ns': 100,
                'reconciliation': {'epoch': epoch, 'sequence': 10}}

    def segment(number, events):
        identifier = f'{number:032x}'
        data = header(epoch, identifier)
        for sequence, event in events:
            payload = {'observed_at_unix_ms': 1000, 'trace': None, 'event': event}
            data += b''.join(chunks(sequence, payload=payload, epoch_value=epoch))
        (directory / f'{epoch}-{identifier}.closed').write_bytes(data)

    source = "(SELECT id FROM eez.lifecycle_sources WHERE instance='outcomes-node')"

    def observed(expression, extra=''):
        return scalar(f'SELECT {expression} FROM eez.lifecycle_native_outcomes WHERE source_id={source} {extra};')

    # A lost admission does not prevent inspecting explicit later ownership, but
    # does not manufacture an acceptance or upgrade the native claim's assurance.
    segment(1, [(3, prepared), (11, terminal)])
    scan()
    assert observed('native_state') == 'missing_record'
    segment(2, [(10, parent)])
    scan()
    assert observed("native_state||':'||evidence_class") == 'coherent:unknown'
    assert scalar(f"SELECT native_state||':'||evidence_state FROM eez.lifecycle_cost_evidence WHERE source_id={source};") == 'coherent:missing_snapshot'
    assert observed("decision#>>'{l1_snapshot,number}'") == '12'
    assert observed("decision#>>'{post,block,number}'") == '11'
    assert observed("decision#>>'{applied,endpoint,hash}'") == hash32(32)
    assert observed("decision#>>'{l2_snapshot,hash}'") == hash32(33)
    assert scalar(f'SELECT count(*) FROM eez.lifecycle_admission_status WHERE source_id={source};') == '0'
    scan()
    assert observed('count(*)') == '1'
    # Wrong context remains an independently keyed failed check.
    alternate = copy.deepcopy(config)
    alternate['rollup_id'] += 1
    write_index(alternate)
    scan()
    assert observed("count(*) FILTER(WHERE native_state='coherent')||':'||count(*) FILTER(WHERE native_state='identity')") == '1:1'
    write_index(config)
    # Invalidate the parent between passes: the reader must retract immediately.
    result = run(psql, data=f"BEGIN; UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id={source} AND sequence=10; SELECT native_state FROM eez.lifecycle_native_outcomes WHERE source_id={source} AND state='coherent'; ROLLBACK;")
    assert 'changed_evidence' in result.stdout
    # A newly complete duplicate preparation invalidates the exact checked
    # lineage even though all three previous digest references still match.
    saved = json.loads(observed('row_to_json(lifecycle_native_outcomes)', "AND state='coherent'"))
    segment(3, [(4, prepared)])
    scan()
    assert observed('native_state', f"AND context_l2={saved['context_l2']}") == 'lineage'
    # Recreate the old cache in a rollback transaction to check the view at the
    # importer/projection boundary, before a subsequent worker could refresh it.
    refs = json.dumps(saved['record_refs']).replace("'", "''")
    decision = json.dumps(saved['decision']).replace("'", "''")
    result = run(psql, data=f"BEGIN; UPDATE eez.lifecycle_native_checks SET state='coherent',record_refs='{refs}'::jsonb,decision='{decision}'::jsonb,parent_sequence={saved['parent_sequence']},generation={saved['generation']} WHERE source_id={source} AND context_l2={saved['context_l2']}; SELECT native_state FROM eez.lifecycle_native_outcomes WHERE source_id={source} AND state='coherent'; ROLLBACK;")
    assert 'changed_lineage' in result.stdout
    # The loader bounds the inventory before fetching all manifest bodies.
    bounded_owner = {**owner, 'generation': 2}
    bounded_prepared = copy.deepcopy(prepared)
    bounded_prepared['attempt'] = bounded_owner
    bounded_parent = copy.deepcopy(parent)
    bounded_parent['attempt'] = bounded_owner
    bounded_parent['details']['resolutions'][0]['owner'] = bounded_owner
    bounded_terminal = copy.deepcopy(terminal)
    bounded_terminal['attempt'] = bounded_owner
    bounded_terminal['reconciliation']['sequence'] = 150
    events = [(20, bounded_prepared)]
    previous = bounded_owner
    for offset in range(128):
        replacement = {**bounded_owner, 'post_batch_hash': hash32(6 if offset % 2 == 0 else 5)}
        events.append((21 + offset, {'event': 'attempt_repriced', 'previous': previous, 'replacement': replacement}))
        previous = replacement
    events.extend([(150, bounded_parent), (151, bounded_terminal)])
    segment(4, events)
    scan()
    assert observed('native_state', 'AND sequence=151') == 'limit'
    # A failed paid post can requeue every source. Cost projection must consume
    # the reconciliation itself, without requiring a nonexistent terminal row.
    failed_prepared = copy.deepcopy(prepared)
    failed_prepared['attempt']['generation'] = 3
    failed_parent = copy.deepcopy(parent)
    failed_parent['attempt']['generation'] = 3
    failed_parent['details']['post']['success'] = False
    failed_parent['details']['applied'] = None
    failed_source = failed_parent['details']['sources'][0]
    failed_source.update(source_block=None, basis='nonce', receipt=None, consumed=[], observed_nonce=3)
    failed_parent['details']['resolutions'][0]['owner']['generation'] = 3
    failed_parent['details']['resolutions'][0]['disposition'] = {'state':'requeued','charged_attempts':1}
    segment(5, [(195, failed_prepared), (200, failed_parent)])
    scan()
    assert scalar(f"SELECT state||':'||jsonb_array_length(decision->'population') FROM eez.lifecycle_cost_checks WHERE source_id={source} AND sequence=200;") == 'coherent:1'
    assert scalar(f"SELECT count(*) FROM eez.source_posting_allocations WHERE l1_chain_id={saved['context_l1']};") == '0'
    scan()
    assert scalar(f"SELECT count(*) FROM eez.lifecycle_cost_checks WHERE source_id={source} AND sequence=200;") == '1'
    report['checks'].append('paid_attempt_projection_requeued_without_terminal_and_missing_chain_evidence_remains_unallocated')
    run(psql, data='SET ROLE eez_observer_reader; SELECT count(*) FROM eez.lifecycle_native_outcomes;')
    report['checks'].append('native_coherence_projection_late_parent_scope_fence_dedup_missing_admission_digest_and_lineage_retraction_without_canonical_promotion')

    # A burst must drain without sleeping five seconds after each full batch.
    # These are complete wire records sharing a deliberately invalid lineage;
    # the real importer/checker must retain every gap and keep repairing older
    # checks, rather than treating a malformed claim as successful completion.
    burst_start, burst_count = 1000, 120
    segment(6, [(burst_start + value, terminal) for value in range(burst_count)])
    prior_attempt = observed('last_attempt_at::text', f"AND sequence=11 AND context_l2={saved['context_l2']}")
    continuous = index[:-1]
    assert index[-1] == '--once' and continuous[:2] == ['docker', 'exec']
    # The in-container deadline owns the observer process as well as the Docker
    # client; no orphan background worker remains after this bounded test.
    result = run(continuous[:3] + ['timeout', '--signal=TERM', '--kill-after=5s', '20s'] + continuous[3:], check=False)
    assert result.returncode == 124, result.stdout
    count = observed('count(*)', f"AND sequence >= {burst_start} AND sequence < {burst_start+burst_count} AND context_l2={saved['context_l2']}")
    assert count == str(burst_count), (count, result.stdout)
    assert observed('count(*)', f"AND sequence >= {burst_start} AND native_state='coherent'") == '0'
    assert observed('last_attempt_at::text', f"AND sequence=11 AND context_l2={saved['context_l2']}") != prior_attempt
    assert 'native outcome projection failed' not in result.stdout, result.stdout
    # Restarting a one-shot pass preserves the same terminal population.
    scan()
    assert observed('count(*)', f"AND sequence >= {burst_start} AND context_l2={saved['context_l2']}") == str(burst_count)
    report['checks'].append('native_outcome_burst_120_gapped_terminals_drains_with_fair_repairs_and_restart_deduplication')
