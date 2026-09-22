"""Matched CLI lifecycle comparisons using immutable images and fresh Compose stacks."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import math
import os
import shutil
import statistics
import subprocess
import threading
import time
import uuid
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def output(command: list[str]) -> str:
    return subprocess.check_output(command, text=True).strip()


def worker(args: argparse.Namespace) -> None:
    from profile_pipeline import run_case
    from profile_report import write_report

    args.out.mkdir(parents=True, exist_ok=True)
    with (args.out / "dbt-deps.log").open("w") as log:
        subprocess.run(
            ["dbt", "deps", "--project-dir", "dbt"],
            stdout=log,
            stderr=subprocess.STDOUT,
            check=True,
        )
    result = run_case(
        args.out,
        args.case,
        args.iteration,
        detailed=args.profile_mode == "detailed",
        label="run",
        corpus_root=args.corpus_root,
        compare_outputs=True,
    )
    result.update(variant=args.variant, profile_mode=args.profile_mode)
    fingerprint = result["fingerprint"]
    expected = {
        "cgroup_cpu_max": "2.0",
        "cgroup_memory_max": 6 * 1024**3,
        "er_duckdb_threads": 2,
        "er_duckdb_memory_limit": "4GB",
    }
    for key, value in expected.items():
        if fingerprint[key] != value:
            raise AssertionError(f"NON_COMPARABLE: {key}: {fingerprint[key]} != {value}")
    (args.out / "run/result.json").write_text(json.dumps(result, indent=2))
    write_report(args.out)


def capture_services(compose: list[str], destination: Path, stop: threading.Event) -> None:
    with destination.open("w") as handle:
        while not stop.is_set():
            found = subprocess.run([*compose, "ps", "-q"], capture_output=True, text=True)
            ids = found.stdout.split()
            if ids:
                sampled = subprocess.run(
                    ["docker", "stats", "--no-stream", "--format", "{{json .}}", *ids],
                    capture_output=True,
                    text=True,
                )
                for line in sampled.stdout.splitlines():
                    sample = json.loads(line)
                    sample["observed_at_ns"] = time.time_ns()
                    handle.write(json.dumps(sample) + "\n")
                handle.flush()
            stop.wait(1)


def trial(args: argparse.Namespace, variant: str, case: str, iteration: int, mode: str) -> Path:
    free = shutil.disk_usage(args.out).free
    minimum = (8 if case == "10k" else 4) * 1024**3
    if free < minimum:
        raise RuntimeError(
            f"{case} needs {minimum / 1024**3:g} GiB free; have {free / 1024**3:.2f}"
        )
    name = f"{case}-{variant}-{mode}-{iteration}"
    directory = args.out / "trials" / name
    image = getattr(args, f"{variant}_image")
    digest = output(["docker", "image", "inspect", image, "--format", "{{.Id}}"])
    existing = directory / "run/result.json"
    if args.resume and existing.exists():
        previous = json.loads(existing.read_text())
        if previous["status"] != "succeeded":
            raise RuntimeError(f"preserve the failed trial before retrying: {directory}")
        if previous["fingerprint"]["image_digest"] != digest:
            raise RuntimeError(f"cannot resume with a different image: {directory}")
        print(f"[compare] retained successful trial {name}", flush=True)
        return existing
    directory.mkdir(parents=True)
    source_sha = output(
        [
            "docker",
            "image",
            "inspect",
            image,
            "--format",
            '{{ index .Config.Labels "er.source_sha256" }}',
        ]
    )
    # Both images execute the same measurement harness, mounted read-only.
    override = {
        "services": {
            service: {"image": digest} for service in ("benchmark", "pipeline", "catalog-init")
        }
    }
    override["services"]["benchmark"]["volumes"] = [
        f"{args.out / 'harness'}:/app/benchmarks:ro",
    ]
    override_path = directory / "compose.json"
    override_path.write_text(json.dumps(override))
    project = f"er-perf-{uuid.uuid4().hex[:12]}"
    compose = [
        "docker",
        "compose",
        "-p",
        project,
        "-f",
        str(ROOT / "docker/compose.yaml"),
        "-f",
        str(override_path),
        "--profile",
        "bench",
    ]
    container_out = "/app/" + str(directory.relative_to(ROOT))
    container_corpus = "/app/" + str((args.out / "inputs").relative_to(ROOT))
    print(f"[compare] {name}; image={digest}; free={free / 1024**3:.1f} GiB", flush=True)
    stop = threading.Event()
    monitor = threading.Thread(
        target=capture_services,
        args=(compose, directory / "services-resources.jsonl", stop),
        daemon=True,
    )
    monitor.start()
    started = time.monotonic()
    try:
        with (directory / "console.log").open("w") as log:
            subprocess.run(
                [
                    *compose,
                    "run",
                    "--rm",
                    "-e",
                    f"ER_IMAGE_DIGEST={digest}",
                    "-e",
                    f"ER_GIT_SHA={args.git_sha}",
                    "-e",
                    f"ER_SOURCE_SHA={source_sha}",
                    "benchmark",
                    "python",
                    "benchmarks/performance.py",
                    "--worker",
                    "--out",
                    container_out,
                    "--corpus-root",
                    container_corpus,
                    "--case",
                    case,
                    "--variant",
                    variant,
                    "--profile-mode",
                    mode,
                    "--iteration",
                    str(iteration),
                ],
                stdout=log,
                stderr=subprocess.STDOUT,
                check=True,
                env={
                    **os.environ,
                    "ER_CPU_LIMIT": "2",
                    "ER_MEM_LIMIT": "6g",
                    "ER_DUCKDB_MEMORY_LIMIT": "4GB",
                },
            )
    finally:
        stop.set()
        monitor.join(timeout=10)
        with (directory / "services.log").open("w") as log:
            subprocess.run([*compose, "logs", "--no-color"], stdout=log, stderr=subprocess.STDOUT)
        subprocess.run(
            [*compose, "down", "-v", "--remove-orphans"], capture_output=True, check=True
        )
    result = json.loads((directory / "run/result.json").read_text())
    print(
        f"[compare] {name}: processing={result['processing_ms'] / 1000:.3f}s; "
        f"trial including setup={time.monotonic() - started:.1f}s",
        flush=True,
    )
    return directory / "run/result.json"


def compare_outputs(first: Path, second: Path) -> dict[str, Any]:
    a, b = (json.loads(path.read_text()) for path in (first, second))
    fields = [
        f"{phase}_{field}"
        for phase in ("base", "batch")
        for field in ("counts", "partition_hash", "semantic_hashes")
    ]
    differences = [
        field
        for field in [*fields, "input_sha256", "semantic_config_sha256"]
        if a[field] != b[field]
    ]
    for field in (
        "duckdb_version",
        "splink_version",
        "dbt_core_version",
        "dbt_duckdb_version",
        "ducklake_extension_version",
        "cgroup_cpu_max",
        "cgroup_memory_max",
        "er_duckdb_threads",
        "er_duckdb_memory_limit",
    ):
        if a["fingerprint"][field] != b["fingerprint"][field]:
            differences.append(f"fingerprint.{field}")
    max_score_delta = 0.0
    for phase in ("base", "batch"):
        with gzip.open(first.parent / f"{phase}-scores.json.gz", "rt") as handle:
            left = json.load(handle)
        with gzip.open(second.parent / f"{phase}-scores.json.gz", "rt") as handle:
            right = json.load(handle)
        if len(left) != len(right):
            differences.append(f"{phase}_score_count")
            continue
        for old, new in zip(left, right, strict=True):
            if old[:2] != new[:2] or old[3:] != new[3:]:
                differences.append(f"{phase}_score_identity")
                break
            if not all(math.isfinite(value) for value in (old[2], new[2])):
                differences.append(f"{phase}_nonfinite_score")
                break
            delta = abs(old[2] - new[2])
            max_score_delta = max(max_score_delta, delta)
            if delta > 1e-10:
                differences.append(f"{phase}_score_probability")
                break
    return {
        "baseline": str(first),
        "candidate": str(second),
        "differences": differences,
        "max_score_delta": max_score_delta,
        "matching": not differences,
    }


def processing_resources(path: Path, run: dict[str, Any]) -> dict[str, Any]:
    samples = [
        json.loads(line) for line in (path.parent / "resources.jsonl").read_text().splitlines()
    ]
    commands = [entry for entry in run["commands"] if entry["phase"] != "setup"]
    cpu = throttle = 0.0
    peaks: list[int] = []
    storage: dict[str, list[int]] = {"spill_bytes": [], "parquet_scratch_bytes": []}
    for command in commands:
        start, end = command["started_ns"], command["ended_ns"]
        selected = [s for s in samples if start <= s["monotonic_ns"] <= end]
        peaks.extend(s["memory.current"] for s in selected if s.get("memory.current") is not None)
        for key, values in storage.items():
            values.extend(s[key] for s in selected if key in s)
        if "cpu" in command:
            if any(command["cpu"].get(key) is None for key in ("usage_usec", "throttled_usec")):
                raise AssertionError(f"command CPU counters unavailable: {path}")
            cpu += command["cpu"]["usage_usec"] / 1e6
            throttle += command["cpu"]["throttled_usec"] / 1e6
            continue
        if len(selected) < 2 or any(s.get("usage_usec") is None for s in selected):
            raise AssertionError(f"CPU sampling unavailable for {command['command']} in {path}")
        cpu += (selected[-1]["usage_usec"] - selected[0]["usage_usec"]) / 1e6
        throttle += (selected[-1]["throttled_usec"] - selected[0]["throttled_usec"]) / 1e6
    if not peaks:
        raise AssertionError(f"processing memory samples unavailable: {path}")
    return {
        "cpu_s": cpu,
        "throttled_s": throttle,
        "sampled_memory_peak_bytes": max(peaks) if peaks else None,
        **{f"sampled_{key}_peak": max(values) for key, values in storage.items() if values},
    }


def variation(values: list[float]) -> float:
    return statistics.pstdev(values) / statistics.mean(values) if len(values) > 1 else 0.0


def detailed_metrics(path: Path) -> dict[str, Any]:
    from profile_report import STAGES, summarize

    summary = summarize(path.parent)
    windows = {
        entry["invocation_id"]: (entry["started_ns"], entry["ended_ns"])
        for entry in summary["run"]["commands"]
        if entry["phase"] != "setup"
    }

    def processing_event(event: dict[str, Any]) -> bool:
        # The worker retains its last invocation ID while validating outputs.
        # Identity alone therefore cannot distinguish processing from validation.
        window = windows.get(event.get("invocation_id"))
        return window is not None and window[0] <= event["monotonic_ns"] <= window[1]

    spans = [event for event in summary["spans"] if processing_event(event)]
    names = ("command.import", "command.preflight", "lake.configure_attach", "dbt.invocation")
    return {
        "sql_profiles": sum(processing_event(query) for query in summary["queries"]),
        "connection_opens": sum(event["name"] == "lake.open" for event in spans),
        "dbt_invocations": sum(event["name"] == "dbt.invocation" for event in spans),
        "timings_s": {
            name: sum(event["duration_ms"] for event in spans if event["name"] == name) / 1000
            for name in names
        },
        "stages": [
            {
                "phase": event["phase"],
                "name": event["name"],
                "duration_s": event["duration_ms"] / 1000,
                "cpu_s": event["metrics"]["usage_usec"] / 1e6,
                "memory_bytes": event["sampled_memory_peak_bytes"],
                "rows_in": event["metrics"].get("rows_in"),
                "rows_out": event["metrics"].get("rows_out"),
                "input_unit": event["metrics"].get("input_unit"),
                "output_unit": event["metrics"].get("output_unit"),
            }
            for event in spans
            if event["name"] in STAGES
        ],
    }


def report(out: Path) -> dict[str, Any]:
    paths = sorted(out.glob("trials/*/run/result.json"))
    runs = [(path, json.loads(path.read_text())) for path in paths]
    checks = []
    for case in ("tiny", "smoke", "10k"):
        selected = [
            (path, run)
            for path, run in runs
            if run["case"] == case and run["status"] == "succeeded"
        ]
        if selected:
            reference = next(
                (path for path, run in selected if run["variant"] == "baseline"), selected[0][0]
            )
            checks.extend(
                compare_outputs(reference, path) for path, _ in selected if path != reference
            )
    measurements: dict[str, Any] = {}
    for variant in ("baseline", "candidate"):
        selected = [
            (path, run)
            for path, run in runs
            if run["case"] == "10k"
            and run.get("profile_mode") == "standard"
            and run["variant"] == variant
            and run["status"] == "succeeded"
        ]
        values = [run["processing_ms"] / 1000 for _, run in selected]
        if not values:
            continue
        phases: dict[str, list[float]] = {}
        resources = [processing_resources(path, run) for path, run in selected]
        for _, run in selected:
            totals: dict[str, float] = {}
            for command in run["commands"]:
                for stage in command["stages"]:
                    if command["phase"] == "setup":
                        continue
                    name = f"{command['phase']} / {stage['stage']}"
                    totals[name] = totals.get(name, 0) + stage["duration_ms"] / 1000
            for name, seconds in totals.items():
                phases.setdefault(name, []).append(seconds)
            phases.setdefault("outside stage ledger", []).append(
                run["processing_ms"] / 1000 - sum(totals.values())
            )
        measurements[variant] = {
            "seconds": values,
            "median_s": statistics.median(values),
            "cv": variation(values),
            "cpu_median_s": statistics.median(r["cpu_s"] for r in resources),
            "throttled_median_s": statistics.median(r["throttled_s"] for r in resources),
            "peak_memory_median_bytes": statistics.median(
                r["sampled_memory_peak_bytes"] for r in resources
            ),
            "phases": {key: statistics.median(value) for key, value in phases.items()},
        }
    result: dict[str, Any] = {"measurements": measurements, "output_checks": checks}
    lines = [
        "# Pipeline performance comparison",
        "",
        "Fresh Compose stacks, identical inputs, 2 CPUs, 6 GiB container memory, "
        "4 GB DuckDB limit. Processing includes all CLI command lifetimes; initialization, "
        "corpus generation, correctness checks and report generation are excluded.",
        "",
        "Normal stage logging and 250 ms resource sampling remain enabled during standard "
        "runs. Detailed SQL runs are separate and do not contribute to the headline speedup.",
        "",
        "| Version | Runs | Median s | Range s | CV | CPU s | Memory peak MiB |",
        "|---|---:|---:|---|---:|---:|---:|",
    ]
    for variant, data in measurements.items():
        lines.append(
            f"| {variant} | {len(data['seconds'])} | {data['median_s']:.3f} | "
            f"{min(data['seconds']):.3f}–{max(data['seconds']):.3f} | {100 * data['cv']:.2f}% | "
            f"{data['cpu_median_s']:.3f} | {data['peak_memory_median_bytes'] / 1024**2:.1f} |"
        )
    if len(measurements) == 2:
        a, b = (measurements[name] for name in ("baseline", "candidate"))
        result.update(
            runtime_reduction_percent=100 * (1 - b["median_s"] / a["median_s"]),
            speedup=a["median_s"] / b["median_s"],
            seconds_saved=a["median_s"] - b["median_s"],
            cpu_change_percent=100 * (b["cpu_median_s"] / a["cpu_median_s"] - 1),
            memory_change_percent=100
            * (b["peak_memory_median_bytes"] / a["peak_memory_median_bytes"] - 1),
        )
        lines += [
            "",
            f"**Runtime reduction: {result['runtime_reduction_percent']:.1f}%; "
            f"speedup: {result['speedup']:.2f}×; saved: {result['seconds_saved']:.2f}s.**",
            "",
            f"CPU change: {result['cpu_change_percent']:+.1f}%; "
            f"sampled memory peak change: {result['memory_change_percent']:+.1f}%.",
            "",
            "| Stage | Baseline s | Candidate s | Saved s | Reduction |",
            "|---|---:|---:|---:|---:|",
        ]
        for stage, before in a["phases"].items():
            after = b["phases"].get(stage, 0)
            lines.append(
                f"| {stage} | {before:.3f} | {after:.3f} | {before - after:.3f} | "
                f"{100 * (1 - after / before) if before else 0:.1f}% |"
            )
    detailed = {}
    for path, run in runs:
        if (
            run["case"] != "10k"
            or run.get("profile_mode") != "detailed"
            or run["status"] != "succeeded"
        ):
            continue
        metrics = detailed_metrics(path)
        variant = run["variant"]
        standard = measurements.get(variant)
        metrics["processing_s"] = run["processing_ms"] / 1000
        metrics["overhead_percent"] = (
            100 * (metrics["processing_s"] / standard["median_s"] - 1) if standard else None
        )
        detailed[variant] = metrics
    result["detailed"] = detailed
    if detailed:
        lines += [
            "",
            "## Separate diagnostic runs",
            "",
            "These runs include SQL collection overhead. Timing columns are inclusive and overlap.",
            "",
            "| Version | Processing s | Overhead vs standard | SQL profiles | "
            "CLI lake opens | dbt processes | Imports s | Attach s |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
        for variant, metrics in detailed.items():
            timings = metrics["timings_s"]
            overhead = metrics["overhead_percent"]
            overhead_text = f"{overhead:.1f}%" if overhead is not None else "unavailable"
            lines.append(
                f"| {variant} | {metrics['processing_s']:.3f} | "
                f"{overhead_text} | {metrics['sql_profiles']} | "
                f"{metrics['connection_opens']} | {metrics['dbt_invocations']} | "
                f"{timings['command.import']:.3f} | {timings['lake.configure_attach']:.3f} |"
            )
    large = next(
        (run for _, run in runs if run["case"] == "10k" and run["status"] == "succeeded"), None
    )
    if large:
        lines += [
            "",
            "## Output counts",
            "",
            "All compared versions must have identical values and membership, as well as counts.",
            "",
            "| Relation | After full processing | After incremental processing |",
            "|---|---:|---:|",
        ]
        for table, count in large["base_counts"].items():
            lines.append(f"| {table} | {count} | {large['batch_counts'][table]} |")
    lines += [
        "",
        f"Output comparisons: {len(checks)}; mismatches: "
        f"{sum(not check['matching'] for check in checks)}.",
        "",
        "CPU uses cgroup counter deltas at processing command boundaries. Memory uses "
        "250 ms samples inside those windows and includes container page cache. "
        "The table uses the median of each run's sampled peak, not cumulative lifetime peaks.",
        "",
        "## Runs",
        "",
        "| Run | Mode | Processing s | Detailed report |",
        "|---|---|---:|---|",
    ]
    for path, run in runs:
        directory = path.parent.parent.relative_to(out)
        lines.append(
            f"| [{directory.name}]({directory}/run/result.json) | "
            f"{run.get('profile_mode')} | {run.get('processing_ms', 0) / 1000:.3f} | "
            f"[Report]({directory}/report.md) |"
        )
    (out / "comparison.json").write_text(json.dumps(result, indent=2))
    (out / "comparison.md").write_text("\n".join(lines) + "\n")
    if any(not check["matching"] for check in checks):
        raise AssertionError("semantic outputs differ; see comparison.json")
    return result


def campaign(args: argparse.Namespace) -> None:
    args.out.mkdir(parents=True, exist_ok=True)
    args.git_sha = output(["git", "-C", str(ROOT), "rev-parse", "HEAD"])
    for variant in ("baseline", "candidate"):
        name = f"{variant}_image"
        setattr(
            args,
            name,
            output(["docker", "image", "inspect", getattr(args, name), "--format", "{{.Id}}"]),
        )
    manifest = {
        "baseline_image": args.baseline_image,
        "candidate_image": args.candidate_image,
        "harness_sha256": {
            str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted((ROOT / "benchmarks").rglob("*"))
            if path.is_file() and "__pycache__" not in path.parts
        },
        "compose_sha256": hashlib.sha256((ROOT / "docker/compose.yaml").read_bytes()).hexdigest(),
        "minimum_repeats": args.repeat,
    }
    manifest_path = args.out / "campaign-manifest.json"
    if manifest_path.exists():
        if not args.resume or json.loads(manifest_path.read_text()) != manifest:
            raise RuntimeError("campaign exists or inputs changed; use a fresh output directory")
    else:
        manifest_path.write_text(json.dumps(manifest, indent=2))
        shutil.copytree(
            ROOT / "benchmarks", args.out / "harness", ignore=shutil.ignore_patterns("__pycache__")
        )
    for case in ("tiny", "smoke"):
        for variant in ("baseline", "candidate"):
            trial(args, variant, case, 1, "detailed")
            report(args.out)
    for iteration in range(1, 11):
        variants = ("baseline", "candidate") if iteration % 2 else ("candidate", "baseline")
        for variant in variants:
            trial(args, variant, "10k", iteration, "standard")
            summary = report(args.out)
        if iteration >= args.repeat and (
            iteration == 10
            or all(value["cv"] <= 0.05 for value in summary["measurements"].values())
        ):
            break
    for variant in ("baseline", "candidate"):
        trial(args, variant, "10k", 1, "detailed")
        report(args.out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--baseline-image")
    parser.add_argument("--candidate-image")
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--report", action="store_true")
    parser.add_argument("--resume", action="store_true", help="retain successful identical trials")
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--case", choices=("tiny", "smoke", "10k"), default="10k")
    parser.add_argument("--variant", choices=("baseline", "candidate"), default="candidate")
    parser.add_argument("--profile-mode", choices=("standard", "detailed"), default="standard")
    parser.add_argument("--iteration", type=int, default=1)
    parser.add_argument("--corpus-root", type=Path)
    args = parser.parse_args()
    args.out = args.out.resolve()
    if args.worker:
        worker(args)
    elif args.report:
        report(args.out)
    else:
        if not args.baseline_image or not args.candidate_image:
            parser.error("both immutable comparison images are required")
        if not 1 <= args.repeat <= 10:
            parser.error("repeat must be between 1 and 10")
        campaign(args)


if __name__ == "__main__":
    main()
