"""Exercise the actual CI shell with synchronized, offline process fixtures."""
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest


REPO = Path(__file__).resolve().parents[3]
WORKER = '''#!/usr/bin/env python3
import os, signal, subprocess, sys, time
from pathlib import Path
root = Path(os.environ['FIXTURE'])
role = Path(sys.argv[0]).name
(root / (role + '-started')).touch()
if role == 'network' and not all((root / marker).exists() for marker in ('build-done', 'integration-done')):
    print('network started before fixture compilation finished', file=sys.stderr)
    raise SystemExit(90)
if os.environ.get('FAIL_ROLE') == role or (
    role == 'forge' and os.environ.get('FAIL_BUILD') == Path(sys.argv[3]).name
):
    raise SystemExit(19 if role == 'forge' else 17)
if os.environ.get('HANG_ROLE') == role:
    child = subprocess.Popen([sys.executable, '-c',
        'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)'])
    (root / 'child-pid').write_text(str(child.pid))
    time.sleep(60)
if role == 'forge':
    target = Path(sys.argv[3])
    assert target in (root / 'eez-core-protocol', root / 'contracts')
    assert sys.argv[1:] == ['build', '--root', str(target), '--threads', '1']
    # Leave enough time for the former parallel startup to reach its assertion.
    time.sleep(.2)
    (root / ('build-done' if target.name == 'eez-core-protocol' else 'integration-done')).touch()
'''


class KurtosisPreparationTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='eez-kurtosis-preparation.')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.package = self.root / 'testing/kurtosis'
        scripts = self.package / 'scripts'
        scripts.mkdir(parents=True)
        tools = self.root / 'bin'
        tools.mkdir()
        for name in ('run-ci.sh', 'ci-args.yaml'):
            shutil.copy2(REPO / 'testing/kurtosis' / name, self.package / name)
        (self.root / 'scripts').mkdir()
        (self.root / 'scripts/update-eezl2-genesis.sh').write_text('exit 0\n')
        (self.package / 'start.sh').write_text('exec network\n')
        (self.package / 'stop.sh').write_text('touch "$FIXTURE/stopped"\n')
        (self.package / 'ports.sh').write_text('EEZ_DEVNET_L1_RPC=l1\nEEZ_DEVNET_L2_RPC=l2\n')
        (scripts / 'verify-eezl2-deployment.sh').write_text('exit 0\n')
        (scripts / 'verify-cross-chain-waves.sh').write_text('touch "$FIXTURE/waves-done"\n')
        (scripts / 'run-protocol-e2e.sh').write_text('''set -e
test -f "$FIXTURE/build-done"
test -f "$FIXTURE/integration-done"
test -f "$FIXTURE/waves-done"
flock -n "$FIXTURE/eez-core-protocol/tmp/.eez-network-e2e.lock" true
touch "$FIXTURE/scenarios-started"
mkdir -p "$EEZ_CI_RESULT_DIR/protocol-e2e"
printf '%s\\n' "$SCENARIO_SUMMARY" > "$EEZ_CI_RESULT_DIR/protocol-e2e/summary.json"
''')
        self.tool('git', '''#!/bin/bash
case "$*" in
    *ls-files*) printf '160000 '; printf '%040d\\n' "${PIN:-1}" ;;
    *) printf '%040d\\n' 1 ;;
esac
''')
        self.tool('forge', WORKER)
        self.tool('network', WORKER)
        self.tool('cast', '#!/bin/sh\necho 0\n')
        self.tool('kurtosis', '''#!/bin/sh
echo 'validator registrations call processed numRegistrations=128'
echo 'Updated validator registrations len=128'
echo '"event_name":"eez.deriver.safe.advanced"'
echo eez.proof_signer.window_signed
echo eez.prover_client.attested
''')
        self.env = dict(os.environ, FIXTURE=str(self.root),
                        PATH=str(tools) + os.pathsep + os.environ['PATH'],
                        KURTOSIS_ENCLAVE='fixture-only',
                        SCENARIO_SUMMARY=json.dumps({
                            'suite': {'complete': True, 'excluded_from_all': []},
                            'skipped': []}),
                        KURTOSIS_ARGS_FILE=str(self.package / 'ci-args.yaml'),
                        EEZ_CI_RESULT_DIR=str(self.root / 'results'))

    def tool(self, name, text):
        path = self.root / 'bin' / name
        path.write_text(text)
        path.chmod(0o755)

    def command(self):
        return ['bash', str(self.package / 'run-ci.sh')]

    def run_fixture(self, **settings):
        return subprocess.run(self.command(), env=dict(self.env, **settings),
                              capture_output=True, text=True, timeout=12)

    def assert_stopped(self):
        pid_file = self.root / 'child-pid'
        if not pid_file.exists():
            return
        pid = int(pid_file.read_text())
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                if Path(f'/proc/{pid}/stat').read_text().split(') ', 1)[1].split()[0] == 'Z':
                    return
            except (FileNotFoundError, ProcessLookupError):
                return
            time.sleep(.01)
        self.fail(f'owned descendant {pid} survived cancellation')

    def test_compilation_finishes_before_network_and_all_checks_precede_scenarios(self):
        result = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((self.root / 'scenarios-started').exists())
        rows = [json.loads(row) for row in (self.root / 'results/timings.jsonl').read_text().splitlines()]
        self.assertEqual({row['phase'] for row in rows}, {
            'network-start', 'settlement-readiness', 'waves-and-state-chaining',
            'network-checks', 'protocol-build', 'integration-build', 'protocol-suite'})
        self.assertTrue(all(row['exit_code'] == 0 for row in rows))

    def test_build_failure_never_starts_network_or_scenarios(self):
        result = self.run_fixture(FAIL_ROLE='forge')
        self.assertEqual(result.returncode, 19, result.stdout + result.stderr)
        self.assertFalse((self.root / 'network-started').exists())
        self.assertFalse((self.root / 'scenarios-started').exists())

    def test_successful_runner_cannot_qualify_incomplete_or_skipped_suite(self):
        for summary in (
            {'suite': {'complete': False, 'excluded_from_all': []}, 'skipped': []},
            {'suite': {'complete': True, 'excluded_from_all': ['scenario']}, 'skipped': []},
            {'suite': {'complete': True, 'excluded_from_all': []}, 'skipped': ['scenario']},
            {},
        ):
            with self.subTest(summary=summary):
                result = self.run_fixture(SCENARIO_SUMMARY=json.dumps(summary))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('CI requires the complete pinned protocol suite', result.stderr)

    def test_integration_compile_failure_never_starts_network(self):
        result = self.run_fixture(FAIL_BUILD='contracts')
        self.assertEqual(result.returncode, 19, result.stdout + result.stderr)
        self.assertTrue((self.root / 'build-done').exists())
        self.assertFalse((self.root / 'integration-done').exists())
        self.assertFalse((self.root / 'network-started').exists())
        self.assertFalse((self.root / 'scenarios-started').exists())

    def test_build_timeout_stops_descendants_and_fails_the_suite(self):
        timeout = shutil.which('timeout')
        self.tool('timeout', f'''#!/bin/bash
if [[ "$1" == --foreground ]]; then
    shift 3
    exec {timeout} --foreground --kill-after=.1s .2s "$@"
fi
exec {timeout} "$@"
''')
        result = self.run_fixture(HANG_ROLE='forge')
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertFalse((self.root / 'network-started').exists())
        self.assertFalse((self.root / 'scenarios-started').exists())
        self.assert_stopped()

    def test_network_failure_preserves_failure_and_never_runs_scenarios(self):
        result = self.run_fixture(FAIL_ROLE='network')
        self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
        self.assertTrue((self.root / 'build-done').exists())
        self.assertFalse((self.root / 'scenarios-started').exists())

    def test_signal_stops_owned_descendants_but_not_unrelated_process(self):
        with subprocess.Popen(['sleep', '60']) as unrelated:
            try:
                process = subprocess.Popen(self.command(), env=dict(self.env, HANG_ROLE='forge'),
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                try:
                    deadline = time.monotonic() + 5
                    while not (self.root / 'child-pid').exists():
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(.01)
                    process.send_signal(signal.SIGTERM)
                    out, err = process.communicate(timeout=8)
                    self.assertEqual(process.returncode, 143, out + err)
                    self.assert_stopped()
                    self.assertIsNone(unrelated.poll())
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.communicate()
            finally:
                unrelated.terminate()
                unrelated.wait()

    def test_pin_mismatch_or_busy_lock_does_not_start_or_stop_network(self):
        result = self.run_fixture(PIN='2')
        self.assertNotEqual(result.returncode, 0)
        lock = self.root / 'eez-core-protocol/tmp/.eez-network-e2e.lock'
        lock.parent.mkdir(parents=True, exist_ok=True)
        evidence = self.root / 'results/result.json'
        evidence.parent.mkdir()
        evidence.write_text('existing evidence')
        with lock.open('w') as stream:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.run_fixture()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('already running', result.stderr)
            self.assertEqual(evidence.read_text(), 'existing evidence')
        for marker in ('forge-started', 'network-started', 'stopped'):
            self.assertFalse((self.root / marker).exists())

    def test_wave_timings_preserve_failure_and_do_not_run_later_checks(self):
        scripts = self.package / 'scripts'
        shutil.copy2(REPO / 'testing/kurtosis/scripts/verify-cross-chain-waves.sh', scripts)
        (scripts / 'cross-chain-wave.sh').write_text('''echo "$EEZ_WAVE_MODE"
[[ "$EEZ_WAVE_MODE" != outbound ]] || exit 17
''')
        result = subprocess.run(['bash', str(scripts / 'verify-cross-chain-waves.sh')],
                                env=self.env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
        rows = [json.loads(row) for row in (self.root / 'results/timings.jsonl').read_text().splitlines()]
        self.assertEqual([(row['phase'], row['exit_code']) for row in rows],
                         [('cross-chain-wave-inbound', 0), ('cross-chain-wave-outbound', 17)])


if __name__ == '__main__':
    unittest.main()
