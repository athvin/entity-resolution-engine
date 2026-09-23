"""Offline full/incremental evidence reports. No lake, Docker or network required."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import pstats
import re
import statistics
from collections import defaultdict
from pathlib import Path
from typing import Any

import duckdb
from full_pipeline import summarize_run
from workload_validation import compare_score_files


def statement_fingerprint(sql: str) -> str:
    sql = re.sub(r"/\* er_profile:[a-f0-9]+ \*/", "", sql)
    sql = re.sub(r"[a-f0-9]{32,64}|[0-9A-HJKMNP-TV-Z]{26}", "<id>", sql)
    return hashlib.sha256(" ".join(sql.split()).encode()).hexdigest()


def index_profiles(directory: Path, run: dict[str, Any]) -> dict[str, Any]:
    """Stream native trees; aggregate their compact index inside DuckDB."""
    invocations = {e["invocation_id"]: e["phase"] for e in run["commands"]}
    phases = {}
    for event_path in directory.glob("events-*.jsonl"):
        with event_path.open() as handle:
            for line in handle:
                event = json.loads(line)
                if event["event"] == "sql_profile":
                    phases[Path(event["profile_path"]).name] = invocations.get(
                        event.get("invocation_id"), event.get("phase", "unknown")
                    )
    query_path, operator_path = directory / "query-index.jsonl", directory / "operator-index.jsonl"
    count = 0
    indexed = 0
    indexed_paths: set[str] = set()
    exclusions: dict[str, int] = defaultdict(int)
    with query_path.open("w") as queries, operator_path.open("w") as operators:
        for path in sorted((directory / "sql").glob("*.json")):
            count += 1
            known_phase = phases.get(path.name)
            if known_phase is not None and known_phase not in ("base", "batch"):
                exclusions[known_phase] += 1
                continue
            envelope = json.loads(path.read_text())
            native = envelope["profile"]
            # New profiles carry invocation in their immutable envelope. Older
            # profiles can still be indexed through their phase field.
            phase = invocations.get(envelope.get("invocation_id"), envelope.get("phase", "unknown"))
            if phase not in ("base", "batch"):
                exclusions[phase] += 1
                continue
            sql = native.get("query_name", "")
            identity = {
                "phase": phase,
                "stage": envelope.get("stage"),
                "query_id": envelope["query_id"],
                "profile_path": str(path.relative_to(directory)),
                "fingerprint": statement_fingerprint(sql),
                "dbt_model": envelope.get("dbt_model"),
                "substage": envelope.get("name"),
                "span_id": envelope.get("span_id"),
                "connection_id": envelope.get("connection_id"),
                "invocation_id": envelope.get("invocation_id"),
                "method": envelope.get("method"),
                "blocking_rule": envelope.get("blocking_rule"),
            }
            row = {
                **identity,
                "latency": native.get("latency"),
                "cpu_time": native.get("cpu_time"),
                "rows_scanned": native.get("cumulative_rows_scanned"),
                "rows_returned": native.get("rows_returned"),
                "buffer_peak": native.get("system_peak_buffer_memory"),
                "spill_peak": native.get("system_peak_temp_dir_size"),
                "bytes_read": native.get("total_bytes_read"),
                "bytes_written": native.get("total_bytes_written"),
                "sql_preview": re.sub(r"/\* er_profile:[a-f0-9]+ \*/\s*", "", sql)[:1000],
            }
            queries.write(json.dumps(row) + "\n")
            indexed += 1
            indexed_paths.add(path.name)
            stack = [(str(i), node) for i, node in enumerate(native.get("children", []))]
            while stack:
                position, node = stack.pop()
                operators.write(
                    json.dumps(
                        {
                            **identity,
                            "node": position,
                            "operator": node.get("operator_name", node.get("operator_type")),
                            "seconds": node.get("operator_timing"),
                            "rows": node.get("operator_cardinality"),
                            "rows_scanned": node.get("operator_rows_scanned"),
                            "extra_info": json.dumps(node.get("extra_info", {}), sort_keys=True),
                        }
                    )
                    + "\n"
                )
                stack.extend(
                    (f"{position}.{i}", child) for i, child in enumerate(node.get("children", []))
                )
    missing = sorted(
        name
        for name, phase in phases.items()
        if phase in ("base", "batch") and name not in indexed_paths
    )
    result: dict[str, Any] = {
        "profiles": count,
        "indexed_profiles": indexed,
        "excluded_phases": dict(exclusions),
        "missing_workload_profiles": missing,
        "workloads": {},
        "status": "failed" if missing else "captured" if indexed else "unavailable",
    }
    if not indexed:
        (directory / "sql-summary.json").write_text(json.dumps(result, indent=2))
        return result
    with duckdb.connect() as connection:
        connection.execute("SET threads=2")
        connection.execute("SET memory_limit='1GB'")
        connection.execute(
            "CREATE TABLE queries AS SELECT * FROM read_json_auto(?)", [str(query_path)]
        )
        for phase in ("base", "batch"):
            cursor = connection.execute(
                'SELECT fingerprint, stage, count(*) executions, sum(latency) AS "seconds", '
                "max(latency) slowest_seconds, sum(rows_scanned) rows_scanned, "
                "max(spill_peak) connection_spill_high_water, "
                "first(profile_path ORDER BY latency DESC) example, "
                "first(sql_preview ORDER BY latency DESC) sql_preview FROM queries WHERE phase=? "
                'GROUP BY fingerprint, stage ORDER BY "seconds" DESC LIMIT 15',
                [phase],
            )
            names = [column[0] for column in cursor.description]
            result["workloads"][phase] = [
                dict(zip(names, row, strict=True)) for row in cursor.fetchall()
            ]
        connection.execute(
            "COPY queries TO ? (FORMAT PARQUET)", [str(directory / "queries.parquet")]
        )
    (directory / "sql-summary.json").write_text(json.dumps(result, indent=2))
    return result


def python_summary(directory: Path, run: dict[str, Any]) -> dict[str, Any]:
    identities = {}
    invocations = {entry["invocation_id"]: entry for entry in run["commands"]}
    for event_path in directory.glob("events-*.jsonl"):
        for line in event_path.read_text().splitlines():
            event = json.loads(line)
            if event["event"] == "python_profile":
                identities[Path(event["profile_path"]).name] = event
    rows = []
    for path in sorted((directory / "python").glob("*.pstats")):
        identity = identities.get(path.name, {})
        command = invocations.get(identity.get("invocation_id"), {})
        stats = pstats.Stats(str(path))
        functions = []
        inconsistent = 0
        for (filename, line, function), (
            _primitive,
            calls,
            self_time,
            cumulative,
            _callers,
        ) in stats.stats.items():
            if self_time == 0:
                continue
            timing_consistent = self_time >= 0 and cumulative + 1e-9 >= self_time
            inconsistent += int(not timing_consistent)
            category = "python"
            if filename == "~":
                category = "native_or_boundary"
            if any(word in function.lower() for word in ("wait", "poll", "select")):
                category = "waiting_or_boundary"
            functions.append(
                {
                    "file": filename,
                    "line": line,
                    "function": function,
                    "calls": calls,
                    "self_seconds": self_time,
                    "cumulative_seconds": cumulative,
                    "category": category,
                    "timing_consistent": timing_consistent,
                }
            )
        rows.append(
            {
                "path": str(path.relative_to(directory)),
                "phase": command.get("phase", identity.get("phase", "unknown")),
                "component": identity.get("component"),
                "invocation_id": identity.get("invocation_id"),
                "command": command.get("command"),
                "total_seconds": stats.total_tt,
                "timing_status": "inconsistent" if inconsistent else "internally_consistent",
                "inconsistent_timing_functions": inconsistent,
                "most_called": sorted(functions, key=lambda row: row["calls"], reverse=True)[:40],
                "functions": sorted(functions, key=lambda row: row["self_seconds"], reverse=True)[
                    :40
                ],
            }
        )
    result = {
        "profiles": rows,
        "interpretation": (
            "Wall-clock cProfile timings include native calls and waits; "
            "they are not pure Python CPU. Profiles with self time exceeding cumulative time "
            "are marked inconsistent: use their call counts, not duration rankings. "
            "Native query profiles and control stage times remain the timing evidence."
        ),
    }
    (directory / "python-summary.json").write_text(json.dumps(result, indent=2))
    return result


def comparison_errors(
    control: dict[str, Any], profiled: dict[str, Any], *, require_same_image: bool = True
) -> list[str]:
    errors = []
    for key in (
        "input_sha256",
        "semantic_config_sha256",
        "generator_profile",
        "generator_seed",
        "incremental_scenario",
        "base_records",
        "incremental_records",
        "base_partition_hash",
        "batch_partition_hash",
        "base_semantic_hashes",
        "batch_semantic_hashes",
        "base_quality",
        "batch_quality",
    ):
        if key not in control or key not in profiled or control[key] != profiled[key]:
            errors.append(f"control/profile mismatch or missing field: {key}")
    for key in (
        "image_digest",
        "er_duckdb_threads",
        "er_duckdb_memory_limit",
        "cgroup_cpu_max",
        "cgroup_memory_max",
        "duckdb_version",
        "splink_version",
        "ducklake_extension_version",
        "dbt_core_version",
        "dbt_duckdb_version",
        "matching_runtime",
    ):
        baseline = control.get("fingerprint", {}).get(key)
        candidate = profiled.get("fingerprint", {}).get(key)
        if key == "image_digest" and not require_same_image and baseline and candidate:
            continue  # Explicit baseline/candidate source comparison; all other gates stay exact.
        if baseline is None or candidate is None or baseline != candidate:
            errors.append(f"NON_COMPARABLE: {key}")
    for mode, run in (("control", control), ("profiled", profiled)):
        if run.get("incremental_equivalence", {}).get("status") != "passed":
            errors.append(f"{mode}: missing incremental/full equivalence")
        before, after = run.get("base_model_fingerprint"), run.get("batch_model_fingerprint")
        if not before or not after or before != after:
            errors.append(f"{mode}: model/TF changed within trial or is missing")
    for phase in ("base", "batch"):
        left = control.get(f"{phase}_model_fingerprint", {}).get("tf_sha256")
        right = profiled.get(f"{phase}_model_fingerprint", {}).get("tf_sha256")
        if not left or not right or left != right:
            errors.append(f"{phase}: frozen TF differs between trials or is missing")
    return errors


def compare_model_parameters(left: Any, right: Any) -> dict[str, Any]:
    """Only learned probabilities may vary by roundoff between independent fits."""
    errors: list[str] = []
    maximum = 0.0

    def visit(a: Any, b: Any, path: str, key: str = "") -> None:
        nonlocal maximum
        if isinstance(a, dict) and isinstance(b, dict) and a.keys() == b.keys():
            for name in a:
                visit(a[name], b[name], f"{path}.{name}", name)
        elif isinstance(a, list) and isinstance(b, list) and len(a) == len(b):
            for index, (old, new) in enumerate(zip(a, b, strict=True)):
                visit(old, new, f"{path}[{index}]", key)
        elif (
            key in {"m_probability", "u_probability", "probability_two_random_records_match"}
            and isinstance(a, (int, float))
            and isinstance(b, (int, float))
        ):
            delta = abs(a - b)
            maximum = max(maximum, delta)
            if not math.isfinite(a) or not math.isfinite(b) or delta > 1e-12:
                errors.append(path)
        elif type(a) is not type(b) or a != b:
            errors.append(path)

    visit(left, right, "model")
    return {
        "status": "passed" if not errors else "failed",
        "mismatches": errors,
        "max_parameter_delta": maximum,
        "tolerance": 1e-12,
    }


def trained_parameters(directory: Path, run: dict[str, Any]) -> tuple[dict[str, Any], str]:
    path = directory / "base-model.json"
    if path.exists():
        return json.loads(path.read_text()), "model_json"
    # Older diagnostic artifacts preserved complete fitted summaries in CLI logs.
    # Keep the evidence type visible; do not present these as recovered model bytes.
    command = next(c for c in run["commands"] if c["command"][1] == "train")
    path = directory / "commands" / command["invocation_id"] / "stdout.log"
    output = [json.loads(line) for line in path.read_text().splitlines() if line.startswith("{")]
    metrics = output[-1]["metrics"]
    return {k: v for k, v in metrics.items() if k != "tf_snapshot_id"}, "training_metrics"


def write_campaign_report(root: Path) -> dict[str, Any]:
    manifest = json.loads((root / "campaign.json").read_text())
    result: dict[str, Any] = {"schema_version": 1, "manifest": manifest, "trials": [], "errors": []}
    for mode in ("control", "profiled"):
        for path in sorted((root / mode).glob("run-*/result.json")):
            try:
                run = json.loads(path.read_text())
            except (OSError, ValueError) as error:
                result["errors"].append(f"{mode}/{path.parent.name}: invalid result: {error}")
                continue
            if run.get("status") != "succeeded":
                result["errors"].append(
                    f"{mode}/{path.parent.name}: {run.get('error', 'incomplete')}"
                )
                continue
            summary = summarize_run(path)
            diagnostic = index_profiles(path.parent, run) if mode == "profiled" else None
            if mode == "profiled":
                python_summary(path.parent, run)
                if diagnostic is None or diagnostic["status"] != "captured":
                    result["errors"].append(
                        f"{mode}/{path.parent.name}: missing native workload artifacts; "
                        "see sql-summary.json"
                    )
            result["trials"].append(
                {
                    "mode": mode,
                    "directory": str(path.parent.relative_to(root)),
                    "run": run,
                    "summary": summary,
                    "sql": diagnostic,
                }
            )
    controls = [trial for trial in result["trials"] if trial["mode"] == "control"]
    profiles = [trial for trial in result["trials"] if trial["mode"] == "profiled"]
    if len(controls) != manifest["repeat"] or len(profiles) != manifest["repeat"]:
        result["errors"].append("incomplete paired campaign")
    for control, profiled in zip(controls, profiles, strict=False):
        result["errors"].extend(comparison_errors(control["run"], profiled["run"]))
        old_model, old_evidence = trained_parameters(root / control["directory"], control["run"])
        new_model, new_evidence = trained_parameters(root / profiled["directory"], profiled["run"])
        compared_model = compare_model_parameters(old_model, new_model)
        compared_model["evidence"] = [old_evidence, new_evidence]
        compared_model["byte_identical"] = (
            control["run"]["base_model_fingerprint"]["model_sha256"]
            == profiled["run"]["base_model_fingerprint"]["model_sha256"]
        )
        profiled["model_comparison"] = compared_model
        if compared_model["status"] != "passed":
            result["errors"].append(f"independent model fits differ: {compared_model}")
        profiled["score_comparisons"] = {}
        for phase in ("base", "batch"):
            compared = compare_score_files(
                root / control["directory"] / f"{phase}-scores.json.gz",
                root / profiled["directory"] / f"{phase}-scores.json.gz",
            )
            profiled["score_comparisons"][phase] = compared
            if compared["status"] != "passed":
                result["errors"].append(f"{phase}: control/profile score mismatch: {compared}")
    result["status"] = (
        "passed" if not result["errors"] and manifest["status"] == "succeeded" else "failed"
    )
    for phase, title, metric, filename in (
        ("base", "Full reload", "processing_seconds", "full-report.md"),
        ("batch", "Incremental delivery", "incremental_seconds", "incremental-report.md"),
    ):
        lines = [
            f"# {title}",
            "",
            f"Campaign validation: **{result['status']}**.",
            "",
            "Times include pipeline commands and their orchestration. Setup, generation, "
            "snapshot validation and the full-rescore reference are excluded.",
            "",
            "| Trial | Processing seconds | Records delivered |",
            "|---|---:|---:|",
        ]
        for trial in result["trials"]:
            records = trial["run"]["base_records" if phase == "base" else "incremental_records"]
            lines.append(f"| {trial['directory']} | {trial['summary'][metric]:.3f} | {records:,} |")
        if controls and profiles:
            control_time = statistics.median(t["summary"][metric] for t in controls)
            profile_time = statistics.median(t["summary"][metric] for t in profiles)
            lines += [
                "",
                f"Control median: **{control_time:.3f}s**. "
                f"Diagnostic median: **{profile_time:.3f}s**.",
                "Indicative instrumentation overhead: "
                f"{100 * (profile_time / control_time - 1):.1f}%.",
                "A single pair is not a performance-regression baseline. "
                "Fresh state does not imply cold OS caches.",
            ]
        lines += [
            "",
            "## Resources and output",
            "",
            "Memory is the sampled cgroup peak in workload command intervals; "
            "spill is a sampled temporary-directory peak, not bytes written.",
            "",
            "| Trial | CPU seconds | Throttled seconds | Peak memory GiB | Peak spill GiB | "
            "Active records | Golden records | Candidate pairs in resulting corpus |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
        for trial in result["trials"]:
            resources = trial["summary"][
                "resources" if phase == "base" else "incremental_resources"
            ]
            counts = trial["run"][f"{phase}_counts"]
            spill = resources.get("sampled_spill_bytes_peak")
            spill_text = "unavailable" if spill is None else f"{spill / 1024**3:.3f}"
            lines.append(
                f"| {trial['directory']} | {resources['cpu_s']:.3f} | "
                f"{resources['throttled_s']:.3f} | "
                f"{resources['sampled_memory_peak_bytes'] / 1024**3:.3f} | {spill_text} | "
                f"{counts['int_std_records']:,} | {counts['golden_records']:,} | "
                f"{trial['run'][f'{phase}_candidate_pairs']:,} |"
            )
        for trial in controls:
            lines += [
                "",
                f"Quality ({trial['directory']}):",
                "",
                "```json",
                json.dumps(trial["run"][f"{phase}_quality"], indent=2),
                "```",
            ]
        lines += [
            "",
            "## Stage ledger",
            "",
            "Stage duration excludes some command startup. It is not added to command totals.",
            "",
            "| Trial | Stage | Seconds | Input rows | Output rows |",
            "|---|---|---:|---:|---:|",
        ]
        for trial in result["trials"]:
            stages: dict[str, dict[str, float]] = defaultdict(
                lambda: {"seconds": 0, "rows_in": 0, "rows_out": 0}
            )
            for command in trial["run"]["commands"]:
                if command["phase"] != phase:
                    continue
                for stage in command["stages"]:
                    target = stages[stage["stage"]]
                    target["seconds"] += stage["duration_ms"] / 1000
                    target["rows_in"] += stage.get("rows_in") or 0
                    target["rows_out"] += stage.get("rows_out") or 0
            for name, values in stages.items():
                lines.append(
                    f"| {trial['directory']} | {name} | {values['seconds']:.3f} | "
                    f"{values['rows_in']:g} | {values['rows_out']:g} |"
                )
        for trial in profiles:
            directory = trial["directory"]
            lines += [
                "",
                f"## Expensive SQL: {directory}",
                "",
                "SQL latency sums are diagnostic costs, not stage wall time.",
                "",
                "| Stage | Executions | SQL seconds | Scanned rows | Evidence |",
                "|---|---:|---:|---:|---|",
            ]
            for row in trial["sql"]["workloads"].get(phase, []):
                lines.append(
                    f"| {row['stage']} | {row['executions']} | {row['seconds']:.3f} | "
                    f"{row['rows_scanned']} | [profile]({directory}/{row['example']}) |"
                )
            lines += [
                "",
                f"[SQL index]({directory}/query-index.jsonl), "
                f"[operator index]({directory}/operator-index.jsonl), "
                f"[Python boundaries]({directory}/python-summary.json), "
                f"[DuckLake layout]({directory}/{phase}-lake-layout.json), "
                f"[query coverage]({directory}/query-coverage.json).",
            ]
        lines += [
            "",
            "## Interpretation",
            "",
            "Rank next experiments using measured costs and their SQL, not operator names alone. "
            "DuckLake does not support ART indexes. "
            "Ordered writes and partitions need actual pruning evidence "
            "and must include load, incremental and maintenance costs. "
            "Python profiles include native engine calls "
            "and waits. Python summaries flag inconsistent timings; use call counts only "
            "for those profiles. Memory/spill high-water marks are not additive. "
            "Local object-store results do not model "
            "remote S3 latency.",
        ]
        (root / filename).write_text("\n".join(lines) + "\n")
    lines = [
        "# Full reload and incremental profiling",
        "",
        f"Validation: **{result['status']}**.",
        "",
        "- [Full reload](full-report.md)",
        "- [Incremental delivery](incremental-report.md)",
        "- [Machine-readable comparison](analysis.json)",
        "",
        "Both workloads are primary. Diagnostic timings quantify collection overhead; "
        "they do not establish optimization gains.",
    ]
    for trial in profiles:
        compared = trial.get("model_comparison", {})
        lines += [
            "",
            f"Model comparison ({trial['directory']}): "
            f"byte-identical={compared.get('byte_identical')}; maximum learned-parameter "
            f"delta={compared.get('max_parameter_delta')} (tolerance 1e-12). "
            f"Evidence: {compared.get('evidence')}. Non-learned fields in that evidence compare "
            "exactly. Model/TF byte hashes must stay "
            "identical within each trial. Scores compare within 1e-10; threshold classes, "
            "clusters, golden values and lineage compare exactly.",
        ]
    if result["errors"]:
        lines += [
            "",
            "## Incomplete or mismatched evidence",
            "",
            *[f"- {error}" for error in result["errors"]],
        ]
    (root / "report.md").write_text("\n".join(lines) + "\n")
    (root / "analysis.json").write_text(json.dumps(result, indent=2))
    if result["status"] != "passed":
        raise ValueError("paired campaign validation failed; see report.md")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    write_campaign_report(parser.parse_args().directory)
