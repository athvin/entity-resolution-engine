#!/usr/bin/env python3
"""Partition every server integration test and overlap the real rotation wait."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
ROTATION = "pg_satoken::the_projected_token_binds_overrides_and_refuses"
PREFIXES = tuple("pg_" + letter for letter in "abcdefghijkl")


def partition(names):
    if not names or len(set(names)) != len(names) or ROTATION not in names:
        raise ValueError("expected unique integration tests including the real rotation test")
    early = [n for n in names if n != ROTATION and
             ("pg_" not in n or any(prefix in n for prefix in PREFIXES))]
    late = [n for n in names if n != ROTATION and n not in early]
    if not early or not late:
        raise ValueError("both server slices must contain tests")
    assert set(early + late + [ROTATION]) == set(names)
    return early, late


def build():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
        cwd=ROOT / "rust", text=True))
    package, = [p for p in metadata["packages"] if p["name"] == "er-server"]
    raw = subprocess.check_output([
        "cargo", "test", "--locked", "-p", "er-server", "--test", "main", "--no-run",
        "--message-format=json-render-diagnostics",
    ], cwd=ROOT / "rust", text=True)
    binaries = set()
    for line in raw.splitlines():
        event = json.loads(line)
        if (event.get("reason") == "compiler-artifact" and event["package_id"] == package["id"]
                and event["target"]["name"] == "main" and event["target"]["kind"] == ["test"]
                and event.get("executable")):
            binaries.add(event["executable"])
    if len(binaries) != 1:
        raise ValueError("expected exactly one built er-server integration executable")
    return binaries.pop(), Path(package["manifest_path"]).parent


def run(suite):
    if not os.environ.get("ER_TEST_PG_DSN") or os.environ.get("DEDUPE_E2E_DUCKLAKE") != "1":
        raise ValueError("server integration gate requires real Postgres and DuckLake")
    binary, cwd = build()
    listed = subprocess.check_output([binary, "--list"], cwd=cwd, text=True)
    names = re.findall(r"^(.+): test$", listed, re.M)
    early, late = partition(names)
    # The rotation case owns its database and HTTP listeners, and shares no
    # lake fixtures with the other process. Start its real 61-second wait now,
    # instead of near the end of libtest's alphabetical queue. Other cases
    # stay together to retain their process-local fixture coordination.
    groups = [[ROTATION], early] if suite == "pg-al" else [late]
    started = time.monotonic()
    processes = []
    # libtest prints a singleton's name before its result. Buffer complete
    # lines per process so another group's output cannot split that identity.
    def relay(stream):
        with stream:
            for line in stream:
                print(line, end="", flush=True)

    with ThreadPoolExecutor(max_workers=len(groups)) as readers:
        output = []
        try:
            for group in groups:
                workers = 1 if group == [ROTATION] else 4
                print(f"Running {suite}: {len(group)} integration tests ({workers} workers)", flush=True)
                process = subprocess.Popen(
                    [binary, "--exact", f"--test-threads={workers}", *group], cwd=cwd,
                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                processes.append(process)
                output.append(readers.submit(relay, process.stdout))
            codes = [process.wait() for process in processes]
            for reader in output:
                reader.result()
            if any(codes):
                raise RuntimeError(f"server integration group failed: {codes}")
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    process.wait()
    print(f"SERVER_TESTS: {suite}, {sum(map(len, groups))} selected, "
          f"{time.monotonic() - started:.3f}s execution wall", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=["pg-al", "pg-mz"])
    run(parser.parse_args().suite)
