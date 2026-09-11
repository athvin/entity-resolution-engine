"""`benchmarks/report.py` — the single benchmark entrypoint (S10.3, S10.4, M24).

One tool, three modes:

* ``--run --scale S`` runs ``--repeat N`` measured passes, aggregates them (per-phase,
  per-metric MEDIAN, plus the coefficient of variation of ``wall_ms``), writes the result
  JSON and a sibling ``report.md``, and prints a verdict.
* ``--compare RUN --baselines-dir DIR --scale S`` loads an already-measured run and
  compares its phase medians to the committed baseline.
* ``--write-baseline`` freezes a run as the baseline for its scale — and REFUSES a
  ``NON_COMPARABLE`` run, because a baseline taken on the wrong-shaped machine silently
  redefines every later comparison.

Four verdicts and their exit codes are the contract (S10.3): ``OK`` 0, ``NO_BASELINE`` 0
(the first run at a scale is a bootstrap, not a failure), ``REGRESSION`` 1,
``NON_COMPARABLE`` 3. ``2`` is reserved for bad arguments and unreadable input — these are
NOT ``er`` exit codes, and ``3`` here means NON_COMPARABLE, never an S4.0 precondition.

Comparability (S10.4) is decided before any threshold: a run measured outside its scale's
CPU/memory envelope, at the wrong scale, or with a noisy phase (CV > 0.15) carries no gate
authority. The regression threshold compares phase MEDIANS and its boundary is inclusive:
exactly ``F ×`` baseline is ``OK``; strictly above is ``REGRESSION``.
"""

from __future__ import annotations

import argparse
import json
import statistics
from collections.abc import Mapping, Sequence
from enum import StrEnum
from pathlib import Path
from typing import Any

from scales import Scale, _memory_bytes, get_scale
from schema import BenchResultError, validate_bench_result, write_result

__all__ = [
    "CV_CEILING",
    "DEFAULT_FAIL_THRESHOLD",
    "DEFAULT_REPEAT",
    "Verdict",
    "aggregate_passes",
    "compare_to_baseline",
    "comparability_violations",
    "main",
    "render_report_md",
    "write_baseline",
]

DEFAULT_REPEAT = 3
DEFAULT_FAIL_THRESHOLD = 1.25
#: A phase whose wall-time CV exceeds this is too noisy to gate on (S10.4).
CV_CEILING = 0.15

#: The per-phase metrics aggregated by median across passes (S10.3).
_MEDIAN_METRICS = (
    "wall_ms",
    "records_per_sec",
    "candidate_pair_count",
    "pairs_above_auto_merge",
    "memory_peak_bytes",
    "snapshot_count",
)

_EXIT_BAD_USAGE = 2


class Verdict(StrEnum):
    """S10.3's four verdicts and their tool exit codes (NOT ``er`` codes)."""

    OK = "OK"
    NO_BASELINE = "NO_BASELINE"
    REGRESSION = "REGRESSION"
    NON_COMPARABLE = "NON_COMPARABLE"

    @property
    def exit_code(self) -> int:
        return {
            Verdict.OK: 0,
            Verdict.NO_BASELINE: 0,
            Verdict.REGRESSION: 1,
            Verdict.NON_COMPARABLE: 3,
        }[self]


def _phase_map(result: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    return {str(phase["name"]): phase for phase in result["phases"]}


def aggregate_passes(passes: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    """Aggregate N single-pass results into one, per S10.3.

    Each phase metric becomes the MEDIAN across passes; `wall_ms_cv` becomes the
    coefficient of variation (``stdev / mean``) of that phase's `wall_ms`, 0.0 when a
    single pass leaves it undefined. The fingerprint, scale, memory and quality blocks are
    taken from the first pass (they are properties of the run, not of a single sample);
    `repeat` records how many passes were folded.

    Raises:
        ValueError: no passes, or the passes disagree on their phase set.
    """
    if not passes:
        raise ValueError("aggregate_passes needs at least one pass")
    phase_names = [str(phase["name"]) for phase in passes[0]["phases"]]
    for index, one in enumerate(passes):
        names = [str(phase["name"]) for phase in one["phases"]]
        if names != phase_names:
            raise ValueError(f"pass {index} has phases {names}, expected {phase_names}")

    base = passes[0]
    phases: list[dict[str, Any]] = []
    for name in phase_names:
        samples = [_phase_map(one)[name] for one in passes]
        wall = [float(sample["wall_ms"]) for sample in samples]
        mean = statistics.mean(wall)
        cv = (statistics.stdev(wall) / mean) if len(wall) > 1 and mean else 0.0
        phase: dict[str, Any] = {"name": name, "wall_ms_cv": cv}
        for metric in _MEDIAN_METRICS:
            values = [sample[metric] for sample in samples]
            median = statistics.median(values)
            # Integer metrics stay integers; the median of an even count of ints can be a
            # float, so a metric whose samples are all ints is coerced back.
            phase[metric] = (
                int(round(median)) if all(isinstance(v, int) for v in values) else float(median)
            )
        phases.append(phase)

    ratios = [float(one["incremental_ratio"]) for one in passes]
    return {
        "scale": base["scale"],
        "verdict": Verdict.NO_BASELINE.value,
        "repeat": len(passes),
        "incremental_ratio": statistics.median(ratios),
        "blocking_recall": base["blocking_recall"],
        "fingerprint": base["fingerprint"],
        "phases": phases,
        "memory": base["memory"],
        "quality": base["quality"],
    }


def comparability_violations(result: Mapping[str, Any], scale: Scale) -> list[str]:
    """Every S10.4 condition under which `result` carries no gate authority.

    Each is a separate, individually-named reason so a `NON_COMPARABLE` verdict says which
    envelope it fell outside of. An empty list means the run is comparable.
    """
    violations: list[str] = []
    fingerprint = result["fingerprint"]

    if str(fingerprint["scale"]) != scale.name:
        violations.append(
            f"scale mismatch: fingerprint scale {fingerprint['scale']!r} != {scale.name!r}"
        )

    cpu_max = str(fingerprint["cgroup_cpu_max"])
    try:
        cpu_allowed = float(cpu_max)
    except ValueError:
        cpu_allowed = None
    if cpu_allowed is None or cpu_allowed != float(scale.cpu_limit):
        violations.append(f"cgroup cpu quota {cpu_max!r} != cpu_limit {scale.cpu_limit}")

    expected_mem = _memory_bytes(scale.mem_limit, scale.name, "mem_limit")
    if int(fingerprint["cgroup_memory_max"]) != expected_mem:
        violations.append(
            f"cgroup memory.max {fingerprint['cgroup_memory_max']} != mem_limit "
            f"{scale.mem_limit} ({expected_mem} bytes)"
        )

    if int(fingerprint["er_duckdb_threads"]) != scale.cpu_limit:
        violations.append(
            f"ER_DUCKDB_THREADS {fingerprint['er_duckdb_threads']} != cpu_limit {scale.cpu_limit}"
        )

    if str(fingerprint["er_duckdb_memory_limit"]) != scale.duckdb_memory_limit:
        violations.append(
            f"ER_DUCKDB_MEMORY_LIMIT {fingerprint['er_duckdb_memory_limit']!r} != "
            f"duckdb_memory_limit {scale.duckdb_memory_limit!r}"
        )

    for phase in result["phases"]:
        cv = float(phase["wall_ms_cv"])
        if cv > CV_CEILING:
            violations.append(
                f"phase {phase['name']!r} wall_ms CV {cv:.3f} exceeds the {CV_CEILING} ceiling"
            )

    return violations


def compare_to_baseline(
    run: Mapping[str, Any],
    baseline: Mapping[str, Any] | None,
    scale: Scale,
    *,
    fail_threshold: float = DEFAULT_FAIL_THRESHOLD,
) -> tuple[Verdict, list[str]]:
    """The verdict for `run`, and the lines explaining it (S10.3, S10.4).

    Comparability is decided first: a run outside its envelope is `NON_COMPARABLE` and is
    never threshold-compared. With no baseline the run is `NO_BASELINE` (a bootstrap, exit
    0). Otherwise each phase median is compared to the baseline's; strictly above
    ``fail_threshold ×`` baseline is a `REGRESSION` naming the phase, and the boundary is
    inclusive (exactly ``F ×`` is `OK`).
    """
    violations = comparability_violations(run, scale)
    if violations:
        return Verdict.NON_COMPARABLE, violations
    if baseline is None:
        return Verdict.NO_BASELINE, ["no committed baseline for this scale; this run bootstraps it"]

    base_phases = _phase_map(baseline)
    regressions: list[str] = []
    for phase in run["phases"]:
        name = str(phase["name"])
        if name not in base_phases:
            continue
        run_wall = float(phase["wall_ms"])
        base_wall = float(base_phases[name]["wall_ms"])
        ceiling = base_wall * fail_threshold
        if run_wall > ceiling:
            regressions.append(
                f"phase {name!r}: wall_ms {run_wall:.1f} > {fail_threshold}x baseline "
                f"{base_wall:.1f} (ceiling {ceiling:.1f})"
            )
    if regressions:
        return Verdict.REGRESSION, regressions
    return Verdict.OK, ["within threshold of the baseline for every phase"]


def write_baseline(run: Mapping[str, Any], baselines_dir: Path, scale: Scale) -> Path:
    """Freeze `run` as `<baselines_dir>/<scale>.json`, refusing a NON_COMPARABLE run.

    Raises:
        BenchResultError: the run is not comparable — a baseline taken off the wrong-shaped
            machine would redefine every later comparison, so it is refused and the
            directory is left untouched.
    """
    violations = comparability_violations(run, scale)
    if violations:
        raise BenchResultError(
            "refusing to write a baseline from a NON_COMPARABLE run: " + "; ".join(violations)
        )
    baselines_dir.mkdir(parents=True, exist_ok=True)
    destination = baselines_dir / f"{scale.name}.json"
    write_result(run, destination)
    return destination


def render_report_md(result: Mapping[str, Any]) -> str:
    """A one-row-per-phase Markdown table plus the run's verdict and ratio (S10.3)."""
    lines = [
        f"# Benchmark — scale `{result['scale']}` (repeat {result['repeat']})",
        "",
        f"- verdict: **{result['verdict']}**",
        f"- incremental_ratio: {float(result['incremental_ratio']):.3f}",
        f"- blocking_recall: {float(result['blocking_recall']):.3f}",
        "",
        "| phase | wall_ms | wall_ms_cv | records_per_sec | cand_pairs "
        "| >=auto_merge | mem_peak | snapshots |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for phase in result["phases"]:
        lines.append(
            f"| {phase['name']} | {float(phase['wall_ms']):.1f} | {float(phase['wall_ms_cv']):.3f} "
            f"| {float(phase['records_per_sec']):.1f} | {phase['candidate_pair_count']} "
            f"| {phase['pairs_above_auto_merge']} | {phase['memory_peak_bytes']} "
            f"| {phase['snapshot_count']} |"
        )
    quality = result["quality"]
    lines += [
        "",
        "## Quality (reported, never gated)",
        "",
        "| family | precision | recall | f1 |",
        "|---|---|---|---|",
        f"| edge | {float(quality['edge_precision']):.3f} | {float(quality['edge_recall']):.3f} "
        f"| {float(quality['edge_f1']):.3f} |",
        f"| cluster | {float(quality['cluster_precision']):.3f} "
        f"| {float(quality['cluster_recall']):.3f} | {float(quality['cluster_f1']):.3f} |",
        f"| blocking | — | {float(result['blocking_recall']):.3f} | — |",
    ]
    return "\n".join(lines) + "\n"


def _load_json(path: Path) -> dict[str, Any]:
    document: Any = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, dict):
        raise BenchResultError(f"{path}: expected a JSON object, got {type(document).__name__}")
    return document


def _load_baseline(baselines_dir: Path | None, scale_name: str) -> dict[str, Any] | None:
    if baselines_dir is None:
        return None
    path = baselines_dir / f"{scale_name}.json"
    if not path.exists():
        return None
    return _load_json(path)


def _emit(result: dict[str, Any], out: Path | None) -> None:
    if out is None:
        return
    write_result(result, out)
    (out.parent / "report.md").write_text(render_report_md(result), encoding="utf-8")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="report.py", description="Run or compare a benchmark.")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--run", action="store_true", help="measure --repeat passes of a scale")
    mode.add_argument("--compare", metavar="RUN_JSON", help="compare an existing run JSON")
    parser.add_argument("--scale", help="scale name (smoke, 10k, ...)")
    parser.add_argument("--repeat", type=int, default=DEFAULT_REPEAT, help="measured passes")
    parser.add_argument("--out", type=Path, help="where to write the run JSON (+ report.md)")
    parser.add_argument("--baselines-dir", type=Path, help="directory of <scale>.json baselines")
    parser.add_argument("--fail-threshold", type=float, default=DEFAULT_FAIL_THRESHOLD)
    parser.add_argument("--write-baseline", action="store_true", help="freeze this run as baseline")
    return parser


def _resolve_run(args: argparse.Namespace) -> dict[str, Any]:
    """The run document for this invocation: loaded (`--compare`) or measured (`--run`)."""
    if args.compare is not None:
        run = _load_json(Path(args.compare))
        validate_bench_result(run)
        return run
    return _measure_and_aggregate(args.scale, repeat=args.repeat)


#: The three source systems a delivery is laid out under for `er ingest` (S4.1).
_SOURCES = ("crm", "billing", "webforms")


class _SubprocessRunner:
    """The production `Runner`: shells out to `er`/`dbt`, reads the lake off one cursor.

    Used only by `--run` (the in-image path ER-101 exercises); the unit-tested comparison
    functions never touch it. `standardize` is the NoOp timing verb plus the dbt staging +
    intermediate build that actually materialises `int_std_records`.
    """

    def __init__(
        self, connection: Any, *, drop_for_run: Mapping[str, Path], auto_merge: float
    ) -> None:
        import os

        self._connection = connection
        self._drop_for_run = drop_for_run
        self._auto_merge = auto_merge
        self._env = dict(os.environ)

    def _sh(self, argv: Sequence[str]) -> None:
        import subprocess

        result = subprocess.run(
            list(argv), capture_output=True, text=True, env=self._env, check=False
        )
        if result.returncode not in (0, 10):
            raise RuntimeError(
                f"{' '.join(argv)} -> {result.returncode}\n{result.stdout}\n{result.stderr}"
            )

    def er(self, args: Sequence[str]) -> None:
        from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR

        argv = list(args)
        if argv[0] == "standardize":
            self._sh(
                [
                    "dbt",
                    "build",
                    "--select",
                    "staging",
                    "intermediate",
                    "--target",
                    "lake",
                    "--project-dir",
                    DBT_PROJECT_DIR,
                    "--profiles-dir",
                    DBT_PROFILES_DIR,
                ]
            )
        elif argv[0] == "ingest":
            run_id = argv[argv.index("--run-id") + 1]
            argv += ["--path", str(self._drop_for_run[run_id])]
        self._sh(["er", *argv])

    def stage_rows(self, run_id: str) -> list[Any]:
        from run_benchmark import StageRow

        return [
            StageRow(str(s), int(d or 0), int(ri or 0), int(ro or 0), int(a or 0), int(b or 0))
            for s, d, ri, ro, a, b in self._connection.execute(
                "SELECT stage, duration_ms, rows_in, rows_out, snapshot_start, snapshot_end "
                "FROM lake.main.run_stages WHERE run_id = ? AND status = 'succeeded'",
                [run_id],
            ).fetchall()
        ]

    def candidate_pair_count(self) -> int:
        row = self._connection.execute(
            "SELECT count(*) FROM (SELECT DISTINCT a.record_key, b.record_key "
            "FROM lake.main.int_blocking_keys a JOIN lake.main.int_blocking_keys b "
            "ON a.key_type = b.key_type AND a.key_value = b.key_value "
            "AND a.record_key < b.record_key)"
        ).fetchone()
        return int(row[0]) if row else 0

    def pairs_above_auto_merge(self, run_id: str) -> int:
        row = self._connection.execute(
            "SELECT count(*) FROM lake.main.match_scores "
            "WHERE run_id = ? AND match_probability >= ?",
            [run_id, self._auto_merge],
        ).fetchone()
        return int(row[0]) if row else 0


def _single_pass_result(
    records: Sequence[Any], fingerprint: Mapping[str, Any], peaks: Any
) -> dict[str, Any]:
    """One measured pass as a schema-valid result (quality is ER-100's; zeros here)."""
    from run_benchmark import incremental_ratio

    return {
        "scale": fingerprint["scale"],
        "verdict": Verdict.NO_BASELINE.value,
        "repeat": 1,
        "incremental_ratio": incremental_ratio(records),
        "blocking_recall": 0.0,
        "fingerprint": dict(fingerprint),
        "phases": [
            {
                "name": r.name,
                "wall_ms": r.wall_ms,
                "wall_ms_cv": 0.0,
                "records_per_sec": r.records_per_sec,
                "candidate_pair_count": r.candidate_pair_count,
                "pairs_above_auto_merge": r.pairs_above_auto_merge,
                "memory_peak_bytes": peaks.memory_peak_bytes,
                "snapshot_count": r.snapshot_count,
            }
            for r in records
        ],
        "memory": {
            "duckdb_buffer_peak_bytes": peaks.duckdb_memory_bytes,
            "rss_peak_bytes": peaks.rss_bytes,
            "cgroup_peak_bytes": peaks.cgroup_peak_bytes or 0,
        },
        "quality": {
            "edge_precision": 0.0,
            "edge_recall": 0.0,
            "edge_f1": 0.0,
            "cluster_precision": 0.0,
            "cluster_recall": 0.0,
            "cluster_f1": 0.0,
        },
    }


def _measure_and_aggregate(scale_name: str, *, repeat: int) -> dict[str, Any]:
    """Generate the corpus once, then measure `repeat` passes and aggregate them (S10.3).

    The in-image path. Generation and `er init` run before measurement and are excluded
    from it (S10.3); the corpus is reused across passes.
    """
    import os
    import shutil
    import subprocess
    import tempfile

    from fingerprint import environment_fingerprint
    from memory import MemorySampler, current_duckdb_memory_bytes
    from run_benchmark import run_pass
    from ulid import ULID

    from er.config.hashing import config_hash
    from er.config.loader import load_config
    from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR
    from er.lake.ducklake import connect

    scale = get_scale(scale_name)
    cfg = load_config(Path(os.environ["ER_CONFIG"]))
    staging = Path(tempfile.mkdtemp(prefix="er-bench-corpus-"))
    subprocess.run(
        [
            "python",
            "-m",
            "fixtures.generator.cli",
            "--personas",
            str(scale.personas),
            "--records",
            str(scale.records),
            "--batch",
            str(scale.incremental_batch),
            "--seed",
            str(cfg.generator.seed),
            "--out",
            str(staging),
            "--config",
            os.environ["ER_CONFIG"],
        ],
        check=True,
    )
    drops: dict[str, Path] = {}
    for label in ("base", "batch"):
        root = staging / f"_drop_{label}"
        for source in _SOURCES:
            (root / source).mkdir(parents=True, exist_ok=True)
            src = (
                staging / f"{source}.csv"
                if label == "base"
                else staging / "batch" / f"{source}.csv"
            )
            shutil.copy(src, root / source / f"{source}.csv")
        drops[label] = root

    subprocess.run(["er", "init"], check=False)
    # Seed the reference tables (e.g. nickname_variants) the staging models read, once,
    # before any measured pass. Excluded from the timing like generation and init (S10.3).
    subprocess.run(
        [
            "dbt",
            "seed",
            "--target",
            "lake",
            "--project-dir",
            DBT_PROJECT_DIR,
            "--profiles-dir",
            DBT_PROFILES_DIR,
        ],
        check=True,
    )
    passes: list[dict[str, Any]] = []
    with connect() as connection:
        for _ in range(repeat):
            run_ids = {"base": str(ULID()), "train": str(ULID()), "incremental": str(ULID())}
            runner = _SubprocessRunner(
                connection,
                drop_for_run={
                    run_ids["base"]: drops["base"],
                    run_ids["incremental"]: drops["batch"],
                },
                auto_merge=cfg.thresholds.auto_merge,
            )
            sampler = MemorySampler(duckdb_source=lambda: current_duckdb_memory_bytes(connection))
            sampler.start()
            try:
                records = run_pass(runner, run_ids)
            finally:
                # Always join the sampler, so a failed pass cannot leave a thread sampling
                # a connection that the enclosing `with` is about to close.
                peaks = sampler.stop()
            active = connection.execute(
                "SELECT model_version, tf_snapshot_id FROM lake.main.model_registry "
                "WHERE status='active'"
            ).fetchone()
            fingerprint = environment_fingerprint(
                scale=scale.name,
                connection=connection,
                config_hash=config_hash(cfg),
                generator_seed=cfg.generator.seed,
                model_version=str(active[0]) if active else "",
                tf_snapshot_id=str(active[1]) if active else "",
            )
            passes.append(_single_pass_result(records, fingerprint, peaks))
        # Quality is a property of the corpus, not of a sample, so it is computed once over
        # the final lake state against the generator's truth — reported, never gated (S10.5).
        contribution = _quality_contribution(connection, staging, cfg.thresholds.auto_merge)
    result = aggregate_passes(passes)
    result["blocking_recall"] = contribution["blocking_recall"]
    result["quality"] = contribution["quality"]
    return result


def _quality_contribution(connection: Any, staging: Path, auto_merge: float) -> dict[str, Any]:
    """`blocking_recall` and the quality block over the generated corpus's truth (S10.5)."""
    import csv

    from quality import quality_block, truth_pairs_from_rows

    rows: list[tuple[str, str]] = []
    for truth_csv in (staging / "truth.csv", staging / "batch" / "truth.csv"):
        if not truth_csv.exists():
            continue
        with truth_csv.open(newline="", encoding="utf-8") as handle:
            for row in csv.DictReader(handle):
                record_key = f"{row['source_system']}:{row['source_record_id']}"
                rows.append((row["persona_id"], record_key))
    truth = truth_pairs_from_rows(rows)
    return quality_block(connection, truth, auto_merge=auto_merge)


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_error:
        # argparse exits 2 on a usage error already; normalise any other code to 2.
        return _EXIT_BAD_USAGE if exit_error.code not in (0, None) else int(exit_error.code or 0)

    if not args.scale:
        print("error: --scale is required", flush=True)
        return _EXIT_BAD_USAGE

    try:
        scale = get_scale(args.scale)
        run = _resolve_run(args)
    except (OSError, ValueError, BenchResultError, json.JSONDecodeError) as error:
        print(f"error: {error}", flush=True)
        return _EXIT_BAD_USAGE

    baseline = _load_baseline(args.baselines_dir, scale.name)
    verdict, reasons = compare_to_baseline(run, baseline, scale, fail_threshold=args.fail_threshold)
    run["verdict"] = verdict.value

    if args.write_baseline:
        try:
            destination = write_baseline(
                run, args.baselines_dir or Path("benchmarks/baselines"), scale
            )
        except (OSError, BenchResultError) as error:
            print(f"error: {error}", flush=True)
            # A refused baseline keeps the run's own verdict exit code (NON_COMPARABLE -> 3).
            return verdict.exit_code if verdict is Verdict.NON_COMPARABLE else _EXIT_BAD_USAGE
        print(f"wrote baseline {destination}", flush=True)

    try:
        _emit(run, args.out)
    except (OSError, BenchResultError) as error:
        print(f"error: {error}", flush=True)
        return _EXIT_BAD_USAGE

    for reason in reasons:
        print(reason, flush=True)
    print(verdict.value, flush=True)  # the verdict is always the last line of stdout
    return verdict.exit_code


if __name__ == "__main__":
    raise SystemExit(main())
