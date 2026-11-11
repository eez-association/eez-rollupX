"""Offline orchestration guards; these do not claim live settlement coverage."""
import ast
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[3]


class ChiadoMixedValidationTest(unittest.TestCase):
    def test_underfunding_is_rejected_before_output_or_wallet_locks(self):
        for optimize in ('', '2'):
            with self.subTest(optimize=optimize), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary)/'evidence'
                env = dict(os.environ, STRESS_DEPLOYMENT_ROOT=temporary,
                           EEZ_MIXED_OUTPUT_DIR=str(output), EEZ_TEST_FUND_AMOUNT_ETH='0.004',
                           STRESS_NODE_IMAGE_ID='fixture', STRESS_SIGNER_IMAGE_ID='fixture',
                           STRESS_SOURCE_REVISION='fixture', STRESS_POSTER_ADDRESS='fixture',
                           STRESS_FOLLOWER_CONTAINER='fixture', STRESS_FOLLOWER_IMAGE='fixture',
                           STRESS_SENDER_ADDRESS='fixture', STRESS_L2_CHAIN_ID='906969',
                           PYTHONOPTIMIZE=optimize)
                result = subprocess.run(['bash', 'scripts/run-chiado-mixed-validation.sh'],
                                        cwd=REPO, env=env, capture_output=True, text=True, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('does not cover wave deposits', result.stderr)
                self.assertFalse(output.exists())

    def test_ordinary_sender_uses_the_active_queue_check(self):
        tree = ast.parse((REPO / 'scripts/chiado-ordinary-stress.py').read_text())
        calls = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
                 and isinstance(node.func, ast.Name) and node.func.id == 'check_queue']
        self.assertEqual(len(calls), 1)
        self.assertEqual(ast.literal_eval(calls[0].keywords[0].value), False)
        self.assertTrue(any(isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                            and node.func.id == 'check_completion' for node in ast.walk(tree)))

    def test_missing_explicit_funding_aborts_before_any_operation(self):
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(('STRESS_', 'EEZ_'))}
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'evidence'
            env.update(STRESS_DEPLOYMENT_ROOT=temporary, EEZ_MIXED_OUTPUT_DIR=str(output))
            result = subprocess.run(['bash', 'scripts/run-chiado-mixed-validation.sh'],
                                    cwd=REPO, env=env, capture_output=True, text=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Explicit test actor funding is required', result.stderr)
            self.assertFalse(output.exists())

    def test_failed_workload_cannot_be_relabelled_by_independent_checker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'live-mixed.exit').write_text('1\n')
            result = subprocess.run([sys.executable, 'scripts/verify-chiado-mixed-load.py',
                                     '--evidence', str(root), '--waves', '30'], cwd=REPO,
                                    capture_output=True, text=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('AssertionError', result.stderr)
            self.assertFalse((root / 'independent-cross-chain-summary.json').exists())

    def test_full_wave_count_is_required_before_rpc(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'live-mixed.exit').write_text('0\n')
            (root / 'live-waves.log').write_text('')
            result = subprocess.run([sys.executable, 'scripts/verify-chiado-mixed-load.py',
                                     '--evidence', str(root), '--waves', '30'], cwd=REPO,
                                    capture_output=True, text=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('AssertionError', result.stderr)
            self.assertFalse((root / 'independent-cross-chain-summary.json').exists())


if __name__ == '__main__':
    unittest.main()
