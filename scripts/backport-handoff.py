#!/usr/bin/env python3
"""Validate a frozen review's draft handoff and render it without GitHub writes."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit


class Invalid(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise Invalid(message)


def obj(value, keys, path):
    require(isinstance(value, dict), f"{path}: expected an object")
    require(set(value) == set(keys),
            f"{path}: expected fields {', '.join(sorted(keys))}; "
            f"missing {sorted(set(keys) - set(value))}, unknown {sorted(set(value) - set(keys))}")


def text(value, path):
    require(isinstance(value, str) and value.strip(), f"{path}: expected nonempty text")


def array(value, path, nonempty=False):
    require(isinstance(value, list), f"{path}: expected an array")
    require(not nonempty or value, f"{path}: must not be empty")


def strings(value, path, nonempty=False):
    array(value, path, nonempty)
    for item in value:
        text(item, path)
    require(len(value) == len(set(value)), f"{path}: duplicate entries")


def identifier(value, path):
    text(value, path)
    require(re.fullmatch(r"[A-Z][A-Z0-9_-]*", value), f"{path}: use IDs such as F1 or N1")


def sha(value, path):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value)
            and value != "0" * 40, f"{path}: expected a full nonzero commit SHA")


def repository(value, path):
    require(isinstance(value, str) and re.fullmatch(r"[\w.-]+/[\w.-]+", value, re.ASCII),
            f"{path}: expected owner/repository")


def issue_url(value, path, target=None):
    require(isinstance(value, str) and re.fullmatch(
        r"https://github\.com/[\w.-]+/[\w.-]+/issues/[1-9][0-9]*", value, re.ASCII),
        f"{path}: expected a GitHub issue URL")
    if target is not None:
        require(value.startswith(f"https://github.com/{target}/issues/"),
                f"{path}: existing issue must belong to the target repository")


def evidence(value, path, baselines, source_required=False):
    array(value, path, nonempty=True)
    pins = set()
    for index, entry in enumerate(value):
        where = f"{path}[{index}]"
        obj(entry, {"label", "url"}, where)
        text(entry["label"], where + ".label")
        text(entry["url"], where + ".url")
        url = urlsplit(entry["url"])
        match = re.fullmatch(r"/([\w.-]+/[\w.-]+)/blob/([0-9a-f]{40})/(.+)", url.path, re.ASCII)
        require(url.scheme == "https" and url.netloc == "github.com" and not url.query
                and match and not re.search(r"\s|[<>]", entry["url"]),
                f"{where}: expected a GitHub blob URL pinned to a full commit SHA")
        sha(match[2], where + ".url")
        pins.add((match[1], match[2]))
    sides = ("source", "target") if source_required else ("target",)
    for side in sides:
        baseline = baselines[side]
        require((baseline["repository"], baseline["commit"]) in pins,
                f"{path}: missing evidence at the frozen {side} revision")


def validate_manifest(manifest):
    obj(manifest, {"version", "baselines", "scope", "findings"}, "manifest")
    require(type(manifest["version"]) is int and manifest["version"] == 1,
            "manifest.version: only version 1 is supported")
    text(manifest["scope"], "manifest.scope")
    baselines = manifest["baselines"]
    obj(baselines, {"source", "target", "merge_base"}, "manifest.baselines")
    sha(baselines["merge_base"], "manifest.baselines.merge_base")
    for side in ("source", "target"):
        value = baselines[side]
        obj(value, {"repository", "commit", "protocol_commit"}, f"baselines.{side}")
        repository(value["repository"], f"baselines.{side}.repository")
        sha(value["commit"], f"baselines.{side}.commit")
        sha(value["protocol_commit"], f"baselines.{side}.protocol_commit")
    array(manifest["findings"], "manifest.findings")
    ids = set()
    for finding in manifest["findings"]:
        obj(finding, {"id", "title", "origin"}, "manifest.finding")
        identifier(finding["id"], "finding.id")
        text(finding["title"], "finding.title")
        require(finding["origin"] in ("source", "target"), "finding.origin: use source or target")
        require(finding["id"] not in ids, f"duplicate finding {finding['id']}")
        ids.add(finding["id"])


def digest(manifest):
    encoded = json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(encoded.encode()).hexdigest()


def validate(manifest, handoff):
    validate_manifest(manifest)
    obj(handoff, {"manifest_sha256", "resolutions", "drafts"}, "handoff")
    require(handoff["manifest_sha256"] == digest(manifest),
            "manifest_sha256: frozen inputs changed or this handoff belongs to another review")
    baselines = manifest["baselines"]
    findings = {entry["id"]: entry for entry in manifest["findings"]}
    array(handoff["resolutions"], "resolutions")
    array(handoff["drafts"], "drafts")
    routes, resolved = {}, set()
    for resolution in handoff["resolutions"]:
        require(isinstance(resolution, dict), "resolution: expected an object")
        action = resolution.get("action")
        require(action in ("drafts", "existing_issue", "no_action"),
                "resolution.action: choose drafts, existing_issue, or no_action; unresolved is not a handoff")
        fields = {"finding", "action", "drafts"} if action == "drafts" else {
            "finding", "action", "reason", "evidence"}
        if action == "existing_issue":
            fields.add("issue")
        obj(resolution, fields, "resolution")
        fid = resolution["finding"]
        identifier(fid, "resolution.finding")
        require(fid in findings, f"unknown finding {fid}")
        require(fid not in resolved, f"duplicate resolution for {fid}")
        resolved.add(fid)
        if action == "drafts":
            strings(resolution["drafts"], f"{fid}.drafts", nonempty=True)
            for did in resolution["drafts"]:
                routes.setdefault(did, []).append(fid)
        else:
            text(resolution["reason"], f"{fid}.reason")
            evidence(resolution["evidence"], f"{fid}.evidence", baselines,
                     findings[fid]["origin"] == "source")
            if action == "existing_issue":
                issue_url(resolution["issue"], f"{fid}.issue", baselines["target"]["repository"])
    require(resolved == set(findings), f"missing findings: {', '.join(sorted(set(findings) - resolved))}")
    drafts = {}
    update_urls = set()
    for draft in handoff["drafts"]:
        require(isinstance(draft, dict), "draft: expected an object")
        did = draft.get("id")
        identifier(did, "draft.id")
        kind = draft.get("kind")
        require(kind in ("implementation", "investigation", "decision"), f"{did}.kind: invalid kind")
        is_update = draft.get("issue") is not None
        fields = {"id", "kind", "issue", "title", "summary", "evidence", "dependencies", "priority", "effort"}
        fields.add("proposed_changes" if is_update else "done_when")
        if not is_update and kind != "decision":
            fields.add("issue_type")
        if kind == "investigation":
            fields.add("evidence_needed")
        obj(draft, fields, did)
        require(did not in drafts, f"duplicate draft {did}")
        require(did in routes, f"{did}: draft is not linked to any manifest finding")
        drafts[did] = draft
        text(draft["title"], f"{did}.title")
        text(draft["summary"], f"{did}.summary")
        evidence(draft["evidence"], f"{did}.evidence", baselines,
                 any(findings[fid]["origin"] == "source" for fid in routes[did]))
        strings(draft["dependencies"], f"{did}.dependencies")
        if is_update:
            issue_url(draft["issue"], f"{did}.issue", baselines["target"]["repository"])
            require(draft["issue"] not in update_urls, f"{did}: combine updates for the same issue")
            update_urls.add(draft["issue"])
            strings(draft["proposed_changes"], f"{did}.proposed_changes")
        else:
            strings(draft["done_when"], f"{did}.done_when", nonempty=True)
            if kind != "decision":
                require(draft["issue_type"] in ("Bug", "Task", "Feature"), f"{did}.issue_type: invalid type")
        if kind == "investigation":
            strings(draft["evidence_needed"], f"{did}.evidence_needed", nonempty=True)
        for field, choices in (("priority", ("P0", "P1", "P2", "P3")), ("effort", ("S", "M", "L", "Unknown"))):
            obj(draft[field], {"value", "reason"}, f"{did}.{field}")
            require(draft[field]["value"] in choices, f"{did}.{field}.value: invalid option")
            text(draft[field]["reason"], f"{did}.{field}.reason")
    require(set(routes) == set(drafts), f"unknown draft references: {sorted(set(routes) - set(drafts))}")
    for did, draft in drafts.items():
        for dependency in draft["dependencies"]:
            if dependency in drafts:
                require(dependency != did, f"{did}: cannot depend on itself")
            else:
                issue_url(dependency, f"{did}.dependencies: unknown draft or issue")
    visiting, visited = set(), set()
    def visit(did):
        require(did not in visiting, f"cyclic draft dependencies at {did}")
        if did in visited:
            return
        visiting.add(did)
        for dependency in drafts[did]["dependencies"]:
            if dependency in drafts:
                visit(dependency)
        visiting.remove(did)
        visited.add(did)
    for did in drafts:
        visit(did)


def link(label, url):
    label = label.replace("\\", "\\\\").replace("[", "\\[").replace("]", "\\]")
    return f"[{label}](<{url}>)"


def render(manifest, handoff):
    validate(manifest, handoff)
    lines = ["# Backport review handoff", "", "Drafts for approval. Structural validation is not technical verification.",
             "", f"Manifest SHA-256: `{digest(manifest)}`", "", "## Frozen scope", "", manifest["scope"], ""]
    for side in ("source", "target"):
        pin = manifest["baselines"][side]
        lines += [f"- {side.title()}: {pin['repository']} at `{pin['commit']}`; protocol `{pin['protocol_commit']}`."]
    lines += [f"- Merge base: `{manifest['baselines']['merge_base']}`.", "", "## Finding coverage", ""]
    titles = {f["id"]: f["title"] for f in manifest["findings"]}
    def add_evidence(entries):
        lines.extend(f"- {link(e['label'], e['url'])}" for e in entries)
    for resolution in handoff["resolutions"]:
        fid = resolution["finding"]
        if resolution["action"] == "drafts":
            outcome = "Drafts: " + ", ".join(resolution["drafts"])
        else:
            destination = link("Existing issue", resolution["issue"]) if resolution["action"] == "existing_issue" else "No action"
            refs = ", ".join(link(e["label"], e["url"]) for e in resolution["evidence"])
            outcome = f"{destination}: {resolution['reason']} Evidence: {refs}"
        lines += [f"- **{fid} — {titles[fid]}:** {outcome}"]
    lines.append("")
    for heading, drafts in (
        ("New issue drafts", [d for d in handoff["drafts"] if d["issue"] is None and d["kind"] != "decision"]),
        ("Additive issue updates", [d for d in handoff["drafts"] if d["issue"] is not None]),
        ("Human decisions", [d for d in handoff["drafts"] if d["issue"] is None and d["kind"] == "decision"]),
    ):
        if not drafts:
            continue
        lines += [f"## {heading}", ""]
        for draft in drafts:
            lines += [f"### {draft['id']} — {draft['title']}", "", f"Kind: {draft['kind']}.", ""]
            if draft["issue"]:
                lines += ["Proposed additive comment for " + link("existing issue", draft["issue"]) + ".",
                          "Existing scope and acceptance criteria remain unchanged. This is not a replacement body.", ""]
            elif "issue_type" in draft:
                lines += [f"Suggested type: {draft['issue_type']}.", ""]
            lines += [draft["summary"], "", "**Evidence**", ""]
            add_evidence(draft["evidence"])
            if "done_when" in draft:
                lines += ["", "**Done when**", ""] + [f"- [ ] {item}" for item in draft["done_when"]]
            if "evidence_needed" in draft:
                lines += ["", "**Investigation evidence needed**", ""] + [f"- {item}" for item in draft["evidence_needed"]]
            if draft.get("proposed_changes"):
                lines += ["", "**Proposed scope/criteria changes — separate approval required**", ""]
                lines += [f"- {item}" for item in draft["proposed_changes"]]
            label = "Coordination only; no new closure gates" if draft["issue"] else "Dependencies"
            deps = [link(item, item) if item.startswith("https://") else item for item in draft["dependencies"]]
            lines += ["", f"**{label}:** " + (", ".join(deps) or "None."), ""]
            for field in ("priority", "effort"):
                value = draft[field]
                lines += [f"Proposed {field}: **{value['value']}** — {value['reason']}", ""]
    return "\n".join(lines).rstrip() + "\n"


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def invalid_constant(value):
    raise Invalid(f"invalid JSON constant: {value}")


def load(path):
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object,
                      parse_constant=invalid_constant)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("init", "check", "render"))
    parser.add_argument("manifest", type=Path)
    parser.add_argument("handoff", type=Path)
    args = parser.parse_args()
    try:
        manifest = load(args.manifest)
        validate_manifest(manifest)
        if args.command == "init":
            handoff = {"manifest_sha256": digest(manifest),
                       "resolutions": [{"finding": f["id"], "action": "unresolved"} for f in manifest["findings"]],
                       "drafts": []}
            with args.handoff.open("x", encoding="utf-8") as stream:
                stream.write(json.dumps(handoff, indent=2) + "\n")
            print(f"Manifest SHA-256: {digest(manifest)}")
            print("Created handoff skeleton. Resolve every finding before check/render.")
        else:
            handoff = load(args.handoff)
            if args.command == "render":
                print(render(manifest, handoff), end="")
            else:
                validate(manifest, handoff)
                print("Handoff structure valid; evidence truth and scope preservation still require judgment.")
    except (ValueError, OSError, RecursionError) as error:
        print(f"Invalid handoff: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
