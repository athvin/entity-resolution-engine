#!/usr/bin/env python3
"""Run complete package suites without shell-interpolating selector output."""
import json
import os
import re
import subprocess


def run(packages, extensions=False):
    if not isinstance(packages, list) or not packages or not all(
        isinstance(p, str) and re.fullmatch(r"[a-z][a-z0-9_-]*", p) for p in packages
    ):
        raise ValueError("expected a nonempty package list")
    core_id = None
    if extensions:
        if "dedupe-core" not in packages:
            raise ValueError("extension tests require dedupe-core in the selection")
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"], text=True))
        core_id, = [p["id"] for p in metadata["packages"] if p["name"] == "dedupe-core"]

    # Retain Cargo's artifact path from THIS feature graph. A second `cargo
    # test -p dedupe-core` changes feature unification and can rebuild DuckDB
    # and its dependents even though this command already built the test.
    command = ["cargo", "test", "--locked", "--message-format=json-render-diagnostics",
               *[arg for p in packages for arg in ("-p", p)]]
    binaries = set()
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True) as process:
        for line in process.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                event = None
            if isinstance(event, dict) and event.get("reason") == "compiler-artifact":
                target = event["target"]
                if (event["package_id"] == core_id and target["name"] == "main"
                        and target["kind"] == ["test"] and event.get("executable")):
                    binaries.add(event["executable"])
            elif not (isinstance(event, dict) and event.get("reason") in (
                "build-script-executed", "build-finished"
            )):
                print(line, end="", flush=True)
        if process.wait():
            raise subprocess.CalledProcessError(process.returncode, command)

    if extensions:
        if len(binaries) != 1:
            raise RuntimeError("expected exactly one built dedupe-core integration binary")
        binary, = binaries
        # Refuse a renamed/empty filter instead of silently running zero tests.
        listed = subprocess.check_output([binary, "extensions::", "--list"], text=True)
        if "extensions::loads_a_community_extension_and_exposes_its_functions: test" not in listed.splitlines():
            raise RuntimeError("community-extension test missing from the built binary")
        print("Running extension tests from the already-built dedupe-core binary", flush=True)
        subprocess.run([binary, "extensions::"], check=True,
                       env={**os.environ, "DEDUPE_E2E_EXTENSIONS": "1"})


if __name__ == "__main__":
    run(json.loads(os.environ["SELECTED_PACKAGES"]), os.environ.get("RUN_EXTENSIONS") == "true")
