"""Bounded Linux resource evidence and owned OTLP sink for the native pilot.

No process is stopped or reconfigured. Resource totals bracket a phase; sampled
RSS is not a claim of its exact peak. Missing observations invalidate evidence.
"""
import http.server
import http.client
import hashlib
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import socket
import subprocess
import threading
import time
import urllib.parse


def read(path):
    with open(path, 'r') as source:
        value = source.read(1_048_577)
    if len(value) > 1_048_576:
        raise ValueError('resource input exceeds one MiB')
    return value


def counters(text):
    result = {}
    for line in text.splitlines():
        key, value, *unit = line.split()
        if unit and unit != ['kB']:
            raise ValueError('unexpected resource counter unit')
        number = int(value)
        if number < 0 or key.rstrip(':') in result:
            raise ValueError('negative or duplicate resource counter')
        result[key.rstrip(':')] = number
    return result


def process_stat(text):
    # comm may contain spaces and parentheses; fields after its final ')' have
    # stable positions. Never parse a process command line or environment.
    fields = text.rsplit(')', 1)[1].split()
    return {'start_ticks': int(fields[19]),
            'cpu_ticks': int(fields[11]) + int(fields[12]),
            'minor_faults': int(fields[7]), 'major_faults': int(fields[9])}


def process(pid, proc=Path('/proc')):
    directory = proc / str(pid)
    identity = process_stat(read(directory / 'stat'))
    status = {}
    for line in read(directory / 'status').splitlines():
        if line.startswith(('VmRSS:', 'VmHWM:', 'Threads:')):
            status.update(counters(line))
    io = counters(read(directory / 'io'))
    after = process_stat(read(directory / 'stat'))
    if identity['start_ticks'] != after['start_ticks']:
        raise ValueError('PID changed during process sample')
    return {'pid': pid, **identity, 'rss_bytes': status['VmRSS'] * 1024,
            'lifetime_hwm_bytes': status['VmHWM'] * 1024,
            'threads': status['Threads'],
            'read_bytes': io['read_bytes'], 'write_bytes': io['write_bytes']}


def pressure(text):
    result = {}
    for line in text.splitlines():
        kind, *values = line.split()
        result[kind] = {key: int(value) if key == 'total' else float(value)
                        for key, value in (item.split('=') for item in values)}
    return result


def host(proc=Path('/proc')):
    stat_lines = read(proc / 'stat').splitlines()
    cpu = stat_lines[0].split()
    if cpu[0] != 'cpu' or len(cpu) < 9:
        raise ValueError('missing aggregate host CPU counters')
    values = list(map(int, cpu[1:9]))  # guest ticks already occur in user/nice
    memory = counters(read(proc / 'meminfo'))
    vm = counters(read(proc / 'vmstat'))
    disks = {}
    for line in read(proc / 'diskstats').splitlines():
        fields = line.split()
        if len(fields) >= 14:
            # Preserve device identities; summing partitions/virtual devices
            # would double-count the same underlying I/O.
            disks[fields[2]] = {'reads': int(fields[3]), 'read_sectors': int(fields[5]),
                               'writes': int(fields[7]), 'write_sectors': int(fields[9]),
                               'io_ms': int(fields[12])}
    return {'cpu_count': sum(bool(re.match(r'^cpu[0-9]+ ', line)) for line in stat_lines),
            'cpu_ticks': dict(zip(('user', 'nice', 'system', 'idle', 'iowait',
                                  'irq', 'softirq', 'steal'), values)),
            'memory_bytes': {key: memory[key] * 1024 for key in
                             ('MemTotal', 'MemAvailable', 'SwapTotal', 'SwapFree')},
            'vm': {key: vm[key] for key in ('pgfault', 'pgmajfault', 'pswpin', 'pswpout',
                                          'pgpgin', 'pgpgout')},
            'pressure': {key: pressure(read(proc / 'pressure' / key))
                         for key in ('cpu', 'memory', 'io')},
            'disk_devices': disks}


def postgres_identity(name, proc=Path('/proc'), cgroups=Path('/sys/fs/cgroup')):
    if not isinstance(name, str) or not re.fullmatch(r'eez-native-observer-[a-zA-Z0-9_-]+', name):
        raise ValueError('expected the explicitly owned native observer container')
    info = json.loads(subprocess.check_output(
        ['docker', 'inspect', '--format', '{{json .}}', name], text=True, timeout=10))
    if info['Name'] != '/' + name or not info['State']['Running'] or info['State']['OOMKilled']:
        raise ValueError('owned PostgreSQL container is not healthy/running')
    pid = info['State']['Pid']
    entry = [line[3:] for line in read(proc / str(pid) / 'cgroup').splitlines()
             if line.startswith('0::')]
    if len(entry) != 1:
        raise ValueError('whole-container resource measurement requires cgroup v2')
    root = cgroups.resolve()
    path = (root / entry[0].lstrip('/')).resolve()
    if path == root or not path.is_relative_to(root):
        raise ValueError('refusing host-wide or escaped PostgreSQL cgroup')
    return {'name': name, 'container_id': info['Id'], 'pid': pid,
            'start_ticks': process_stat(read(proc / str(pid) / 'stat'))['start_ticks'],
            'cgroup': str(path), 'cgroup_inode': path.stat().st_ino,
            'memory_limit_bytes': info['HostConfig']['Memory'],
            'nano_cpus': info['HostConfig']['NanoCpus']}


def postgres(identity, proc=Path('/proc')):
    path = Path(identity['cgroup'])
    if (path.stat().st_ino != identity['cgroup_inode'] or
            process_stat(read(proc / str(identity['pid']) / 'stat'))['start_ticks'] != identity['start_ticks']):
        raise ValueError('owned PostgreSQL process/cgroup identity changed')
    io = {}
    for line in read(path / 'io.stat').splitlines():
        device, *values = line.split()
        io[device] = {key: int(value) for key, value in (item.split('=') for item in values)}
    return {'cpu': counters(read(path / 'cpu.stat')),
            'memory_bytes': int(read(path / 'memory.current')),
            'lifetime_peak_bytes': int(read(path / 'memory.peak')),
            'memory_events': counters(read(path / 'memory.events')),
            'tasks': int(read(path / 'pids.current')), 'io_devices': io}


def in_postgres_cgroup(pid, identity):
    root = Path('/sys/fs/cgroup').resolve()
    for line in read(Path('/proc') / str(pid) / 'cgroup').splitlines():
        if line.startswith('0::'):
            path = (root / line[3:].lstrip('/')).resolve()
            return path.is_relative_to(Path(identity['cgroup']))
    raise ValueError('native resource measurement requires cgroup v2 identity')


def difference(before, after, keys):
    values = {key: after[key] - before[key] for key in keys}
    if any(value < 0 for value in values.values()):
        raise ValueError('resource counter reset inside measured phase')
    return values


def device_differences(before, after):
    # Never assume a newly attached or removed device's missing counters are zero.
    return {'deltas': {name: difference(before[name], after[name], tuple(before[name]))
                       for name in sorted(before.keys() & after.keys())},
            'added': sorted(after.keys() - before.keys()), 'removed': sorted(before.keys() - after.keys())}


def scrape_metrics(address):
    url = urllib.parse.urlsplit('http://' + address)
    if (url.path or url.query or url.fragment or url.username is not None
            or not url.port or not ipaddress.ip_address(url.hostname).is_loopback):
        raise ValueError('resource metrics require an explicit loopback host:port')
    # Ignore operator proxy configuration: these are exclusively owned loopback
    # listeners. Preserve exact selected samples and a digest of the full reply.
    connection = http.client.HTTPConnection(url.hostname, url.port, timeout=1)
    connection.connect()  # bounded connect; no DNS or proxy for literal loopback
    peer = connection.sock
    expired = threading.Event()
    def expire():
        expired.set()
        try:
            peer.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
    timer = threading.Timer(1, expire)
    timer.daemon = True
    timer.start()
    try:
        connection.request('GET', '/metrics')
        with connection.getresponse() as response:
            if response.status != 200:
                raise ValueError('unexpected metrics HTTP status; redirects are not followed')
            body = response.read(2 * 1024**2 + 1)
        if expired.is_set():
            raise TimeoutError('metrics absolute response deadline exceeded')
    finally:
        timer.cancel()
        timer.join()
        connection.close()
    if len(body) > 2 * 1024**2:
        raise ValueError('metrics response exceeds two MiB')
    selected = {}
    for line in body.decode().splitlines():
        if not line or line.startswith('#'):
            continue
        key, _, raw = line.rpartition(' ')
        name = key.partition('{')[0].removeprefix('reth_')
        if name in ('eez_telemetry_dropped_events_total', 'eez_telemetry_writer_errors_total',
                    'eez_trace_exported_spans_total', 'eez_trace_export_enabled',
                    'eez_lifecycle_writer_enabled', 'eez_observer_collection_success',
                    'eez_observer_lifecycle_collection_success'):
            if len(key) > 512 or len(selected) >= 128 or key in selected:
                raise ValueError('unexpected metrics identity/cardinality')
            number = float(raw)
            if not math.isfinite(number) or number < 0:
                raise ValueError('invalid health/drop metric sample')
            selected[key] = raw
    return {'status': 'passed', 'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest(),
            'samples': selected}


def phase_report(samples, phase, ticks):
    start, end = phase['start_unix_ns'], phase['end_unix_ns']
    if type(start) is not int or type(end) is not int or end <= start:
        raise ValueError('invalid phase boundaries')
    left = [item for item in samples if item['finished_unix_ns'] <= start]
    right = [item for item in samples if item['started_unix_ns'] >= end]
    if not left or not right:
        raise ValueError('phase lacks complete before/after resource samples')
    before, after = left[-1], right[0]
    window = [item for item in samples if before['sequence'] <= item['sequence'] <= after['sequence']]
    origin_offset = before['started_unix_ns'] - before['monotonic_ns']
    previous_finish = None
    for item in window:
        if (item['finished_unix_ns'] < item['started_unix_ns']
                or item['finished_monotonic_ns'] < item['monotonic_ns']
                or (previous_finish is not None and item['monotonic_ns'] < previous_finish)
                or abs(item['started_unix_ns'] - item['monotonic_ns'] - origin_offset) > 50_000_000
                or abs(item['finished_unix_ns'] - item['finished_monotonic_ns'] - origin_offset) > 50_000_000):
            raise ValueError('observed clock discontinuity or invalid ordering inside resource bracket')
        previous_finish = item['finished_monotonic_ns']
    elapsed = (after['monotonic_ns'] - before['monotonic_ns']) / 1e9
    wall_elapsed = (after['started_unix_ns'] - before['started_unix_ns']) / 1e9
    if elapsed <= 0 or abs(wall_elapsed - elapsed) > .05:
        raise ValueError('host wall/monotonic clock moved during resource bracket')
    processes = {}
    for role, first in before['processes'].items():
        last = after['processes'][role]
        if (first['pid'], first['start_ticks']) != (last['pid'], last['start_ticks']):
            raise ValueError('PID reuse/restart inside phase')
        delta = difference(first, last, ('cpu_ticks', 'minor_faults', 'major_faults', 'read_bytes', 'write_bytes'))
        processes[role] = {**delta, 'cpu_seconds': delta['cpu_ticks'] / ticks,
                           'pid': first['pid'], 'start_ticks': first['start_ticks'],
                           'sampled_peak_rss_bytes': max(item['processes'][role]['rss_bytes'] for item in window),
                           'observed_lifetime_hwm_bytes': max(item['processes'][role]['lifetime_hwm_bytes'] for item in window)}
    pg_first, pg_last = before['postgres'], after['postgres']
    pg_cpu = difference(pg_first['cpu'], pg_last['cpu'], tuple(pg_first['cpu']))
    memory_events = difference(pg_first['memory_events'], pg_last['memory_events'], tuple(pg_first['memory_events']))
    if memory_events.get('oom', 0) or memory_events.get('oom_kill', 0):
        raise ValueError('PostgreSQL cgroup experienced OOM during phase')
    cpu = difference(before['host']['cpu_ticks'], after['host']['cpu_ticks'], tuple(before['host']['cpu_ticks']))
    total_ticks = sum(cpu.values())
    if not total_ticks:
        raise ValueError('host CPU clock did not advance')
    busy_seconds = (total_ticks - cpu['idle'] - cpu['iowait'] - cpu['steal']) / ticks
    tracked_seconds = sum(item['cpu_seconds'] for item in processes.values()) + pg_cpu['usage_usec'] / 1e6
    cpu_count = before['host']['cpu_count']
    if cpu_count <= 0 or any(item['host']['cpu_count'] != cpu_count for item in window):
        raise ValueError('host CPU population changed during resource bracket')
    # Each tick counter has finite resolution; process/cgroup/host reads are
    # sequential within each timestamped sample. Keep that uncertainty explicit.
    read_skew_seconds = sum((item['finished_monotonic_ns'] - item['monotonic_ns']) / 1e9
                            for item in (before, after))
    accounting_slack = 2 * (cpu_count + len(processes) + 1) / ticks + read_skew_seconds * cpu_count
    residual = busy_seconds - tracked_seconds
    if residual < -accounting_slack:
        raise ValueError('tracked CPU materially exceeds whole-host busy CPU')
    return {'name': phase['name'], 'status': 'passed', 'scope': 'conservative resource bracket, not exact phase boundaries',
            'bracket_seconds': elapsed, 'before_sequence': before['sequence'], 'after_sequence': after['sequence'],
            'leading_slack_ns': start - before['started_unix_ns'],
            'trailing_slack_ns': after['finished_unix_ns'] - end,
            'processes': processes,
            'postgres': {'cpu': pg_cpu, 'cpu_seconds': pg_cpu['usage_usec'] / 1e6,
                         'sampled_peak_memory_bytes': max(item['postgres']['memory_bytes'] for item in window),
                         'observed_lifetime_peak_bytes': max(item['postgres']['lifetime_peak_bytes'] for item in window),
                         'memory_events': memory_events,
                         'io_devices': device_differences(pg_first['io_devices'], pg_last['io_devices'])},
            'host': {'cpu_ticks': cpu, 'busy_fraction': busy_seconds * ticks / total_ticks,
                     'steal_fraction': cpu['steal'] / total_ticks, 'iowait_fraction': cpu['iowait'] / total_ticks,
                     'signed_other_cpu_seconds': residual, 'cpu_accounting_slack_seconds': accounting_slack,
                     'other_cpu_cores': max(0., residual + accounting_slack) / elapsed,
                     'other_cpu_cores_scope': 'conservative upper bound including tick/read uncertainty',
                     'minimum_available_bytes': min(item['host']['memory_bytes']['MemAvailable'] for item in window),
                     'disk_devices': device_differences(before['host']['disk_devices'], after['host']['disk_devices']),
                     'vm': difference(before['host']['vm'], after['host']['vm'], tuple(before['host']['vm']))}}


class ResourceSampler:
    def __init__(self, ready, interval_seconds=1., max_samples=1200, metrics_interval_seconds=15.):
        if not .1 <= interval_seconds <= 10 or type(max_samples) is not int or not 3 <= max_samples <= 10000:
            raise ValueError('invalid bounded resource sampling configuration')
        self.pids = {key: value for key, value in ready['processes'].items() if value is not None}
        if (not self.pids or set(self.pids) - {'composer', 'signer', 'follower', 'observer'}
                or any(type(pid) is not int or pid <= 0 for pid in self.pids.values())
                or len(set(self.pids.values())) != len(self.pids)):
            raise ValueError('invalid or overlapping native process identities')
        if os.getpid() in self.pids.values():
            raise ValueError('native PID cannot also be the sampler/collector')
        self.pids['sampler_collector'] = os.getpid()
        self.metrics = ready.get('metrics', {})
        if (not isinstance(self.metrics, dict) or set(self.metrics) - set(self.pids)
                or not 1 <= metrics_interval_seconds <= 60):
            raise ValueError('invalid metrics sampling roles/cadence')
        self.metrics_interval, self.next_metrics = metrics_interval_seconds, 0.
        self.postgres = postgres_identity(ready['postgres_container'])
        if any(in_postgres_cgroup(pid, self.postgres) for pid in self.pids.values()):
            raise ValueError('a native process duplicates PostgreSQL cgroup accounting')
        self.interval, self.limit = interval_seconds, max_samples
        self.ticks = os.sysconf('SC_CLK_TCK')
        self.samples, self.errors, self.identities = [], [], {}
        self.encoded_bytes = 0
        self.shutdown_error = None
        self.lock, self.stop, self.thread = threading.Lock(), threading.Event(), None

    def sample_now(self):
        with self.lock:
            if self.errors:
                raise RuntimeError(self.errors[0])
            try:
                if len(self.samples) >= self.limit:
                    raise ValueError('resource sample budget exceeded')
                sample = {'sequence': len(self.samples) + 1, 'started_unix_ns': time.time_ns(),
                          'monotonic_ns': time.monotonic_ns()}
                sample['processes'] = {role: process(pid) for role, pid in self.pids.items()}
                for role, value in sample['processes'].items():
                    identity = (value['pid'], value['start_ticks'])
                    if self.identities.setdefault(role, identity) != identity:
                        raise ValueError('process exited/restarted during resource sampling')
                sample['postgres'], sample['host'] = postgres(self.postgres), host()
                if time.monotonic() >= self.next_metrics:
                    sample['metrics'] = {role: scrape_metrics(address) for role, address in self.metrics.items()}
                    self.next_metrics = time.monotonic() + self.metrics_interval
                sample['finished_monotonic_ns'] = time.monotonic_ns()
                sample['finished_unix_ns'] = time.time_ns()
                self.encoded_bytes += len(json.dumps(sample).encode())
                if self.encoded_bytes > 32 * 1024**2:
                    raise ValueError('resource evidence exceeds 32 MiB')
                self.samples.append(sample)
                return sample
            except Exception as error:
                self.errors.append(str(error))
                self.stop.set()
                raise

    def __enter__(self):
        self.sample_now()
        def collect():
            while not self.stop.wait(self.interval):
                try:
                    self.sample_now()
                except Exception:
                    return  # error is retained; never replace it with zeros
        self.thread = threading.Thread(target=collect, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stop.set()
        if self.thread:
            # At most four sequential loopback scrapes, each with one-second
            # connect and response bounds. Preserve an explicit shutdown error
            # if even that allowance is exceeded; never block on its lock later.
            self.thread.join(timeout=10)
            if self.thread.is_alive():
                self.shutdown_error = 'resource sampler did not stop within ten seconds'

    def snapshot(self):
        if not self.lock.acquire(timeout=1):
            return {'samples': [], 'errors': [self.shutdown_error or 'resource snapshot lock timed out']}
        try:
            errors = list(self.errors)
            if self.shutdown_error:
                errors.append(self.shutdown_error)
            return {'samples': list(self.samples), 'errors': errors}
        finally:
            self.lock.release()

    def report(self, phases):
        snapshot = self.snapshot()
        if self.thread and self.thread.is_alive():
            snapshot['errors'].append('stop the sampler before producing its final report')
        result = {'status': 'inconclusive', **snapshot, 'phases': [],
                  'clock_ticks_per_second': self.ticks, 'interval_seconds': self.interval,
                  'max_samples': self.limit,
                  'metrics_interval_seconds': self.metrics_interval,
                  'postgres_identity': self.postgres,
                  'scope': 'native processes, collector/sampler, whole owned PostgreSQL cgroup, host context'}
        if snapshot['errors']:
            return result
        if not isinstance(phases, list) or not 1 <= len(phases) <= 256:
            result['errors'].append('invalid bounded resource phase list')
            return result
        names, previous_end = set(), None
        for phase in phases:
            try:
                name = phase['name']
                if not isinstance(name, str) or not name or len(name) > 64 or name in names:
                    raise ValueError('resource phase names must be bounded and unique')
                if previous_end is not None and phase['start_unix_ns'] < previous_end:
                    raise ValueError('resource phases overlap or are out of order')
                names.add(name)
                previous_end = phase['end_unix_ns']
                result['phases'].append(phase_report(snapshot['samples'], phase, self.ticks))
            except (KeyError, TypeError, ValueError) as error:
                result['errors'].append(str(error))
        if not result['errors']:
            try:
                result['overall'] = phase_report(snapshot['samples'], {
                    'name': 'overall', 'start_unix_ns': phases[0]['start_unix_ns'],
                    'end_unix_ns': phases[-1]['end_unix_ns']}, self.ticks)
            except (KeyError, TypeError, ValueError) as error:
                result['errors'].append(str(error))
        result['status'] = 'passed' if not result['errors'] else 'inconclusive'
        return result


class OtlpSink:
    """Healthy bounded HTTP/protobuf transport sink; no Tempo-storage claim."""
    def __init__(self, max_bytes=64 * 1024**2, max_requests=50000):
        if (type(max_bytes) is not int or not 1 <= max_bytes <= 256 * 1024**2
                or type(max_requests) is not int or not 1 <= max_requests <= 100000):
            raise ValueError('invalid OTLP fixture budget')
        self.max_bytes, self.max_requests = max_bytes, max_requests
        self.lock = threading.Lock()
        self.state = {'requests': 0, 'bytes': 0, 'errors': []}
        self.server, self.thread = None, None
        self.connection = None

    def error(self, message):
        with self.lock:
            if len(self.state['errors']) < 16:
                self.state['errors'].append(message)

    @staticmethod
    def close_connection(connection):
        if connection is not None:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    def snapshot(self):
        with self.lock:
            return {**self.state, 'errors': list(self.state['errors']), 'healthy': not self.state['errors']}

    def __enter__(self):
        owner = self
        class Server(http.server.HTTPServer):
            def get_request(self):
                connection, address = super().get_request()
                with owner.lock:
                    owner.connection = connection
                return connection, address

            def handle_error(self, *_):
                owner.error('OTLP connection handling failed')

        class Handler(http.server.BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(2)
                def deadline():
                    owner.error('OTLP absolute connection deadline exceeded')
                    owner.close_connection(self.connection)
                self.deadline = threading.Timer(2, deadline)
                self.deadline.daemon = True
                self.deadline.start()

            def finish(self):
                self.deadline.cancel()
                self.deadline.join()
                try:
                    super().finish()
                except OSError:
                    owner.error('OTLP response completion failed')
                finally:
                    with owner.lock:
                        if owner.connection is self.connection:
                            owner.connection = None

            def do_POST(self):
                status = 200
                try:
                    length = int(self.headers.get('Content-Length', '-1'))
                    if self.path != '/v1/traces' or not 0 < length <= 2 * 1024**2:
                        raise ValueError('unexpected OTLP path or request length')
                    # Reserve bytes before reading; concurrent callers cannot
                    # each consume the same remaining budget.
                    with owner.lock:
                        if (owner.state['requests'] >= owner.max_requests or
                                owner.state['bytes'] + length > owner.max_bytes):
                            raise ValueError('OTLP sink budget exceeded')
                        owner.state['requests'] += 1
                        owner.state['bytes'] += length
                    if len(self.rfile.read(length)) != length:
                        raise ValueError('truncated OTLP request')
                except (OSError, ValueError) as error:
                    owner.error(str(error))
                    status = 503
                try:
                    self.send_response(status)
                    self.send_header('Content-Type', 'application/x-protobuf')
                    self.send_header('Content-Length', '0')
                    self.end_headers()
                except OSError:
                    owner.error('OTLP response acknowledgment failed')

            def log_message(self, *_):
                pass

        # Serial bounded handling is intentional: identical fixture in A/B,
        # no unbounded thread creation and no retained payloads.
        self.server = Server(('127.0.0.1', 0), Handler)
        self.address = f'127.0.0.1:{self.server.server_port}'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *_):
        with self.lock:
            connection = self.connection
        self.close_connection(connection)
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
