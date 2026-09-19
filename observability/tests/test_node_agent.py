import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT/'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class NodeAgent(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        (self.root/'logs').mkdir()
        self.config = {'environment':'fixture','deployment':'node-agent','observer_instance':'observer',
            'observer_target':'observer:19100','log_directory':str(self.root/'logs'),
            'alert_webhook_url':'http://alerts/','heartbeat_webhook_url':'http://heartbeat/'}
        self.agent = {'instance':'host-one','metrics_target':'node-private:12346',
            'loki_url':'http://127.0.0.1:19310/loki/api/v1/push','read_gid':1000,'sources':[
                {'service':'eez-composer','instance':'composer','kind':'reth_json','directory':'/node/diagnostic'},
                {'service':'eez-composer','instance':'composer','kind':'lifecycle','directory':'/node/lifecycle'},
                {'service':'lighthouse','instance':'consensus','kind':'lighthouse_json','directory':'/node/consensus'}]}

    def test_absent_keeps_existing_backend_and_no_node_profile(self):
        runtime = prepare.prepare(self.config,self.root/'runtime')
        self.assertFalse((runtime/'node-agent').exists())
        prom = json.loads((runtime/'prometheus/prometheus.yaml').read_text())
        self.assertFalse(any(job['job_name'].startswith(('alloy-node-','lighthouse-')) for job in prom['scrape_configs']))

    def test_collector_has_dedicated_read_only_sources_bounded_labels_and_persistent_positions(self):
        runtime = prepare.prepare({**self.config,'node_agent':self.agent},self.root/'runtime')
        compose = json.loads((runtime/'node-agent/compose.yaml').read_text())
        service = compose['services']['alloy-node']
        self.assertEqual(service['network_mode'],'host')
        self.assertEqual(service['user'],'alloy')
        self.assertEqual(service['group_add'],['1000'])
        self.assertIn('--server.http.listen-addr=127.0.0.1:12346',service['command'])
        binds = [volume for volume in service['volumes'] if volume['type']=='bind']
        self.assertEqual(len(binds),4)
        self.assertTrue(all(volume['read_only'] and not volume['bind']['create_host_path'] for volume in binds))
        self.assertEqual(compose['volumes'],{'positions':{}})
        alloy = (runtime/'node-agent/config.alloy').read_text()
        for pattern in ('eez.jsonl*','beacon.log*','*.active','*.closed'):
            self.assertIn(pattern,alloy)
        self.assertNotIn('discv5.log',alloy)
        self.assertIn('regex = "filename"',alloy)
        self.assertNotIn('chain =',alloy)
        prometheus = json.loads((runtime/'prometheus/prometheus.yaml').read_text())
        scrape = next(job for job in prometheus['scrape_configs'] if job['job_name']=='alloy-node-host-one')
        self.assertEqual(scrape['metric_relabel_configs'],[{'source_labels':['path','filename'],'separator':'','regex':'.+','action':'drop'}])
        self.assertIn('sync_period = "5s"',alloy)
        manifest = json.loads((runtime/'node-agent/manifest.json').read_text())
        self.assertEqual(manifest['status'],'configured_not_runtime_verified')
        self.assertIn('unavailable',manifest['signer_diagnostic_file_logging'])
        for invalid in [True, -1, 2**32]:
            with self.assertRaises(ValueError):
                prepare.node_agent_config({**self.config,'node_agent':{**self.agent,'read_gid':invalid}})

    def test_explicit_owner_identity_only_initializes_private_positions_volume(self):
        agent = {**self.agent,'run_uid':10001,'run_gid':10002}
        runtime = prepare.prepare({**self.config,'node_agent':agent},self.root/'runtime')
        compose = json.loads((runtime/'node-agent/compose.yaml').read_text())
        self.assertEqual(compose['services']['alloy-node']['user'],'10001:10002')
        init = compose['services']['positions-owner']
        self.assertEqual(init['command'],['-R','10001:10002','/var/lib/alloy'])
        self.assertEqual(init['network_mode'],'none')
        self.assertEqual(init['cap_add'],['CHOWN','DAC_READ_SEARCH'])
        self.assertTrue(all(v['type']=='volume' for v in init['volumes']))
        for change in [{'run_uid':10001},{'run_gid':10001},{'run_uid':0,'run_gid':1},
                       {'run_uid':1,'run_gid':True},{'run_uid':1,'run_gid':2**32}]:
            with self.assertRaises(ValueError):
                prepare.node_agent_config({**self.config,'node_agent':{**self.agent,**change}})

    def test_source_identity_listener_and_endpoint_fail_closed(self):
        for change in [{'listen':'0.0.0.0:12346'},{'listen':'host:12346'}, {'listen':'127.0.0.1:0'},
                       {'allow_non_loopback':'true'}, {'loki_url':'http://user:password@host/loki/api/v1/push'},
                       {'loki_url':'http://host/push?token=secret'}, {'loki_url':'http://host:0/loki/api/v1/push'}, {'sources':[]}, {'surprise':True}]:
            with self.assertRaises(ValueError):
                prepare.prepare({**self.config,'node_agent':{**self.agent,**change}},self.root/'invalid')
            self.assertFalse((self.root/'invalid').exists())
        for change in [{'kind':'signer_json','service':'eez-proof-signer'}, {'directory':'/'},
                       {'directory':'relative'}, {'directory':'/node/../other'}, {'directory':'/node/$HOME'},
                       {'service':[]}, {'instance':'bad label'}]:
            sources = [{**self.agent['sources'][0],**change}]
            with self.assertRaises(ValueError):
                prepare.node_agent_config({**self.config,'node_agent':{**self.agent,'sources':sources}})
        public = {**self.agent,'listen':'10.0.0.2:12346','allow_non_loopback':True}
        self.assertEqual(prepare.node_agent_config({**self.config,'node_agent':public})['listen'],'10.0.0.2:12346')

    def test_duplicate_directories_and_producer_claims_are_rejected(self):
        first = self.agent['sources'][0]
        for second in [dict(first),{**first,'kind':'lifecycle'},
                       {**first,'directory':'/other'}, {**first,'instance':'other','directory':'/node//diagnostic'}]:
            with self.assertRaises(ValueError):
                prepare.node_agent_config({**self.config,'node_agent':{**self.agent,'sources':[first,second]}})

    def test_lighthouse_target_is_explicit_bounded_and_never_infers_execution_chain(self):
        client = {'service':'lighthouse','instance':'consensus','metrics_target':'consensus-private:5054'}
        runtime = prepare.prepare({**self.config,'client_targets':[client]},self.root/'runtime')
        prom = json.loads((runtime/'prometheus/prometheus.yaml').read_text())
        job = next(job for job in prom['scrape_configs'] if job['job_name']=='lighthouse-consensus')
        labels = job['static_configs'][0]['labels']
        self.assertEqual(labels['service'],'lighthouse')
        self.assertNotIn('chain',labels)
        self.assertNotIn('chain_id',labels)
        self.assertEqual(job['metric_relabel_configs'][0]['action'],'keep')
        self.assertNotIn('gossipsub',job['metric_relabel_configs'][0]['regex'])
        for targets in [[client,client],[{**client,'service':'reth'}],[{**client,'metrics_target':'http://host:5054'}]]:
            with self.assertRaises(ValueError):
                prepare.client_targets({**self.config,'client_targets':targets})

    def test_native_upstream_identifier_samples_are_dropped_whole(self):
        process = {'service':'eez-composer','instance':'native','metrics_target':'native:19101','metrics_listen':'127.0.0.1:19101'}
        runtime = prepare.prepare({**self.config,'native_processes':[process]},self.root/'runtime')
        prom = json.loads((runtime/'prometheus/prometheus.yaml').read_text())
        job = next(job for job in prom['scrape_configs'] if job['job_name']=='eez-composer-native')
        self.assertEqual(job['metric_relabel_configs'],[{'source_labels':['hash','address'],'separator':'','regex':'.+','action':'drop'}, {'source_labels':['__name__'],'regex':'(reth_)?process_cli_args','action':'drop'}])

    def test_reconfiguration_removes_only_generated_node_files(self):
        runtime = prepare.prepare({**self.config,'node_agent':self.agent},self.root/'runtime')
        custom = runtime/'node-agent/operator-notes.txt'
        custom.write_text('preserve')
        prepare.prepare(self.config,runtime)
        self.assertTrue(custom.exists())
        for name in ('compose.yaml','config.alloy','manifest.json'):
            self.assertFalse((runtime/'node-agent'/name).exists())


class MetricInventory(unittest.TestCase):
    def test_quoted_values_cannot_become_label_keys_or_artifact_values(self):
        import sys
        sys.path.insert(0,str(ROOT/'tests'))
        from client_metrics import inventory
        result = inventory('# TYPE fixture gauge\nfixture{role="secret,pretend_key=\\\"account\\\",}tail",valid="private-url"} 1\n')
        self.assertEqual(result['samples'],{'fixture':['role','valid']})
        self.assertNotIn('secret',json.dumps(result))
        self.assertNotIn('private-url',json.dumps(result))
