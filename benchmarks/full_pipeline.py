"""Time an initial load from source CSVs through golden records on a fresh Docker lake."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import statistics
import subprocess
import sys
import time
import uuid
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from build_performance_image import EXCLUDED, PATHS
from performance import processing_resources
from scales import Scale, _memory_bytes, load_scales
from scales import get_scale as standard_scale

ROOT = Path(__file__).resolve().parents[1]
GIB = 1024**3
STAGES = ("ingest", "standardize", "train", "match", "reconcile", "assemble")


def get_scale(name: str) -> Scale:
    """The experimental 10m initial load reuses 1m's envelope, outside scheduled CI."""
    if name == "10m":
        return replace(
            standard_scale("1m"),
            name="10m",
            personas=4_000_000,
            records=10_000_000,
            incremental_batch=0,
        )
    return standard_scale(name)


def output(command: list[str]) -> str:
    return subprocess.check_output(command, text=True, cwd=ROOT).strip()


def capacity_errors(scale: Scale, *, cpus: int, memory: int, disk: int) -> list[str]:
    """Use the configured envelope, including room for the catalog and object store."""
    errors = []
    if cpus < scale.cpu_limit:
        errors.append(f"Docker has {cpus} CPUs; {scale.name} requires {scale.cpu_limit}")
    required_memory = _memory_bytes(scale.mem_limit, scale.name, "mem_limit") + GIB
    if memory < required_memory:
        errors.append(
            f"Docker has {memory / GIB:.1f} GiB RAM; needs {required_memory / GIB:g} GiB "
            f"({scale.mem_limit} container plus 1 GiB for services)"
        )
    if disk < scale.min_free_gb * GIB:
        errors.append(
            f"output filesystem has {disk / GIB:.1f} GiB free; "
            f"{scale.name} requires {scale.min_free_gb} GiB"
        )
    return errors


def local_scale(scale: Scale, *, cpus: int, memory: int) -> Scale:
    """Fit the current Docker host; preset envelopes are not workload minimums."""
    memory_gib = min(
        _memory_bytes(scale.mem_limit, scale.name, "mem_limit") // GIB,
        memory // GIB - 1,
    )
    if memory_gib < 2:
        raise ValueError("local benchmark needs 2 GiB container RAM plus 1 GiB for services")
    duckdb_gib = min(
        _memory_bytes(scale.duckdb_memory_limit, scale.name, "duckdb_memory_limit") // GIB,
        memory_gib * 2 // 3,
        4,  # Leave additional host capacity for reconciliation's Python heap.
    )
    return replace(
        scale,
        # Staging at a million records needs room for each DuckDB worker's
        # buffers. More threads can exhaust memory before spilling can help.
        cpu_limit=min(scale.cpu_limit, max(1, cpus - 2), max(1, duckdb_gib // 2)),
        mem_limit=f"{memory_gib}g",
        duckdb_memory_limit=f"{duckdb_gib}GB",
        min_free_gb=2,
    )


def measured_scale(scale: Scale, manifest: dict[str, Any]) -> Scale:
    """The requested envelope is recorded by the host and verified by the worker."""
    fields = ("cpu_limit", "mem_limit", "duckdb_memory_limit", "min_free_gb")
    return replace(scale, **{field: manifest[field] for field in fields if field in manifest})


def preflight(scale: Scale, out: Path, *, local: bool = False) -> dict[str, Any]:
    directory = out
    while not directory.exists():
        directory = directory.parent
    info = json.loads(output(["docker", "info", "--format", "{{json .}}"]))
    available = {
        "cpus": int(info["NCPU"]),
        "memory": int(info["MemTotal"]),
        "disk": shutil.disk_usage(directory).free,
    }
    if local:
        scale = local_scale(scale, cpus=available["cpus"], memory=available["memory"])
    return {
        "resource_profile": "local" if local else "preset",
        "scale": scale.name,
        "records": scale.records,
        "personas": scale.personas,
        "cpu_limit": scale.cpu_limit,
        "mem_limit": scale.mem_limit,
        "duckdb_memory_limit": scale.duckdb_memory_limit,
        "min_free_gb": scale.min_free_gb,
        "available": available,
        "docker_architecture": info["Architecture"],
        "errors": capacity_errors(scale, **available),
    }


def source_manifest() -> dict[str, str]:
    paths = [path for name in PATHS for path in (ROOT / name).rglob("*")]
    paths += [ROOT / name for name in ("pyproject.toml", "uv.lock", "LICENSE", ".dockerignore")]
    paths += [ROOT / "docker/Dockerfile", ROOT / "docker/compose.yaml"]
    return {
        str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in paths
        if path.is_file()
        and not EXCLUDED.intersection(path.relative_to(ROOT).parts)
        and path.name != ".user.yml"
        and path.suffix not in (".pyc", ".pyo")
    }


def summarize_run(path: Path) -> dict[str, Any]:
    run = json.loads(path.read_text())
    if run["status"] != "succeeded":
        return {"status": run["status"], "error": run.get("error"), "path": str(path)}
    commands = [entry for entry in run["commands"] if entry["phase"] == "base"]
    if [entry["command"][1] for entry in commands] != ["ingest"] * 3 + list(STAGES[1:]):
        raise ValueError(f"incomplete initial-load command sequence: {path}")
    if any(entry["exit_code"] != 0 for entry in commands):
        raise ValueError(f"initial load contained a skipped/failed stage: {path}")
    stages = {
        stage: sum(e["duration_ms"] for e in commands if e["command"][1] == stage) / 1000
        for stage in STAGES
    }
    seconds = sum(stages.values())
    return {
        "status": "succeeded",
        "processing_seconds": seconds,
        "records_per_second": run["base_records"] / seconds,
        "stage_seconds": stages,
        "setup_command_seconds": sum(
            e["duration_ms"] for e in run["commands"] if e["phase"] == "setup"
        )
        / 1000,
        "worker_seconds": run["total_ms"] / 1000,
        "resources": processing_resources(path, run),
        "counts": run["base_counts"],
        "validation": run["output_validation"],
        "partition_sha256": run["base_partition_hash"],
        "input_sha256": run["input_sha256"],
        "quality": run["quality"],
        "fingerprint": run["fingerprint"],
    }


def write_report(out: Path, manifest: dict[str, Any]) -> dict[str, Any]:
    runs = []
    for path in sorted(out.glob("run-*/result.json")):
        try:
            runs.append(summarize_run(path))
        except (OSError, ValueError, KeyError, AssertionError) as error:
            # A killed worker can leave a partial JSON write. Preserve the failure
            # and remaining evidence, including when no pass completed.
            runs.append({"status": "failed", "path": str(path), "error": str(error)})
    successful = [run for run in runs if run["status"] == "succeeded"]
    result = {"schema_version": 1, "manifest": manifest, "runs": runs}
    lines = [
        f"# Full pipeline benchmark: {manifest['scale']}",
        "",
        f"Status: **{manifest['status']}**. Completed passes: {len(successful)} / "
        f"{manifest['repeat']}. Input records per pass: {manifest['records']:,}.",
        "",
        "Processing includes CLI startup, ingestion, cleaning/standardization, model training, "
        "matching, reconciliation and golden record assembly. It excludes image/stack setup, "
        "data generation, validation and teardown. No incremental delivery is run. "
        "Detailed SQL profiling is disabled; resource samples are collected every 250 ms.",
        "",
    ]
    if manifest.get("error"):
        lines += [f"Error: {manifest['error']}", ""]
    if "cpu_limit" in manifest:
        lines += [
            f"Resource profile: **{manifest.get('resource_profile', 'preset')}**. "
            f"{manifest['cpu_limit']} CPUs, {manifest['mem_limit']} container memory; "
            f"DuckDB {manifest['cpu_limit']} threads / {manifest['duckdb_memory_limit']}.",
            "",
        ]
    if successful:
        times = [run["processing_seconds"] for run in successful]
        median = statistics.median(times)
        stages = {
            stage: statistics.median(run["stage_seconds"][stage] for run in successful)
            for stage in STAGES
        }
        bottleneck = max(stages, key=lambda stage: stages[stage])
        result["summary"] = {
            "median_seconds": median,
            "min_seconds": min(times),
            "max_seconds": max(times),
            "cv": statistics.stdev(times) / statistics.mean(times) if len(times) > 1 else None,
            "records_per_second": manifest["records"] / median,
            "stage_median_seconds": stages,
            "bottleneck": bottleneck,
            "bottleneck_percent": 100 * stages[bottleneck] / sum(stages.values()),
            "outputs_repeatable": all(
                run[field] == successful[0][field]
                for run in successful[1:]
                for field in ("counts", "partition_sha256", "input_sha256")
            )
            if len(successful) > 1
            else None,
        }
        lines += [
            f"Median processing: **{median:.2f} seconds ({median / 60:.2f} minutes)**; "
            f"{manifest['records'] / median:,.0f} input records/second.",
            f"Largest stage: **{bottleneck}** "
            f"({result['summary']['bottleneck_percent']:.1f}% of summed stage medians).",
            "",
            "| Stage | Median seconds |",
            "|---|---:|",
            *(f"| {stage} | {seconds:.2f} |" for stage, seconds in stages.items()),
            "",
            "Stage medians may not sum to the total median. "
            + (
                "One pass is an initial estimate; variation is unmeasured."
                if len(times) == 1
                else f"Range: {min(times):.2f}–{max(times):.2f} s; "
                f"CV: {result['summary']['cv']:.2%}. "
                f"Counts, memberships and inputs repeat: {result['summary']['outputs_repeatable']}."
            ),
            "",
            "| Pass | Seconds | Golden records | Lineage rows | CPU seconds | Peak MiB |",
            "|---|---:|---:|---:|---:|---:|",
        ]
        for index, run in enumerate(successful, 1):
            resources = run["resources"]
            lines.append(
                f"| {index} | {run['processing_seconds']:.2f} | "
                f"{run['counts']['golden_records']:,} | {run['counts']['golden_lineage']:,} | "
                f"{resources['cpu_s']:.2f} | "
                f"{resources['sampled_memory_peak_bytes'] / 1024**2:.1f} |"
            )
        lines += [
            "",
            "Peak memory covers the pipeline container, including page cache and child "
            "processes, during processing. It excludes catalog/object-store memory; short "
            "peaks can fall between samples. Full output checks, match-quality measurements, "
            "input hashes and runtime fingerprints are in results.json and each run directory.",
        ]
    (out / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    (out / "report.md").write_text("\n".join(lines) + "\n")
    return result


def worker(args: argparse.Namespace) -> None:
    from profile_pipeline import run_case
    from report import comparability_violations

    run = run_case(
        args.out,
        args.scale,
        args.iteration,
        label=f"run-{args.iteration:03d}",
        detailed=False,
        initial_only=True,
        corpus_root=args.out / "inputs",
        workload=get_scale(args.scale),
    )
    errors = comparability_violations(
        {"fingerprint": run["fingerprint"], "phases": []},
        measured_scale(get_scale(args.scale), json.loads((args.out / "manifest.json").read_text())),
    )
    if errors:
        run.update(status="failed", error="resource envelope mismatch: " + "; ".join(errors))
        (args.out / f"run-{args.iteration:03d}/result.json").write_text(json.dumps(run, indent=2))
        raise RuntimeError(run["error"])


def campaign(args: argparse.Namespace, checked: dict[str, Any]) -> None:
    args.out.mkdir(parents=True, exist_ok=False)
    manifest = {
        **checked,
        "repeat": args.repeat,
        "status": "running",
        "git_sha": output(["git", "rev-parse", "HEAD"]),
        "source_files": source_manifest(),
    }
    manifest["source_sha256"] = hashlib.sha256(
        json.dumps(manifest["source_files"], sort_keys=True).encode()
    ).hexdigest()
    (args.out / "working-tree.patch").write_text(output(["git", "diff", "HEAD", "--binary"]))
    manifest_path = args.out / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2))
    tag = f"er-full-bench:{uuid.uuid4().hex[:12]}"
    scale = measured_scale(get_scale(args.scale), checked)
    env = {
        **os.environ,
        "ER_CPU_LIMIT": str(scale.cpu_limit),
        "ER_MEM_LIMIT": scale.mem_limit,
        "ER_DUCKDB_MEMORY_LIMIT": scale.duckdb_memory_limit,
    }
    started = time.monotonic()
    try:
        print(f"[benchmark] building current source; logs: {args.out}", flush=True)
        with (args.out / "build.log").open("w") as log:
            subprocess.run(
                ["docker", "build", "-f", "docker/Dockerfile", "-t", tag, "."],
                cwd=ROOT,
                stdout=log,
                stderr=subprocess.STDOUT,
                check=True,
            )
        if source_manifest() != manifest["source_files"]:
            raise RuntimeError("source changed during build; rerun with a stable checkout")
        digest = output(["docker", "image", "inspect", tag, "--format", "{{.Id}}"])
        manifest["image_digest"] = digest
        engine_free = int(
            output(
                [
                    "docker",
                    "run",
                    "--rm",
                    digest,
                    "python",
                    "-c",
                    "import shutil; print(shutil.disk_usage('/').free)",
                ]
            )
        )
        manifest["docker_disk_free_bytes"] = engine_free
        if engine_free < scale.min_free_gb * GIB:
            raise RuntimeError(
                f"Docker filesystem has {engine_free / GIB:.1f} GiB free; "
                f"needs {scale.min_free_gb} GiB"
            )
        override = {
            "services": {
                service: {"image": digest} for service in ("benchmark", "pipeline", "catalog-init")
            }
        }
        override["services"]["benchmark"]["volumes"] = [f"{args.out}:/benchmark"]
        override_path = args.out / "compose.json"
        override_path.write_text(json.dumps(override))
        for iteration in range(1, args.repeat + 1):
            # Each pass owns its stack. Never tear down the user's development lake.
            compose = [
                "docker",
                "compose",
                "-p",
                f"er-full-bench-{uuid.uuid4().hex[:12]}",
                "-f",
                str(ROOT / "docker/compose.yaml"),
                "-f",
                str(override_path),
                "--profile",
                "bench",
            ]
            print(
                f"[benchmark] {scale.records:,} records, pass {iteration}/{args.repeat}", flush=True
            )
            passed = False
            try:
                with (args.out / f"pass-{iteration:03d}.log").open("w") as log:
                    subprocess.run(
                        [
                            *compose,
                            "run",
                            "--rm",
                            "-T",
                            "-e",
                            f"ER_IMAGE_DIGEST={digest}",
                            "-e",
                            f"ER_GIT_SHA={manifest['git_sha']}",
                            "-e",
                            f"ER_SOURCE_SHA={manifest['source_sha256']}",
                            "benchmark",
                            "python",
                            "benchmarks/full_pipeline.py",
                            "--worker",
                            "--out",
                            "/benchmark",
                            "--scale",
                            args.scale,
                            "--iteration",
                            str(iteration),
                        ],
                        cwd=ROOT,
                        env=env,
                        stdout=log,
                        stderr=subprocess.STDOUT,
                        check=True,
                    )
                passed = True
            finally:
                with (args.out / f"services-{iteration:03d}.log").open("w") as log:
                    subprocess.run(
                        [*compose, "logs", "--no-color"],
                        env=env,
                        stdout=log,
                        stderr=subprocess.STDOUT,
                        check=False,
                    )
                    if passed or not getattr(args, "keep_failed", False):
                        subprocess.run(
                            [*compose, "down", "-v", "--remove-orphans"],
                            env=env,
                            stdout=log,
                            stderr=subprocess.STDOUT,
                            check=True,
                        )
                    else:
                        manifest["retained_project"] = compose[3]
                        manifest["retained_image"] = tag
                        print(f"[benchmark] retained failed stack: {compose[3]}", flush=True)
            result = write_report(args.out, manifest)
            if len(result["runs"]) != iteration or any(
                run["status"] != "succeeded" for run in result["runs"]
            ):
                raise RuntimeError("incomplete measurement; see report.md and results.json")
            print(
                f"[benchmark] pass {iteration}: "
                f"{result['runs'][-1]['processing_seconds']:.2f} processing seconds",
                flush=True,
            )
        manifest["status"] = "succeeded"
    except BaseException as error:
        manifest.update(status="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        manifest["campaign_seconds"] = time.monotonic() - started
        manifest_path.write_text(json.dumps(manifest, indent=2))
        if "retained_project" not in manifest:
            subprocess.run(["docker", "image", "rm", tag], capture_output=True, check=False)
        write_report(args.out, manifest)
    print(f"[benchmark] complete: {args.out / 'report.md'}", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scale", choices=(*load_scales(), "10m"), default="1m")
    parser.add_argument("--repeat", type=int, default=1, help="fresh initial loads; default: 1")
    parser.add_argument(
        "--local",
        action="store_true",
        help="fit resources to this Docker host instead of the preset",
    )
    parser.add_argument(
        "--keep-failed", action="store_true", help="retain a failed lake for debugging or recovery"
    )
    parser.add_argument(
        "--out", type=Path, help="new output directory; defaults under artifacts/bench"
    )
    parser.add_argument(
        "--check-only", action="store_true", help="check host capacity without building"
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--iteration", type=int, default=1, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.repeat < 1 or args.iteration < 1:
        parser.error("repeat and iteration must be positive")
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    args.out = (args.out or ROOT / f"artifacts/bench/full-{args.scale}-{stamp}").resolve()
    try:
        if args.worker:
            worker(args)
            return 0
        checked = preflight(get_scale(args.scale), args.out, local=args.local)
        print(json.dumps(checked, indent=2), flush=True)
        if checked["errors"]:
            print("Preflight failed; no image build or pipeline run started.", file=sys.stderr)
            return 2
        if not args.check_only:
            campaign(args, checked)
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
