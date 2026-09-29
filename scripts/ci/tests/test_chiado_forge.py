"""The shell bridge transports full calldata before Linux's exec argument limit."""
import errno
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
BRIDGE = ROOT / "scripts/chiado-forge.sh"
TARGET = "script/e2e/shared/Verify.s.sol:VerifyL1BatchCalldata"
SIGNATURE = "run(bytes,address,bytes,bytes,bytes,bool)"


class ChiadoForgeTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-forge-transport-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.contracts = self.root / "contracts"
        self.contracts.mkdir()
        mock = self.bin / "forge"
        mock.write_text("""#!/usr/bin/env python3
import hashlib,json,os,pathlib,stat,sys
args=sys.argv[1:]; result={'args':args}
if '--sig' in args and args[args.index('--sig')+1].startswith('runFromFile('):
    p=pathlib.Path(args[args.index('--sig')+2]); data=p.read_bytes()
    result.update(input_path=str(p),length=len(data),sha256=hashlib.sha256(data).hexdigest(),
                  mode=stat.S_IMODE(p.stat().st_mode))
pathlib.Path(os.environ['RECORD']).write_text(json.dumps(result))
sys.exit(int(os.environ.get('FORGE_EXIT','0')))
""")
        mock.chmod(0o755)
        self.record = self.root / "record.json"
        self.input = self.root / "input.hex"
        self.payload = b"0x" + b"ab" * 70_000
        self.input.write_bytes(self.payload)
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
                        RECORD=str(self.record), INPUT=str(self.input))

    def invoke(self, args, code=0):
        env = dict(self.env, FORGE_EXIT=str(code))
        # Source the real bridge, then scope its temporary files to this fixture.
        script = 'source "$1"; export EEZ_CHIADO_CONTRACTS_ROOT="$2"; shift 2; '
        script += 'payload=$(<"$INPUT"); forge "$@" "$payload" registry expected steps 0x false'
        result = subprocess.run(['bash', '-euo', 'pipefail', '-c', script, 'test',
                                 str(BRIDGE), str(self.contracts), *args], env=env,
                                capture_output=True, text=True)
        return result, json.loads(self.record.read_text())

    def test_large_input_reaches_forge_unchanged_and_file_is_removed(self):
        with self.assertRaises(OSError) as error:
            subprocess.run([str(self.bin / 'forge'), self.payload.decode()], env=self.env)
        self.assertEqual(error.exception.errno, errno.E2BIG)
        result, record = self.invoke(['script', TARGET, '--rpc-url', 'fixture', '--sig', SIGNATURE])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(record['length'], len(self.payload))
        self.assertEqual(record['sha256'], hashlib.sha256(self.payload).hexdigest())
        self.assertEqual(record['mode'], 0o600)
        self.assertFalse(Path(record['input_path']).exists())
        self.assertEqual(record['args'][-8:],
                         [record['input_path'], 'registry', 'expected', 'steps', '0x', 'false',
                          '--root', str(self.contracts)])

    def test_verifier_failure_is_preserved_and_file_is_removed(self):
        result, record = self.invoke(['script', TARGET, '--sig', SIGNATURE], code=23)
        self.assertEqual(result.returncode, 23)
        self.assertFalse(Path(record['input_path']).exists())

    def test_small_input_keeps_pinned_target_and_arguments(self):
        self.input.write_text('0x1234')
        args = ['script', TARGET, '--sig', SIGNATURE]
        result, record = self.invoke(args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(record['args'], args + ['0x1234', 'registry', 'expected', 'steps', '0x', 'false'])


if __name__ == '__main__':
    unittest.main()
