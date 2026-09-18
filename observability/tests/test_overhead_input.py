"""Synthetic adapter contract fixtures, never measured acceptance evidence."""
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from overhead_input import adapt, artifact, canonical, digest, normalize, load, NATIVE_COMMON_KEYS, BACKENDS
from overhead_analysis import analyze
from test_overhead_analysis import fixture as analysis_fixture


def fixture():
    plan_base = analysis_fixture('pilot')
    plan_base['frozen']['measured_waves'] = 1
    plan_base['frozen'].update(arm_seconds=600,experiment_seconds=9000)
    plan_base['frozen']['measurement_scope']['backend_included']=sorted(BACKENDS)
    clock={'source':'admission.prepared.preparation_ms','unit':'ms','resolution_ms':1}
    plan_base['frozen']['preparation_clock']=clock
    manifest = {'benchmark_lock_feature': False, 'head': '0'*40, 'source_map_sha256': 'e'*64,
        'protocol_revision': 'f'*40, 'profile': 'dev optimized + debuginfo',
        'sha256': {**{'bin/'+name: str(index)*64 for index,name in enumerate(
            ('eez-composer','eez-proof-signer','eez-follower','eez-observer'),1)},
            'dynamic_admission':'5'*64,'source-map.json':'e'*64,'rustc.txt':'6'*64}}
    common = {'compiler':{'rustc_sha256':'6'*64,'toolchain':'1.94.0'},'profile':manifest['profile'],
        'cpu_allocation':{'affinity':[0,1]},'workers':{'native_test':4},'genesis':{'sha256':'7'*64},
        'native_l1_profile':'GnosisPrague','protocol':{'settlement':'always'},'logging':{'stdout':'synchronous_json'},
        'postgres_reservation':{'memory_limit_bytes':536870912,'nano_cpus':1000000000},
        'resource_sampling':{'interval_seconds':1,'metrics_interval_seconds':15,'max_samples':632},
        'observer_indexer':{'l1_bootstrap':0,'l2_bootstrap':0,'postgres':{
            'storage':{'type':'owned_bind','filesystem':'ext4','data_directory':'/var/lib/postgresql/18/docker'},
            'shared_buffers_bytes':134217728,'max_connections':30,'memory_bytes':536870912,'nano_cpus':1000000000}},
        'workload':{'measured_waves':1,'stages':copy.deepcopy(plan_base['frozen']['stages']),'arm_seconds':600,'experiment_seconds':9000}}
    genesis_template={'timestamp':'<RUN_UNIX_SECONDS>','config':{'chainId':123},'alloc':{'0x01':{'balance':'0x1'}}}
    common['genesis']={role:{'template_sha256':digest(genesis_template),'timestamp_policy':'testkit_current_unix_seconds_shared_l1_l2'} for role in ('l1','l2')}
    raw = {'schema_version':1,'status':'raw_capture_complete','candidate':manifest,'pairs':2,'waves':1,
           'probe':False,'sample_scope':'shipping_features','arm_seconds':600,'experiment_seconds':9000,'elapsed_seconds':400.0,'runs':[]}
    bundles=[]; arms={}
    host={'other_cpu_cores':.1,'steal_fraction':.002,'iowait_fraction':.001,
          'vm':{'pgmajfault':0,'pswpin':0,'pswpout':0}}
    for index, arm in enumerate('ABBA'):
        pair=index//2+1; directory=f'/tmp/synthetic/{pair}-{arm}'
        cfg={'arm':arm,'probe':False,'waves':1,'receiver':f'127.0.0.1:{40000+index}','directory':directory,'arm_seconds':600,'experiment_seconds':9000}
        profiles={}
        ready={'telemetry_enabled':arm=='B','processes':{'composer':101,'signer':102,'follower':103,'observer':104 if arm=='B' else None},'metrics':{},'postgres_container':f'eez-native-observer-synthetic-{index}'}
        for offset,role in enumerate(('composer','signer','follower')):
            profile={'identity':{'service':'eez-proof-signer' if role=='signer' else 'eez-'+role,'environment':'devnet','deployment':'overhead','instance':'fixture'},
                     'metrics':{'listen':f'127.0.0.1:{41000+10*index+offset}'}}
            if role!='follower': profile['traces']={'receiver':cfg['receiver'],'sample_ratio':1.0,'queue_capacity':2048,'export_timeout_ms':1000}
            if role=='composer': profile['lifecycle']={'directory':directory+'/lifecycle'}
            profiles[role]=profile
            if arm=='B': ready['metrics'][role]=profile['metrics']['listen']
        if arm=='B': ready['metrics']['observer']='127.0.0.1:42000'
        effective={'schema_version':1,'common':copy.deepcopy(common),'telemetry_enabled':arm=='B','observer_running':arm=='B'}
        _,arms[arm]=normalize(cfg,ready,profiles,effective)
        stages=[]
        for prescribed in plan_base['frozen']['stages']:
            stages.append({**prescribed,'elapsed_seconds':10.0,'resource_bracket_elapsed_seconds':17.0,
                'preparation_ms':[100]*40,'preparation_group_counts':[1]*40,
                'waves':[{'elapsed_seconds':10.0,'successful_sources':prescribed['successful_sources']}]})
        evidence={'arm':arm,'probe':False,'measured_waves':1,'semantic_ok':True,'telemetry_healthy':True,
            'sample_overflow':False,'logger_gaps':False,'budget_exhausted':False,'stages':stages,'preparation_clock':clock,
            'resource_phases':[{'name':s['name']} for s in stages]+[{'name':'observer_catchup_and_health'}],
            'health':{role:True for role in (*profiles,'observer')} if arm=='B' else {}}
        resources={'status':'passed','errors':[],'postgres_identity':{**copy.deepcopy(common['postgres_reservation']),'name':ready['postgres_container']},
            'samples':[{'processes':{**{role:{'pid':pid} for role,pid in ready['processes'].items() if pid is not None},'sampler_collector':{'pid':999}}}],
            **common['resource_sampling'],'overall':{'status':'passed','name':'overall','host':copy.deepcopy(host),
                'postgres':{'cpu_seconds':3},'processes':{'composer':{'cpu_seconds':1},'observer':{'cpu_seconds':2}}},
            'phases':[{'status':'passed','name':p['name'],'host':copy.deepcopy(host)} for p in evidence['resource_phases']]}
        run={'pair':pair,'arm':arm,'probe':False,'status':'captured','owned_cleanup_verified':True,
            'directory':directory,'evidence':evidence,'resources_status':'passed','elapsed_seconds':80.0,
            'sink':{'after':{'healthy':True,'errors':[]}}}
        raw['runs'].append(run)
        data={'config':cfg,'ready':ready,**profiles,'effective_config':effective,'evidence':copy.deepcopy(evidence),
              'result':copy.deepcopy(run),'resources':resources}
        genesis=copy.deepcopy(genesis_template); genesis['timestamp']=hex(1000+index)
        for role in ('l1','l2'): data[role+'-genesis']=copy.deepcopy(genesis)
        data['native-effective']={'schema_version':1,**{key:copy.deepcopy(common[key]) for key in NATIVE_COMMON_KEYS}}
        data['genesis_evidence']={role:{'timestamp_seconds':1000+index,'json_sha256':digest(genesis),
            'header_hash':'0x'+str(index+1)*64,'rpc_genesis_hash':'0x'+str(index+1)*64,
            'header_hash_source':'eez_testkit::genesis_block_hash'} for role in ('l1','l2')}
        bundles.append({key:artifact(value) for key,value in data.items()})
    expected=plan_base['frozen']['expected_identity']
    expected.update(shipping_binaries_sha256={k.removeprefix('bin/'):v for k,v in manifest['sha256'].items() if k.startswith('bin/')},
        test_binary_sha256=manifest['sha256']['dynamic_admission'],source_revision=manifest['source_map_sha256'],
        common_config_sha256=digest(common),arm_config_sha256={k:digest(v) for k,v in arms.items()})
    plan={'schema_version':1,'phase':'pilot','frozen':plan_base['frozen'],'config_preimages':{'common':common,'arms':arms}}
    return artifact(raw),artifact(plan),bundles


class Adapter(unittest.TestCase):
    def test_nested_postgres_settings_are_preserved_and_bound_to_the_frozen_configuration(self):
        raw,plan,bundles=fixture()
        expected=plan['value']['config_preimages']['common']['observer_indexer']['postgres']
        result=adapt(raw,plan,bundles)
        self.assertEqual(result['status'],'ready')
        for config in result['normalized_configs']:
            self.assertEqual(config['common']['observer_indexer']['postgres'],expected)
        self.assertNotIn('postgres',result['normalized_configs'][0]['common'])

        for field,value in [('max_connections',100),('shared_buffers_bytes',67108864),
                            ('storage',{'type':'tmpfs','filesystem':'tmpfs'}),('new_unreviewed_setting',1)]:
            raw,plan,bundles=fixture()
            # Rehash both actual/effective evidence so the rejection reaches
            # the predeclared semantic contract, not stale artifact digests.
            for key in ('native-effective','effective_config'):
                data=bundles[1][key]['value']
                common=data if key=='native-effective' else data['common']
                common['observer_indexer']['postgres'][field]=copy.deepcopy(value)
                bundles[1][key]=artifact(data)
            with self.subTest(field=field):
                changed=adapt(raw,plan,bundles)
                self.assertIsNone(changed['analysis_input'])
                self.assertTrue(any('semantic configuration' in issue for issue in changed['issues']))

    def test_complete_synthetic_set_adapts_but_pilot_never_means_acceptance(self):
        raw,plan,bundles=fixture(); result=adapt(raw,plan,bundles)
        self.assertEqual(result['issues'],[])
        self.assertEqual(result['status'],'ready')
        self.assertEqual(analyze(result['analysis_input'])['status'],'pilot_complete')
        self.assertEqual(result['analysis_input']['runs'][0]['stages'][0]['elapsed_seconds'],10)
        self.assertEqual(result['analysis_input']['runs'][0]['resources']['backend']['postgres']['cpu_seconds'],3)
        self.assertEqual(result['analysis_input']['runs'][0]['identity']['source_revision'],'e'*64)
        self.assertNotEqual(result['analysis_input']['runs'][0]['identity']['source_revision'],raw['value']['candidate']['head'])

    def test_only_ephemeral_fields_normalize_while_raw_hashes_remain(self):
        raw,plan,bundles=fixture(); result=adapt(raw,plan,bundles)
        normalized=result['normalized_configs']
        self.assertEqual(normalized[0]['arm_sha256'],normalized[3]['arm_sha256'])
        self.assertNotEqual(result['runs'][0]['artifact_sha256']['composer'],result['runs'][3]['artifact_sha256']['composer'])
        bundles[0]['composer']['value']['metrics']['new_buffer_bytes']=42
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input'])
        self.assertIn('semantic configuration',result['issues'][0])

    def test_missing_effective_config_or_completeness_is_unknown(self):
        for missing in ('effective_config','logger_gaps','sample_overflow','budget_exhausted'):
            raw,plan,bundles=fixture()
            if missing=='effective_config': del bundles[0][missing]
            else:
                del raw['value']['runs'][0]['evidence'][missing]
                bundles[0]['evidence']=artifact(copy.deepcopy(raw['value']['runs'][0]['evidence']))
                bundles[0]['result']=artifact(copy.deepcopy(raw['value']['runs'][0]))
            with self.subTest(missing=missing):
                result=adapt(raw,plan,bundles)
                self.assertEqual(result['status'],'inconclusive'); self.assertIsNone(result['analysis_input'])
                self.assertEqual(len(result['runs']),4)

    def test_failed_and_partial_sets_are_retained_never_selected_away(self):
        raw,plan,bundles=fixture(); raw['value']['status']='inconclusive'
        raw['value']['runs'][1]['status']='failed'
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input']); self.assertEqual(result['runs'][1]['status'],'failed')
        raw,plan,bundles=fixture(); raw['value']['runs'].pop(); bundles.pop()
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input']); self.assertIn('missing complete',result['issues'][0])

    def test_boolean_schema_versions_are_not_integer_version_one(self):
        for location in ('raw','plan','effective'):
            raw,plan,bundles=fixture()
            value={'raw':raw,'plan':plan,'effective':bundles[0]['effective_config']}[location]
            value['value']['schema_version']=True
            with self.subTest(location=location): self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])

    def test_feature_candidate_cannot_masquerade_as_shipping_when_probe_off(self):
        raw,plan,bundles=fixture(); raw['value']['candidate']['benchmark_lock_feature']=True
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input']); self.assertIn('feature',result['issues'][0])

    def test_wrong_chronology_and_enabled_profile_fail_closed(self):
        for mutate in (lambda raw,b:raw['value']['runs'].reverse(),
                       lambda raw,b:b[0]['ready']['value'].update(telemetry_enabled=True)):
            raw,plan,bundles=fixture(); mutate(raw,bundles)
            result=adapt(raw,plan,bundles); self.assertIsNone(result['analysis_input'])
            self.assertTrue(result['issues'])

    def test_config_paths_receivers_and_active_endpoints_must_match(self):
        for mutate in (lambda b:b[1]['composer']['value']['lifecycle'].update(directory='/tmp/other/lifecycle'),
                       lambda b:b[1]['signer']['value']['traces'].update(receiver='127.0.0.1:1234'),
                       lambda b:b[1]['ready']['value']['metrics'].update(composer='127.0.0.1:1234'),
                       lambda b:b[1]['effective_config']['value']['common']['workers'].update(native_test=8)):
            raw,plan,bundles=fixture(); mutate(bundles)
            self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])

    def test_frozen_plan_digest_is_not_recomputed_from_observed_changes(self):
        raw,plan,bundles=fixture(); plan['value']['config_preimages']['common']['workers']['native_test']=8
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input']); self.assertIn('preimage/hash',result['issues'][0])

    def test_catchup_or_outer_bracket_cannot_replace_wave_throughput_clock(self):
        raw,plan,bundles=fixture(); raw['value']['runs'][0]['evidence']['stages'][0]['elapsed_seconds']=17
        bundles[0]['evidence']=artifact(copy.deepcopy(raw['value']['runs'][0]['evidence']))
        bundles[0]['result']=artifact(copy.deepcopy(raw['value']['runs'][0]))
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input']); self.assertIn('sum of measured',result['issues'][0])

    def test_resources_cannot_be_borrowed_from_another_run_or_omit_catchup(self):
        for mutate in (lambda b:b[0]['resources']['value']['postgres_identity'].update(name='other'),
                       lambda b:b[0]['resources']['value']['samples'][0]['processes']['composer'].update(pid=99999),
                       lambda b:b[0]['resources']['value']['phases'].pop()):
            raw,plan,bundles=fixture(); mutate(bundles)
            self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])

    def test_genesis_allows_only_declared_shared_timestamp_and_keeps_actual_hashes(self):
        raw,plan,bundles=fixture(); result=adapt(raw,plan,bundles)
        self.assertEqual(result['status'],'ready')
        self.assertNotEqual(result['runs'][0]['artifact_sha256']['l1-genesis'],result['runs'][1]['artifact_sha256']['l1-genesis'])
        for mutate in (lambda b:b[1]['l1-genesis']['value']['alloc']['0x01'].update(balance='0x2'),
                       lambda b:b[1]['genesis_evidence']['value']['l1'].update(rpc_genesis_hash='0x'+'f'*64),
                       lambda b:b[1]['l2-genesis']['value'].update(timestamp='0x1')):
            raw,plan,bundles=fixture(); mutate(bundles)
            self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])

    def test_file_loader_preserves_exact_bytes_and_total_budget(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'input.json'; encoded=b'{"value": 1}\n'; path.write_bytes(encoded)
            found=load(path,[len(encoded)])
            self.assertEqual(found['sha256'],hashlib.sha256(encoded).hexdigest())
            with self.assertRaisesRegex(ValueError,'combined artifacts'): load(path,[len(encoded)-1])
            path.write_text('{"value":NaN}')
            with self.assertRaises(ValueError): load(path)

    def test_explicit_nanoseconds_convert_without_reinterpreting_old_milliseconds(self):
        raw,plan,bundles=fixture()
        clock={'source':'admission.prepared.preparation_ns','unit':'ns','resolution_ns':1}
        plan['value']['frozen']['preparation_clock']=clock
        plan['value']['frozen']['resolution_ms']=.000001
        for index,run in enumerate(raw['value']['runs']):
            run['evidence']['preparation_clock']=clock
            for stage in run['evidence']['stages']: stage['preparation_ns']=[100_123_456]*40
            bundles[index]['evidence']=artifact(copy.deepcopy(run['evidence']))
            bundles[index]['result']=artifact(copy.deepcopy(run))
        result=adapt(raw,plan,bundles); self.assertEqual(result['issues'],[])
        converted=result['analysis_input']['runs'][0]['stages'][0]
        self.assertEqual(converted['preparation_ms'][0],100.123456)
        self.assertEqual(converted['legacy_preparation_ms'][0],100)
        del bundles[0]['evidence']['value']['stages'][0]['preparation_ns']
        raw['value']['runs'][0]['evidence']=copy.deepcopy(bundles[0]['evidence']['value'])
        bundles[0]['result']=artifact(copy.deepcopy(raw['value']['runs'][0]))
        self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])
        raw,plan,bundles=fixture(); plan['value']['frozen']['resolution_ms']=.000001
        self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])

    def test_cli_loads_exact_owned_files_and_rejects_outside_paths(self):
        raw,plan,bundles=fixture()
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'capture'; root.mkdir()
            for index,run in enumerate(raw['value']['runs']):
                directory=root/f"{run['pair']}-{run['arm']}"; directory.mkdir()
                run['directory']=str(directory)
                bundles[index]['config']['value']['directory']=str(directory)
                bundles[index]['composer']['value']['lifecycle']['directory']=str(directory/'lifecycle')
                bundles[index]['result']=artifact(copy.deepcopy(run))
                for name,item in bundles[index].items(): (directory/(name+'.json')).write_bytes(canonical(item['value']))
            raw_path=root/'report.json'; raw_path.write_text(json.dumps(raw['value'],indent=2)+'\n')
            plan_path=Path(tmp)/'plan.json'; plan_path.write_bytes(canonical(plan['value']))
            output=Path(tmp)/'adapted.json'
            command=[sys.executable,str(Path(__file__).with_name('overhead_input.py')),'--raw',str(raw_path),'--plan',str(plan_path),'--output',str(output)]
            completed=subprocess.run(command,capture_output=True,text=True,timeout=10)
            self.assertEqual(completed.returncode,0,completed.stderr)
            result=json.loads(output.read_text()); self.assertEqual(result['status'],'ready')
            self.assertEqual(result['provenance']['raw_report_sha256'],hashlib.sha256(raw_path.read_bytes()).hexdigest())
            aliases=[raw_path,plan_path,root/'1-A'/'composer.json',output]
            alias=Path(tmp)/'linked-output.json'; alias.symlink_to(raw_path); aliases.append(alias)
            originals={path:path.read_bytes() for path in aliases}
            for alias_path in aliases:
                failed=subprocess.run([*command[:-1],str(alias_path)],capture_output=True,text=True,timeout=10)
                self.assertNotEqual(failed.returncode,0,alias_path)
                self.assertEqual(alias_path.read_bytes(),originals[alias_path])
                self.assertEqual(raw_path.read_bytes(),originals[raw_path])
            # Move the complete bundle while every original absolute path is now absent.
            original_bytes=raw_path.read_bytes(); original_root=root
            root=Path(tmp)/'relocated'; original_root.rename(root)
            raw_path=root/'report.json'; output=Path(tmp)/'relocated-adapted.json'
            command=[sys.executable,str(Path(__file__).with_name('overhead_input.py')),'--raw',str(raw_path),'--plan',str(plan_path),'--output',str(output)]
            completed=subprocess.run(command,capture_output=True,text=True,timeout=10)
            self.assertEqual(completed.returncode,0,completed.stderr)
            self.assertEqual(raw_path.read_bytes(),original_bytes); self.assertFalse(original_root.exists())
            self.assertEqual(json.loads(output.read_text())['runs'][0]['raw']['directory'],str(original_root/'1-A'))
            output.unlink()
            # A local symlink may not redirect file reads outside the relocated bundle.
            outside=Path(tmp)/'outside'; outside.mkdir()
            (root/'1-A').rename(root/'saved-1-A'); (root/'1-A').symlink_to(outside,target_is_directory=True)
            completed=subprocess.run(command,capture_output=True,text=True,timeout=10)
            self.assertNotEqual(completed.returncode,0); self.assertFalse(output.exists())
            self.assertEqual(raw_path.read_bytes(),original_bytes)
            (root/'1-A').unlink(); (root/'saved-1-A').rename(root/'1-A')
            for pair,arm in ((True,'A'),(0,'A'),(7,'A'),('../outside','A'),(1,'../outside')):
                malformed=copy.deepcopy(raw['value']); malformed['runs'][0].update(pair=pair,arm=arm)
                raw_path.write_bytes(canonical(malformed))
                completed=subprocess.run(command,capture_output=True,text=True,timeout=10)
                self.assertNotEqual(completed.returncode,0); self.assertFalse(output.exists())
            raw_path.write_bytes(original_bytes)

    def test_native_effective_values_and_actual_backend_scope_are_bound(self):
        raw,plan,bundles=fixture()
        bundles[0]['native-effective']['value']['workers']['native_test']=99
        result=adapt(raw,plan,bundles)
        self.assertIsNone(result['analysis_input']); self.assertIn('actual native configuration',result['issues'][0])
        for names in (sorted(BACKENDS)+['tempo'], sorted(BACKENDS)+['prometheus_storage'],
                      ['observer','observer','postgresql','otlp_http_sink']):
            raw,plan,bundles=fixture(); plan['value']['frozen']['measurement_scope']['backend_included']=names
            self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])

    def test_budgets_are_explicit_bounded_and_bound_to_actual_sampling(self):
        for field,value in (('arm_seconds',True),('arm_seconds',0),('arm_seconds',1801),
                            ('experiment_seconds',21601),('experiment_seconds',599)):
            raw,plan,bundles=fixture(); plan['value']['frozen'][field]=value
            self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])
        for mutate in (lambda raw,b:raw['value'].update(arm_seconds=700),
                       lambda raw,b:b[0]['config']['value'].update(arm_seconds=700),
                       lambda raw,b:b[0]['resources']['value'].update(max_samples=1200),
                       lambda raw,b:b[0]['config']['value'].update(waves=65)):
            raw,plan,bundles=fixture(); mutate(raw,bundles)
            self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])
        raw,plan,bundles=fixture()
        for role in ('A','B'):
            cfg=copy.deepcopy(bundles[0 if role=='A' else 1]['config']['value'])
            cfg.update(waves=64,arm_seconds=1800,experiment_seconds=21600)
            data={key:item['value'] for key,item in bundles[0 if role=='A' else 1].items()}
            normalize(cfg,data['ready'],{r:data[r] for r in ('composer','signer','follower')},data['effective_config'])

    def test_maximum_predeclared_capacity_flows_through_full_adapter(self):
        raw,plan,bundles=fixture(); waves=64
        selected={'arm_seconds':1800,'experiment_seconds':21600}
        raw['value'].update(waves=waves,**selected)
        frozen=plan['value']['frozen']; frozen.update(measured_waves=waves,**selected)
        for stage in frozen['stages']: stage['successful_sources']*=waves
        common=plan['value']['config_preimages']['common']
        common['workload']={'measured_waves':waves,'stages':copy.deepcopy(frozen['stages']),**selected}
        common['resource_sampling']['max_samples']=1832
        frozen['expected_identity']['common_config_sha256']=digest(common)
        for index,run in enumerate(raw['value']['runs']):
            data=bundles[index]
            data['config']['value'].update(waves=waves,**selected)
            data['effective_config']['value']['common']=copy.deepcopy(common)
            data['native-effective']['value']={'schema_version':1,**{key:copy.deepcopy(common[key]) for key in NATIVE_COMMON_KEYS}}
            data['resources']['value']['max_samples']=1832
            evidence=run['evidence']; evidence['measured_waves']=waves
            for stage,prescribed in zip(evidence['stages'],frozen['stages']):
                stage['waves']=[copy.deepcopy(stage['waves'][0]) for _ in range(waves)]
                stage['successful_sources']=prescribed['successful_sources']
                stage['elapsed_seconds']=10.0*waves
                stage['resource_bracket_elapsed_seconds']=stage['elapsed_seconds']+7
            data['evidence']=artifact(copy.deepcopy(evidence)); data['result']=artifact(copy.deepcopy(run))
        result=adapt(raw,plan,bundles)
        self.assertEqual(result['issues'],[]); self.assertEqual(result['status'],'ready')
        self.assertEqual(result['analysis_input']['frozen']['arm_seconds'],1800)
        self.assertEqual(result['analysis_input']['runs'][0]['measured_waves'],64)
        self.assertEqual(result['normalized_configs'][0]['common']['resource_sampling']['max_samples'],1832)

    def test_actual_elapsed_budget_is_checked_even_if_capture_flags_claim_success(self):
        for scope,budget in (('experiment',9000),('arm',600)):
            for elapsed in (None,True,0,-1,float('nan'),float('inf'),budget+.001):
                raw,plan,bundles=fixture()
                if scope=='experiment': raw['value']['elapsed_seconds']=elapsed
                else:
                    raw['value']['runs'][0]['elapsed_seconds']=elapsed
                    bundles[0]['result']['value']=copy.deepcopy(raw['value']['runs'][0])
                with self.subTest(scope=scope,elapsed=elapsed):
                    result=adapt(raw,plan,bundles)
                    self.assertIsNone(result['analysis_input']); self.assertIn('elapsed',result['issues'][0])
            raw,plan,bundles=fixture()
            if scope=='experiment': raw['value']['elapsed_seconds']=budget
            else:
                raw['value']['runs'][0]['elapsed_seconds']=budget
                bundles[0]['result']=artifact(copy.deepcopy(raw['value']['runs'][0]))
            self.assertEqual(adapt(raw,plan,bundles)['status'],'ready')

    def test_unknown_host_measurement_and_counter_absence_stay_unknown(self):
        raw,plan,bundles=fixture(); plan['value']['frozen']['contamination_limits']['invented']=1
        self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])
        raw,plan,bundles=fixture(); del bundles[0]['resources']['value']['overall']['host']['vm']['pswpin']
        self.assertIsNone(adapt(raw,plan,bundles)['analysis_input'])


if __name__=='__main__': unittest.main()
