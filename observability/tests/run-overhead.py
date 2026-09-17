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


def arm(candidate, directory, pair, name, waves, probe, sink, arm_seconds, experiment_seconds, experiment_deadline):
    directory.mkdir()
    config = {'arm': name, 'probe': probe, 'waves': waves, 'arm_seconds': arm_seconds, 'experiment_seconds': experiment_seconds,
              'receiver': sink.address, 'directory': str(directory)}
    write(directory / 'config.json', config)
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
    with (directory / 'test.log').open('wb') as log:
        child = subprocess.Popen([str(candidate / 'dynamic_admission'), '--ignored', '--exact',
                                  'overhead::observability_overhead_arm', '--nocapture'],
                                 env=environment, stdout=log, stderr=subprocess.STDOUT,
                                 start_new_session=True)
        try:
            ready = wait_file(directory / 'ready.json', child, deadline)
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
            if child.poll() is None:
                # Only this fresh process group belongs to this runner.
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=10)
        finally:
            try:
                if result['status'] == 'failed':
                    # Only names tied to this exact owned native test PID.
                    prefix = f'eez-native-observer-{child.pid}-'
                    inventory = subprocess.run(['docker', 'ps', '-a', '--format', '{{.Names}}'], capture_output=True, text=True, timeout=10, check=True)
                    owned = [name for name in inventory.stdout.splitlines() if name.startswith(prefix)]
                    if owned:
                        subprocess.run(['docker', 'rm', '-f', *owned], capture_output=True, timeout=30, check=True)
                inventory = subprocess.run(['docker', 'ps', '-a', '--format', '{{.Names}}'], capture_output=True, text=True, timeout=10, check=True)
                if any(name.startswith(f'eez-native-observer-{child.pid}-') for name in inventory.stdout.splitlines()):
                    raise RuntimeError('owned PostgreSQL still exists after cleanup')
                result['owned_cleanup_verified'] = True
            except Exception as error:
                result['cleanup_error'] = str(error)
                result['status'] = 'failed'
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
