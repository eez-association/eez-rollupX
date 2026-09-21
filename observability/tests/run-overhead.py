#!/usr/bin/env python3
"""Run serial, owned fixed-binary pilot arms; never builds or changes operator state.

Reports raw pilot evidence only. Confirmatory acceptance is a separate frozen
analysis decision, and this runner does not silently retry unsuccessful arms.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import selectors
import shutil
import tempfile
import uuid
import subprocess
import time

from overhead_resources import OtlpSink, ResourceSampler
from overhead_lock_analysis import analyze as analyze_locks


def digest(path):
    result = hashlib.sha256()
    with open(path, 'rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def write(path, value):
    temporary = path.with_suffix('.pending')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def load(path, limit=32 * 1024**2):
    if path.stat().st_size > limit:
        raise ValueError('JSON evidence exceeds bound')
    return json.loads(path.read_text())


def wait_file(path, child, deadline):
    while not path.exists():
        if child.poll() is not None:
            raise RuntimeError(f'native arm exited {child.returncode} before {path.name}')
        if time.monotonic() >= deadline:
            raise TimeoutError(f'arm deadline before {path.name}')
        time.sleep(.1)
    return load(path)


def lock_events(directory):
    events = []
    logs = list(directory.glob('eez-node-node-*.log'))
    if len(logs) != 1:
        raise ValueError('one complete Composer logfile required')
    if logs[0].stat().st_size > 256 * 1024**2:
        raise ValueError('Composer log exceeds capture limit')
    with logs[0].open() as source:
        for line in source:
            if len(line) > 2 * 1024**2:
                raise ValueError('oversize log record')
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue  # native startup includes non-JSON tool diagnostics
            fields = value.get('fields', {})
            event = fields.get('event_name')
            if isinstance(event, str) and event.startswith('eez.benchmark.held_pool_'):
                events.append(fields)
    return events


def effective_configuration(candidate, directory, ready, sampler):
    manifest = load(candidate / 'manifest.json')
    native = load(directory / 'native-effective.json')
    common = {key: native[key] for key in ('workers', 'native_l1_profile', 'protocol', 'logging', 'observer_indexer', 'workload')}
    common.update(compiler={'rustc_sha256': manifest['sha256']['rustc.txt']}, profile=manifest['profile'])
    cpus = sorted(os.sched_getaffinity(0))
    for pid in ready['processes'].values():
        if pid is not None and sorted(os.sched_getaffinity(pid)) != cpus:
            raise ValueError('runtime and sampler CPU allocations differ')
    cpuinfo = Path('/proc/cpuinfo').read_text()
    if len(cpuinfo) > 1024 * 1024:
        raise ValueError('CPU identity exceeds bound')
    model = next((line.split(':', 1)[1].strip() for line in cpuinfo.splitlines() if line.startswith('model name')), None)
    if model is None:
        raise ValueError('CPU model unavailable')
    common['cpu_allocation'] = {'allowed_cpus': cpus, 'model': model, 'policy': 'same inherited affinity in all owned native roles'}
    common['postgres_reservation'] = {key: sampler.postgres[key] for key in ('memory_limit_bytes', 'nano_cpus')}
    common['resource_sampling'] = {'interval_seconds': 1.0, 'metrics_interval_seconds': 15.0, 'max_samples': sampler.limit}
    common['genesis'] = {}
    genesis = load(directory / 'genesis_evidence.json')
    for role in ('l1', 'l2'):
        path = directory / f'{role}-genesis.json'
        template = load(path)
        template['timestamp'] = '<RUN_UNIX_SECONDS>'
        encoded = json.dumps(template, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()
        common['genesis'][role] = {'template_sha256': hashlib.sha256(encoded).hexdigest(),
                                   'timestamp_policy': 'testkit_current_unix_seconds_shared_l1_l2'}
        genesis[role]['json_sha256'] = digest(path)
    write(directory / 'genesis_evidence.json', genesis)
    write(directory / 'effective_config.json', {'schema_version': 1, 'common': common,
          'telemetry_enabled': ready['telemetry_enabled'], 'observer_running': ready['processes']['observer'] is not None})


def capture_attestation(directory, evidence):
    logs = list(directory.glob('eez-node-node-*.log'))
    if len(logs) != 1 or logs[0].stat().st_size > 256 * 1024**2:
        raise ValueError('bounded complete Composer capture unavailable')
    path = logs[0]
    before = path.stat()
    records = 0
    overflow = False
    with path.open('rb') as source:
        for line in source:
            if b'Unable to write an' in line or b'Unable to format the following event' in line:
                raise ValueError('native synchronous logger reported formatting/write loss')
            if len(line) > 2 * 1024**2 or not line.endswith(b'\n'):
                raise ValueError('oversize or incomplete log record')
            if not line.startswith(b'{'):
                if b'event_name' in line:
                    raise ValueError('malformed native event record')
                continue
            value = json.loads(line)
            records += 1
            if records > 1_000_000:
                raise ValueError('native capture record budget exceeded')
            if value.get('fields', {}).get('event_name') in ('eez.benchmark.held_pool_overflow', 'eez.benchmark.held_pool_clock_error'):
                overflow = True
    after = path.stat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise ValueError('closed native capture changed during verification')
    for stage in evidence['stages']:
        groups = stage['preparation_group_counts']
        if not groups or sum(groups) != stage['successful_sources'] or len(groups) != len(stage['preparation_ns']):
            raise ValueError('prepared-group logs do not cover the exact successful source population')
    if overflow:
        raise ValueError('bounded native probe reported overflow/clock failure')
    return {'sample_overflow': False, 'logger_gaps': False, 'budget_exhausted': False,
            'capture_proof': {'logger': 'pinned Reth synchronous stdout; no lossy appender',
             'file': path.name, 'sha256': digest(path), 'bytes': after.st_size,
             'complete_json_records': records, 'exact_prepared_source_population': True,
             'native_process_exited_successfully': True}}


def verify_candidate(candidate, manifest):
    for relative, expected in manifest['sha256'].items():
        path = (candidate / relative).resolve(strict=True)
        if not path.is_relative_to(candidate) or digest(path) != expected:
            raise ValueError(f'candidate mismatch: {relative}')


def native_manifest_directory(environment):
    """Respect an explicit manifest root; otherwise support repository-root invocation."""
    if 'CARGO_MANIFEST_DIR' in environment:
        value = environment['CARGO_MANIFEST_DIR']
        if not value or not Path(value).is_absolute():
            raise ValueError('explicit CARGO_MANIFEST_DIR must be an absolute native manifest directory')
        manifest = Path(value).resolve(strict=True)
    else:
        manifest = (Path.cwd() / 'crates/eez-node-e2e').resolve(strict=True)
    repo = manifest.parent.parent
    if (manifest.name != 'eez-node-e2e' or manifest.parent.name != 'crates'
            or not (manifest / 'Cargo.toml').is_file()
            or not (repo / 'Cargo.toml').is_file()):
        raise ValueError('invalid native manifest/repository root')
    return manifest


def reserve_observer(repo):
    """Reserve exact private roots before native launch; never infer ownership from PID prefixes."""
    token = uuid.uuid4().hex
    database_root = repo.resolve(strict=True) / 'target/native-observer-databases'
    database_root.mkdir(parents=True, exist_ok=True)
    if database_root.resolve(strict=True) != database_root:
        raise ValueError('database root contains symlink')
    fixture = {'database_parent': str(database_root / ('overhead-' + token)),
               'socket_parent': '/tmp/eez-opg-' + token,
               'container_name': 'eez-native-observer-' + token}
    identities = {}
    try:
        for key in ('database_parent', 'socket_parent'):
            path = Path(fixture[key])
            path.mkdir(mode=0o700)
            stat = path.lstat()
            identities[key] = (stat.st_dev, stat.st_ino, stat.st_uid)
    except Exception:
        for key in identities:
            Path(fixture[key]).rmdir()  # still empty; no native process has been launched
        raise
    return {'fixture': fixture, 'identities': identities, 'repo': repo}


def validate_owned_roots(owned):
    for key, expected in owned['identities'].items():
        path = Path(owned['fixture'][key])
        stat = path.lstat()
        if (stat.st_dev, stat.st_ino, stat.st_uid) != expected or not path.is_dir() or path.is_symlink() or path.resolve(strict=True) != path or stat.st_mode & 0o777 != 0o700:
            raise ValueError('owned reservation identity changed')


def process_identity(pid):
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return (int(fields[2]), int(fields[3]), int(fields[19]), fields[0])
    except FileNotFoundError:
        return None


def stop_owned_processes(child, identity, grace=10):
    """Verify the fresh session, including siblings after its leader exits."""
    if child is None:
        return True
    if identity is None or identity[:2] != (child.pid, child.pid):
        return False
    def verify_leader_generation():
        current = process_identity(child.pid)
        if current is not None and current[:3] != identity[:3]:
            raise ValueError('owned native leader PID was reused or its session changed')

    def members():
        verify_leader_generation()
        found = []
        for entry in Path('/proc').iterdir():
            if not entry.name.isdecimal():
                continue
            value = process_identity(int(entry.name))
            if value and value[1] == child.pid and value[2] >= identity[2] and value[3] != 'Z':
                try:
                    uid = entry.stat().st_uid
                except FileNotFoundError:
                    continue
                if uid != os.getuid():
                    raise ValueError('owned session contains unexpected UID')
                found.append((int(entry.name), value))
        return found
    for sig in (signal.SIGTERM, signal.SIGKILL):
        deadline = time.monotonic() + grace
        while True:
            active = members()
            if not active:
                child.wait(timeout=1)
                return True
            for pid, expected in active:
                try:
                    descriptor = os.pidfd_open(pid)
                    try:
                        verify_leader_generation()
                        if (current := process_identity(pid)) is not None and current[:3] == expected[:3]:
                            signal.pidfd_send_signal(descriptor, sig)
                    finally:
                        os.close(descriptor)
                except ProcessLookupError:
                    pass
            if time.monotonic() >= deadline:
                break
            time.sleep(.05)
    if members():
        return False
    child.wait(timeout=1)
    return True


def docker_inventory(name):
    inventory = subprocess.run(['docker', 'ps', '-a', '--no-trunc', '--filter', f'name=^/{name}$', '--format', '{{.ID}}'],
                               capture_output=True, text=True, timeout=10, check=True)
    ids = inventory.stdout.splitlines()
    if not ids:
        return None
    if len(ids) != 1 or len(ids[0]) != 64 or any(c not in '0123456789abcdef' for c in ids[0]):
        raise ValueError('ambiguous exact container identity')
    inspected = subprocess.run(['docker', 'inspect', ids[0]], capture_output=True, text=True, timeout=10, check=True)
    values = json.loads(inspected.stdout)
    if len(values) != 1 or values[0]['Id'] != ids[0]:
        raise ValueError('container inventory changed')
    return values[0]


def validate_owned_container(owned, inspected):
    validate_owned_roots(owned)
    fixture = owned['fixture']
    if inspected['Name'] != '/' + fixture['container_name'] or inspected['Config'].get('Labels', {}).get('eez.overhead.owner') != fixture['container_name']:
        raise ValueError('foreign container identity/label')
    mounts = inspected['Mounts']
    if len(mounts) != 2 or any(m['Type'] != 'bind' for m in mounts):
        raise ValueError('unexpected container mounts')
    paths = {m['Destination']: Path(m['Source']) for m in mounts}
    if set(paths) != {'/var/lib/postgresql', '/var/run/postgresql'}:
        raise ValueError('unexpected mount destinations')
    data, socket = paths['/var/lib/postgresql'], paths['/var/run/postgresql']
    if data.parent != Path(fixture['database_parent']) or socket.parent.parent != Path(fixture['socket_parent']) or socket.name != 'socket':
        raise ValueError('foreign container mount')
    for path in (data, socket):
        if path.resolve(strict=True) != path or not path.is_dir():
            raise ValueError('container mount contains symlink')
    receipt = Path(fixture['database_parent']) / 'container.json'
    if not receipt.is_file() or receipt.is_symlink() or load(receipt, 1024) != {'name': fixture['container_name'], 'id': inspected['Id']}:
        raise ValueError('completed container creation receipt unavailable')


def bounded_diagnostic(args, path, seconds, limit):
    # Never use Popen as a context manager: __exit__ waits without a deadline
    # when selector/read/write fails (for example, a full diagnostic disk).
    deadline = time.monotonic() + seconds
    with path.open('xb') as output, path.with_suffix(path.suffix + '.stderr').open('xb') as errors:
        child = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        counts = {output: 0, errors: 0}
        complete, reason, code, reaped = True, None, None, False
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ, output)
                selector.register(child.stderr, selectors.EVENT_READ, errors)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        complete, reason = False, 'timeout'
                        break
                    for key, _ in selector.select(min(remaining, .1)):
                        chunk = os.read(key.fileobj.fileno(), 65536)
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        target = key.data
                        available = limit - counts[target]
                        target.write(chunk[:available])
                        counts[target] += min(available, len(chunk))
                        if len(chunk) > available:
                            complete, reason = False, 'byte_limit'
                            break
                    if not complete:
                        break
                if complete:
                    code = child.wait(timeout=max(.1, deadline - time.monotonic()))
                    reaped = True
        except subprocess.TimeoutExpired:
            complete, reason = False, 'timeout'
        except Exception as error:
            complete, reason = False, f'{type(error).__name__}: {error}'
        finally:
            # Unconditional bounded stop/reap, including selector and disk errors.
            # A cleanup failure remains incomplete and never invokes an unbounded wait.
            try:
                if child.poll() is None:
                    child.kill()
                code = child.wait(timeout=2)
                reaped = True
            except Exception as error:
                complete, reason = False, f'{reason or "cleanup"}; child cleanup: {error}'
            finally:
                child.stdout.close()
                child.stderr.close()
        return {'exit_code': code, 'complete': complete and code == 0 and reaped,
                'reason': reason, 'bytes': counts[output], 'child_reaped': reaped}


def preserve_failure(owned, container_id):
    root = owned['repo'] / 'target/native-observer-failures'
    root.mkdir(parents=True, exist_ok=True)
    if root.resolve(strict=True) != root:
        raise ValueError('failure diagnostic root contains symlink')
    directory = Path(tempfile.mkdtemp(prefix='controller-', dir=root))
    commands = {
        'postgres.dump': (['docker', 'exec', container_id, 'pg_dump', '-U', 'postgres', '-d', 'eez_observer', '--format=custom'], 15, 128 * 1024**2),
        'postgres.log': (['docker', 'logs', '--tail', '20000', container_id], 5, 4 * 1024**2),
        'memory.txt': (['docker', 'exec', container_id, 'sh', '-c', 'cat /sys/fs/cgroup/memory.events /sys/fs/cgroup/memory.peak /sys/fs/cgroup/memory.stat'], 5, 1024**2),
    }
    captures = {}
    for name, (args, seconds, limit) in commands.items():
        try:
            captures[name] = bounded_diagnostic(args, directory / name, seconds, limit)
        except Exception as error:
            captures[name] = {'complete': False, 'error': str(error)}
    record = {'path': str(directory), 'status': 'preserved' if all(c['complete'] for c in captures.values()) else 'incomplete', 'captures': captures}
    write(directory / 'capture.json', record)
    return record


def cleanup_observer(owned, failed, processes_stopped):
    fixture = owned['fixture']
    result = {'owned_processes_stopped': processes_stopped, 'owned_container_absent': False,
              'owned_parents_removed': False, 'owned_cleanup_verified': False,
              'retained_owned_paths': [fixture[k] for k in ('database_parent', 'socket_parent')]}
    try:
        validate_owned_roots(owned)
        if not processes_stopped:
            raise ValueError('owned processes still active; data retained')
        inspected = docker_inventory(fixture['container_name'])
        preserved = True
        if inspected is not None:
            validate_owned_container(owned, inspected)
            if failed:
                try:
                    result['failure_diagnostics'] = preserve_failure(owned, inspected['Id'])
                    preserved = result['failure_diagnostics']['status'] == 'preserved'
                except Exception as error:
                    result['failure_diagnostics'] = {'status': 'incomplete', 'error': str(error)}
                    preserved = False
            removed = subprocess.run(['docker', 'rm', '--force', '--volumes', inspected['Id']], capture_output=True, text=True, timeout=15)
            if removed.returncode:
                raise RuntimeError('owned container removal failed')
            if docker_inventory(fixture['container_name']) is not None:
                raise RuntimeError('owned container remains or identity changed after removal')
            result['owned_container_absent'] = True
        else:
            result['owned_container_absent'] = True
            # A killed Docker CLI can leave a daemon-side create pending. Only
            # untouched empty reservations justify absence without its receipt.
            if any(any(Path(fixture[key]).iterdir()) for key in ('database_parent', 'socket_parent')):
                raise ValueError('nonempty reservation without live verified container; retain for reconciliation')
        if not preserved:
            raise ValueError('failure preservation incomplete; owned data retained')
        validate_owned_roots(owned)
        for key in ('database_parent', 'socket_parent'):
            shutil.rmtree(fixture[key])
        result.update(owned_parents_removed=True, owned_cleanup_verified=True, retained_owned_paths=[])
    except Exception as error:
        result['cleanup_error'] = str(error)
        result['retained_owned_paths'] = [fixture[k] for k in ('database_parent', 'socket_parent') if Path(fixture[k]).exists() or Path(fixture[k]).is_symlink()]
    return result


def arm(candidate, directory, pair, name, waves, probe, sink, arm_seconds, experiment_seconds, experiment_deadline):
    directory.mkdir()
    owned = None
    config = {'arm': name, 'probe': probe, 'waves': waves, 'arm_seconds': arm_seconds, 'experiment_seconds': experiment_seconds,
              'receiver': sink.address, 'directory': str(directory)}
    environment = os.environ.copy()
    # Operator telemetry must not leak into the explicitly disabled arm.
    environment.pop('EEZ_TELEMETRY_CONFIG', None)
    environment.update({'RUSTUP_TOOLCHAIN': '1.94.0', 'EEZ_TEST_PREBUILT': '1',
                        'EEZ_TEST_RUNTIME_BIN_DIR': str(candidate / 'bin'),
                        'EEZ_TEST_PROOF_SIGNER_BIN': str(candidate / 'bin/eez-proof-signer'),
                        'EEZ_TEST_LOG_DIR': str(directory),
                        'EEZ_OVERHEAD_ARM_CONFIG': str(directory / 'config.json'),
                        'EEZ_TEST_LOG': 'warn,eez_node=info,eez_l1=info,eez_composer=info,eez_deriver=info'})
    before = sink.snapshot()
    started = time.monotonic()
    deadline = min(started + arm_seconds, experiment_deadline)
    result = {'pair': pair, 'arm': name, 'probe': probe, 'status': 'failed', 'directory': str(directory)}
    ready = None
    sampler = None
    child = None
    identity = None
    known_processes = []
    with (directory / 'test.log').open('wb') as log:
        try:
            manifest_directory = native_manifest_directory(environment)
            environment['CARGO_MANIFEST_DIR'] = str(manifest_directory)
            repo = manifest_directory.parent.parent
            owned = reserve_observer(repo)
            config['observer_fixture'] = owned['fixture']
            write(directory / 'ownership.json', {'fixture': owned['fixture'], 'identities': owned['identities']})
            write(directory / 'config.json', config)
            child = subprocess.Popen([str(candidate / 'dynamic_admission'), '--ignored', '--exact',
                                      'overhead::observability_overhead_arm', '--nocapture'],
                                     env=environment, stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            identity = process_identity(child.pid)
            ready = wait_file(directory / 'ready.json', child, deadline)
            for pid in ready['processes'].values():
                if pid is None:
                    continue
                value = process_identity(pid)
                if value is None:
                    raise ValueError('ready native process already exited')
                known_processes.append((pid, value[:3]))
                if value[1] != child.pid or value[2] < identity[2]:
                    raise ValueError('ready native process is outside the owned session')
            with ResourceSampler(ready, interval_seconds=1.0, metrics_interval_seconds=15.0, max_samples=arm_seconds+32) as sampler:
                effective_configuration(candidate, directory, ready, sampler)
                (directory / 'start').touch()
                evidence = wait_file(directory / 'evidence.json', child, deadline)
                sampler.sample_now()
            resources = sampler.report(evidence.get('resource_phases', evidence['stages']))
            write(directory / 'resources.json', resources)
            (directory / 'finish').touch()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('declared budget exhausted before native teardown')
            code = child.wait(timeout=min(30, remaining))
            if code:
                raise RuntimeError(f'native arm exit {code}')
            if time.monotonic() > deadline:
                raise TimeoutError('declared arm budget exhausted before capture completed')
            evidence.update(capture_attestation(directory, evidence))
            write(directory / 'evidence.json', evidence)
            after = sink.snapshot()
            result.update(evidence=evidence, resources_status=resources['status'],
                          sink={'before': before, 'after': after}, elapsed_seconds=time.monotonic()-started)
            if probe:
                locks = analyze_locks(lock_events(directory), evidence['stages'])
                write(directory / 'locks.json', locks)
                result['lock_status'] = locks['status']
            result['status'] = 'captured' if (resources['status'] == 'passed' and after['healthy']
                and (not probe or result['lock_status'] == 'passed')) else 'inconclusive'
        except Exception as error:
            result['error'] = str(error)
            if sampler is not None:
                write(directory / 'partial-resources.json', sampler.snapshot())
            if (directory / 'progress.json').exists():
                result['partial_evidence'] = load(directory / 'progress.json')
            (directory / 'finish').touch()
        finally:
            try:
                stopped = stop_owned_processes(child, identity)
                for pid, expected in known_processes:
                    current = process_identity(pid)
                    if current is not None and current[:3] == expected and current[3] != 'Z':
                        stopped = False
                if owned is not None:
                    result.update(cleanup_observer(owned, result['status'] == 'failed', stopped))
                else:
                    result.update(owned_processes_stopped=stopped, owned_container_absent=True,
                                  owned_parents_removed=True, retained_owned_paths=[], owned_cleanup_verified=stopped)
                if not result['owned_cleanup_verified']:
                    result['status'] = 'failed'
            except Exception as error:
                # Shutdown/identity errors must not erase known ownership. No
                # resource removal is attempted after uncertain process teardown.
                result.update(owned_cleanup_verified=False, owned_processes_stopped=False,
                              owned_container_absent=False, owned_parents_removed=False,
                              retained_owned_paths=[owned['fixture'][key] for key in
                                  ('database_parent', 'socket_parent')] if owned is not None else [],
                              cleanup_error=str(error), status='failed')
            result['elapsed_seconds'] = time.monotonic() - started
            if time.monotonic() > deadline:
                result['status'] = 'budget_exhausted'
                result['budget_exhausted'] = True
                if 'evidence' in result:
                    result['evidence']['budget_exhausted'] = True
                    write(directory / 'evidence.json', result['evidence'])
            write(directory / 'result.json', result)
    return result


def finalize_budget(report, elapsed):
    """Final state includes sink teardown and every candidate rehash."""
    if type(elapsed) not in (int, float) or not math.isfinite(elapsed) or elapsed < 0:
        raise ValueError('invalid final experiment elapsed time')
    report['elapsed_seconds'] = elapsed
    report['budget_exhausted'] = elapsed > report['experiment_seconds'] or report['status'] == 'budget_exhausted'
    if report['budget_exhausted']:
        report['status'] = 'budget_exhausted'
    return report


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--waves', type=int, default=1)
    parser.add_argument('--arm-seconds', type=int, default=600)
    parser.add_argument('--experiment-seconds', type=int, default=9000)
    parser.add_argument('--probe', action='store_true')
    parser.add_argument('--pairs', type=int, choices=(1, 2, 6), default=2)
    args = parser.parse_args(argv)
    if not 1 <= args.waves <= 64:
        parser.error('waves must be 1..64')
    if not 1 <= args.arm_seconds <= 1800:
        parser.error('arm-seconds must be 1..1800')
    if not 1 <= args.experiment_seconds <= 21600 or args.experiment_seconds < args.arm_seconds:
        parser.error('experiment-seconds must be 1..21600 and at least arm-seconds')
    return args


def main():
    args = arguments()
    candidate = args.candidate.resolve(strict=True)
    manifest = load(candidate / 'manifest.json')
    verify_candidate(candidate, manifest)
    required = {'dynamic_admission', *(f'bin/{name}' for name in
                ('eez-composer', 'eez-follower', 'eez-proof-signer', 'eez-observer'))}
    if not required.issubset(manifest['sha256']):
        raise ValueError('incomplete frozen candidate manifest')
    if args.probe and not manifest['benchmark_lock_feature']:
        raise ValueError('probe requires feature candidate')
    output = args.output.resolve()
    output.mkdir(exist_ok=False)
    report = {'schema_version': 1, 'status': 'running', 'candidate': manifest,
              'waves': args.waves, 'pairs': args.pairs, 'probe': args.probe, 'runs': [],
              'arm_seconds': args.arm_seconds, 'experiment_seconds': args.experiment_seconds,
              'sample_scope': 'probe_variant' if manifest['benchmark_lock_feature'] else 'shipping_features',
              'scope': 'owned native roles, live observer, PostgreSQL, healthy OTLP sink; excludes Tempo storage'}
    started = time.monotonic()
    experiment_deadline = started + args.experiment_seconds
    with OtlpSink() as sink:
        for pair in range(1, args.pairs + 1):
            for name in ('AB' if pair % 2 else 'BA'):
                if time.monotonic() - started > args.experiment_seconds - args.arm_seconds:
                    report['status'] = 'budget_exhausted'
                    break
                result = arm(candidate, output / f'{pair}-{name}', pair, name, args.waves, args.probe, sink, args.arm_seconds, args.experiment_seconds, experiment_deadline)
                try:
                    verify_candidate(candidate, manifest)
                except Exception as error:
                    result['candidate_error'] = str(error)
                    result['status'] = 'failed'
                    write(output / f'{pair}-{name}' / 'result.json', result)
                report['runs'].append(result)
                write(output / 'report.json', report)
                if result['status'] != 'captured':
                    report['status'] = 'budget_exhausted' if result['status'] == 'budget_exhausted' else 'inconclusive'
                    break
            if report['status'] != 'running':
                break
    if report['status'] == 'running':
        report['status'] = 'raw_capture_complete'
    finalize_budget(report, time.monotonic() - started)
    write(output / 'report.json', report)
    print(json.dumps({'status': report['status'], 'report': str(output / 'report.json')}), flush=True)
    return 0 if report['status'] == 'raw_capture_complete' else 1


if __name__ == '__main__':
    raise SystemExit(main())
