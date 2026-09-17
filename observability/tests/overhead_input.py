"""Strict offline conversion of owned raw overhead evidence; never selects a plan.

Only literal loopback ports and exact owned run/spool paths are normalized.
Missing or rejected evidence is retained in the adapter report, with no runnable
analysis input. Raw capture, adaptation and statistical acceptance are distinct.
"""
import argparse
import copy
import hashlib
import ipaddress
import json
import math
import re
from pathlib import Path

MAX_BYTES = 32 * 1024**2
MAX_INPUT_BYTES = 128 * 1024**2
ROLES = ('composer', 'signer', 'follower')
NATIVE_COMMON_KEYS = {'workers', 'native_l1_profile', 'protocol', 'logging', 'observer_indexer', 'workload'}
BACKENDS = {'observer', 'postgresql', 'otlp_http_sink', 'metrics_scrape_transport'}
BINARIES = ('eez-composer', 'eez-proof-signer', 'eez-follower', 'eez-observer')
COMMON_KEYS = {'compiler', 'profile', 'cpu_allocation', 'workers', 'genesis',
               'native_l1_profile', 'protocol', 'logging', 'postgres_reservation',
               'resource_sampling', 'observer_indexer', 'workload'}
ARTIFACT_NAMES = ('config', 'ready', 'composer', 'signer', 'follower', 'effective_config',
                  'resources', 'evidence', 'result', 'native-effective', 'l1-genesis', 'l2-genesis', 'genesis_evidence')


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def artifact(value):
    """Synthetic/in-memory artifact; file loading preserves the original bytes."""
    return {'value': value, 'sha256': digest(value)}


def load(path, budget=None):
    if budget is not None:
        require(path.stat().st_size <= budget[0], 'combined artifacts exceed 128 MiB')
    with path.open('rb') as stream:
        raw = stream.read(MAX_BYTES + 1)
    if len(raw) > MAX_BYTES:
        raise ValueError('artifact exceeds 32 MiB')
    if budget is not None:
        budget[0] -= len(raw)
        require(budget[0] >= 0, 'combined artifacts exceed 128 MiB')
    value = json.loads(raw, parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite JSON')))
    return {'value': value, 'sha256': hashlib.sha256(raw).hexdigest()}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(value):
    return isinstance(value, str) and len(value) == 64 and all(c in '0123456789abcdef' for c in value)


def loopback(value, token):
    require(isinstance(value, str), 'explicit loopback endpoint required')
    host, port = value.rsplit(':', 1)
    address = ipaddress.ip_address(host.strip('[]'))
    require(address.is_loopback and port.isascii() and port.isdecimal() and 0 < int(port) < 65536,
            'endpoint must be literal loopback with a nonzero port')
    require((address.version == 6) == (host.startswith('[') and host.endswith(']')),
            'invalid IP endpoint brackets')
    # Host identity is retained; only the ephemeral port is replaced.
    return f'{host}:<{token}>'


def budgets(value):
    result = {}
    for key, maximum in (('arm_seconds', 1800), ('experiment_seconds', 21600)):
        amount = value[key]
        require(type(amount) is int and 1 <= amount <= maximum, f'{key} outside bounded integer range')
        result[key] = amount
    require(result['experiment_seconds'] >= result['arm_seconds'], 'experiment budget is shorter than one arm')
    return result


def elapsed_within(value, budget, scope):
    require(type(value) in (int, float) and math.isfinite(value) and 0 < value <= budget,
            f'{scope} elapsed is missing/invalid or exceeds frozen wall-time budget')


def normalize(config, ready, profiles, effective):
    """Return semantic preimages without discarding unknown profile fields."""
    require(set(config) == {'arm', 'probe', 'waves', 'receiver', 'directory', 'arm_seconds', 'experiment_seconds'}, 'unknown/missing arm configuration')
    require(config['arm'] in ('A', 'B') and config['probe'] is False, 'shipping arm requires disabled probe')
    require(type(config['waves']) is int and 1 <= config['waves'] <= 64, 'invalid measured wave count')
    budgets(config)
    directory = Path(config['directory'])
    require(directory.is_absolute() and '..' not in directory.parts, 'owned absolute run directory required')
    require(set(effective) == {'schema_version', 'common', 'telemetry_enabled', 'observer_running'}
            and type(effective['schema_version']) is int and effective['schema_version'] == 1, 'effective configuration contract missing or unknown')
    require(set(effective['common']) == COMMON_KEYS, 'effective common configuration incomplete or unknown')
    enabled = config['arm'] == 'B'
    require(ready.get('telemetry_enabled') is enabled and effective['telemetry_enabled'] is enabled
            and effective['observer_running'] is enabled, 'actual enabled profile differs from arm')
    processes = ready['processes']
    require(set(processes) == {*ROLES, 'observer'}, 'runtime process roles incomplete/unknown')
    require(all(type(processes[role]) is int and processes[role] > 0 for role in ROLES), 'native process identity missing')
    require((type(processes['observer']) is int and processes['observer'] > 0) if enabled
            else processes['observer'] is None, 'observer activity differs from arm')
    require(set(ready['metrics']) == ({*ROLES, 'observer'} if enabled else set()), 'actual metrics roles differ from arm')
    if enabled:
        loopback(ready['metrics']['observer'], 'OBSERVER_METRICS_PORT')
    normalized = copy.deepcopy(profiles)
    receiver = loopback(config['receiver'], 'OTLP_PORT')
    for role in ROLES:
        profile = normalized[role]
        require(profile['identity']['service'] == ('eez-proof-signer' if role == 'signer' else 'eez-' + role),
                'native profile has wrong role identity')
        endpoint = profile['metrics']['listen']
        if enabled:
            require(endpoint == ready['metrics'][role], 'enabled exporter endpoint differs from ready identity')
        profile['metrics']['listen'] = loopback(endpoint, role.upper() + '_METRICS_PORT')
        if role in ('composer', 'signer'):
            require(profile['traces']['receiver'] == config['receiver'], 'trace receiver differs from owned sink')
            profile['traces']['receiver'] = receiver
        if role == 'composer':
            require(profile['lifecycle']['directory'] == str(directory / 'lifecycle'), 'spool is not exact owned child')
            profile['lifecycle']['directory'] = '<RUN>/lifecycle'
    arm = {'telemetry_enabled': enabled, 'observer_running': enabled, 'probe': False,
           'receiver': receiver, 'directory': '<RUN>', 'profiles': normalized}
    return copy.deepcopy(effective['common']), arm


def validate_genesis(data, common):
    """Validate the sole declared JSON mutation; native code owns header hashing."""
    timestamps = []
    evidence = data['genesis_evidence']
    require(set(evidence) == {'l1', 'l2'} and set(common['genesis']) == {'l1', 'l2'}, 'explicit two-chain genesis evidence required')
    for role in ('l1', 'l2'):
        genesis = copy.deepcopy(data[role + '-genesis'])
        observed = evidence[role]
        value = genesis['timestamp']
        require(type(value) is int or isinstance(value, str) and re.fullmatch(r'0x[0-9a-fA-F]+', value), 'genesis timestamp representation invalid')
        timestamp = int(value, 16) if isinstance(value, str) else value
        require(0 < timestamp < 2**64 and observed['timestamp_seconds'] == timestamp, 'actual genesis timestamp differs from declaration')
        require(observed['header_hash_source'] == 'eez_testkit::genesis_block_hash'
                and isinstance(observed['header_hash'], str) and observed['header_hash'].startswith('0x')
                and sha(observed['header_hash'][2:]) and observed['header_hash'] == observed['rpc_genesis_hash'],
                'native genesis header/RPC identity not established')
        genesis['timestamp'] = '<RUN_UNIX_SECONDS>'
        require(common['genesis'][role] == {'template_sha256': digest(genesis),
                'timestamp_policy': 'testkit_current_unix_seconds_shared_l1_l2'}, 'genesis differs beyond the declared timestamp field')
        timestamps.append(timestamp)
    require(timestamps[0] == timestamps[1], 'fixture L1/L2 timestamp policy differs')


def preparation_samples(stage, observed, frozen):
    """Explicit clock conversion; old integer milliseconds never become nanos."""
    require(observed == frozen['preparation_clock'], 'preparation clock differs from predeclared source/unit')
    require(frozen['time_unit'] == 'ms', 'analysis unit must remain explicit milliseconds')
    if observed == {'source': 'admission.prepared.preparation_ms', 'unit': 'ms', 'resolution_ms': 1}:
        require(frozen['resolution_ms'] >= 1, 'legacy millisecond precision cannot be improved by conversion')
        return
    require(set(observed) == {'source', 'unit', 'resolution_ns'}
            and observed['source'] == 'admission.prepared.preparation_ns' and observed['unit'] == 'ns'
            and type(observed['resolution_ns']) is int and observed['resolution_ns'] > 0,
            'unsupported preparation clock')
    require(frozen['resolution_ms'] >= observed['resolution_ns'] / 1_000_000,
            'analysis resolution is finer than declared producer resolution')
    nanos, millis = stage['preparation_ns'], stage['preparation_ms']
    require(isinstance(nanos, list) and len(nanos) == len(millis)
            and all(type(n) is int and 0 <= n <= 2**53 for n in nanos),
            'nanosecond samples missing or not exactly representable in numeric analysis')
    require(all(type(m) is int and m == n // 1_000_000 for n,m in zip(nanos,millis)),
            'legacy milliseconds differ from the same-duration nanoseconds')
    stage['legacy_preparation_ms'] = list(millis)
    stage['preparation_ms'] = [n / 1_000_000 for n in nanos]
    stage['preparation_source_clock'] = copy.deepcopy(observed)


def contamination(resources, names):
    require(resources.get('status') == 'passed' and resources.get('errors') == [], 'resource evidence is not complete')
    overall = resources['overall']
    require(overall['status'] == 'passed', 'overall resource bracket is unknown')
    phases = resources['phases']
    require(isinstance(phases, list) and phases and all(p['status'] == 'passed' for p in phases), 'resource phase missing')
    host = overall['host']
    values = {'other_cpu_cores': max(p['host']['other_cpu_cores'] for p in [overall, *phases]),
              'cpu_steal_percent': max(p['host']['steal_fraction'] for p in [overall, *phases]) * 100,
              'cpu_iowait_percent': max(p['host']['iowait_fraction'] for p in [overall, *phases]) * 100,
              'major_faults': host['vm']['pgmajfault'],
              'swap_in_pages': host['vm']['pswpin'], 'swap_out_pages': host['vm']['pswpout']}
    require(set(names) <= set(values), 'unsupported frozen contamination measurement')
    require(all(type(v) in (int, float) and math.isfinite(v) and v >= 0 for v in values.values()), 'invalid host counters')
    return {name: values[name] for name in names}


def adapt(raw_artifact, plan_artifact, artifacts):
    """Return ready input or inconclusive evidence without a runnable input.

    artifacts is an ordered list of maps: artifact name -> {value, sha256}.
    These attestations bind captured bytes, not truth or precollection timing.
    """
    raw, plan = raw_artifact['value'], plan_artifact['value']
    require(isinstance(raw, dict) and isinstance(plan, dict), 'raw report and frozen plan must be objects')
    result = {'status': 'inconclusive', 'issues': [], 'analysis_input': None, 'runs': [],
              'provenance': {'raw_report_sha256': raw_artifact['sha256'], 'frozen_plan_sha256': plan_artifact['sha256'],
                             'candidate': raw.get('candidate'),
                             'artifacts_sha256': [{key: value['sha256'] for key, value in files.items()} for files in artifacts]}, 'normalized_configs': []}
    try:
        require(type(plan['schema_version']) is int and plan['schema_version'] == 1 and plan['phase'] in ('pilot', 'confirmatory'), 'explicit versioned frozen plan required')
        frozen = copy.deepcopy(plan['frozen'])
        require(frozen['sample_scope'] == 'shipping', 'plan must be shipping scope')
        backends = frozen['measurement_scope']['backend_included']
        require(isinstance(backends, list) and len(backends) == len(BACKENDS) and set(backends) == BACKENDS,
                'backend scope must name exactly the actual four collected components')
        frozen_budgets = budgets(frozen)
        require(budgets(raw) == frozen_budgets, 'raw experiment budgets differ from frozen plan')
        elapsed_within(raw.get('elapsed_seconds'), frozen_budgets['experiment_seconds'], 'experiment')
        require(type(frozen['measured_waves']) is int and 1 <= frozen['measured_waves'] <= 64
                and type(raw['waves']) is int, 'wave count must be an integer in 1..64')
        preimages = plan['config_preimages']
        expected = frozen['expected_identity']
        require(digest(preimages['common']) == expected['common_config_sha256'], 'frozen common preimage/hash disagree')
        require(set(preimages['arms']) == {'A', 'B'} and all(digest(preimages['arms'][a]) == expected['arm_config_sha256'][a]
                for a in ('A', 'B')), 'frozen arm preimage/hash disagree')
        candidate = raw['candidate']
        require(candidate['benchmark_lock_feature'] is False and raw.get('probe') is False
                and raw.get('sample_scope') == 'shipping_features', 'probe/unknown-feature candidate cannot enter shipping analysis')
        hashes = candidate['sha256']
        require(sha(candidate['source_map_sha256']) and hashes['source-map.json'] == candidate['source_map_sha256'],
                'dirty source content map identity missing')
        identity = {'shipping_binaries_sha256': {name: hashes['bin/' + name] for name in BINARIES},
                    'test_binary_sha256': hashes['dynamic_admission'], 'source_revision': candidate['source_map_sha256'],
                    'protocol_revision': candidate['protocol_revision']}
        require(all(identity[key] == expected[key] for key in identity), 'candidate differs from frozen identity')
        require(all(sha(value) for value in identity['shipping_binaries_sha256'].values())
                and sha(identity['test_binary_sha256']) and sha(hashes['rustc.txt']), 'binary/compiler provenance missing')
        pairs = 2 if plan['phase'] == 'pilot' else 6
        require(type(raw.get('schema_version')) is int and raw.get('schema_version') == 1 and raw.get('status') == 'raw_capture_complete', 'raw capture incomplete/failed')
        require(raw.get('pairs') == pairs and raw.get('waves') == frozen['measured_waves'], 'raw declared population differs from plan')
        require(isinstance(raw['runs'], list) and len(raw['runs']) <= 12 and len(artifacts) == len(raw['runs']), 'bounded artifact/run correspondence required')
        if len(raw['runs']) != pairs * 2:
            result['issues'].append('missing complete chronological run pairs')
    except (KeyError, TypeError, ValueError, OverflowError) as error:
        result['issues'].append(str(error))
        # Preserve every failed/partial raw run even when global identity fails.
        result['runs'] = copy.deepcopy(raw.get('runs', []))
        return result
    converted = []
    for index, (run, files) in enumerate(zip(raw['runs'], artifacts)):
        name = f"{run.get('pair')}-{run.get('arm')}"
        record = {'run_id': name, 'raw': copy.deepcopy(run), 'artifact_sha256': {key: value['sha256'] for key, value in files.items()}}
        result['runs'].append(record)
        try:
            require(run['pair'] == index // 2 + 1 and run['arm'] == ('AB' if index // 2 % 2 == 0 else 'BA')[index % 2],
                    'runs must retain declared chronological AB/BA order')
            elapsed_within(run.get('elapsed_seconds'), frozen_budgets['arm_seconds'], 'arm')
            require(run['status'] == 'captured' and run.get('probe') is False and run.get('owned_cleanup_verified') is True,
                    'run failed/unknown, probe enabled, or cleanup incomplete')
            require(set(files) == set(ARTIFACT_NAMES) and all(sha(v['sha256']) for v in files.values()), 'missing/unbounded artifact contract')
            data = {key: value['value'] for key, value in files.items()}
            require(data['result'] == run and data['evidence'] == run['evidence'], 'raw report differs from retained result/evidence')
            cfg, evidence, resources = data['config'], data['evidence'], data['resources']
            require(cfg['directory'] == run['directory'] and cfg['arm'] == run['arm'] and cfg['waves'] == frozen['measured_waves'],
                    'actual arm configuration differs from report/plan')
            require(budgets(cfg) == frozen_budgets, 'actual arm budgets differ from frozen plan')
            common, arm = normalize(cfg, data['ready'], {r: data[r] for r in ROLES}, data['effective_config'])
            native = data['native-effective']
            require(set(native) == NATIVE_COMMON_KEYS | {'schema_version'}
                    and type(native['schema_version']) is int and native['schema_version'] == 1
                    and all(native[key] == common[key] for key in NATIVE_COMMON_KEYS),
                    'effective common fields differ from actual native configuration')
            validate_genesis(data, common)
            require(all(data['genesis_evidence'][role]['json_sha256'] == files[role + '-genesis']['sha256']
                        for role in ('l1','l2')), 'actual genesis JSON identity differs')
            require(common['profile'] == candidate['profile'] and common['compiler']['rustc_sha256'] == hashes['rustc.txt'],
                    'effective compiler/profile differs from candidate')
            require(common['workload'] == {'measured_waves': frozen['measured_waves'], 'stages': frozen['stages'], **frozen_budgets},
                    'effective workload differs from frozen workload')
            pg = resources['postgres_identity']
            require(pg['name'] == data['ready']['postgres_container'], 'resource cgroup belongs to another run')
            require(resources['samples'], 'resource process identity samples missing')
            native_pids = {role: pid for role, pid in data['ready']['processes'].items() if pid is not None}
            for sample in (resources['samples'][0], resources['samples'][-1]):
                require(set(sample['processes']) == {*native_pids, 'sampler_collector'}
                        and all(sample['processes'][role]['pid'] == pid for role, pid in native_pids.items()),
                        'resource process identities differ from ready run')
            require(all(common['postgres_reservation'][key] == pg[key] for key in ('memory_limit_bytes', 'nano_cpus')),
                    'backend reservation differs from effective configuration')
            require(all(common['resource_sampling'][key] == resources[key] for key in ('interval_seconds', 'metrics_interval_seconds', 'max_samples')),
                    'resource sampling differs from effective configuration')
            require(type(resources['max_samples']) is int and resources['max_samples'] == cfg['arm_seconds'] + 32,
                    'resource sample capacity differs from actual arm budget')
            common_hash, arm_hash = digest(common), digest(arm)
            result['normalized_configs'].append({'run_id': name, 'common': common, 'arm': arm,
                                                 'common_sha256': common_hash, 'arm_sha256': arm_hash})
            require(common_hash == expected['common_config_sha256'] and arm_hash == expected['arm_config_sha256'][run['arm']],
                    'effective semantic configuration differs from frozen preimage')
            require(evidence.get('semantic_ok') is True and evidence.get('telemetry_healthy') is True, 'semantic/telemetry health not established')
            require(evidence.get('arm') == run['arm'] and evidence.get('probe') is False
                    and evidence.get('measured_waves') == frozen['measured_waves'], 'evidence arm/population mismatch')
            require(all(evidence.get(key) is False for key in ('sample_overflow', 'logger_gaps', 'budget_exhausted')),
                    'overflow/logger completeness/budget attestations missing or failed')
            enabled = run['arm'] == 'B'
            require(evidence.get('health') == ({r: True for r in (*ROLES, 'observer')} if enabled else {}), 'enabled collection health incomplete')
            require(run['sink']['after']['healthy'] is True and run['sink']['after']['errors'] == [], 'collector health unknown/failed')
            require(resources['status'] == run['resources_status'] == 'passed', 'resource collection failed/unknown')
            require([p['name'] for p in resources['phases']] == [p['name'] for p in evidence['resource_phases']]
                    and evidence['resource_phases'][-1]['name'] == 'observer_catchup_and_health'
                    and resources['overall']['name'] == 'overall', 'resource phases omit collection catchup/overall bracket')
            stages = copy.deepcopy(evidence['stages'])
            require(len(stages) == len(frozen['stages']), 'stage population incomplete')
            for stage, prescribed in zip(stages, frozen['stages']):
                require(stage['name'] == prescribed['name'] and stage['successful_sources'] == prescribed['successful_sources'], 'stage order/population differs')
                preparation_samples(stage, evidence['preparation_clock'], frozen)
                waves = stage['waves']
                require(len(waves) == frozen['measured_waves'], 'measured wave count differs')
                elapsed = sum(w['elapsed_seconds'] for w in waves)
                require(all(type(w['elapsed_seconds']) in (int, float) and math.isfinite(w['elapsed_seconds']) and w['elapsed_seconds'] > 0 for w in waves)
                        and math.isclose(stage['elapsed_seconds'], elapsed, rel_tol=1e-12, abs_tol=1e-9), 'throughput must use sum of measured wave intervals')
                require(sum(w['successful_sources'] for w in waves) == stage['successful_sources'], 'wave population differs')
            overall = resources['overall']
            run_identity = dict(identity, common_config_sha256=common_hash, arm_config_sha256=arm_hash)
            converted.append({'run_id': name, 'pair': run['pair'], 'arm': run['arm'], 'phase': plan['phase'],
                'sample_scope': 'shipping', 'identity': run_identity, 'measured_waves': evidence['measured_waves'],
                'semantic_ok': evidence['semantic_ok'], 'telemetry_healthy': evidence['telemetry_healthy'],
                **{key: evidence[key] for key in ('sample_overflow', 'logger_gaps', 'budget_exhausted')},
                'lock_probe': {'enabled': False}, 'contamination': contamination(resources, frozen['contamination_limits']),
                'stages': stages, 'resources': {'native': {key: value for key, value in overall['processes'].items() if key in ROLES},
                    'backend': {'postgres': overall['postgres'], 'processes': {key: value for key, value in overall['processes'].items() if key not in ROLES}}}})
        except (KeyError, TypeError, ValueError, OverflowError) as error:
            result['issues'].append(f'{name}: {error}')
    if not result['issues']:
        result['analysis_input'] = {'schema_version': 1, 'phase': plan['phase'], 'frozen': frozen, 'runs': converted}
        result['status'] = 'ready'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--raw', type=Path, required=True)
    parser.add_argument('--plan', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='adapter report; ready analysis input is inside analysis_input')
    args = parser.parse_args()
    budget = [MAX_INPUT_BYTES]
    raw, plan = load(args.raw, budget), load(args.plan, budget)
    require(isinstance(raw['value'].get('runs'), list) and len(raw['value']['runs']) <= 12, 'bounded raw runs required')
    input_paths = {args.raw.resolve(), args.plan.resolve()}
    root = args.raw.resolve().parent
    bundles = []
    for run in raw['value']['runs']:
        require(type(run.get('pair')) is int and 1 <= run['pair'] <= 6 and run.get('arm') in ('A', 'B'),
                'artifact path requires integer pair 1..6 and literal arm A/B')
        name = f"{run['pair']}-{run['arm']}"
        directory = (root / name).resolve()
        require(directory.parent == root and directory.name == name, 'artifact path escapes or aliases owned run directory')
        files = {}
        if not directory.is_dir():
            bundles.append(files)
            continue
        for name in ARTIFACT_NAMES:
            path = directory / (name + '.json')
            if path.exists():
                require(path.resolve().parent == directory, 'artifact symlink escapes run directory')
                input_paths.add(path.resolve())
                files[name] = load(path, budget)
        bundles.append(files)
    require(args.output.resolve() not in input_paths, 'output aliases retained input evidence')
    result = adapt(raw, plan, bundles)
    encoded = json.dumps(result, indent=2, allow_nan=False)
    require(len(encoded.encode()) <= MAX_BYTES, 'adapted evidence exceeds 32 MiB')
    # Exclusive creation also rejects existing symlinks/hardlinks and races.
    with args.output.open('x') as destination:
        destination.write(encoded + '\n')
    return 0 if result['status'] == 'ready' else 1


if __name__ == '__main__':
    raise SystemExit(main())
