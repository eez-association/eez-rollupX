"""Regression checks for omissions, revision drift and destructive draft updates."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / "scripts/backport-handoff.py"
SPEC = importlib.util.spec_from_file_location("backport_handoff", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
EXAMPLES = REPO / "docs/examples/backport-handoff"


class HandoffTest(unittest.TestCase):
    def setUp(self):
        self.manifest = MODULE.load(EXAMPLES / "manifest.json")
        self.handoff = MODULE.load(EXAMPLES / "handoff.json")

    def invalid(self, expected):
        with self.assertRaisesRegex(MODULE.Invalid, expected):
            MODULE.validate(self.manifest, self.handoff)

    def update(self):
        draft = self.handoff["drafts"][0]
        draft["issue"] = "https://github.com/eez-association/eez-rollupX/issues/133"
        draft["proposed_changes"] = []
        del draft["issue_type"], draft["done_when"]
        return draft

    def add_draft(self, did):
        draft = copy.deepcopy(self.handoff["drafts"][0])
        draft["id"] = did
        self.handoff["drafts"].append(draft)
        self.handoff["resolutions"][0]["drafts"].append(did)
        return draft

    def test_complete_example_and_deterministic_markdown(self):
        first = MODULE.render(self.manifest, self.handoff)
        self.assertEqual(first, MODULE.render(self.manifest, self.handoff))
        self.assertIn("**Done when**", first)
        self.assertIn("**Investigation evidence needed**", first)
        for side in ("source", "target"):
            self.assertIn(self.manifest["baselines"][side]["commit"], first)
        for entry in self.handoff["drafts"][0]["evidence"]:
            self.assertIn("](<" + entry["url"] + ">)", first)

    def test_added_f7_cannot_silently_disappear(self):
        self.manifest["findings"].append({"id": "F7", "title": "Opaque rejection", "origin": "source"})
        self.handoff["manifest_sha256"] = MODULE.digest(self.manifest)
        self.invalid("missing findings: F7")

    def test_changed_baselines_and_inventory_fail_digest(self):
        original = copy.deepcopy(self.manifest)
        for field in ("commit", "protocol_commit"):
            for side in ("source", "target"):
                with self.subTest(side=side, field=field):
                    self.manifest = copy.deepcopy(original)
                    self.manifest["baselines"][side][field] = "a" * 40
                    self.invalid("frozen inputs changed")
        self.manifest = copy.deepcopy(original)
        self.manifest["findings"][0]["title"] = "Rewritten finding"
        self.invalid("frozen inputs changed")

    def test_empty_review_can_return_no_drafts(self):
        self.manifest["findings"] = []
        self.handoff = {"manifest_sha256": MODULE.digest(self.manifest), "resolutions": [], "drafts": []}
        MODULE.validate(self.manifest, self.handoff)
        self.assertNotIn("## New issue drafts", MODULE.render(self.manifest, self.handoff))

    def test_source_issue_cannot_stand_in_for_existing_target_work(self):
        draft = self.update()
        draft["issue"] = "https://github.com/eez-association/eez-rollup0/issues/133"
        self.invalid("target repository")
        self.handoff["resolutions"] = [{"finding": "F1", "action": "existing_issue",
            "issue": draft["issue"], "reason": "Already tracked", "evidence": draft["evidence"]}]
        self.handoff["drafts"] = []
        self.invalid("target repository")

    def test_json_formatting_does_not_change_digest(self):
        reordered = json.loads(json.dumps(self.manifest, sort_keys=True))
        self.assertEqual(MODULE.digest(reordered), self.handoff["manifest_sha256"])

    def test_missing_or_duplicate_resolutions(self):
        original = copy.deepcopy(self.handoff)
        cases = [([], "missing findings"),
                 (original["resolutions"] * 2, "duplicate resolution"),
                 ([{"finding": "F9", "action": "drafts", "drafts": ["N1"]}], "unknown finding"),
                 ([{"finding": "F1", "action": "unresolved"}], "unresolved is not a handoff")]
        for resolutions, error in cases:
            with self.subTest(error=error):
                self.handoff = copy.deepcopy(original)
                self.handoff["resolutions"] = resolutions
                self.invalid(error)

    def test_dangling_duplicate_and_unrouted_drafts(self):
        self.handoff["resolutions"][0]["drafts"].append("N7")
        self.invalid("unknown draft references")
        self.handoff["resolutions"][0]["drafts"].pop()
        self.add_draft("N1")
        self.invalid("duplicate entries")
        self.handoff["resolutions"][0]["drafts"].pop()
        self.invalid("duplicate draft")
        self.handoff["drafts"][1]["id"] = "N2"
        self.invalid("not linked to any")

    def test_no_action_and_existing_issue_require_reason_and_evidence(self):
        evidence = self.handoff["drafts"][0]["evidence"]
        self.handoff["drafts"] = []
        resolution = {"finding": "F1", "action": "no_action", "reason": "Already covered by target behavior.", "evidence": evidence}
        self.handoff["resolutions"] = [resolution]
        MODULE.validate(self.manifest, self.handoff)
        self.assertIn("No action:", MODULE.render(self.manifest, self.handoff))
        resolution["action"] = "existing_issue"
        resolution["issue"] = "https://github.com/eez-association/eez-rollupX/issues/133"
        MODULE.validate(self.manifest, self.handoff)
        resolution["reason"] = " "
        self.invalid("nonempty text")
        resolution["reason"] = "Covered."
        resolution["evidence"] = []
        self.invalid("must not be empty")

    def test_unpinned_and_wrong_baseline_evidence(self):
        draft = self.handoff["drafts"][0]
        baseline = self.manifest["baselines"]["target"]["commit"]
        original = draft["evidence"][1]["url"]
        for replacement in ("main", baseline[:8], "0" * 40):
            with self.subTest(replacement=replacement):
                draft["evidence"][1]["url"] = original.replace(baseline, replacement)
                self.invalid("SHA|pinned")
        draft["evidence"][1]["url"] = original.replace(baseline, "a" * 40)
        self.invalid("frozen target revision")
        draft["evidence"][1]["url"] = original
        draft["evidence"].pop(0)
        self.invalid("frozen source revision")

    def test_target_only_finding_does_not_require_source_evidence(self):
        self.manifest["findings"][0]["origin"] = "target"
        self.handoff["manifest_sha256"] = MODULE.digest(self.manifest)
        self.handoff["drafts"][0]["evidence"].pop(0)
        MODULE.validate(self.manifest, self.handoff)

    def test_required_draft_fields_and_estimates(self):
        original = copy.deepcopy(self.handoff)
        for key in ("done_when", "evidence_needed", "dependencies", "summary", "priority", "effort"):
            with self.subTest(key=key):
                self.handoff = copy.deepcopy(original)
                del self.handoff["drafts"][0][key]
                self.invalid("missing")
        for key in ("done_when", "evidence_needed"):
            self.handoff = copy.deepcopy(original)
            self.handoff["drafts"][0][key] = []
            self.invalid("must not be empty")
        for key, value in (("priority", "P4"), ("effort", "XL")):
            self.handoff = copy.deepcopy(original)
            self.handoff["drafts"][0][key]["value"] = value
            self.invalid("invalid option")
        self.handoff = copy.deepcopy(original)
        self.handoff["drafts"][0]["effort"]["reason"] = ""
        self.invalid("nonempty text")

    def test_update_has_no_replacement_body_or_done_when(self):
        draft = self.update()
        draft["proposed_changes"] = ["Propose adding a boundary case; retain all existing requirements."]
        output = MODULE.render(self.manifest, self.handoff)
        self.assertIn("Existing scope and acceptance criteria remain unchanged", output)
        self.assertIn("separate approval required", output)
        self.assertNotIn("**Done when**", output)
        for key, value in (("done_when", ["Replace old criteria"]), ("body", "Replacement issue")):
            draft[key] = value
            self.invalid("unknown")
            del draft[key]

    def test_duplicate_issue_updates_and_dependency_cycles(self):
        first = self.update()
        second = self.add_draft("N2")
        self.invalid("combine updates")
        second["issue"] = "https://github.com/eez-association/eez-rollupX/issues/11"
        first["dependencies"] = ["N2"]
        second["dependencies"] = ["N1"]
        self.invalid("cyclic")
        second["dependencies"] = []
        first["dependencies"] = ["N1"]
        self.invalid("itself")
        first["dependencies"] = ["UNKNOWN"]
        self.invalid("unknown draft or issue")
        first["dependencies"] = ["N2"]
        MODULE.validate(self.manifest, self.handoff)
        self.assertIn("Coordination only; no new closure gates", MODULE.render(self.manifest, self.handoff))

    def test_decisions_are_separate_from_implementation(self):
        draft = self.handoff["drafts"][0]
        draft["kind"] = "decision"
        del draft["issue_type"], draft["evidence_needed"]
        output = MODULE.render(self.manifest, self.handoff)
        self.assertIn("## Human decisions", output)
        self.assertNotIn("## New issue drafts", output)

    def test_cli_init_render_and_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            manifest = directory / "manifest.json"
            handoff = directory / "handoff.json"
            manifest.write_text(json.dumps(self.manifest))
            def run(command):
                return subprocess.run([sys.executable, str(SCRIPT), command, str(manifest), str(handoff)],
                                      text=True, capture_output=True)
            result = run("init")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(run("init").returncode, 1)  # Never overwrites.
            result = run("render")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stdout, "")
            handoff.write_text(json.dumps(self.handoff))
            self.assertEqual(run("check").returncode, 0)
            self.assertEqual(run("render").stdout, MODULE.render(self.manifest, self.handoff))
            for bad in ('{"drafts": [], "drafts": []}', '{"value": NaN}', '[]', '{broken'):
                handoff.write_text(bad)
                result = run("render")
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stdout, "")
                self.assertNotIn("Traceback", result.stderr)
