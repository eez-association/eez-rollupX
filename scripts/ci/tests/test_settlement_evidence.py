"""Regressions for the two failed mixed-load harness assumptions."""
import copy
import importlib.util
from pathlib import Path
import unittest
import subprocess
import sys
import traceback
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('evidence', REPO / 'scripts/settlement_evidence.py')
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


def digest(n):
    return '0x' + f'{n:064x}'


class SettlementEvidenceTest(unittest.TestCase):
    def test_signer_failure_never_persists_key_argv_or_stderr(self):
        secret = 'private-fixture-sentinel'
        original = subprocess.CalledProcessError(9, ['cast', 'mktx', '--private-key', secret],
                                                 stderr=secret)
        with patch.object(evidence.subprocess, 'check_output', side_effect=original):
            try:
                evidence.cast_with_key(['mktx', 'destination'], secret)
            except RuntimeError as error:
                rendered = ''.join(traceback.format_exception(error))
                self.assertIn('cast mktx failed with exit 9', rendered)
                self.assertNotIn(secret, rendered)
            else:
                self.fail('signing failure must propagate')

    def test_optimized_python_is_rejected_instead_of_skipping_gates(self):
        result = subprocess.run([sys.executable, '-O', str(REPO/'scripts/settlement_evidence.py')],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Validation requires Python assertions', result.stderr)

    def test_runtime_replacement_restart_and_stop_are_rejected(self):
        original = dict(name='/fixture', id='id', image='image', started='start', restarts=0)
        current = dict(Name='/fixture', Id='id', Image='image', RestartCount=0,
                       State=dict(Running=True, StartedAt='start'))
        evidence.check_runtime([original], [current])
        for key, value in (('Id', 'other'), ('Image', 'other'), ('RestartCount', 1),
                           ('State', dict(Running=False, StartedAt='start')),
                           ('State', dict(Running=True, StartedAt='later'))):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                evidence.check_runtime([original], [dict(current, **{key: value})])

    def test_reorg_during_reads_rejects_snapshot(self):
        snapshot = dict(number='0x1', hash=digest(1))
        evidence.check_snapshot(lambda *args: snapshot, 'unused', snapshot)
        with self.assertRaisesRegex(AssertionError, 'snapshot changed'):
            evidence.check_snapshot(lambda *args: dict(snapshot, hash=digest(2)), 'unused', snapshot)

    def test_full_wave_value_budget_is_checked_before_gas(self):
        self.assertEqual(evidence.check_funding('0.006'), 6_000_000_000_000_000)
        for amount in ('0.004', '0.00465', '0', '-1', 'NaN', 'Infinity',
                       '0.0060000000000000001'):
            with self.subTest(amount=amount), self.assertRaises(AssertionError):
                evidence.check_funding(amount)

    def fixture(self):
        registry, topics = '0x' + '1' * 40, [digest(1), digest(2)]
        posting = dict(transactionHash=digest(10), blockHash=digest(20),
                       blockNumber='0x14', transactionIndex='0x0', status='0x1')
        events = [dict(address=registry, topics=topics, data=digest(commitment) + '0' * 64,
                       blockHash=digest(20), blockNumber='0x14', transactionHash=digest(10 + i),
                       transactionIndex=hex(i), logIndex=hex(i), removed=False)
                  for i, commitment in enumerate((100, 101))]
        completion = dict(posting, transactionHash=digest(11), transactionIndex='0x1', logs=[events[-1]])
        correlation = dict(canonicalL2=True, l1TransactionHash=digest(10),
                           l2Range=dict(lastBlockHash=digest(101)))
        return [correlation, posting, events, copy.deepcopy(events), completion, registry, topics]

    def test_mixed_queue_is_not_an_idle_queue(self):
        queue = dict(queued=1, inFlight=4, blocked=0, unknown=1)
        with self.assertRaisesRegex(AssertionError, 'not idle'):
            evidence.check_queue(queue, idle=True)
        evidence.check_queue(queue, idle=False)
        evidence.check_queue(dict.fromkeys(queue, 0), idle=True)
        with self.assertRaisesRegex(AssertionError, 'Invalid'):
            evidence.check_queue(dict(queue, unknown=-1), idle=False)

    def test_deferred_companion_completes_posting_prefix(self):
        args = self.fixture()
        self.assertNotEqual(args[2][0]['data'][:66], args[0]['l2Range']['lastBlockHash'])
        evidence.check_completion(*args)

    def test_posting_only_completion_is_also_valid(self):
        args = self.fixture()
        args[0]['l2Range']['lastBlockHash'] = digest(100)
        args[2], args[3] = args[2][:1], args[3][:1]
        args[4] = dict(args[1], logs=args[2])
        evidence.check_completion(*args)

    def test_prefix_receipt_does_not_prove_completion(self):
        args = self.fixture()
        args[2], args[3] = args[2][:1], args[3][:1]
        with self.assertRaisesRegex(AssertionError, 'commitment'):
            evidence.check_completion(*args)

    def test_incomplete_disagreeing_or_misattributed_evidence_rejected(self):
        mutations = [
            lambda a: a[0].update(canonicalL2=False),
            lambda a: a[0].update(l1TransactionHash=digest(99)),
            lambda a: a[1].update(status='0x0'),
            lambda a: a[2].clear(),
            lambda a: a[3][-1].update(data=digest(99) + '0' * 64),
            lambda a: a[2][-1].update(removed=True),
            lambda a: a[4].update(status='0x0'),
            lambda a: a[4].update(transactionHash=digest(99)),
            lambda a: a[4].update(logs=[]),
        ]
        for mutation in mutations:
            args = self.fixture()
            mutation(args)
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                evidence.check_completion(*args)

    def test_matching_providers_cannot_authorize_wrong_scope_order_or_endpoint(self):
        for key, value in (('blockHash', digest(99)), ('address', '0x' + '9' * 40),
                           ('topics', [digest(9)]), ('data', digest(99) + '0' * 64),
                           ('transactionIndex', '0x0'), ('data', '0x01')):
            args = self.fixture()
            args[2][-1][key] = value
            args[3] = copy.deepcopy(args[2])
            with self.subTest(key=key, value=value), self.assertRaises(AssertionError):
                evidence.check_completion(*args)
        args = self.fixture()
        args[2].reverse()
        args[3] = copy.deepcopy(args[2])
        with self.assertRaisesRegex(AssertionError, 'Unordered'):
            evidence.check_completion(*args)


if __name__ == '__main__':
    unittest.main()
