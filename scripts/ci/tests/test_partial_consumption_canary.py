"""The live canary reads node events without settlement storage."""

import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "partial_canary", REPO / "testing/kurtosis/scripts/verify-partial-consumption.py"
)
canary = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(canary)


class PartialConsumptionCanaryTest(unittest.TestCase):
    def test_mandatory_policy_uses_sync_invocation_as_the_wave_barrier(self):
        self.assertTrue(canary.is_sync_boundary({
            "event_name": "eez.composer.sync_slot.invoked",
        }))
        for event_name in (
            "eez.composer.phase1.bundle.dispatched",
            "eez.composer.bundle.dispatched",
            "eez.deriver.safe.advanced",
        ):
            with self.subTest(event_name=event_name):
                self.assertFalse(canary.is_sync_boundary({"event_name": event_name}))

    def test_extracts_only_the_final_included_postbatch_identity(self):
        post = "0x" + "12" * 32
        self.assertEqual(canary.included_post_hash({
            "event_name": "eez.composer.bundle.observed",
            "outcome": f"Ok(Included {{ tx_hash: {post}, l1_block: 8 }})",
        }), post)
        for event in (
            {"event_name": "eez.composer.bundle.dispatched", "outcome": ""},
            {"event_name": "eez.composer.bundle.observed",
             "outcome": f"Ok(Dropped {{ tx_hash: {post} }})"},
            {"event_name": "eez.composer.bundle.observed", "outcome": "Err(unavailable)"},
        ):
            with self.subTest(event=event):
                self.assertIsNone(canary.included_post_hash(event))

    def test_preserves_dispatch_identity_and_reconciliation_fields(self):
        post = "0x" + "12" * 32
        user = "0x" + "34" * 32
        events = [
            {"event_name": "eez.composer.bundle.dispatched", "post_batch_hash": post,
             "sync_height": 5, "user_hashes": f"[{user}]", "attempt_counts": "[1]"},
            {"event_name": "eez.composer.settlement.reconciled", "post_batch_hash": post,
             "outcomes": "[Terminal(Committed)]", "charge_attempt": False},
        ]
        log = "\n".join(json.dumps({"fields": event}) for event in events)
        self.assertEqual(canary.parse_signals(log), events)

    def test_ignores_non_event_output_and_incomplete_lines(self):
        log = '\n'.join([
            'Starting node', '{"fields":', 'null', '[]', '{"fields": null}',
            '{"fields": []}', '{"fields": {"message": "ready"}}',
            '{"fields": {"event_name": 1}}',
            '{"fields": {"event_name": "eez.composer.phase1.bundle.dispatched"}}',
        ])
        self.assertEqual(canary.parse_signals(log), [
            {"event_name": "eez.composer.phase1.bundle.dispatched"},
        ])

    def test_docker_events_are_read_from_both_output_streams(self):
        stdout = '{"fields": {"event_name": "dispatch"}}\n'
        stderr = '{"fields": {"event_name": "settled"}}\n'
        result = subprocess.CompletedProcess([], 0, stdout, stderr)
        with patch.object(canary.subprocess, "run", return_value=result):
            log = canary.command("docker", "logs", "test-node", include_stderr=True)
            self.assertEqual(canary.parse_signals(log), [
                {"event_name": "dispatch"}, {"event_name": "settled"},
            ])
            self.assertEqual(canary.command("cast", "keccak", "0x"), stdout.strip())

    def test_failed_log_command_cannot_be_used_as_evidence(self):
        result = subprocess.CompletedProcess([], 1, '', 'No such container')
        with patch.object(canary.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "docker failed"):
                canary.command("docker", "logs", "missing", include_stderr=True)
