"""Exercise the runner's real profile boundary without keys, RPC or wallet use."""
import ast
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


class StagedFundingTest(unittest.TestCase):
    def setUp(self):
        source = (REPO / "scripts/run-chiado-staged-performance.sh").read_text()
        python = source.split("python3 - <<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
        tree = ast.parse(python)
        function = next(n for n in tree.body
                        if isinstance(n, ast.FunctionDef) and n.name == "funding_requirement")
        namespace = {}
        exec(compile(ast.Module(body=[function], type_ignores=[]), str(REPO), "exec"), namespace)
        self.requirement = namespace["funding_requirement"]

    def test_funded_pool_needs_no_faucet_reserve(self):
        self.assertEqual(self.requirement([3300] * 770, 3300, 3300, 100), (0, 0))

    def test_only_deficient_workers_count_and_chunk_boundary_is_exact(self):
        target = 3_300_000_000_000_000
        for deficient, chunks in [(1, 1), (100, 1), (101, 2)]:
            balances = [target - 10] * deficient + [target] * (770 - deficient)
            self.assertEqual(self.requirement(balances, target, target, 100),
                             (10 * deficient + chunks * 5 * 10**16 + 10**17, deficient))

    def test_floor_equal_or_above_skips_worker_even_below_target(self):
        self.assertEqual(self.requirement([2000, 2500, 4000, 1999], 3300, 2000, 100),
                         (1301 + 5 * 10**16 + 10**17, 1))

    def test_empty_pool_retains_original_full_requirement(self):
        self.assertEqual(self.requirement([0] * 770, 3_300_000_000_000_000,
                                         3_300_000_000_000_000, 100),
                         (3_041_000_000_000_000_000, 770))


class StagedPerformanceConfigTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-staged-config-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        (self.root / "contracts").mkdir()
        (self.root / "eez-core-protocol").mkdir()
        self.script = self.root / "scripts/run-chiado-staged-performance.sh"
        shutil.copy2(REPO / "scripts/run-chiado-staged-performance.sh", self.script)
        shutil.copy2(REPO / "scripts/chiado-forge.sh", self.root / "scripts/chiado-forge.sh")
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.marker = self.root / "tool-invoked"
        for name in ("cast", "forge", "curl", "jq", "bc", "flock", "timeout", "python3"):
            tool = self.tools / name
            tool.write_text("#!/bin/sh\n"
                            f"touch {shlex.quote(str(self.marker))}\nexit 73\n")
            tool.chmod(0o755)

    def run_helper(self, profile_scale=None, environment_scale=None, args=("--check",)):
        profile = "SOURCE_PK=unused-public-fixture\n"
        if profile_scale is not None:
            profile += "EEZ_STAGED_SCALE=" + shlex.quote(profile_scale) + "\n"
        (self.root / "eez-core-protocol/chain.env").write_text(profile)
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("EEZ_", "DEVNET_", "SOURCE_"))}
        env["PATH"] = str(self.tools) + os.pathsep + os.environ["PATH"]
        if environment_scale is not None:
            env["EEZ_STAGED_SCALE"] = environment_scale
        self.marker.unlink(missing_ok=True)
        return subprocess.run(["bash", str(self.script), *args], cwd=self.root,
                              env=env, capture_output=True, text=True)

    def test_invalid_profile_scale_is_rejected_before_any_tool_execution(self):
        for value in ("", "0", "-1", "11", "010", "1+1", "x", "9" * 100):
            with self.subTest(value=value):
                result = self.run_helper(value, "4")
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn("integer from 1 to 10", result.stderr)
                self.assertFalse(self.marker.exists())

    def test_valid_effective_scale_reaches_the_key_boundary(self):
        # Cast deliberately fails: reaching it proves profile validation accepted
        # the effective value, without letting the helper fund or send anything.
        for profile, environment in ((None, None), (None, ""), (None, "10"),
                                     ("1", None), ("4", "11"), ("10", "0")):
            with self.subTest(profile=profile, environment=environment):
                result = self.run_helper(profile, environment)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("Could not derive the funding address", result.stderr)
                self.assertTrue(self.marker.exists())

    def test_resend_is_rejected_before_loading_a_profile_or_executing_tools(self):
        result = self.run_helper(args=("--resend", "old-network-run"))
        self.assertEqual(result.returncode, 2)
        self.assertIn("Unknown argument: --resend", result.stderr)
        self.assertFalse(self.marker.exists())

    def test_verify_mode_reaches_the_locked_runner_and_check_never_sends(self):
        args_file = self.root / "runner-args"
        lock_file = self.root / "lock-observed"
        for name, body in {
            "cast": "echo 0x" + "11" * 20,
            "python3": "exit 0",
            "flock": "touch " + shlex.quote(str(lock_file)),
        }.items():
            (self.tools / name).write_text("#!/bin/sh\n" + body + "\n")
        runner = self.root / "eez-core-protocol/script/e2e/run/network/staged.sh"
        runner.parent.mkdir(parents=True)
        runner.write_text("#!/bin/bash\nset -eu\n"
                          + "test -f " + shlex.quote(str(lock_file)) + "\n"
                          + '[[ $(type -t forge) == function ]]\n'
                          + 'test "$EEZ_CHIADO_CONTRACTS_ROOT" = '
                          + shlex.quote(str(self.root / "contracts")) + "\n"
                          + "printf '%s\\n' \"$@\" > " + shlex.quote(str(args_file)) + "\n")
        for args, verify in (((), False), (("--verify",), True)):
            with self.subTest(args=args):
                result = self.run_helper(args=args)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                forwarded = args_file.read_text().splitlines()
                self.assertEqual("--no-verify" in forwarded, not verify)
                self.assertIn("counter:100", forwarded)
                self.assertIn("revertFromOtherChainL2:50", forwarded)
        args_file.unlink()
        result = self.run_helper(args=("--verify", "--check"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(args_file.exists())
