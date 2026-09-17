import copy
import http.server
import json
import os
import socket
from pathlib import Path
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch
import urllib.error
import urllib.request
import uuid

from overhead_resources import OtlpSink, ResourceSampler, counters, device_differences, phase_report, process, process_stat, scrape_metrics


def sample(sequence, second):
    return {'sequence': sequence, 'started_unix_ns': second * 10**9,
            'finished_unix_ns': second * 10**9 + 10, 'monotonic_ns': second * 10**9,
            'finished_monotonic_ns': second * 10**9 + 10,
            'processes': {'composer': {'pid': 20, 'start_ticks': 5, 'cpu_ticks': 10 * second,
                'minor_faults': second, 'major_faults': 0, 'rss_bytes': 100,
                'lifetime_hwm_bytes': 150, 'read_bytes': second, 'write_bytes': second}},
            'postgres': {'cpu': {'usage_usec': 10000 * second, 'user_usec': 5000 * second,
                                'system_usec': 5000 * second},
                         'memory_bytes': 200, 'lifetime_peak_bytes': 300,
                         'memory_events': {'oom': 0, 'oom_kill': 0, 'max': second},
                         'io_devices': {'1:2': {'rbytes': second}}},
            'host': {'cpu_count': 2, 'cpu_ticks': {'user': 50 * second, 'system': 30 * second,
                     'idle': 100 * second, 'iowait': 10 * second, 'steal': 10 * second},
                     'memory_bytes': {'MemAvailable': 1000}, 'vm': {'pgmajfault': second},
                     'disk_devices': {'vda': {'read_sectors': second}}}}


PHASE = {'name': 'busy', 'start_unix_ns': 1_500_000_000, 'end_unix_ns': 2_500_000_000}


class ResourceEvidence(unittest.TestCase):
    def test_metrics_capture_exact_selected_samples_and_never_follow_redirects(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                body = (b'# TYPE reth_eez_telemetry_dropped_events_total counter\n'
                        b'reth_eez_telemetry_dropped_events_total{signal="trace",reason="queue_full"} 0\n'
                        b'reth_eez_trace_exported_spans_total 7\nother_unknown NaN\n')
                self.send_response(self.server.response_status)
                self.send_header('Location', 'http://example.invalid/')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            def log_message(self, *_): pass
        server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        server.response_status = 200
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            address = f'127.0.0.1:{server.server_port}'
            result = scrape_metrics(address)
            self.assertEqual(result['status'], 'passed')
            self.assertEqual(result['samples']['reth_eez_trace_exported_spans_total'], '7')
            self.assertEqual(len(result['samples']), 2)
            server.response_status = 302
            with self.assertRaisesRegex(ValueError, 'redirects'):
                scrape_metrics(address)
            with self.assertRaises(ValueError): scrape_metrics('192.0.2.1:80')
        finally:
            server.shutdown(); server.server_close(); thread.join(timeout=3)

    def test_proc_stat_comm_spaces_parentheses_do_not_shift_counters(self):
        fields = ['S'] + ['0'] * 22
        for index, value in [(7, 4), (9, 5), (11, 6), (12, 7), (19, 99)]:
            fields[index] = str(value)
        self.assertEqual(process_stat('123 (worker (odd) name)) ' + ' '.join(fields)),
                         {'start_ticks': 99, 'cpu_ticks': 13, 'minor_faults': 4, 'major_faults': 5})

    def test_resource_units_and_duplicate_counters_fail_explicitly(self):
        self.assertEqual(counters('VmRSS: 42 kB\nThreads: 3'), {'VmRSS': 42, 'Threads': 3})
        for invalid in ('cpu 4 MB', 'cpu -1', 'cpu 1\ncpu 2'):
            with self.assertRaises(ValueError): counters(invalid)

    def test_actual_current_process_identity_rss_and_io(self):
        result = process(os.getpid())
        self.assertEqual(result['pid'], os.getpid())
        self.assertGreater(result['start_ticks'], 0)
        self.assertGreater(result['rss_bytes'], 0)
        self.assertGreaterEqual(result['lifetime_hwm_bytes'], result['rss_bytes'])

    def test_device_changes_keep_missing_io_unknown(self):
        result = device_differences({'old': {'bytes': 9}, 'same': {'bytes': 4}},
                                    {'new': {'bytes': 5}, 'same': {'bytes': 7}})
        self.assertEqual(result, {'deltas': {'same': {'bytes': 3}}, 'added': ['new'], 'removed': ['old']})

    def test_resource_bracket_keeps_native_pg_and_other_host_load_separate(self):
        result = phase_report([sample(1, 1), sample(2, 2), sample(3, 3)], PHASE, 100)
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['processes']['composer']['cpu_seconds'], .2)
        self.assertEqual(result['postgres']['cpu_seconds'], .02)
        self.assertAlmostEqual(result['host']['signed_other_cpu_seconds'], 1.38)
        self.assertAlmostEqual(result['host']['other_cpu_cores'], .73)
        self.assertEqual(result['host']['steal_fraction'], .05)
        self.assertEqual(result['leading_slack_ns'], 500_000_000)
        self.assertEqual(result['trailing_slack_ns'], 500_000_010)
        self.assertEqual(result['processes']['composer']['sampled_peak_rss_bytes'], 100)

    def test_missing_boundary_reused_pid_clock_step_reset_and_oom_cannot_pass(self):
        before, after = sample(1, 1), sample(2, 3)
        bad = []
        for mutate in (
                lambda x: x['processes']['composer'].update(start_ticks=55),
                lambda x: x['processes']['composer'].update(cpu_ticks=0),
                lambda x: x['postgres']['memory_events'].update(oom_kill=1),
                lambda x: x.update(monotonic_ns=x['monotonic_ns'] - 100_000_000)):
            changed = copy.deepcopy(after); mutate(changed); bad.append([before, changed])
        bad.extend(([before], [after]))
        for samples in bad:
            with self.subTest(samples=samples), self.assertRaises(ValueError):
                phase_report(samples, PHASE, 100)

    def test_interior_clock_step_cannot_disappear_between_outer_samples(self):
        samples = [sample(1, 1), sample(2, 2), sample(3, 3)]
        samples[1]['started_unix_ns'] += 100_000_000
        samples[1]['finished_unix_ns'] += 100_000_000
        with self.assertRaisesRegex(ValueError, 'clock discontinuity'):
            phase_report(samples, PHASE, 100)

    def test_impossible_cpu_accounting_is_not_zero_background_load(self):
        samples = [sample(1, 1), sample(2, 3)]
        samples[1]['processes']['composer']['cpu_ticks'] = 20000
        with self.assertRaisesRegex(ValueError, 'materially exceeds'):
            phase_report(samples, PHASE, 100)

    def test_reserved_roles_and_postgres_members_cannot_double_count(self):
        with self.assertRaises(ValueError):
            ResourceSampler({'processes': {'sampler_collector': 12345678}, 'postgres_container': 'fixture'})
        with patch('overhead_resources.postgres_identity', return_value={}), \
                patch('overhead_resources.in_postgres_cgroup', return_value=True):
            with self.assertRaisesRegex(ValueError, 'duplicates PostgreSQL'):
                ResourceSampler({'processes': {'composer': 12345678}, 'postgres_container': 'fixture'})

    def test_sampler_failure_is_retained_instead_of_zero_measurement(self):
        ready = {'processes': {'composer': 12345678}, 'postgres_container': 'fixture'}
        with patch('overhead_resources.postgres_identity', return_value={}), \
                patch('overhead_resources.in_postgres_cgroup', return_value=False):
            sampler = ResourceSampler(ready)
        with patch('overhead_resources.process', side_effect=FileNotFoundError('process exited')):
            with self.assertRaises(FileNotFoundError): sampler.sample_now()
        report = sampler.report([PHASE])
        self.assertEqual(report['status'], 'inconclusive')
        self.assertEqual(report['samples'], [])
        self.assertIn('process exited', report['errors'][0])

    def test_sampler_budget_and_restart_are_explicit_errors(self):
        ready = {'processes': {'composer': 12345678}, 'postgres_container': 'fixture'}
        base = sample(1, 1)
        with patch('overhead_resources.postgres_identity', return_value={}), \
                patch('overhead_resources.in_postgres_cgroup', return_value=False), \
                patch('overhead_resources.process', return_value=base['processes']['composer']), \
                patch('overhead_resources.postgres', return_value=base['postgres']), \
                patch('overhead_resources.host', return_value=base['host']):
            sampler = ResourceSampler(ready, max_samples=3)
            for _ in range(3): sampler.sample_now()
            with self.assertRaisesRegex(ValueError, 'sample budget'): sampler.sample_now()
            self.assertEqual(len(sampler.snapshot()['samples']), 3)
            self.assertEqual(sampler.report([PHASE])['max_samples'], 3)
            other = ResourceSampler(ready)
            other.sample_now()
            with patch('overhead_resources.process', return_value={**base['processes']['composer'], 'start_ticks': 99}):
                with self.assertRaisesRegex(ValueError, 'restarted'): other.sample_now()

    def test_final_report_cannot_race_live_sampler_or_hide_shutdown_error(self):
        ready = {'processes': {'composer': 12345678}, 'postgres_container': 'fixture'}
        with patch('overhead_resources.postgres_identity', return_value={}), \
                patch('overhead_resources.in_postgres_cgroup', return_value=False):
            sampler = ResourceSampler(ready)
        sampler.samples = [sample(1, 1), sample(2, 3)]
        with patch.object(sampler, 'thread') as thread:
            thread.is_alive.return_value = True
            report = sampler.report([PHASE])
            self.assertEqual(report['status'], 'inconclusive')
            self.assertIn('stop the sampler', report['errors'][0])
        sampler.shutdown_error = 'fixture stop timeout'
        self.assertEqual(sampler.report([PHASE])['errors'], ['fixture stop timeout'])

    def test_sink_actual_http_success_and_overflow_are_observable(self):
        with OtlpSink(max_bytes=5) as sink:
            request = urllib.request.Request('http://' + sink.address + '/v1/traces',
                data=b'abc', headers={'Content-Type': 'application/x-protobuf'})
            with urllib.request.urlopen(request, timeout=3) as response:
                self.assertEqual(response.status, 200)
                self.assertEqual(response.read(), b'')
            self.assertEqual(sink.snapshot()['bytes'], 3)
            self.assertTrue(sink.snapshot()['healthy'])
            with self.assertRaises(urllib.error.HTTPError) as failure:
                urllib.request.urlopen(request, timeout=3)
            self.assertEqual(failure.exception.code, 503)
            failure.exception.close()
            self.assertFalse(sink.snapshot()['healthy'])
            self.assertEqual(sink.snapshot()['bytes'], 3)

    def test_sink_wrong_path_does_not_fake_success(self):
        with OtlpSink() as sink:
            with self.assertRaises(urllib.error.HTTPError) as failure:
                urllib.request.urlopen(urllib.request.Request('http://' + sink.address + '/wrong', data=b'a'))
            failure.exception.close()
            self.assertEqual(sink.snapshot()['requests'], 0)
            self.assertFalse(sink.snapshot()['healthy'])

    def test_sink_shutdown_closes_incomplete_connection_with_bounded_wait(self):
        sink = OtlpSink()
        sink.__enter__()
        connection = socket.create_connection(('127.0.0.1', sink.server.server_port), timeout=3)
        try:
            connection.sendall(b'POST /v1/traces HTTP/1.1\r\nHost: fixture\r\n')
            deadline = time.monotonic() + 1
            while sink.connection is None and time.monotonic() < deadline:
                time.sleep(.001)
            self.assertIsNotNone(sink.connection)
            start = time.monotonic()
            sink.__exit__(None, None, None)
            self.assertLess(time.monotonic() - start, 3)
            self.assertFalse(sink.thread.is_alive())
        finally:
            connection.close()

    def test_sink_absolute_deadline_stops_trickled_body(self):
        with OtlpSink() as sink:
            with socket.create_connection(('127.0.0.1', sink.server.server_port), timeout=3) as connection:
                connection.sendall(b'POST /v1/traces HTTP/1.1\r\nHost: fixture\r\nContent-Length: 1000\r\n\r\n')
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and sink.snapshot()['healthy']:
                    try:
                        connection.sendall(b'x')
                    except OSError:
                        break
                    time.sleep(.05)
                self.assertFalse(sink.snapshot()['healthy'])
                self.assertIn('OTLP absolute connection deadline exceeded', sink.snapshot()['errors'])

    @unittest.skipUnless(os.environ.get('EEZ_TEST_RESOURCE_DOCKER') == '1', 'requires owned real PostgreSQL Docker fixture')
    def test_entire_postgres_cgroup_includes_query_worker_and_cleans_up(self):
        root = Path(__file__).resolve().parents[1]
        images = json.loads((root / 'images.lock.json').read_text())
        image = next(name + '@' + digest for name, digest in images.items() if name.startswith('postgres:'))
        name = 'eez-native-observer-resources-' + uuid.uuid4().hex[:10]
        def docker(*args):
            return subprocess.check_output(['docker', *args], text=True, stderr=subprocess.STDOUT, timeout=30)
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
        try:
            docker('run', '-d', '--name', name, '--network', 'none', '--memory', '512m', '--cpus', '1',
                   '--tmpfs', '/var/lib/postgresql:rw,size=268435456', '--env', 'POSTGRES_HOST_AUTH_METHOD=trust', image)
            deadline = time.monotonic() + 30
            while True:
                try:
                    # The image briefly starts a temporary bootstrap server.
                    # Require the final exec'd postmaster before sampling it.
                    if docker('exec', name, 'cat', '/proc/1/comm').strip() == 'postgres':
                        docker('exec', name, 'pg_isready', '-U', 'postgres'); break
                except subprocess.CalledProcessError:
                    if time.monotonic() >= deadline: raise
                if time.monotonic() >= deadline:
                    self.fail('final PostgreSQL postmaster did not start')
                time.sleep(.2)
            with ResourceSampler({'processes': {'composer': child.pid}, 'postgres_container': name}, interval_seconds=.1) as sampler:
                start = time.time_ns()
                docker('exec', name, 'psql', '-U', 'postgres', '-c',
                       'SELECT sum(i) FROM generate_series(1,2000000) AS i')
                end = time.time_ns()
                sampler.sample_now()
            report = sampler.report([{'name': 'query_worker', 'start_unix_ns': start, 'end_unix_ns': end}])
            self.assertEqual(report['status'], 'passed', report['errors'])
            self.assertGreater(report['phases'][0]['postgres']['cpu_seconds'], 0)
            self.assertEqual(report['overall']['postgres'], report['phases'][0]['postgres'])
            self.assertGreater(report['phases'][0]['postgres']['sampled_peak_memory_bytes'], 0)
            if destination := os.environ.get('EEZ_TEST_RESOURCE_REPORT'):
                Path(destination).write_text(json.dumps(report, indent=2) + '\n')
        finally:
            child.terminate(); child.wait(timeout=5)
            docker('rm', '--force', '--volumes', name)
        self.assertNotIn(name, docker('ps', '--all', '--format', '{{.Names}}').splitlines())


if __name__ == '__main__':
    unittest.main()
