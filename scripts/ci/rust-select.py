#!/usr/bin/env python3
"""Conservative package selection from Cargo's workspace dependency graph."""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def select(paths, metadata, full=False):
    packages = {p["name"]: p for p in metadata["packages"] if p["id"] in metadata["workspace_members"]}
    roots = {str(Path(p["manifest_path"]).parent.relative_to(ROOT)) + "/": name for name, p in packages.items()}
    selected = set()
    for path in paths:
        if path.startswith("rust/"):
            owner = next((name for root, name in roots.items() if path.startswith(root)), None)
            if owner is None or path.endswith(("Cargo.toml", "Cargo.lock")):
                full = True
            else:
                selected.add(owner)
        elif path.startswith("frontend/src/lib/entities/") or path.startswith("frontend/drizzle/meta/"):
            # Covered by entities, independently of the package test selection.
            continue
        elif path.startswith("frontend/"):
            # Every frontend path reaching this selector has been re-admitted
            # by DETECT_COUPLED: server tests embed these contract artifacts.
            selected.add("er-server")
        else:
            full = True
    if full:
        selected = set(packages)
    else:
        while True:
            widened = selected | {name for name, p in packages.items()
                                  if any(d["name"] in selected for d in p["dependencies"])}
            if widened == selected:
                break
            selected = widened
    if not selected <= packages.keys():
        raise ValueError("selected package absent from workspace")
    names = sorted(selected)
    matrix = []
    for label, members in [("er-match", [n for n in names if n == "er-match"]),
                           ("rest", [n for n in names if n != "er-match"])]:
        if members:
            matrix.append({"slice": label, "packages": members})
    return {
        "packages": names,
        # A nonempty matrix parses even when the caller skips the entire job.
        "test_matrix": {"include": matrix or [{"slice": "none", "packages": []}]},
        "lake": "er-cli" in selected,
        "server": bool(selected & {"er-server", "dedupe-cli", "er-entity-diff"}),
        "extensions": "dedupe-core" in selected,
        "full": full,
    }


def main():
    paths = sys.stdin.read().splitlines()
    metadata = {"packages": [], "workspace_members": []}
    if paths and any(paths) or os.environ.get("SELECT_FULL") == "true":
        metadata = json.loads(subprocess.check_output([
            "cargo", "metadata", "--no-deps", "--format-version", "1", "--locked",
            "--manifest-path", str(ROOT / "rust/Cargo.toml"),
        ], text=True))
    paths = [p for p in paths if p]
    result = select(paths, metadata, os.environ.get("SELECT_FULL") == "true")
    for key, value in result.items():
        print(f"{key}={json.dumps(value, separators=(',', ':'))}")


if __name__ == "__main__":
    main()
