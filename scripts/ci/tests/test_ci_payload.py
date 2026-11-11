"""OCI handoff is data-only, immutable, scoped, and independent of Actions quota."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import uuid


REPO = Path(__file__).resolve().parents[3]
MOCK = r'''#!/usr/bin/env python3
import json, os, shutil, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['TRACE'], 'a') as stream:
    stream.write(json.dumps(args)+'\n')
if args[:2] == ['image', 'inspect']:
    if '--format' in args:
        print(json.dumps([args[-1].split(':sha-')[0]+'@sha256:'+'a'*64]))
    else:
        print(json.dumps([dict(Os='linux', Architecture='amd64', Config=dict(Labels={
            'org.opencontainers.image.source': 'https://github.com/'+os.environ['GITHUB_REPOSITORY'],
            'org.opencontainers.image.revision': os.environ.get('WRONG_REVISION', os.environ['GITHUB_SHA']),
            'io.eez.protocol.revision': os.environ['PROTOCOL'],
            'io.eez.ci.payload': os.environ['KIND']}))]))
elif args[0] == 'create':
    print('b'*64)
elif args[0] == 'cp':
    if os.environ.get('FAIL_COPY'):
        raise SystemExit(17)
    shutil.copytree(os.environ['PAYLOAD'], args[-1], dirs_exist_ok=True)
elif args[0] in ('pull', 'push', 'rm') or args[:2] == ['buildx', 'build']:
    pass
else:
    raise SystemExit('unexpected command: '+repr(args))
'''


class PayloadTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="eez-payload-test.")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        tool = self.root / "docker"
        tool.write_text(MOCK)
        tool.chmod(0o755)
        self.payload = self.root / "payload"
        self.payload.mkdir()
        (self.payload / "manifest.json").write_text("{}")
        (self.payload / "runtime-binaries.tar.gz").write_text("runtime")
        (self.payload / "target").mkdir()
        (self.payload / "target/test").write_text("test")
        (self.payload / "diagnostics.log").write_text("must not publish")
        self.trace = self.root / "trace"
        self.env = dict(os.environ, PATH=str(self.root)+os.pathsep+os.environ["PATH"],
                        TRACE=str(self.trace), PAYLOAD=str(self.payload), RUNNER_TEMP=str(self.root),
                        GITHUB_REPOSITORY="eduadiez/rollupX", GITHUB_SHA="c"*40,
                        PROTOCOL=subprocess.check_output(["git", "-C", str(REPO / "eez-core-protocol"),
                                                         "rev-parse", "HEAD"], text=True).strip(),
                        GITHUB_RUN_ID="123", GITHUB_RUN_ATTEMPT="2", GITHUB_OUTPUT=str(self.root / "output"))

    def run_script(self, operation, kind, *args):
        return subprocess.run(["bash", str(REPO / "scripts/ci/ci-payload.sh"), operation, kind, *map(str, args)],
                              env=dict(self.env, KIND=kind), text=True, capture_output=True, timeout=10)

    def ref(self, kind):
        return f"ghcr.io/{self.env['GITHUB_REPOSITORY'].lower()}-ci-{kind}@sha256:"+"a"*64

    def calls(self):
        return [json.loads(line) for line in self.trace.read_text().splitlines()] if self.trace.exists() else []

    def test_publish_only_execution_inputs_and_output_immutable_digest(self):
        for kind, expected in (("runtime", {"manifest.json", "runtime-binaries.tar.gz"}),
                               ("tests", {"manifest.json", "target"})):
            with self.subTest(kind=kind):
                result = self.run_script("publish", kind, self.payload)
                self.assertEqual(result.returncode, 0, result.stderr)
                command = next(c for c in reversed(self.calls()) if c[:2] == ["buildx", "build"])
                self.assertEqual({p.name for p in Path(command[-1]).iterdir()}, expected)
                self.assertIn(f"{kind}-ref={self.ref(kind)}", (self.root / "output").read_text())

    def test_fetch_checks_identity_and_removes_only_its_unstarted_container(self):
        result = self.run_script("fetch", "tests", self.ref("tests"), self.root / "fetched")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertEqual(calls[-1], ["rm", "b"*64])
        self.assertEqual(next(c for c in calls if c[0] == "create")[-1], "/not-executed")
        self.assertFalse(any(c[0] in ("run", "start", "exec") for c in calls))

    def test_transfer_publishes_and_fetches_in_new_namespace_but_rejects_old_payloads(self):
        old_ref = self.ref("runtime")
        self.env["GITHUB_REPOSITORY"] = "eez-association/rollupX"
        for kind in ("runtime", "tests"):
            with self.subTest(kind=kind):
                result = self.run_script("publish", kind, self.payload)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"{kind}-ref={self.ref(kind)}", (self.root / "output").read_text())
                command = next(c for c in reversed(self.calls()) if c[:2] == ["buildx", "build"])
                self.assertIn("org.opencontainers.image.source=https://github.com/eez-association/rollupX", command)
                result = self.run_script("fetch", kind, self.ref(kind), self.root / kind)
                self.assertEqual(result.returncode, 0, result.stderr)
        self.trace.write_text("")
        result = self.run_script("fetch", "runtime", old_ref, self.root / "old")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("repository-bound immutable payload digest", result.stderr)
        self.assertFalse(self.calls())

    def test_bad_revision_or_wrong_registry_rejected_before_container_creation(self):
        for ref in ("ghcr.io/other/repo@sha256:"+"a"*64, "ghcr.io/eduadiez/rollupx-ci-tests:latest"):
            result = self.run_script("fetch", "tests", ref, self.root / "fetched")
            self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.calls())
        self.env["WRONG_REVISION"] = "d"*40
        result = self.run_script("fetch", "tests", self.ref("tests"), self.root / "fetched")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[0] == "create" for c in self.calls()))

    def test_failed_copy_preserves_failure_and_cleans_up(self):
        self.env["FAIL_COPY"] = "1"
        result = self.run_script("fetch", "runtime", self.ref("runtime"), self.root / "fetched")
        self.assertEqual(result.returncode, 17)
        self.assertEqual(self.calls()[-1], ["rm", "b"*64])

    def test_existing_destination_is_not_overwritten(self):
        result = self.run_script("fetch", "tests", self.ref("tests"), self.payload)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.payload / "target/test").read_text(), "test")
        self.assertFalse(self.calls())


@unittest.skipUnless(os.environ.get("EEZ_TEST_DOCKER_PAYLOAD") == "1", "opt-in real data-only Docker image")
class DockerPayloadTest(unittest.TestCase):
    def test_scratch_payload_copies_out_without_execution_or_permission_loss(self):
        tag = "eez-ci-payload-test:"+uuid.uuid4().hex
        container = None
        with tempfile.TemporaryDirectory(prefix="eez-payload-docker.") as directory:
            root = Path(directory)
            context = root / "context"
            context.mkdir()
            source = context / "target/bin/test"
            source.parent.mkdir(parents=True)
            source.write_text("#!/bin/sh\nexit 0\n")
            source.chmod(0o755)
            (context / "manifest.json").write_text("{}")
            try:
                subprocess.run(["docker", "buildx", "build", "--builder", "default", "--provenance=false",
                                "--platform", "linux/amd64", "--load", "-t", tag, "-f",
                                str(REPO / "Dockerfile.ci-payload"), str(context)], check=True)
                container = subprocess.check_output(["docker", "create", tag, "/not-executed"], text=True).strip()
                destination = root / "download"
                destination.mkdir()
                subprocess.run(["docker", "cp", f"{container}:/payload/.", str(destination)], check=True)
                copied = destination / "target/bin/test"
                self.assertEqual(copied.read_bytes(), source.read_bytes())
                self.assertTrue(os.access(copied, os.X_OK))
                self.assertEqual(subprocess.check_output(["docker", "inspect", "--format", "{{.State.Status}}",
                                                         container], text=True).strip(), "created")
            finally:
                if container:
                    subprocess.run(["docker", "rm", container], check=True)
                subprocess.run(["docker", "image", "rm", tag], check=False)


if __name__ == "__main__":
    unittest.main()
