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

class OwnedCleanupTests(unittest.TestCase):
    """Local fake Docker plus real private subprocesses; never native acceptance."""
    def test_shutdown_reaps_leader_when_it_exits_at_final_deadline(self):
        import os
        import signal
        import subprocess
        import sys
        from unittest.mock import patch
        marker = self.repo / 'resistant-ready'
        child = subprocess.Popen([sys.executable, '-c',
            'import pathlib,signal,sys,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);'
            'pathlib.Path(sys.argv[1]).touch();time.sleep(30)', str(marker)],
            start_new_session=True)
        send = signal.pidfd_send_signal

        def stop_and_observe(descriptor, sig):
            send(descriptor, sig)
            if sig == signal.SIGKILL:
                deadline = time.monotonic() + 5
                while os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOWAIT | os.WNOHANG) is None:
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.005)

        try:
            deadline = time.monotonic() + 5
            while not marker.exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.005)
            identity = runner.process_identity(child.pid)
            # Observe exit without reaping after KILL, at the final grace edge.
            with patch.object(runner.signal, 'pidfd_send_signal', side_effect=stop_and_observe):
                self.assertTrue(runner.stop_owned_processes(child, identity, grace=0))
            with self.assertRaises(ChildProcessError):
                os.waitpid(child.pid, os.WNOHANG)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=2)

    def setUp(self):
        import shutil
        self.root = tempfile.TemporaryDirectory()
        self.repo = Path(self.root.name)
        self.owned = runner.reserve_observer(self.repo)
        self.addCleanup(self.root.cleanup)
        self.addCleanup(lambda: shutil.rmtree(self.owned['fixture']['socket_parent'], ignore_errors=True))
        self.inspected = None
        self.removal_fails = False
        self.calls = []

    def container(self):
        fixture = self.owned['fixture']
        data = Path(fixture['database_parent']) / 'fixture-data'
        data.mkdir()
        socket = Path(fixture['socket_parent']) / 'fixture-config/socket'
        socket.mkdir(parents=True)
        runner.write(Path(fixture['database_parent']) / 'container.json', {'id': 'a'*64, 'name': fixture['container_name']})
        self.inspected = {'Id': 'a'*64, 'Name': '/' + fixture['container_name'],
                          'Config': {'Labels': {'eez.overhead.owner': fixture['container_name']}},
                          'Mounts': [{'Type':'bind','Source':str(data),'Destination':'/var/lib/postgresql'},
                                     {'Type':'bind','Source':str(socket),'Destination':'/var/run/postgresql'}]}

    def docker(self, args, **kwargs):
        import subprocess
        self.calls.append(args)
        self.assertEqual(args[0], 'docker')
        if args[1] == 'ps':
            self.assertIn('name=^/' + self.owned['fixture']['container_name'] + '$', args)
            return subprocess.CompletedProcess(args, 0, self.inspected['Id']+'\n' if self.inspected else '', '')
        if args[1] == 'inspect':
            self.assertEqual(args[2], self.inspected['Id'])
            return subprocess.CompletedProcess(args, 0, json.dumps([self.inspected]), '')
        if args[1] == 'rm':
            self.assertEqual(args[-1], self.inspected['Id'])
            if not self.removal_fails:
                self.inspected = None
            return subprocess.CompletedProcess(args, int(self.removal_fails), '', '')
        self.fail(f'unexpected Docker command {args}')

    def cleanup(self, failed=False, stopped=True, diagnostic='preserved'):
        from unittest.mock import patch
        with patch.object(runner.subprocess, 'run', side_effect=self.docker), patch.object(runner, 'preserve_failure', return_value={'path':'synthetic-diagnostic', 'status':diagnostic}) as capture:
            result = runner.cleanup_observer(self.owned, failed, stopped)
        return result, capture

    def test_before_ready_untouched_reservations_are_removed(self):
        result, capture = self.cleanup(failed=True)
        self.assertTrue(result['owned_cleanup_verified'])
        self.assertEqual(result['retained_owned_paths'], [])
        capture.assert_not_called()

    def test_success_also_removes_exact_container_and_parents(self):
        self.container()
        result, capture = self.cleanup()
        self.assertTrue(result['owned_cleanup_verified'])
        self.assertIsNone(self.inspected)
        capture.assert_not_called()

    def test_failed_arm_preserves_before_removal(self):
        self.container()
        result, capture = self.cleanup(failed=True)
        self.assertTrue(result['owned_cleanup_verified'])
        capture.assert_called_once_with(self.owned, 'a'*64)
        self.assertEqual(result['failure_diagnostics']['status'], 'preserved')

    def test_failed_dump_retains_data_after_stopping_container(self):
        self.container()
        result, _ = self.cleanup(failed=True, diagnostic='incomplete')
        self.assertFalse(result['owned_cleanup_verified'])
        self.assertTrue(result['owned_container_absent'])
        self.assertFalse(result['owned_parents_removed'])
        self.assertEqual(len(result['retained_owned_paths']), 2)
        self.assertTrue((Path(self.owned['fixture']['database_parent'])/'fixture-data').is_dir())

    def test_failed_rm_retains_data_and_reports_live_container(self):
        self.container()
        self.removal_fails = True
        result, _ = self.cleanup(failed=True)
        self.assertFalse(result['owned_cleanup_verified'])
        self.assertFalse(result['owned_container_absent'])
        self.assertEqual(len(result['retained_owned_paths']), 2)

    def test_partial_daemon_creation_without_receipt_fails_closed(self):
        (Path(self.owned['fixture']['database_parent'])/'fixture-data').mkdir()
        result, _ = self.cleanup(failed=True)
        self.assertFalse(result['owned_cleanup_verified'])
        self.assertFalse(result['owned_parents_removed'])
        self.assertIn('reconciliation', result['cleanup_error'])
        self.assertFalse(any(cmd[1] == 'rm' for cmd in self.calls))

    def test_foreign_label_or_mount_or_receipt_never_removed(self):
        self.container()
        for mutate in (
            lambda: self.inspected['Config']['Labels'].update({'eez.overhead.owner':'foreign'}),
            lambda: self.inspected['Mounts'][0].update(Source='/tmp/foreign'),
            lambda: runner.write(Path(self.owned['fixture']['database_parent'])/'container.json', {'id':'b'*64, 'name':self.owned['fixture']['container_name']}),
        ):
            import copy
            before = copy.deepcopy(self.inspected)
            mutate()
            result, capture = self.cleanup(failed=True)
            self.assertFalse(result['owned_cleanup_verified'])
            self.assertFalse(any(cmd[1] == 'rm' for cmd in self.calls))
            capture.assert_not_called()
            self.inspected = before

    def test_symlink_parent_or_mount_never_removed(self):
        import shutil
        self.container()
        path = Path(self.inspected['Mounts'][0]['Source'])
        path.rmdir()
        foreign = self.repo/'foreign'
        foreign.mkdir()
        (foreign/'sentinel').write_text('retain')
        path.symlink_to(foreign, target_is_directory=True)
        result, _ = self.cleanup()
        self.assertFalse(result['owned_cleanup_verified'])
        self.assertTrue((foreign/'sentinel').exists())
        path.unlink()
        path.mkdir()
        parent = Path(self.owned['fixture']['database_parent'])
        shutil.rmtree(parent)
        parent.symlink_to(foreign, target_is_directory=True)
        result, _ = self.cleanup()
        self.assertFalse(result['owned_cleanup_verified'])
        self.assertTrue((foreign/'sentinel').exists())
        parent.unlink()

    def test_live_native_processes_prevent_container_or_data_removal(self):
        self.container()
        result, capture = self.cleanup(stopped=False)
        self.assertFalse(result['owned_cleanup_verified'])
        self.assertFalse(self.calls)
        capture.assert_not_called()

    def test_controller_error_before_and_after_ready_is_failed_with_verified_cleanup(self):
        import os
        import sys
        from unittest.mock import patch, MagicMock
        manifest_dir = self.repo/'crates/eez-node-e2e'
        manifest_dir.mkdir(parents=True)
        (manifest_dir/'Cargo.toml').write_text('[package]\nname="synthetic-native"\n')
        (self.repo/'Cargo.toml').write_text('[workspace]\n')
        candidate = self.repo/'candidate'
        candidate.mkdir()
        executable = candidate/'dynamic_admission'
        executable.write_text(f'#!{sys.executable}\nimport time\ntime.sleep(30)\n')
        executable.chmod(0o700)
        sink = Mock(address='127.0.0.1:12345')
        sink.snapshot.return_value = {'healthy':True}
        for after_ready in (False, True):
            if after_ready:
                self.owned = runner.reserve_observer(self.repo)
                self.container()
            sampler = MagicMock()
            sampler.__enter__.return_value = sampler
            sampler.snapshot.return_value = {}
            waiting = [TimeoutError('injected before ready')] if not after_ready else [
                {'processes':{}}, TimeoutError('injected after ready')]
            with patch.dict(os.environ, {'CARGO_MANIFEST_DIR':str(manifest_dir)}), \
                    patch.object(runner, 'reserve_observer', return_value=self.owned), \
                    patch.object(runner, 'wait_file', side_effect=waiting), \
                    patch.object(runner, 'ResourceSampler', return_value=sampler), \
                    patch.object(runner, 'effective_configuration'), \
                    patch.object(runner, 'preserve_failure', return_value={'status':'preserved','path':'synthetic'}), \
                    patch.object(runner.subprocess, 'run', side_effect=self.docker):
                result = runner.arm(candidate, self.repo/str(after_ready), 1, 'B', 20, False,
                                    sink, 900, 9000, time.monotonic()+9000)
            self.assertEqual(result['status'], 'failed')
            self.assertTrue(result['owned_processes_stopped'])
            self.assertTrue(result['owned_cleanup_verified'], result)
            self.assertIn('injected', result['error'])
            self.assertIsNone(self.inspected)
            self.assertEqual(runner.load(self.repo/str(after_ready)/'result.json'), result)

    def test_native_launch_failure_cleans_pre_reserved_empty_roots(self):
        import os
        from unittest.mock import patch
        manifest_dir = self.repo/'crates/eez-node-e2e'
        manifest_dir.mkdir(parents=True)
        (manifest_dir/'Cargo.toml').write_text('[package]\nname="synthetic-native"\n')
        (self.repo/'Cargo.toml').write_text('[workspace]\n')
        sink = Mock(address='127.0.0.1:12345')
        sink.snapshot.return_value = {'healthy':True}
        with patch.dict(os.environ, {'CARGO_MANIFEST_DIR':str(manifest_dir)}), \
                patch.object(runner, 'reserve_observer', return_value=self.owned), \
                patch.object(runner.subprocess, 'run', side_effect=self.docker):
            result = runner.arm(self.repo/'missing', self.repo/'run', 1, 'B', 20, False,
                                sink, 900, 9000, time.monotonic()+9000)
        self.assertEqual(result['status'], 'failed')
        self.assertTrue(result['owned_cleanup_verified'], result)

    def test_diagnostic_write_error_kills_and_reaps_sleeping_child_promptly(self):
        import errno
        import os
        import subprocess
        import sys
        from unittest.mock import patch
        destination = self.repo/'write-error.dump'
        original_open = Path.open
        original_popen = subprocess.Popen
        children = []
        class FailingOutput:
            def __init__(self, output):
                self.output = output
            def __enter__(self):
                return self
            def __exit__(self, *args):
                self.output.close()
            def write(self, data):
                raise OSError(errno.ENOSPC, 'injected full diagnostic disk')
        def open_file(path, *args, **kwargs):
            output = original_open(path, *args, **kwargs)
            return FailingOutput(output) if path == destination else output
        def launch(*args, **kwargs):
            child = original_popen(*args, **kwargs)
            children.append(child)
            return child
        started = time.monotonic()
        try:
            with patch.object(Path, 'open', open_file), patch.object(runner.subprocess, 'Popen', side_effect=launch):
                result = runner.bounded_diagnostic([sys.executable, '-c',
                    'import time;print("dump output",flush=True);time.sleep(30)'], destination, 30, 1024)
            self.assertLess(time.monotonic()-started, 4)
            self.assertFalse(result['complete'])
            self.assertTrue(result['child_reaped'])
            self.assertIn('injected full diagnostic disk', result['reason'])
            self.assertEqual(len(children), 1)
            self.assertIsNotNone(children[0].returncode)
            with self.assertRaises(ChildProcessError):
                os.waitpid(children[0].pid, os.WNOHANG)
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=2)

    def test_manifest_root_absent_explicit_precedence_and_invalid_explicit(self):
        from unittest.mock import patch
        native = self.repo/'crates/eez-node-e2e'
        native.mkdir(parents=True)
        (native/'Cargo.toml').write_text('[package]\nname="synthetic-native"\n')
        (self.repo/'Cargo.toml').write_text('[workspace]\n')
        with patch.object(Path, 'cwd', return_value=self.repo):
            self.assertEqual(runner.native_manifest_directory({}), native)
        with patch.object(Path, 'cwd', return_value=Path('/nonexistent-cwd')):
            self.assertEqual(runner.native_manifest_directory({'CARGO_MANIFEST_DIR':str(native)}), native)
        for invalid in ('', 'relative/path', str(self.repo), str(self.repo/'missing')):
            with patch.object(Path, 'cwd', return_value=self.repo), self.assertRaises((ValueError, FileNotFoundError)):
                runner.native_manifest_directory({'CARGO_MANIFEST_DIR':invalid})
        (native/'Cargo.toml').unlink()
        with self.assertRaises(ValueError):
            runner.native_manifest_directory({'CARGO_MANIFEST_DIR':str(native)})

    def test_repo_root_arm_sets_child_manifest_when_environment_is_absent(self):
        import os
        from unittest.mock import patch
        native = self.repo/'crates/eez-node-e2e'
        native.mkdir(parents=True)
        (native/'Cargo.toml').write_text('[package]\nname="synthetic-native"\n')
        (self.repo/'Cargo.toml').write_text('[workspace]\n')
        sink = Mock(address='127.0.0.1:12345')
        sink.snapshot.return_value = {'healthy':True}
        environment = {k:v for k,v in os.environ.items() if k != 'CARGO_MANIFEST_DIR'}
        with patch.dict(os.environ, environment, clear=True), \
                patch.object(Path, 'cwd', return_value=self.repo), \
                patch.object(runner, 'reserve_observer', return_value=self.owned), \
                patch.object(runner.subprocess, 'Popen', side_effect=FileNotFoundError('synthetic missing native')) as launch, \
                patch.object(runner.subprocess, 'run', side_effect=self.docker):
            result = runner.arm(self.repo/'missing', self.repo/'run', 1, 'B', 20, False,
                                sink, 900, 9000, time.monotonic()+9000)
        self.assertEqual(launch.call_args.kwargs['env']['CARGO_MANIFEST_DIR'], str(native))
        self.assertEqual(result['status'], 'failed')
        self.assertTrue(result['owned_cleanup_verified'])

    def test_shutdown_exception_retains_complete_ownership_failure_evidence(self):
        import os
        from unittest.mock import patch
        native = self.repo/'crates/eez-node-e2e'
        native.mkdir(parents=True)
        (native/'Cargo.toml').write_text('[package]\nname="synthetic-native"\n')
        (self.repo/'Cargo.toml').write_text('[workspace]\n')
        sink = Mock(address='127.0.0.1:12345')
        sink.snapshot.return_value = {'healthy':True}
        with patch.dict(os.environ, {'CARGO_MANIFEST_DIR':str(native)}), \
                patch.object(runner, 'reserve_observer', return_value=self.owned), \
                patch.object(runner, 'stop_owned_processes', side_effect=OSError('injected proc failure')), \
                patch.object(runner.subprocess, 'run', side_effect=self.docker):
            result = runner.arm(self.repo/'missing', self.repo/'run', 1, 'B', 20, False,
                                sink, 900, 9000, time.monotonic()+9000)
        self.assertEqual(result['status'], 'failed')
        for key in ('owned_cleanup_verified', 'owned_processes_stopped', 'owned_container_absent', 'owned_parents_removed'):
            self.assertIs(result[key], False)
        self.assertEqual(result['retained_owned_paths'], [self.owned['fixture'][k] for k in ('database_parent', 'socket_parent')])
        self.assertTrue(all(Path(p).is_dir() for p in result['retained_owned_paths']))
        self.assertIn('injected proc failure', result['cleanup_error'])
        self.assertFalse(self.calls)

    def test_bounded_diagnostics_preserve_output_and_reject_timeout_or_overflow(self):
        import sys
        for name, code, seconds, limit, complete in [
            ('ok', 'print("preserved")', 2, 100, True),
            ('large', 'print("x"*1000)', 2, 100, False),
            ('timeout', 'import time;time.sleep(10)', .05, 100, False),
            ('error', 'raise SystemExit(3)', 2, 100, False),
        ]:
            result = runner.bounded_diagnostic([sys.executable, '-c', code], self.repo/name, seconds, limit)
            self.assertEqual(result['complete'], complete)
            self.assertLessEqual((self.repo/name).stat().st_size, limit)

    def test_sigterm_leader_exit_does_not_hide_owned_sibling(self):
        import os
        import signal
        import subprocess
        import sys
        pidfile = self.repo/'sibling.pid'
        sibling = 'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);print("ready",flush=True);time.sleep(30)'
        code = ('import subprocess,sys,time,pathlib;'
                f'p=subprocess.Popen([sys.executable,"-c",{sibling!r}],stdout=subprocess.PIPE);'
                'p.stdout.readline();'
                f'pathlib.Path({str(pidfile)!r}).write_text(str(p.pid));time.sleep(30)')
        child = subprocess.Popen([sys.executable, '-c', code], start_new_session=True)
        identity = runner.process_identity(child.pid)
        try:
            until = time.monotonic()+3
            while not pidfile.exists() and time.monotonic()<until:
                time.sleep(.01)
            self.assertTrue(pidfile.exists())
            sibling_pid = int(pidfile.read_text())
            child.terminate()
            child.wait(timeout=2)
            self.assertNotEqual(runner.process_identity(sibling_pid)[3], 'Z')
            self.assertTrue(runner.stop_owned_processes(child, identity, grace=.1))
            value = runner.process_identity(sibling_pid)
            self.assertTrue(value is None or value[3] == 'Z')
        finally:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=2)

class LeaderGenerationTests(unittest.TestCase):
    def scenario(self, identities, entries, open_effect=None):
        from unittest.mock import patch
        child=Mock(pid=100,returncode=0)
        child.wait.return_value=0
        saved=(100,100,10,'S')
        proc_entries=[Mock(name=str(pid)) for pid in entries]
        for entry,pid in zip(proc_entries,entries):
            entry.name=str(pid)
            entry.stat.return_value=Mock(st_uid=123)
        def send(*args):
            identities.clear()  # emulate exit after signaling, without real work
        with patch.object(runner,'process_identity',side_effect=lambda pid:identities.get(pid)), \
                patch.object(runner.Path,'iterdir',side_effect=lambda:iter(proc_entries)), \
                patch.object(runner.os,'getuid',return_value=123), \
                patch.object(runner.os,'pidfd_open',side_effect=open_effect or (lambda pid:999)) as opened, \
                patch.object(runner.os,'close') as closed, \
                patch.object(runner.signal,'pidfd_send_signal',side_effect=send) as signal:
            try:
                result=runner.stop_owned_processes(child,saved,grace=0)
                error=None
            except ValueError as caught:
                result=False;error=str(caught)
            return result,error,signal.call_count,opened.call_count,closed.call_count,child.wait.call_count

    def test_visible_reused_leader_is_rejected_before_any_signal(self):
        result,error,signals,opened,_,waited=self.scenario({100:(100,100,50,'S')},[100])
        self.assertEqual(signals,0)
        self.assertFalse(result)
        self.assertIn('reused',error or '')
        self.assertEqual((signals,opened,waited),(0,0,0))

    def test_visible_reuse_after_scan_is_rejected_before_pidfd_signal(self):
        identities={101:(100,100,11,'S')}
        def opened(pid):
            identities[100]=(100,100,50,'S')
            return 999
        result,error,signals,opened_count,closed,_=self.scenario(identities,[101],opened)
        self.assertEqual(signals,0)
        self.assertFalse(result)
        self.assertIn('reused',error or '')
        self.assertEqual((signals,opened_count,closed),(0,1,1))

    def test_absent_reaped_leader_still_allows_original_session_sibling(self):
        result,error,signals,_,_,waited=self.scenario({101:(100,100,11,'S')},[101])
        self.assertTrue(result)
        self.assertIsNone(error)
        self.assertEqual((signals,waited),(1,1))

    def test_original_live_leader_remains_owned(self):
        result,error,signals,_,_,waited=self.scenario({100:(100,100,10,'R')},[100])
        self.assertTrue(result)
        self.assertIsNone(error)
        self.assertEqual((signals,waited),(1,1))
