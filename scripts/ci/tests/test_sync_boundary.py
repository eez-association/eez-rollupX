"""The state-ordering fixture follows Sync execution independently of posting."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / "testing/kurtosis/scripts/verify-state-chaining.sh"


class SyncBoundaryTest(unittest.TestCase):
    def test_source_receipts_wait_for_safe_canonical_recomposition(self):
        source = SCRIPT.read_text()
        function = source[source.index("wait_for_receipts() {"):
                          source.index("\nwrapped_result() {")]
        for initial_safe in (9, 20):
            with self.subTest(initial_safe=initial_safe), tempfile.TemporaryDirectory() as temporary:
                counter = Path(temporary) / "reads"
                counter.write_text("0")
                env = dict(os.environ, READS=str(counter), L2="http://l2.invalid",
                           RECEIPT_WAIT_SECS="10", INITIAL_SAFE=str(initial_safe))
                harness = '''set -euo pipefail
receipt_status() { echo 1; }
receipt_json() {
    count=$(( $(cat "$READS") + 1 )); echo "$count" > "$READS"
    if (( count == 1 )); then
        echo '{"result":{"blockNumber":"0xa","blockHash":"0xold"}}'
    else
        echo '{"result":{"blockNumber":"0xf","blockHash":"0xnew"}}'
    fi
}
cast() {
    [[ "$1" == block ]] || return 77
    if [[ "$2" == safe ]]; then
        if (( $(cat "$READS") == 1 )); then echo "$INITIAL_SAFE"; else echo 15; fi
    elif [[ "$2" == 0xf ]]; then echo 0xnew
    else echo 0xcanonicalreplacement; fi
}
sleep() { :; }
'''
                result = subprocess.run(["bash", "-c", harness + function +
                                         '\nwait_for_receipts "$L2" 0xtx'], env=env,
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(counter.read_text().strip(), "2")

    def wait_for_boundary(self, marker):
        source = SCRIPT.read_text()
        function = source[source.index("wait_for_sync_boundary() {"):
                          source.index("\nwait_for_receipts() {")]
        with tempfile.TemporaryDirectory(prefix="eez-sync-boundary.") as temporary:
            env = dict(os.environ, NODE_LOG=str(Path(temporary) / "node.log"),
                       EEZ_SYNC_BOUNDARY_WAIT_SECS="2", NEXT_EVENT=marker)
            harness = '''set -euo pipefail
calls=0
strip_ansi() { cat; }
refresh_node_log() {
    calls=$((calls + 1))
    : > "$NODE_LOG"
    if (( calls > 1 )); then
        printf '%s\\n' "$NEXT_EVENT" > "$NODE_LOG"
    fi
}
'''
            return subprocess.run(["bash", "-c", harness + function + "\nwait_for_sync_boundary"],
                                  env=env, capture_output=True, text=True, timeout=5)

    def test_committed_empty_sync_advances_without_a_drain_or_post(self):
        event = '[eez-node] {"fields":{"message":"produced block 12 hash=0x00 ts=24 kind=sync",' \
                '"slot.kind":"sync","block.number":12},"target":"eez_driver::sequencer"}'
        result = self.wait_for_boundary(event)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_live_execution_and_pool_drain_do_not_claim_a_committed_sync(self):
        event = '[eez-node] {"fields":{"message":"produced block 13 kind=live","slot.kind":"live"}}\n' \
                '[eez-node] {"fields":{"event_name":"eez.composer.sync_slot.drain"}}'
        result = self.wait_for_boundary(event)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("timed out waiting for a fresh Sync-slot boundary", result.stderr)


if __name__ == "__main__":
    unittest.main()
