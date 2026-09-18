"""Runner mechanics only; synthetic fixtures are never performance evidence."""
import importlib.util
import json
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import Mock

spec = importlib.util.spec_from_file_location('overhead_runner', Path(__file__).with_name('run-overhead.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class RunnerTests(unittest.TestCase):
    def test_budget_defaults_maxima_and_invalid_combinations(self):
        import contextlib
        import io
        required = ['--candidate','/tmp/candidate','--output','/tmp/output']
        default = runner.arguments(required)
        self.assertEqual((default.waves,default.arm_seconds,default.experiment_seconds),(1,600,9000))
        maximum = runner.arguments(required+['--waves','64','--arm-seconds','1800','--experiment-seconds','21600'])
        self.assertEqual((maximum.waves,maximum.arm_seconds,maximum.experiment_seconds),(64,1800,21600))
        for values in [['--waves','0'],['--waves','65'],['--arm-seconds','0'],['--arm-seconds','1801'],['--experiment-seconds','0'],['--experiment-seconds','21601'],['--experiment-seconds','599']]:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                runner.arguments(required+values)

    def test_final_teardown_and_rehash_time_cannot_pass_budget(self):
        for elapsed, expected in [(9000, 'raw_capture_complete'), (9000.001, 'budget_exhausted')]:
            report = {'status':'raw_capture_complete','experiment_seconds':9000,'runs':[{'status':'captured'}]}
            runner.finalize_budget(report, elapsed)
            self.assertEqual(report['status'],expected)
            self.assertEqual(report['elapsed_seconds'],elapsed)
            self.assertEqual(report['budget_exhausted'],expected=='budget_exhausted')
            self.assertEqual(report['runs'],[{'status':'captured'}])
        report = {'status':'budget_exhausted','experiment_seconds':9000}
        self.assertEqual(runner.finalize_budget(report,100)['status'],'budget_exhausted')
        for invalid in [True,-1,float('nan'),float('inf')]:
            with self.assertRaises(ValueError):
                runner.finalize_budget({'status':'raw_capture_complete','experiment_seconds':9000},invalid)

    def test_atomic_bounded_json_round_trip(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'data.json'
            runner.write(path, {'stage': 'busy_waves', 'count': 128})
            self.assertEqual(runner.load(path)['count'], 128)
            self.assertFalse(path.with_suffix('.pending').exists())
            with self.assertRaises(ValueError):
                runner.load(path, limit=1)

    def test_missing_handshake_checks_exit_and_deadline(self):
        with tempfile.TemporaryDirectory() as root:
            child = Mock(returncode=7)
            child.poll.return_value = 7
            with self.assertRaisesRegex(RuntimeError, 'exited 7'):
                runner.wait_file(Path(root)/'absent', child, time.monotonic()+1)
            child.poll.return_value = None
            with self.assertRaises(TimeoutError):
                runner.wait_file(Path(root)/'absent', child, time.monotonic()-1)

    def test_exact_probe_event_field_and_complete_log_identity(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)/'eez-node-node-1-1.log'
            path.write_text('startup diagnostic\n'+json.dumps({'fields': {'event_name':'eez.benchmark.held_pool_sample','sequence':1}})+'\n'+json.dumps({'fields':{'event_name':'eez.composer.admission.prepared'}})+'\n')
            self.assertEqual(runner.lock_events(Path(root)), [{'event_name':'eez.benchmark.held_pool_sample','sequence':1}])
            (Path(root)/'eez-node-node-2-1.log').write_text('')
            with self.assertRaisesRegex(ValueError, 'one complete'):
                runner.lock_events(Path(root))

    def test_capture_attestation_requires_complete_population_and_records(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory/'eez-node-node-1-1.log'
            path.write_text(json.dumps({'fields': {'event_name':'eez.composer.admission.prepared'}})+'\n')
            evidence = {'stages':[{'preparation_group_counts':[8], 'successful_sources':8, 'preparation_ns':[71_123_456]}]}
            proof = runner.capture_attestation(directory, evidence)
            self.assertFalse(proof['logger_gaps'])
            self.assertFalse(proof['sample_overflow'])
            self.assertTrue(proof['capture_proof']['exact_prepared_source_population'])
            evidence['stages'][0]['successful_sources'] = 9
            with self.assertRaisesRegex(ValueError, 'exact successful'):
                runner.capture_attestation(directory, evidence)
            evidence['stages'][0]['successful_sources'] = 8
            path.write_text('{"fields":{}}')
            with self.assertRaisesRegex(ValueError, 'incomplete'):
                runner.capture_attestation(directory, evidence)
            path.write_text('[tracing-subscriber] Unable to write an event to the Writer\n')
            with self.assertRaisesRegex(ValueError, 'logger reported'):
                runner.capture_attestation(directory, evidence)
            path.write_text(json.dumps({'fields':{'event_name':'eez.benchmark.held_pool_overflow'}})+'\n')
            with self.assertRaisesRegex(ValueError, 'overflow'):
                runner.capture_attestation(directory, evidence)

    def test_effective_configuration_records_actual_reservation_and_only_genesis_timestamp_policy(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            candidate = directory/'candidate'
            candidate.mkdir()
            runner.write(candidate/'manifest.json', {'profile':'synthetic','sha256':{'rustc.txt':'a'*64}})
            native = {key:{} for key in ('workers','native_l1_profile','protocol','logging','observer_indexer','workload')}
            native['observer_indexer']['postgres'] = {
                'storage':{'type':'owned_bind','filesystem':'ext4','data_directory':'/var/lib/postgresql/18/docker'},
                'shared_buffers_bytes':134217728,'max_connections':30,'memory_bytes':123,'nano_cpus':456}
            runner.write(directory/'native-effective.json', native)
            runner.write(directory/'genesis_evidence.json', {'l1':{},'l2':{}})
            for role in ('l1','l2'):
                runner.write(directory/f'{role}-genesis.json', {'timestamp':'0x1','config':{'chainId':7}})
            ready = {'processes':{'composer':__import__('os').getpid(),'observer':None}, 'telemetry_enabled':False}
            sampler = Mock(postgres={'memory_limit_bytes':123,'nano_cpus':456},limit=632)
            runner.effective_configuration(candidate,directory,ready,sampler)
            effective = runner.load(directory/'effective_config.json')
            self.assertEqual(effective['common']['postgres_reservation'], sampler.postgres)
            self.assertEqual(effective['common']['observer_indexer']['postgres'],native['observer_indexer']['postgres'])
            self.assertNotIn('postgres',effective['common'])
            self.assertFalse(effective['observer_running'])
            self.assertEqual(effective['common']['genesis']['l1'],effective['common']['genesis']['l2'])
            self.assertEqual(runner.load(directory/'l1-genesis.json')['timestamp'],'0x1')
            self.assertEqual(runner.load(directory/'genesis_evidence.json')['l1']['json_sha256'],runner.digest(directory/'l1-genesis.json'))

    def test_candidate_verification_rejects_replacement_and_escape(self):
        with tempfile.TemporaryDirectory() as root:
            candidate = Path(root) / 'candidate'
            candidate.mkdir()
            path = candidate / 'binary'
            path.write_bytes(b'original')
            manifest = {'sha256': {'binary': runner.digest(path)}}
            runner.verify_candidate(candidate, manifest)
            path.write_bytes(b'replaced')
            with self.assertRaisesRegex(ValueError, 'candidate mismatch'):
                runner.verify_candidate(candidate, manifest)
            outside = Path(root) / 'outside'
            outside.write_bytes(b'outside')
            with self.assertRaisesRegex(ValueError, 'candidate mismatch'):
                runner.verify_candidate(candidate, {'sha256': {'../outside': runner.digest(outside)}})

    def test_digest_changes_with_candidate(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)/'binary'
            path.write_bytes(b'a')
            first = runner.digest(path)
            path.write_bytes(b'b')
            self.assertNotEqual(first, runner.digest(path))
