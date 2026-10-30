#!/usr/bin/env python3
"""Classify changed paths into conservative CI routes."""
import argparse
import json
from pathlib import PurePosixPath
import subprocess

DOC_FILES = frozenset({
    "README.md",
    "README.zh-CN.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "CODE_OF_CONDUCT.md",
    "SECURITY.md",
    "ACCESSIBILITY.md",
    "BUILDING.md",
    ".github/pull_request_template.md",
})
DOC_EXTENSIONS = frozenset({".md", ".yml", ".yaml"})


def is_documentation(path):
    path = str(PurePosixPath(path.replace("\\", "/"))).removeprefix("./")
    if path in DOC_FILES:
        return True
    if path.startswith(".trellis/spec/"):
        return path.endswith(".md")
    if path.startswith(".github/ISSUE_TEMPLATE/"):
        suffix = PurePosixPath(path).suffix.lower()
        return "/" not in path[len(".github/ISSUE_TEMPLATE/"):] and suffix in DOC_EXTENSIONS
    return False


def classify_paths(paths):
    paths = tuple(paths)
    docs_changed = any(is_documentation(path) for path in paths)
    code_paths = tuple(path for path in paths if not is_documentation(path))
    if not code_paths and paths:
        scope = "docs"
    elif code_paths and all(path.startswith("gui/") for path in code_paths):
        scope = "gui"
    elif code_paths and all(path.startswith("core-cli/") for path in code_paths):
        scope = "cli"
    else:
        scope = "full"
    return {
        "scope": scope,
        "docs_changed": docs_changed,
        "run_cli": scope in {"cli", "full"},
        "run_gui": scope in {"gui", "full"},
    }


def changed_paths(event_name, event):
    if event_name == "pull_request":
        pull_request = event["pull_request"]
        base, head = pull_request["base"]["sha"], pull_request["head"]["sha"]
        revision = f"{base}...{head}"
    elif event_name == "push":
        base, head = event["before"], event["after"]
        if not base or set(base) == {"0"}:
            return None
        revision = f"{base}..{head}"
    else:
        return None
    result = subprocess.run(
        ["git", "diff", "--no-renames", "--name-only", "-z", revision],
        check=True,
        stdout=subprocess.PIPE,
    )
    return [path.decode("utf-8") for path in result.stdout.split(b"\0") if path]

def classify_event(event_name, event):
    paths = changed_paths(event_name, event)
    return classify_paths(paths) if paths is not None else {
        "scope": "full",
        "docs_changed": False,
        "run_cli": True,
        "run_gui": True,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--event-name", required=True)
    parser.add_argument("--event-path", required=True)
    parser.add_argument("--github-output", required=True)
    args = parser.parse_args()

    with open(args.event_path, encoding="utf-8") as source:
        event = json.load(source)
    result = classify_event(args.event_name, event)
    with open(args.github_output, "a", encoding="utf-8") as output:
        for name, value in result.items():
            if isinstance(value, bool):
                value = str(value).lower()
            output.write(f"{name}={value}\n")


if __name__ == "__main__":
    main()
