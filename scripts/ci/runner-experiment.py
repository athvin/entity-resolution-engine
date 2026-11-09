#!/usr/bin/env python3
"""Compare free runners using the real rest-suite gate and a fresh build target."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def identities(log, artifacts, packages):
    """Attribute libtest names to Cargo targets, including repeated extension tests."""
    targets = {}
    for event in artifacts:
        if (event.get("reason") == "compiler-artifact" and event.get("executable")
                and event["profile"]["test"]):
            target = event["target"]
            targets[Path(event["executable"]).name] = (
                f'{packages[event["package_id"]]}/{target["name"]}/{",".join(target["kind"])}')
    suite = None
    found = []
    for line in log.splitlines():
        running = re.match(r"\s*Running .* \(([^)]+)\)$", line)
        docs = re.match(r"\s*Doc-tests (\S+)$", line)
        test = re.match(r"test (.+) \.\.\. (ok|ignored)(?:,.*)?$", line)
        if running:
            suite = targets[Path(running[1]).name]  # Unknown targets fail closed.
        elif docs:
            suite = "doctest/" + docs[1]
        elif line.startswith("Running extension tests from the already-built"):
            suite = "dedupe-core/main/test"
        elif test:
            if suite is None:
                raise ValueError("test result without a known Cargo target")
            found.append(f"{suite}/{test[1]}:{test[2]}")
    if not found:
        raise ValueError("no test identities found")
    totals = re.findall(r"test result: ok\. (\d+) passed; 0 failed; (\d+) ignored", log)
    if sum(int(passed) + int(ignored) for passed, ignored in totals) != len(found):
        raise ValueError("attributed identities do not cover every reported test")
    return sorted(found)


def reset_target(target, workspace):
    # Keep normal CI's absolute paths so its compiler-cache keys are comparable.
    # main() restricts this cleanup to a disposable GitHub-hosted checkout.
    if (target.is_symlink() or target.parent.is_symlink()
            or target.parent.resolve() / target.name != workspace.resolve() / "rust/target"):
        raise ValueError("refusing to remove anything except this checkout's Rust target")
    if target.exists():
        shutil.rmtree(target)


def validate_comparison(reports):
    if reports[0]["tests"] != reports[1]["tests"]:
        raise ValueError("primed and fresh-target test identities differ")
    stats = reports[1]["cache"]["stats"]
    if stats["cache_read_errors"] or stats["cache_write_errors"]:
        raise ValueError("cache errors invalidate a warm comparison")
    native_hits = stats["cache_hits"]["counts"].get("C/C++", 0)
    native_misses = stats["cache_misses"]["counts"].get("C/C++", 0)
    if not native_hits or native_misses:
        raise ValueError("sample did not reuse every cacheable native compilation")
    return native_hits


def main():
    if os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("priming needs the protected main compiler cache")
    if (os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("EXPERIMENT_HOSTED") != "github-hosted"
            or Path(os.environ["GITHUB_WORKSPACE"]).resolve() != ROOT):
        raise ValueError("full experiment requires its own GitHub-hosted checkout")
    runner_temp = Path(os.environ["RUNNER_TEMP"])
    target = Path(os.environ["CARGO_TARGET_DIR"])
    output = runner_temp / "runner-results"
    output.mkdir(exist_ok=True)
    env = {**os.environ, "RUN_EXTENSIONS": "true", "CARGO_TERM_COLOR": "never", "RUST_TEST_THREADS": "4"}
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"], cwd=ROOT / "rust", text=True))
    package_names = {p["id"]: p["name"] for p in metadata["packages"]}
    selected = sorted(p for p in package_names.values() if p != "er-match")
    env["SELECTED_PACKAGES"] = json.dumps(selected)
    cargo = ["cargo", "test", "--locked", *[arg for p in selected for arg in ("-p", p)]]
    reports = []
    for phase in ("prime", "sample"):
        reset_target(target, ROOT)
        subprocess.run(["sccache", "--zero-stats"], check=True)
        log = output / f"{phase}.log"
        start = time.monotonic()
        with log.open("w") as stream:
            command = [
                "/usr/bin/time", "-v", "-o", str(output / f"{phase}-resources.txt"),
                "python3", str(ROOT / "scripts/ci/run-selected-tests.py"),
            ]
            with subprocess.Popen(command, cwd=ROOT / "rust", env=env, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True) as process:
                for line in process.stdout:
                    stream.write(line)
                    print(line, end="", flush=True)
                if process.wait():
                    raise subprocess.CalledProcessError(process.returncode, command)
        elapsed = time.monotonic() - start
        # Discover the already-built targets AFTER timing; never warm a sample
        # with its own metadata build. The package selection is identical.
        raw = subprocess.check_output(cargo + ["--no-run", "--message-format=json"],
                                      cwd=ROOT / "rust", env=env, text=True)
        artifacts = [json.loads(line) for line in raw.splitlines()]
        if any(e.get("reason") == "compiler-artifact" and not e["fresh"] for e in artifacts):
            raise ValueError("metadata rebuilt artifacts outside the timed gate")
        tests = identities(log.read_text(), artifacts, package_names)
        stats = json.loads(subprocess.check_output(
            ["sccache", "--show-stats", "--stats-format", "json"], text=True))
        report = dict(phase=phase, seconds=round(elapsed, 3), tests=tests, cache=stats)
        (output / f"{phase}.json").write_text(json.dumps(report, indent=2) + "\n")
        reports.append(report)
        print(f"{phase}: {elapsed:.3f}s, {len(tests)} test results", flush=True)
    native_hits = validate_comparison(reports)
    result = dict(runner=os.environ["EXPERIMENT_RUNNER"], arch=platform.machine(),
                  compiler=subprocess.check_output(["rustc", "--version"], text=True).strip(),
                  sha=os.environ["GITHUB_SHA"], prime_seconds=reports[0]["seconds"],
                  seconds=reports[1]["seconds"], test_count=len(reports[1]["tests"]),
                  test_digest=hashlib.sha256(json.dumps(reports[1]["tests"]).encode()).hexdigest(),
                  image=os.environ.get("ImageVersion"), native_hits=native_hits)
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    (output / "cpu.json").write_text(subprocess.check_output(["lscpu", "--json"], text=True))
    print("RUNNER_EXPERIMENT: " + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
