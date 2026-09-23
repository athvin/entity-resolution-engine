"""Compare unprofiled implementation trials with full and incremental correctness gates."""

from __future__ import annotations

import argparse
import json
import statistics
from pathlib import Path

from full_pipeline import summarize_run
from workload_report import compare_model_parameters, comparison_errors, trained_parameters
from workload_validation import compare_score_files


def trials(paths: list[Path]) -> list[dict]:
    result = []
    for path in paths:
        files = [path] if path.is_file() else sorted(path.glob("run-*/result.json"))
        if not files:
            raise ValueError(f"no trials in {path}")
        for file in files:
            run = json.loads(file.read_text())
            if run.get("status") != "succeeded" or run.get("detailed"):
                raise ValueError(f"{file}: need a successful unprofiled trial")
            result.append({"path": str(file.resolve()), "run": run, "summary": summarize_run(file)})
    return result


def report(baseline: list[Path], candidate: list[Path], out: Path) -> dict:
    old, new = trials(baseline), trials(candidate)
    result: dict = {"baseline": old, "candidate": new, "comparisons": [], "errors": []}
    reference = old[0]
    ref_dir = Path(reference["path"]).parent
    ref_model, _ = trained_parameters(ref_dir, reference["run"])
    for trial in [*old[1:], *new]:
        directory = Path(trial["path"]).parent
        errors = comparison_errors(reference["run"], trial["run"], require_same_image=False)
        model, evidence = trained_parameters(directory, trial["run"])
        fitted = compare_model_parameters(ref_model, model)
        scores = {
            phase: compare_score_files(
                ref_dir / f"{phase}-scores.json.gz", directory / f"{phase}-scores.json.gz"
            )
            for phase in ("base", "batch")
        }
        if fitted["status"] != "passed" or any(s["status"] != "passed" for s in scores.values()):
            errors.append("model or pair probabilities differ beyond allowed roundoff")
        result["comparisons"].append(
            {
                "path": trial["path"],
                "model": fitted,
                "model_evidence": evidence,
                "scores": scores,
                "errors": errors,
            }
        )
        result["errors"].extend(errors)
    for arm, values in (("baseline", old), ("candidate", new)):
        images = {trial["run"]["fingerprint"]["image_digest"] for trial in values}
        if len(images) != 1:
            result["errors"].append(f"{arm}: multiple application images")
    result["workloads"] = {}
    for name, key in (("reload", "processing_seconds"), ("incremental", "incremental_seconds")):
        a = [t["summary"][key] for t in old]
        b = [t["summary"][key] for t in new]
        result["workloads"][name] = {
            "baseline_seconds": a,
            "candidate_seconds": b,
            "baseline_median": statistics.median(a),
            "candidate_median": statistics.median(b),
            "median_reduction_percent": 100 * (1 - statistics.median(b) / statistics.median(a)),
            "baseline_cv": statistics.pstdev(a) / statistics.mean(a),
            "candidate_cv": statistics.pstdev(b) / statistics.mean(b),
            "ranges_overlap": max(min(a), min(b)) <= min(max(a), max(b)),
        }
        workload = result["workloads"][name]
        resource_key = "resources" if name == "reload" else "incremental_resources"
        workload["resources"] = {
            arm: {
                metric: [trial["summary"][resource_key][metric] for trial in values]
                for metric in ("cpu_s", "sampled_memory_peak_bytes")
            }
            for arm, values in (("baseline", old), ("candidate", new))
        }
        phase = "base" if name == "reload" else "batch"
        stage_values: dict[str, dict[str, list[float]]] = {}
        for arm, values in (("baseline", old), ("candidate", new)):
            for trial in values:
                stages: dict[str, float] = {}
                for command in trial["run"]["commands"]:
                    if command["phase"] != phase:
                        continue
                    for stage in command.get("stages", []):
                        label = stage["stage"]
                        stages[label] = stages.get(label, 0) + stage["duration_ms"] / 1000
                for label, seconds in stages.items():
                    stage_values.setdefault(label, {"baseline": [], "candidate": []})[arm].append(
                        seconds
                    )
        workload["stage_ledger_seconds"] = stage_values
    result["correctness"] = "passed" if not result["errors"] else "failed"
    result["repeated"] = min(len(old), len(new)) >= 3
    result["both_medians_improved"] = all(
        w["median_reduction_percent"] > 0 for w in result["workloads"].values()
    )
    out.mkdir(parents=True, exist_ok=True)
    (out / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    lines = [
        "# Workload tuning comparison",
        "",
        f"Correctness: **{result['correctness']}**.",
        "",
        "Unprofiled complete command times. Full reference/validation are excluded.",
        "",
        "| Workload | Baseline median | Candidate median | Reduction |",
        "|---|---:|---:|---:|",
    ]
    for name, w in result["workloads"].items():
        lines.append(
            f"| {name} | {w['baseline_median']:.3f}s | {w['candidate_median']:.3f}s | "
            f"{w['median_reduction_percent']:.2f}% |"
        )
    lines += [
        "",
        "Stage ledger medians exclude command startup; they do not sum to the command totals.",
        "",
        "| Workload / stage | Baseline median | Candidate median |",
        "|---|---:|---:|",
    ]
    for name, workload in result["workloads"].items():
        for label, times in workload["stage_ledger_seconds"].items():
            lines.append(
                f"| {name} / {label} | {statistics.median(times['baseline']):.3f}s | "
                f"{statistics.median(times['candidate']):.3f}s |"
            )
    lines += [
        "",
        "| Workload / arm | Total-time range | Median CPU seconds | Largest sampled peak GiB |",
        "|---|---:|---:|---:|",
    ]
    for name, workload in result["workloads"].items():
        for arm in ("baseline", "candidate"):
            resources = workload["resources"][arm]
            times = workload[f"{arm}_seconds"]
            lines.append(
                f"| {name} / {arm} | {min(times):.3f}–{max(times):.3f}s | "
                f"{statistics.median(resources['cpu_s']):.3f} | "
                f"{max(resources['sampled_memory_peak_bytes']) / 1024**3:.3f} |"
            )
    lines += [
        "",
        "Memory includes pipeline children and page cache; it excludes catalog/object-store "
        "memory. Short peaks can fall between samples.",
    ]
    lines += [
        "",
        f"Trials: {len(old)} baseline / {len(new)} candidate. "
        f"Both workload medians improved: {result['both_medians_improved']}.",
        "A single comparison is preliminary. Inspect repetitions, spread, resources "
        "and stage costs before promotion.",
        "",
        "[Machine-readable evidence](comparison.json)",
    ]
    if result["errors"]:
        lines += ["", *[f"- {error}" for error in result["errors"]]]
    (out / "report.md").write_text("\n".join(lines) + "\n")
    if result["errors"]:
        raise ValueError("tuning comparison failed correctness/provenance gates")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, nargs="+", required=True)
    parser.add_argument("--candidate", type=Path, nargs="+", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    report(args.baseline, args.candidate, args.out)
