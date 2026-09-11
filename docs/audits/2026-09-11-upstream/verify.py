#!/usr/bin/env python3
"""Verify the frozen upstream audit inventory against Git objects, without writes.

This verifies inventory completeness and recorded content identities. It does not
prove that adapted backports are behaviorally equivalent or run project tests.
"""

import argparse
import hashlib
import json
import re
import subprocess
from collections import Counter
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True, help="Rollup repository or bare repository")
    parser.add_argument("--protocol-repo", type=Path, required=True)
    args = parser.parse_args()
    inventory = json.loads(Path(__file__).with_name("inventory.json").read_text())

    def git(*command, repo=args.repo):
        return subprocess.check_output(["git", "-C", str(repo), *command], text=True).strip()

    def require(condition, description):
        if not condition:
            raise SystemExit("FAIL: " + description)

    def tree(ref):
        return {
            row.split("\t", 1)[1]: row.split("\t", 1)[0].split()[2]
            for row in git("ls-tree", "-r", ref).splitlines()
        }

    refs = inventory["refs"]
    base, upstream, fork, head = (refs[key] for key in
                                  ("common_base", "upstream_main", "fork_main", "working_head"))
    require(git("merge-base", fork, upstream) == base, "common base changed")
    require(git("merge-base", fork, head) == fork, "working baseline does not contain fork main")
    expected = set(git("rev-list", fork + ".." + upstream).splitlines())
    commits = inventory["source_commits"]
    require(len(commits) == len(expected), "source commit count")
    require({c["sha"] for c in commits} == expected, "source commit coverage")
    require(inventory["counts"]["upstream_commits_not_in_fork_ancestry"] == len(expected), "reported commit count")
    mainline = git("rev-list", "--first-parent", "--reverse", base + ".." + upstream).splitlines()
    units = inventory["mainline_units"]
    require([u["commit"] for u in units] == mainline, "mainline unit coverage/order")
    require(inventory["counts"]["mainline_units"] == len(units), "reported unit count")
    require(dict(Counter(u["status"] for u in units)) == inventory["counts"]["statuses"], "reported statuses")
    fork_tree, head_tree = tree(fork), tree(head)
    mapped = []
    file_count = 0
    commits_by_sha = {c["sha"]: c for c in commits}
    for unit in units:
        sha, parent = unit["commit"], unit["first_parent"]
        require(git("rev-parse", sha + "^1") == parent, "parent of " + sha)
        owned = git("rev-list", "--reverse", "--topo-order", parent + ".." + sha).splitlines()
        require(owned == unit["source_commits"], "unit ownership for " + sha)
        mapped.extend(owned)
        for commit in owned:
            record = commits_by_sha[commit]
            require(record["upstream_pr"] == unit["pr"], "PR assignment for " + commit)
            require(record["work_item"] == unit["work_item"], "work assignment for " + commit)
            require(record["status"] == unit["status"], "status for " + commit)
            require(record["subject"] == git("show", "-s", "--format=%s", commit), "subject for " + commit)
            require(record["parents"] == git("show", "-s", "--format=%P", commit).split(), "parents for " + commit)
        for backport in unit["backport_commits"]:
            require(git("merge-base", backport, fork) == backport, "backport absent from fork main: " + backport)
        paths = git("diff", "--name-only", "--no-renames", parent, sha).splitlines()
        require([f["path"] for f in unit["files"]] == paths, "file coverage for " + sha)
        old, new = tree(parent), tree(sha)
        for file in unit["files"]:
            path = file["path"]
            require(bool(file["action"]), "unassigned file " + path)
            for key, source in (("before_blob", old), ("after_blob", new),
                                ("fork_main_blob", fork_tree), ("working_head_blob", head_tree)):
                require(file[key] == source.get(path), key + " for " + path)
            for key, source in (("fork_main_relation", fork_tree), ("working_head_relation", head_tree)):
                relation = (("same-blob" if path in new else "absent-in-both")
                            if new.get(path) == source.get(path) else
                            ("different-blob" if path in source else "absent-in-target"))
                require(file[key] == relation, key + " for " + path)
            patch = git("diff", "--no-renames", "--no-ext-diff", "--no-textconv", "--binary",
                        "--full-index", "--no-color", "--unified=3", "--diff-algorithm=myers",
                        "--no-indent-heuristic", "--src-prefix=a/", "--dst-prefix=b/",
                        parent, sha, "--", path)
            require(file["patch_sha256"] == hashlib.sha256(patch.encode()).hexdigest(), "patch for " + path)
            require(file["hunks"] == len(re.findall(r"^@@ ", patch, re.M)), "hunk count for " + path)
        file_count += len(paths)
    require(len(mapped) == len(set(mapped)) == len(expected), "duplicate or unassigned commit")
    require(file_count == inventory["counts"]["file_change_records"], "file-change count")
    require(sum(len(c["parents"]) < 2 for c in commits) == inventory["counts"]["upstream_nonmerge_commits"],
            "nonmerge commit count")
    net = inventory["net_changed_files"]
    require([f["path"] for f in net] == git("diff", "--name-only", "--no-renames", base, upstream).splitlines(),
            "net changed file coverage")
    require(len(net) == inventory["counts"]["net_changed_paths"], "net changed path count")
    base_tree, upstream_tree = tree(base), tree(upstream)
    for file in net:
        for key, source in (("base_blob", base_tree), ("upstream_blob", upstream_tree),
                            ("fork_main_blob", fork_tree), ("working_head_blob", head_tree)):
            require(file[key] == source.get(file["path"]), "net " + key + " for " + file["path"])
    protocol = inventory["protocol"]
    for key, source in (("fork_main_pin", fork_tree), ("working_head_pin", head_tree),
                        ("upstream_rollup_pin", upstream_tree)):
        require(protocol[key] == source["eez-core-protocol"], "protocol " + key)
    for ref_key, tree_key in (("live_main", "live_main_tree"), ("working_head_pin", "working_head_tree")):
        require(git("rev-parse", protocol[ref_key] + "^{tree}", repo=args.protocol_repo) == protocol[tree_key], tree_key)
    require(protocol["live_main_tree"] == protocol["working_head_tree"], "protocol content parity")
    protocol_range = protocol["fork_main_pin"] + ".." + protocol["working_head_pin"]
    require([c["sha"] for c in protocol["pending_content_commits"]] ==
            git("rev-list", "--reverse", protocol_range, repo=args.protocol_repo).splitlines(), "protocol commit coverage")
    require(protocol["pending_files"] == git("diff", "--name-only", protocol["fork_main_pin"],
            protocol["working_head_pin"], repo=args.protocol_repo).splitlines(), "protocol file coverage")
    test_names = []
    for path in sorted(upstream_tree):
        if not path.endswith(".rs") or not (path.startswith("crates/eez-node-e2e/tests/")
                                            or path.startswith("crates/eez-testkit/")):
            continue
        source = git("show", upstream + ":" + path)
        old_path = path.replace("crates/eez-node-e2e/tests/", "crates/eez-node/tests/")
        local = git("show", head + ":" + old_path) if old_path in head_tree else ""
        for match in re.finditer(r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*(?:async\s+)?fn\s+(\w+)\(", source):
            test_names.append((path, match.group(1),
                               bool(re.search(r"\bfn\s+" + re.escape(match.group(1)) + r"\(", local))))
    require(test_names == [(t["path"], t["name"], t["same_name_in_working_tree"])
                           for t in inventory["upstream_e2e_and_testkit_tests"]], "test-name coverage")
    require(len(test_names) == inventory["counts"]["e2e_and_testkit_test_names"], "test-name count")
    for field, count in (("upstream_branches", "upstream_branches"), ("open_upstream_prs", "open_upstream_prs"),
                         ("side_merged_prs", "merged_prs_without_merge_sha_in_main")):
        require(len(inventory[field]) == inventory["counts"][count], "reported " + count)
    print(f"PASS: {len(expected)} commits, {len(units)} mainline units, {file_count} file-change records, "
          f"{len(net)} net paths, and protocol content parity verified.")
    print("Historical inventory verified; runtime equivalence and live ref freshness require separate checks.")


if __name__ == "__main__":
    main()
