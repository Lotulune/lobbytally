#!/usr/bin/env python3
"""Route CI from the complete Git diff; uncertainty always runs normal checks."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys


KNOWN_DOCUMENTS = {
    "README.md", "README.en.md", "AGENTS.md",
    "web/README.md", "e2e-tests/README.md", "design/icon-concepts-v2/README.md",
}
DOCUMENT_IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"}
NORMAL_JOBS = {"web", "quality", "build-linux", "cloud-container"}
FULL_JOBS = {"build", "desktop-smoke", "desktop-e2e"}
COMMIT = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")


def is_document(path: str) -> bool:
    if not path or any(ord(character) < 32 or ord(character) == 127 for character in path):
        return False
    # Git paths are POSIX paths. Do not normalize suspicious or unknown names.
    parts = path.split("/")
    if any(part in {"", ".", ".."} for part in parts) or "\\" in path:
        return False
    if path in KNOWN_DOCUMENTS:
        return True
    if not path.startswith("docs/"):
        return False
    suffix = PurePosixPath(path).suffix
    return suffix == ".md" or (
        path.startswith("docs/images/") and suffix in DOCUMENT_IMAGE_SUFFIXES
    )


def git(repository: Path, *arguments: str) -> bytes:
    return subprocess.run(
        ["git", "-C", str(repository), *arguments],
        check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60,
    ).stdout


def valid_commit(repository: Path, value: object) -> bool:
    if not isinstance(value, str) or not COMMIT.fullmatch(value) or set(value) == {"0"}:
        return False
    git(repository, "cat-file", "-e", f"{value}^{{commit}}")
    return True


def decision(run_checks: bool, reason: str, run_full: bool = False) -> dict[str, str]:
    return {"run_checks": str(run_checks).lower(), "run_full": str(run_full).lower(), "reason": reason}


def route(repository: Path, event_name: str, event: dict, ref: str) -> dict[str, str]:
    if ref.startswith("refs/tags/"):
        return decision(True, "tag", run_full=ref.startswith("refs/tags/v"))
    if event_name == "workflow_dispatch":
        full = event.get("inputs", {}).get("full")
        return decision(True, "manual", run_full=full is True or full == "true")
    if event_name not in {"push", "pull_request"}:
        return decision(True, "unsupported_event")
    try:
        if event_name == "pull_request":
            pull_request = event["pull_request"]
            base = pull_request["base"]["sha"]
            head = pull_request["head"]["sha"]
        else:
            if event.get("created") or event.get("deleted"):
                return decision(True, "new_or_deleted_ref")
            base, head = event.get("before"), event.get("after")
        if not valid_commit(repository, base) or not valid_commit(repository, head):
            return decision(True, "missing_base_or_head")
        if event_name == "pull_request":
            base = git(repository, "merge-base", base, head).decode("ascii").strip()
            if not COMMIT.fullmatch(base):
                return decision(True, "missing_merge_base")
        # -z avoids quoting/splitting filenames; disabling rename detection keeps
        # both the deleted and added paths in scope, including code -> docs moves.
        raw = git(repository, "diff", "--name-only", "--no-renames", "-z", base, head, "--")
        if not raw:
            return decision(True, "empty_diff")
        if not raw.endswith(b"\0"):
            return decision(True, "invalid_diff")
        paths = [path.decode("utf-8") for path in raw[:-1].split(b"\0")]
        if paths and all(is_document(path) for path in paths):
            return decision(False, "documentation_only")
        return decision(True, "code_or_unknown_paths")
    except (KeyError, TypeError, UnicodeError, ValueError, OSError, subprocess.SubprocessError):
        return decision(True, "diff_unavailable")


def failed_results(needs: dict) -> list[str]:
    routing = needs.get("route", {})
    if routing.get("result") != "success":
        return ["CI routing did not succeed"]
    outputs = routing.get("outputs", {})
    normal, full = outputs.get("run_checks"), outputs.get("run_full")
    if normal not in {"true", "false"} or full not in {"true", "false"}:
        return ["CI routing outputs are missing or invalid"]
    if full == "true" and normal != "true":
        return ["Full CI requires normal checks"]
    if normal == "false" and outputs.get("reason") != "documentation_only":
        return ["Only a confirmed documentation-only route may skip normal checks"]
    failures = []
    for job in sorted(NORMAL_JOBS | FULL_JOBS):
        should_run = normal == "true" if job in NORMAL_JOBS else full == "true"
        expected = "success" if should_run else "skipped"
        actual = needs.get(job, {}).get("result", "missing")
        if actual != expected:
            failures.append(f"{job}: expected {expected}, received {actual}")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["route", "result"])
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    arguments = parser.parse_args()
    if arguments.mode == "result":
        try:
            failures = failed_results(json.loads(os.environ["CI_NEEDS"]))
        except (KeyError, TypeError, ValueError):
            failures = ["CI job results are unavailable or invalid"]
        for message in failures:
            print(message, file=sys.stderr)
        if not failures:
            print("All checks required by CI routing succeeded.")
        return int(bool(failures))
    try:
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
        if not isinstance(event, dict):
            raise ValueError("event must be an object")
        output = route(arguments.repository, os.environ.get("GITHUB_EVENT_NAME", ""),
                       event, os.environ.get("GITHUB_REF", ""))
    except (KeyError, OSError, ValueError, TypeError):
        output = decision(True, "event_unavailable")
    print(json.dumps(output, sort_keys=True))
    if destination := os.environ.get("GITHUB_OUTPUT"):
        with open(destination, "a", encoding="utf-8") as handle:
            handle.writelines(f"{key}={value}\n" for key, value in output.items())
    if destination := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(destination, "a", encoding="utf-8") as handle:
            handle.write(f"CI route: **{output['reason']}**. Normal checks: `{output['run_checks']}`; "
                         f"full matrix: `{output['run_full']}`.\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
