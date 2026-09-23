"""Alternate initial/incremental/correction trials across explicit performance arms."""

from __future__ import annotations

import argparse
import copy
import json
import statistics
import subprocess
import sys
from dataclasses import asdict
from pathlib import Path

import yaml
from acceptance import evaluate

from er.eval.metrics import PairwiseMetrics

ROOT = Path(__file__).resolve().parents[1]
ARMS = ("current", "em_1m", "em_5m", "em_10m", "without_name_postal", "narrow_name_postal")


def configuration(original: dict, arm: str, seed: int) -> dict:
    document = copy.deepcopy(original)
    document["generator"]["seed"] = seed
    if arm.startswith("em_"):
        document["training"]["em"]["max_pairs"] = int(arm[3:-1]) * 1_000_000
    elif arm == "without_name_postal":
        document["blocking"] = [
            row for row in document["blocking"] if row["key_type"] != "name_postal"
        ]
    elif arm == "narrow_name_postal":
        for row in document["blocking"]:
            if row["key_type"] == "name_postal":
                row["expr"] = "(" + row["expr"] + ") || '|' || substr(given_name,1,1)"
    elif arm != "current":
        raise ValueError(arm)
    return document


def report(out: Path, scale: str) -> dict:
    trials = [json.loads(path.read_text()) for path in sorted(out.glob("*/trial.json"))]
    grouped = {}
    for trial in trials:
        grouped.setdefault((trial["profile"], trial["seed"], trial["arm"]), []).append(trial)
    results = []
    workloads = {
        "initial": ("processing_seconds", "initial_quality"),
        "incremental": ("incremental_seconds", "quality"),
        "correction": ("correction_seconds", "correction_quality"),
    }
    for (profile, seed, arm), candidates in grouped.items():
        baseline = grouped.get((profile, seed, "current"))
        if arm == "current" or not baseline:
            continue
        # Each iteration has a fresh baseline on the same corpus. A slowdown in
        # every paired repeat is evidence against promotion, even if the target
        # workload's median improves. Fewer than three repeats cannot establish it.
        baselines_by_repeat = {t["iteration"]: t for t in baseline}
        slowdowns = []
        for other, (other_timing, _) in workloads.items():
            paired = [t for t in candidates if t["iteration"] in baselines_by_repeat]
            if len(paired) >= 3 and all(
                t["run"][other_timing] > baselines_by_repeat[t["iteration"]]["run"][other_timing]
                for t in paired
            ):
                slowdowns.append(other)
        for workload, (timing, quality) in workloads.items():
            base_time = statistics.median(t["run"][timing] for t in baseline)
            candidate_time = statistics.median(t["run"][timing] for t in candidates)
            # Never average away a quality failure in a repetition.
            gates = [
                asdict(
                    evaluate(
                        PairwiseMetrics(**baseline[0]["run"][quality]["families"]["cluster"]),
                        PairwiseMetrics(**trial["run"][quality]["families"]["cluster"]),
                        baseline_seconds=base_time,
                        candidate_seconds=candidate_time,
                        records=10_000_000 if scale == "10m" else 1_000_000 if scale == "1m" else 0,
                        repetitions=min(len(baseline), len(candidates)),
                        other_workload_regressed=any(w != workload for w in slowdowns),
                    )
                )
                for trial in candidates
            ]
            results.append(
                {
                    "profile": profile,
                    "seed": seed,
                    "arm": arm,
                    "workload": workload,
                    "baseline_seconds": base_time,
                    "candidate_seconds": candidate_time,
                    "repeatable_slowdowns": slowdowns,
                    "gates": gates,
                }
            )
    result = {
        "scale": scale,
        "trials": trials,
        "comparisons": results,
        "promotion": "Requires all held-out datasets and review of other-workload regressions.",
    }
    (out / "campaign.json").write_text(json.dumps(result, indent=2))
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, help="immutable image ID or digest")
    parser.add_argument("--image-source", help="source commit/hash that produced --image")
    parser.add_argument("--scale", choices=("smoke", "100k", "1m", "10m"), default="1m")
    parser.add_argument(
        "--profiles", nargs="+", choices=("baseline", "hard-v1"), default=["baseline", "hard-v1"]
    )
    parser.add_argument("--seeds", nargs="+", type=int, default=[20260102, 20260103])
    parser.add_argument("--arms", nargs="+", choices=ARMS, default=list(ARMS))
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--config", type=Path, default=ROOT / "configs/default.yaml")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--local", action="store_true")
    args = parser.parse_args()
    if args.repeat < 1 or "current" not in args.arms:
        parser.error("a campaign requires positive repeat and the current arm")
    if not (args.image.startswith("sha256:") or "@sha256:" in args.image):
        parser.error("--image must be immutable")
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=False)
    source = yaml.safe_load(args.config.read_text())
    for profile in args.profiles:
        for seed in args.seeds:
            for iteration in range(args.repeat):
                arms = args.arms if iteration % 2 == 0 else list(reversed(args.arms))
                for arm in arms:
                    name = f"{profile}-{seed}-{iteration + 1}-{arm}"
                    directory = args.out / name
                    configuration_path = args.out / (name + ".yaml")
                    configuration_path.write_text(yaml.safe_dump(configuration(source, arm, seed)))
                    command = [
                        sys.executable,
                        str(ROOT / "benchmarks/full_pipeline.py"),
                        "--image",
                        args.image,
                        "--scale",
                        args.scale,
                        "--repeat",
                        "1",
                        "--with-incremental",
                        "--with-correction",
                        "--generator-profile",
                        profile,
                        "--config",
                        str(configuration_path),
                        "--out",
                        str(directory),
                        "--corpus-root",
                        str(args.out / "inputs"),
                    ]
                    if args.local:
                        command.append("--local")
                    if args.image_source:
                        command += ["--image-source", args.image_source]
                    subprocess.run(command, cwd=ROOT, check=True)
                    summary = json.loads((directory / "results.json").read_text())
                    (directory / "trial.json").write_text(
                        json.dumps(
                            {
                                "profile": profile,
                                "seed": seed,
                                "arm": arm,
                                "iteration": iteration + 1,
                                "run": summary["runs"][0],
                            },
                            indent=2,
                        )
                    )
                    report(args.out, args.scale)


if __name__ == "__main__":
    main()
