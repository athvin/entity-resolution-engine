"""Build readable reports from durable traces, without opening a lake connection."""

from __future__ import annotations

import json
import re
import statistics
from collections import defaultdict
from pathlib import Path
from typing import Any

STAGES = {"ingest", "standardize", "train", "match", "reconcile", "assemble"}


def events_at(directory: Path) -> list[dict[str, Any]]:
    return [
        json.loads(line)
        for path in directory.glob("events-*.jsonl")
        for line in path.read_text().splitlines()
        if line
    ]


def resource_window(samples: list[dict[str, Any]], start: int, end: int) -> dict[str, Any]:
    selected = [sample for sample in samples if start <= sample["monotonic_ns"] <= end]
    memory = [
        sample["memory.current"] for sample in selected if sample.get("memory.current") is not None
    ]
    rss = [
        sum(sample["process_rss_bytes"].values())
        for sample in selected
        if sample.get("process_rss_bytes")
    ]
    return {
        "sample_count": len(selected),
        "sampled_memory_peak_bytes": max(memory) if memory else None,
        "sampled_process_rss_sum_peak_bytes": max(rss) if rss else None,
    }


def _number(value: Any, divisor: float = 1, digits: int = 3) -> str:
    return "unavailable" if value is None else f"{value / divisor:.{digits}f}"


def service_peaks(out: Path) -> list[dict[str, Any]]:
    path = out / "services-resources.jsonl"
    services: dict[str, dict[str, Any]] = {}
    if not path.exists():
        return []
    for line in path.read_text().splitlines():
        try:
            sample = json.loads(line)
        except json.JSONDecodeError:
            continue  # The host monitor can be interrupted during its final sample.
        name = sample.get("Name", "")
        if not name:
            continue
        entry = services.setdefault(
            name, {"name": name, "samples": 0, "cpu_percent": 0.0, "memory_bytes": None}
        )
        entry["samples"] += 1
        entry["cpu_percent"] = max(entry["cpu_percent"], float(sample["CPUPerc"].rstrip("%")))
        match = re.match(r"([\d.]+)([KMGT]?i?B)", sample.get("MemUsage", ""))
        if match:
            number, unit = match.groups()
            power = "BKMGT".index(unit[0])
            value = float(number) * (1024 if "i" in unit else 1000) ** power
            entry["memory_bytes"] = max(entry["memory_bytes"] or 0, int(value))
    return list(services.values())


def summarize(directory: Path) -> dict[str, Any]:
    cached = directory / "summary.json"
    result_path = directory / "result.json"
    if cached.exists() and cached.stat().st_mtime_ns >= result_path.stat().st_mtime_ns:
        document = json.loads(cached.read_text())
        if document.get("summary_version") == 2:
            return document
    run = json.loads((directory / "result.json").read_text())
    events = events_at(directory)
    resources = directory / "resources.jsonl"
    samples = (
        [json.loads(line) for line in resources.read_text().splitlines()]
        if resources.exists()
        else []
    )
    spans = []
    commands = {entry.get("invocation_id"): entry for entry in run["commands"]}
    for event in events:
        if event["event"] != "span_end":
            continue
        end = event["monotonic_ns"]
        values = {
            **event,
            **resource_window(samples, int(end - event["duration_ms"] * 1_000_000), end),
        }
        command = commands.get(event.get("invocation_id"), {})
        values["phase"] = command.get("phase", event.get("phase", "setup"))
        metrics = event["metrics"]
        values["input_per_second"] = (
            metrics["rows_in"] / (event["duration_ms"] / 1000)
            if metrics.get("rows_in") is not None and event["duration_ms"] > 0
            else None
        )
        spans.append(values)
    queries = [event for event in events if event["event"] == "sql_profile"]
    operators: dict[str, dict[str, Any]] = {}
    batches: dict[str, dict[str, Any]] = {}
    for query in queries:
        profile = directory / "sql" / Path(query["profile_path"]).name
        if not profile.exists():
            continue
        native = json.loads(profile.read_text())["profile"]
        batch = batches.setdefault(
            query["query_id"],
            {
                "query_id": query["query_id"],
                "executions": 0,
                "engine_ms": 0.0,
                "first_capture_ns": query["monotonic_ns"],
                "last_capture_ns": query["monotonic_ns"],
                "statement": re.sub(
                    r"/\* er_profile:[a-f0-9]+ \*/\s*", "", native.get("query_name", "")
                ),
            },
        )
        batch["executions"] += 1
        batch["engine_ms"] += query["duration_ms"]
        batch["first_capture_ns"] = min(batch["first_capture_ns"], query["monotonic_ns"])
        batch["last_capture_ns"] = max(batch["last_capture_ns"], query["monotonic_ns"])
        stack = list(native.get("children", []))
        while stack:
            node = stack.pop()
            stack.extend(node.get("children", []))
            name = node.get("operator_name", node.get("operator_type", "unknown"))
            entry = operators.setdefault(
                name, {"name": name, "seconds": 0.0, "rows": 0, "calls": 0}
            )
            entry["seconds"] += node.get("operator_timing", 0)
            entry["rows"] += node.get("operator_cardinality", 0)
            entry["calls"] += 1
    summary = {
        "schema_version": 1,
        "summary_version": 2,
        "run": run,
        "resources": resource_window(samples, 0, 2**63),
        "spans": spans,
        "queries": sorted(queries, key=lambda item: item["duration_ms"], reverse=True),
        "operators": sorted(operators.values(), key=lambda item: item["seconds"], reverse=True),
        "repeated_statements": sorted(
            (batch for batch in batches.values() if batch["executions"] > 1),
            key=lambda item: item["last_capture_ns"] - item["first_capture_ns"],
            reverse=True,
        ),
    }
    (directory / "summary.json").write_text(json.dumps(summary, indent=2))
    return summary


def write_report(out: Path) -> None:
    summaries = [summarize(path.parent) for path in sorted(out.glob("*/result.json"))]
    lines = [
        "# Pipeline profiling results",
        "",
        "Each pass uses a fresh lake namespace and the same seeded input. Processing includes "
        "CLI startup, ingestion, standardization, training (except tiny), full resolution and an "
        "incremental cycle. Setup, corpus generation and correctness checks are separately timed.",
        "",
        "Container CPU includes CLI/dbt descendants. "
        "Memory is memory.current sampled every 250 ms; "
        "short-lived peaks may be missed. Process RSS sums count shared pages more than once. "
        "memory.peak in the fingerprint is a container lifetime high-water mark, not a phase peak. "
        "SQL buffer/spill peaks are DuckDB connection high-water marks. "
        "Parent spans include children; "
        "do not sum them. Missing measurements are unavailable, never zero.",
        "",
        "| Run | Status | Processing seconds | Total seconds | Sampled peak MiB | SQL profiles |",
        "|---|---|---:|---:|---:|---:|",
    ]
    for summary in summaries:
        run = summary["run"]
        label = run.get("label") or (
            f"{run['case']}-{run['iteration']}" if run["detailed"] else f"{run['case']}-control"
        )
        lines.append(
            f"| [{label}]({label}/result.json) | {run['status']} | "
            f"{_number(run.get('processing_ms'), 1000, 2)} | "
            f"{_number(run.get('total_ms'), 1000, 2)} | "
            f"{_number(summary['resources']['sampled_memory_peak_bytes'], 1024**2, 1)} | "
            f"{len(summary['queries'])} |"
        )
    services = service_peaks(out)
    if services:
        lines += [
            "",
            "Service peaks sampled by Docker stats over the whole campaign:",
            "",
            "| Container | Samples | Peak CPU % | Peak memory MiB |",
            "|---|---:|---:|---:|",
        ]
        for service in services:
            lines.append(
                f"| {service['name']} | {service['samples']} | "
                f"{service['cpu_percent']:.1f} | "
                f"{_number(service['memory_bytes'], 1024**2, 1)} |"
            )
    measured = [
        s
        for s in summaries
        if s["run"]["case"] == "10k" and s["run"]["detailed"] and s["run"]["status"] == "succeeded"
    ]
    controls = [
        s
        for s in summaries
        if s["run"]["case"] == "10k"
        and not s["run"]["detailed"]
        and s["run"]["status"] == "succeeded"
    ]
    if measured:
        times = [s["run"]["processing_ms"] / 1000 for s in measured]
        cv = statistics.stdev(times) / statistics.mean(times) if len(times) > 1 else None
        lines += [
            "",
            f"10k processing median: **{statistics.median(times):.2f}s**; CV: {_number(cv)}.",
        ]
        all_runs = [summary["run"] for summary in [*measured, *controls]]
        identical = all(
            run.get(field) == all_runs[0].get(field)
            for run in all_runs[1:]
            for field in (
                "base_counts",
                "batch_counts",
                "base_partition_hash",
                "batch_partition_hash",
            )
        )
        lines += [
            "",
            f"Output counts and entity partitions across {len(all_runs)} completed "
            f"10k passes: **{'identical' if identical else 'DIFFERENT'}**.",
        ]
        ranked: dict[str, list[float]] = defaultdict(list)
        for summary in measured:
            for event in summary["spans"]:
                if event["name"] in STAGES:
                    ranked[f"{event['phase']} / {event['name']}"].append(
                        event["duration_ms"] / 1000
                    )
        lines += ["", "Largest measured stage invocations (median across passes):"]
        for name, values in sorted(
            ranked.items(), key=lambda item: statistics.median(item[1]), reverse=True
        )[:5]:
            lines.append(f"- {name}: {statistics.median(values):.3f}s")
        if controls:
            control = controls[0]["run"]["processing_ms"] / 1000
            lines += [
                "",
                f"Control: {control:.2f}s. Detailed profiling overhead estimate: "
                f"{100 * (statistics.median(times) / control - 1):.1f}%. "
                "One control pass; indicative only, with cache/order and scheduling effects.",
            ]
            stage_totals: dict[str, list[float]] = defaultdict(list)
            control_totals: dict[str, float] = defaultdict(float)
            for summary in [*measured, controls[0]]:
                totals: dict[str, float] = defaultdict(float)
                for command in summary["run"]["commands"]:
                    for stage in command["stages"]:
                        if stage["stage"] in STAGES:
                            totals[f"{command['phase']} / {stage['stage']}"] += (
                                stage["duration_ms"] / 1000
                            )
                for name, total in totals.items():
                    if summary["run"]["detailed"]:
                        stage_totals[name].append(total)
                    else:
                        control_totals[name] = total
            lines += [
                "",
                "Stage-ledger times in the profiled and control runs:",
                "",
                "| Phase / stage | Profiled median s | Control s | Difference s |",
                "|---|---:|---:|---:|",
            ]
            for name, values in stage_totals.items():
                median = statistics.median(values)
                baseline = control_totals.get(name)
                lines.append(
                    f"| {name} | {median:.3f} | {_number(baseline)} | "
                    f"{_number(None if baseline is None else median - baseline)} |"
                )
    for summary in summaries:
        run = summary["run"]
        label = run.get("label") or (
            f"{run['case']}-{run['iteration']}" if run["detailed"] else f"{run['case']}-control"
        )
        lines += [
            "",
            f"## {label}",
            "",
            f"[All transformation metrics]({label}/summary.json) · "
            f"[Stage ledger and validation]({label}/result.json)",
        ]
        if run.get("error"):
            lines += ["", f"Failed: {run['error']}"]
        fingerprint = run.get("fingerprint", {})
        if fingerprint:
            lines += [
                "",
                f"CPU quota {fingerprint['cgroup_cpu_max']}; container limit "
                f"{fingerprint['cgroup_memory_max'] / 1024**3:.1f} GiB; DuckDB "
                f"{fingerprint['er_duckdb_threads']} threads / "
                f"{fingerprint['er_duckdb_memory_limit']}. "
                f"DuckDB {fingerprint['duckdb_version']}, Splink {fingerprint['splink_version']}, "
                f"dbt {fingerprint['dbt_core_version']}.",
            ]
        lines += [
            "",
            "| Phase / transformation | Seconds | Input (unit) | Output (unit) | "
            "Input/s | CPU s | Throttled s | Peak MiB |",
            "|---|---:|---|---|---:|---:|---:|---:|",
        ]
        relevant = [
            e
            for e in summary["spans"]
            if e["name"] in STAGES
            or e["name"].startswith("dbt.")
            or e["name"] == "train.estimation_call"
        ]
        for event in sorted(relevant, key=lambda item: item["monotonic_ns"]):
            metrics = event["metrics"]
            name = event.get("method", event["name"])
            lines.append(
                f"| {event['phase']} / {name} | {_number(event['duration_ms'], 1000)} | "
                f"{metrics.get('rows_in', 'unavailable')} "
                f"({metrics.get('input_unit', event.get('unit') or 'unknown')}) | "
                f"{metrics.get('rows_out', 'unavailable')} "
                f"({metrics.get('output_unit', event.get('unit') or 'unknown')}) | "
                f"{_number(event['input_per_second'], digits=1)} | "
                f"{_number(metrics.get('usage_usec'), 1e6)} | "
                f"{_number(metrics.get('throttled_usec'), 1e6)} | "
                f"{_number(event['sampled_memory_peak_bytes'], 1024**2, 1)} |"
            )
        lines += [
            "",
            "### Longest nested transformations",
            "",
            "Parent and child timings overlap. Counts retain their measured units/names.",
            "",
            "| Transformation | Seconds | Counts |",
            "|---|---:|---|",
        ]
        nested = [
            event
            for event in summary["spans"]
            if event["name"] not in STAGES and not event["name"].startswith(("command.", "dbt."))
        ]
        for event in sorted(nested, key=lambda item: item["duration_ms"], reverse=True)[:15]:
            counts = {
                key: value
                for key, value in event["metrics"].items()
                if key.endswith(("_in", "_out", "_count"))
                or key in ("parameter_sets", "iterations", "tf_rows", "labels_changed")
            }
            label_counts = (
                ", ".join(f"{key}={value}" for key, value in counts.items()) or "unavailable"
            )
            lines.append(
                f"| {event['name']} | {_number(event['duration_ms'], 1000)} | {label_counts} |"
            )
        lines += [
            "",
            "### Slowest SQL statements",
            "",
            "| Transformation | Seconds | Scanned | Returned | "
            "Buffer peak MiB | Spill peak MiB | Profile |",
            "|---|---:|---:|---:|---:|---:|---|",
        ]
        for query in summary["queries"][:12]:
            lines.append(
                f"| {query.get('name') or query.get('stage') or 'setup/ledger'} | "
                f"{_number(query['duration_ms'], 1000)} | "
                f"{query.get('rows_scanned')} | {query.get('rows_returned')} | "
                f"{_number(query.get('buffer_peak_bytes'), 1024**2, 1)} | "
                f"{_number(query.get('spill_peak_bytes'), 1024**2, 1)} | "
                f"[JSON]({label}/sql/{Path(query['profile_path']).name}) |"
            )
        lines += [
            "",
            "### Repeated SQL executions",
            "",
            "Capture intervals run from the first profile emitted to the last. "
            "They include collector backpressure and are not native execution timers. "
            "Engine time sums DuckDB query latency; repeated parameter bindings appear here "
            "even when every individual query is fast.",
            "",
            "| Statement | Executions | Engine s | Capture interval s |",
            "|---|---:|---:|---:|",
        ]
        for batch in summary["repeated_statements"][:10]:
            statement = batch["statement"].replace("\n", " ").replace("|", "&#124;")[:180]
            elapsed = (batch["last_capture_ns"] - batch["first_capture_ns"]) / 1e9
            lines.append(
                f"| `{statement}` | {batch['executions']} | "
                f"{batch['engine_ms'] / 1000:.3f} | {elapsed:.3f} |"
            )
        lines += [
            "",
            "### SQL operators",
            "",
            "Operator timings sum worker execution time across queries; "
            "they are not stage wall time.",
            "",
            "| Operator | Seconds | Emitted rows | Instances |",
            "|---|---:|---:|---:|",
        ]
        for entry in summary["operators"][:12]:
            lines.append(
                f"| {entry['name']} | {entry['seconds']:.3f} | {entry['rows']} | {entry['calls']} |"
            )
        if run.get("quality"):
            lines += [
                "",
                "Quality (reported separately from performance):",
                "",
                "```json",
                json.dumps(run["quality"], indent=2),
                "```",
            ]
    (out / "report.md").write_text("\n".join(lines) + "\n")
    (out / "results.json").write_text(
        json.dumps({"schema_version": 1, "runs": [s["run"] for s in summaries]}, indent=2)
    )


def benchmark_result(directory: Path, run: dict[str, Any]) -> dict[str, Any]:
    """Keep the existing six-phase benchmark JSON contract using measured intervals.

    Every ingestion invocation contributes its own ledger record, even when a
    subsequent source overwrites (run_id, stage) in the lake's current ledger.
    """
    from run_benchmark import PHASES, PhaseRecord, incremental_ratio

    samples = [
        json.loads(line) for line in (directory / "resources.jsonl").read_text().splitlines()
    ]
    events = events_at(directory)
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for command in run["commands"]:
        if command["phase"] == "setup":
            continue
        name = command["command"][1]
        phase = (
            "incremental_cycle"
            if command["phase"] == "batch"
            else "full_match_reconcile"
            if name in ("match", "reconcile")
            else name
        )
        grouped[phase].append(command)
    phases = []
    records = []
    for name in PHASES:
        commands = grouped[name]
        stages = [stage for command in commands for stage in command["stages"]]
        duration = sum(command["duration_ms"] for command in commands)
        assert duration >= sum(stage["duration_ms"] for stage in stages)
        rows_out = sum(stage["rows_out"] or 0 for stage in stages)
        memory = resource_window(samples, commands[0]["started_ns"], commands[-1]["ended_ns"])
        candidate_counts = [
            stage["candidate_pairs"] for stage in stages if stage["candidate_pairs"] is not None
        ]
        # The incremental scorer counts scored pairs, not a full-corpus candidate probe.
        # Record the current-corpus blocking count from the validation query for that phase.
        candidates = max(
            candidate_counts,
            default=run.get("batch_candidate_pairs", 0) if name == "incremental_cycle" else 0,
        )
        record = PhaseRecord(
            name=name,
            wall_ms=duration,
            rows_in=sum(stage["rows_in"] or 0 for stage in stages),
            rows_out=rows_out,
            records_per_sec=rows_out / (duration / 1000),
            candidate_pair_count=candidates,
            pairs_above_auto_merge=sum(stage["pairs_above_auto_merge"] or 0 for stage in stages),
            snapshot_count=sum(stage["snapshot_end"] - stage["snapshot_start"] for stage in stages),
            stage_duration_ms=sum(stage["duration_ms"] for stage in stages),
        )
        records.append(record)
        phases.append(
            {
                "name": name,
                "wall_ms": duration,
                "wall_ms_cv": 0.0,
                "records_per_sec": record.records_per_sec,
                "candidate_pair_count": candidates,
                "pairs_above_auto_merge": record.pairs_above_auto_merge,
                "memory_peak_bytes": memory["sampled_memory_peak_bytes"],
                "snapshot_count": record.snapshot_count,
            }
        )
    overall = resource_window(samples, 0, 2**63)
    return {
        "scale": run["case"],
        "verdict": "NO_BASELINE",
        "repeat": 1,
        "incremental_ratio": incremental_ratio(records),
        "fingerprint": run["fingerprint"],
        "blocking_recall": run["quality"]["blocking_recall"],
        "quality": run["quality"]["quality"],
        "phases": phases,
        "memory": {
            "duckdb_buffer_peak_bytes": max(
                (e.get("buffer_peak_bytes") or 0 for e in events if e["event"] == "sql_profile"),
                default=0,
            ),
            "rss_peak_bytes": overall["sampled_process_rss_sum_peak_bytes"],
            "cgroup_peak_bytes": overall["sampled_memory_peak_bytes"],
        },
    }
