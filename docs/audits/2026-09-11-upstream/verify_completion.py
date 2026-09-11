#!/usr/bin/env python3
"""Verify final backport coverage and pinned code identities without modifying Git.

This checks the completion ledger, not behavioral equivalence. Run verify.py
separately for the original upstream history/patch inventory.
"""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument(
        "--revision", default="HEAD",
        help="Revision to compare with the frozen audit; select an older commit explicitly to verify historical evidence.",
    )
    args = parser.parse_args()
    directory = Path(__file__).parent
    inventory_bytes = (directory / "inventory.json").read_bytes()
    inventory = json.loads(inventory_bytes)
    completion = json.loads((directory / "completion.json").read_text())

    def git(*command):
        return subprocess.check_output(
            ["git", "-C", str(args.repo), *command], text=True
        ).strip()

    def require(condition, message):
        if not condition:
            raise SystemExit("FAIL: " + message)

    code = completion["code_revision"]
    tip = git("rev-parse", args.revision)
    require(
        hashlib.sha256(inventory_bytes).hexdigest() == completion["frozen_inventory_sha256"],
        "frozen source inventory changed",
    )
    require(completion["upstream_main"] == inventory["refs"]["upstream_main"], "upstream revision")
    require(git("merge-base", code, tip) == code, "code revision absent from selected revision")
    base = completion["integration_base"]
    require(git("merge-base", base, code) == base, "integration baseline absent from code")
    require(git("rev-parse", code + ":eez-core-protocol") == completion["protocol_pin"], "protocol pin")
    require(
        not git("diff", "--name-only", code, tip, "--", ".", ":(exclude)docs/audits"),
        "code changed after the recorded validation revision; update the completion audit",
    )
    if args.revision == "HEAD":
        require(
            not git("diff", "--name-only", "HEAD", "--", ".", ":(exclude)docs/audits"),
            "working-tree code differs from HEAD; frozen validation does not cover these changes",
        )
    tree = {
        row.split("\t", 1)[1]: row.split("\t", 1)[0].split()[2]
        for row in git("ls-tree", "-r", code).splitlines()
    }
    upstream_paths = set(git("ls-tree", "-r", "--name-only", completion["upstream_main"]).splitlines())
    source_units = inventory["mainline_units"]
    units = completion["mainline_units"]
    require(
        [u["upstream_commit"] for u in units] == [u["commit"] for u in source_units],
        "mainline unit coverage/order",
    )
    file_count = 0
    commit_count = 0
    for source, unit in zip(source_units, units, strict=True):
        require(unit["pr"] == source["pr"], "PR identity")
        require(unit["source_commits"] == source["source_commits"], "source commit coverage")
        require(unit["disposition"] == "implemented" and unit["adaptation"], "unit disposition")
        require(unit["implementation_commits"], "missing implementation provenance")
        for commit in unit["implementation_commits"]:
            require(git("merge-base", commit, code) == commit, "implementation absent: " + commit)
        require(
            [f["source_path"] for f in unit["files"]] == [f["path"] for f in source["files"]],
            "file coverage for PR " + str(unit["pr"]),
        )
        for original, final in zip(source["files"], unit["files"], strict=True):
            require(final["source_patch_sha256"] == original["patch_sha256"], "source patch identity")
            require(final["source_hunks"] == original["hunks"], "source hunk count")
            require(final["rationale"], "missing per-file rationale")
            targets = final["target_files"]
            if not targets:
                require(final["disposition"] == "removed-by-final-upstream", "unmapped source file")
                require(final["source_path"] not in tree, "obsolete path remains")
                require(final["source_path"] not in upstream_paths, "removed path still required upstream")
            else:
                require(
                    final["disposition"] in {
                        "relocated-or-split", "same-content-as-upstream", "adapted-for-fork"
                    },
                    "unknown file disposition",
                )
                require(len({t["path"] for t in targets}) == len(targets), "duplicate target path")
                for target in targets:
                    require(tree.get(target["path"]) == target["blob"], "target identity: " + target["path"])
            file_count += 1
        commit_count += len(unit["source_commits"])

    expected_tests = {(t["path"], t["name"]) for t in inventory["upstream_e2e_and_testkit_tests"]}
    mapped_tests = completion["test_mapping"]
    require(len(mapped_tests) == len(expected_tests), "mapped test count")
    require(
        {(t["upstream_path"], t["upstream_test"]) for t in mapped_tests} == expected_tests,
        "upstream test coverage",
    )
    for test in mapped_tests:
        source = git("show", code + ":" + test["target_path"])
        require(
            re.search(r"\bfn\s+" + re.escape(test["target_test"]) + r"\s*\(", source),
            "missing mapped test " + test["target_test"],
        )
        require(test["rationale"], "missing test adaptation rationale")
    require(
        completion["counts"] == {
            "mainline_units": len(units), "source_commits": commit_count,
            "file_change_records": file_count, "mapped_upstream_tests": len(mapped_tests),
        },
        "reported coverage counts",
    )
    require(completion["validation"], "missing validation record")
    for validation in completion["validation"]:
        require(validation["command"] and validation["result"], "incomplete validation record")
    runtime = completion["runtime_validation_environment"]
    frozen = runtime["runtime_source_revision"]
    require(git("merge-base", frozen, code) == frozen, "frozen runtime revision absent")
    require(
        not git(
            "diff", "--name-only", frozen, code, "--", ".",
            ":(exclude)docs/audits", ":(exclude)crates/eez-testkit",
            ":(exclude)crates/eez-node-e2e",
        ),
        "runtime sources changed after the tested binaries were frozen",
    )
    integration = completion["integration_validation"]
    require(integration["missing"] == [], "unexecuted integration tests")
    require(integration["passed"] == integration["expected"] == 44, "integration coverage count")
    require(len(integration["tests"]) == 44, "integration evidence count")
    require(len({test["test"] for test in integration["tests"]}) == 44, "duplicate integration evidence")
    for test in integration["tests"]:
        require(test["result"] == "PASS on first attempt", "integration failure or flake")
        require(git("merge-base", test["code_revision"], code) == test["code_revision"], "test revision absent")
        suite, name = test["test"].split()
        require(suite.startswith("eez-node-e2e::"), "unexpected integration suite")
        path = "crates/eez-node-e2e/tests/" + suite.split("::", 1)[1] + ".rs"
        require(re.search(r"\bfn\s+" + re.escape(name) + r"\s*\(", git("show", code + ":" + path)), "missing integration test")
        require(re.fullmatch(r"[0-9a-f]{64}", test["log_sha256"]), "missing integration log identity")
    print(
        f"PASS: {len(units)} units, {commit_count} commits, {file_count} file records, "
        f"{len(mapped_tests)} test mappings; code {code[:12]}"
    )
    print(f"Snapshot coverage verified through {tip[:12]}; later code is not covered by this evidence.")
    print("See completion.md for observed checks and limits.")


if __name__ == "__main__":
    main()
