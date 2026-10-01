"""Startup relay probes accept one healthy relay and never print relay URLs."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


LIBRARY = Path(__file__).resolve().parents[2] / "builder-relays.sh"
SECRET = "fixture-relay-secret"

# Answers by URL substring: "down" is unreachable, "plain" lacks the method,
# "reject" rejects the empty probe bundle (still a bundle API), others accept.
CURL_STUB = """#!/bin/sh
for last; do :; done
printf '%s\\n' "$last" >> "$CURL_LOG"
case "$last" in
    *down*) exit 7 ;;
    *plain*) echo '{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}' ;;
    *reject*) echo '{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"empty bundle"}}' ;;
    *) echo '{"jsonrpc":"2.0","id":1,"result":null}' ;;
esac
"""


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class BuilderRelayProbeTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="eez-builder-relays.")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        curl = self.root / "curl"
        curl.write_text(CURL_STUB)
        curl.chmod(0o755)
        self.log = self.root / "curl.log"

    def probe(self, relays):
        return subprocess.run(
            ["bash", "-c", 'source "$1" && probe_bundle_relays "$2"', "probe",
             str(LIBRARY), relays],
            env={**os.environ, "PATH": f"{self.root}{os.pathsep}{os.environ['PATH']}",
                 "CURL_LOG": str(self.log)},
            capture_output=True, text=True,
        )

    def probed(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_single_relay_keeps_previous_pass_fail_behavior(self):
        for relay, expected in (("https://ok.example/", 0), ("https://reject.example", 0),
                                ("https://down.example", 1), ("https://plain.example", 1)):
            with self.subTest(relay=relay):
                result = self.probe(relay)
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_one_healthy_relay_is_sufficient_and_others_warn(self):
        result = self.probe(
            f" https://u:{SECRET}@down.example/{SECRET} ,"
            f"https://plain.example:8545/?key={SECRET},https://ok.example/{SECRET}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("⚠ builder[0] (down.example) is unreachable", result.stderr)
        self.assertIn("⚠ builder[1] (plain.example) does not expose eth_sendBundle",
                      result.stderr)
        self.assertIn("builder[2] (ok.example) eth_sendBundle endpoint available",
                      result.stdout)
        self.assertIn("1 of 3 builder relays available", result.stderr)
        # Whitespace is trimmed and every relay is contacted with its exact URL.
        self.assertEqual(self.probed(), [
            f"https://u:{SECRET}@down.example/{SECRET}",
            f"https://plain.example:8545/?key={SECRET}",
            f"https://ok.example/{SECRET}",
        ])
        self.assertNotIn(SECRET, result.stdout + result.stderr)

    def test_no_healthy_relay_fails(self):
        result = self.probe("https://down.example,https://plain.example")
        self.assertEqual(result.returncode, 1)
        self.assertIn("none of the 2 configured builder relays", result.stderr)

    def test_empty_list_or_entry_fails_before_contacting_relays(self):
        for relays, message in (("", "at least one"), ("  ", "at least one"),
                                ("https://ok.example,,https://ok2.example", "entry 1 is empty"),
                                (" ,https://ok.example", "entry 0 is empty")):
            with self.subTest(relays=relays):
                result = self.probe(relays)
                self.assertEqual(result.returncode, 1)
                self.assertIn(message, result.stderr)
        # The second case fails at entry 1 after probing entry 0 only.
        self.assertEqual(self.probed(), ["https://ok.example"])

    def test_hosts_are_extracted_without_credentials_ports_or_paths(self):
        for url, host in (("https://u:p@relay.example:443/x?y=z", "relay.example"),
                          ("http://[::1]:8545/", "[::1]"),
                          ("http://[::1]", "[::1]"),
                          ("http://127.0.0.1:8645", "127.0.0.1")):
            with self.subTest(url=url):
                result = subprocess.run(
                    ["bash", "-c", 'source "$1" && builder_relay_host "$2"', "host",
                     str(LIBRARY), url],
                    capture_output=True, text=True, check=True,
                )
                self.assertEqual(result.stdout.strip(), host)


if __name__ == "__main__":
    unittest.main()
