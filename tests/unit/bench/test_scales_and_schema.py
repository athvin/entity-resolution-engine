"""The benchmark harness's inputs, graded against DesignDoc.md (S9.2, S10.2, S10.3, S10.4).

Nothing here starts a container, and that is deliberate: every fact these tests check
is a *property of a committed file*, and the benchmark gate has never executed, so a
defect in the scale table or the run-document contract would otherwise surface for the
first time inside a two-hour `100k` dispatch.

Three authorities, none of them restated:

* **S10.2's two tables** own the scale values. `test_scales_yaml_matches_s10_2` parses
  the markdown and compares column by column, because a table transcribed into YAML by
  hand is a table that drifts, and the drift is invisible -- a `100k` run measured with
  `min_free_gb: 4` simply fills the runner's disk halfway through.
* **`benchmarks/bench_result.schema.json`** owns the run document. The refusal test
  removes one required key at a time rather than asserting against a restated key
  list, so a key added to the schema is covered the moment it is added.
* **`docker/compose.yaml` as authored** owns the `bench` service. It is read with
  `yaml.safe_load` and never through `docker compose config`: S10.2's envelope reaches
  the file as the *expressions* `${ER_CPU_LIMIT:-2}` / `${ER_MEM_LIMIT:-6g}`, and
  rendering would flatten exactly the defaults this test exists to pin.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
COMPOSE_FILE = REPO_ROOT / "docker" / "compose.yaml"
SCALES_YAML = REPO_ROOT / "benchmarks" / "scales.yaml"


def _import(module: str) -> ModuleType:
    """Import a `benchmarks/` module, whose parent is not on the path.

    `benchmarks/` is a directory of scripts rather than a distribution -- S9.2 runs
    `python benchmarks/scales.py` directly, which puts that directory on `sys.path`
    itself -- so the same entry is what makes `scales.py`'s own `from schema import ...`
    resolve here as it does in the image.
    """
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


scales_module = _import("scales")
schema_module = _import("schema")
BenchResultError = schema_module.BenchResultError

#: `${ER_CPU_LIMIT:-2}` -> `2`. The default is the whole point of the assertion: S7.1
#: compiles the smoke/10k row in as the value a local `docker compose run` with no
#: environment set reproduces.
INTERPOLATION_DEFAULT = re.compile(r"^\$\{[A-Z_]+:-(?P<default>[^}]+)\}$")


def section(anchor: str) -> str:
    text = DESIGN_DOC.read_text(encoding="utf-8")
    start = text.find(f'<a id="{anchor}"></a>')
    assert start != -1, f"DesignDoc.md has no anchor {anchor}"
    end = text.find('<a id="', start + 1)
    return text[start:end] if end != -1 else text[start:]


def _table_rows(text: str, width: int) -> dict[str, list[str]]:
    """Markdown data rows of the table whose rows have `width` cells, keyed by scale."""
    rows: dict[str, list[str]] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if len(cells) != width or cells[0] == "Scale" or set(cells[0]) <= {"-", ":"}:
            continue
        rows[_backticked(cells[0])] = cells[1:]
    return rows


def _backticked(cell: str) -> str:
    """The first backtick-quoted token of a cell.

    S10.2's runner column reads ``` `ubuntu-latest` (2 vCPU) ```: the parenthetical is
    prose about the runner, and the label is what `runs-on` takes.
    """
    match = re.search(r"`([^`]+)`", cell)
    assert match, f"expected a backtick-quoted value in {cell!r}"
    return match.group(1)


def spec_scale_table() -> dict[str, dict[str, Any]]:
    """S10.2's two tables, merged into one row per scale."""
    text = section("s10-2")
    corpus = _table_rows(text, 7)
    envelope = _table_rows(text, 5)
    assert corpus and set(corpus) == set(envelope), (
        f"S10.2's two tables disagree on the scale set: {sorted(corpus)} vs {sorted(envelope)}"
    )

    expected: dict[str, dict[str, Any]] = {}
    for name, cells in corpus.items():
        personas, records, batch, min_free_gb, baseline, dispatchable = cells
        runner, cpu_limit, mem_limit, duckdb_memory_limit = envelope[name]
        assert baseline in ("yes", "no") and dispatchable in ("yes", "no")
        expected[name] = {
            "personas": int(personas.replace(",", "")),
            "records": int(records.replace(",", "")),
            "incremental_batch": int(batch.replace(",", "")),
            "min_free_gb": int(min_free_gb.replace(",", "")),
            "baseline_committed": baseline == "yes",
            "dispatchable": dispatchable == "yes",
            "runner": _backticked(runner),
            "cpu_limit": int(_backticked(cpu_limit)),
            "mem_limit": _backticked(mem_limit),
            "duckdb_memory_limit": _backticked(duckdb_memory_limit),
        }
    return expected


def valid_document() -> dict[str, Any]:
    """A run document carrying every required key, and plausible values in each.

    Not a fixture file: the schema is the contract, and a committed sample would be a
    second statement of it that a schema edit could leave behind.
    """
    return {
        "scale": "smoke",
        "verdict": "NO_BASELINE",
        "repeat": 3,
        "incremental_ratio": 0.12,
        "blocking_recall": 0.97,
        "fingerprint": {
            "scale": "smoke",
            "image_digest": "sha256:" + "0" * 64,
            "git_sha": "0" * 40,
            "runner": "ubuntu-latest",
            "cgroup_cpu_max": "200000 100000",
            "cgroup_memory_max": 6 * 1024**3,
            "cgroup_memory_peak": 3 * 1024**3,
            "nproc": 4,
            "er_duckdb_threads": 2,
            "er_duckdb_memory_limit": "4GB",
            "duckdb_version": "1.5.5",
            "splink_version": "4.0.16",
            "dbt_core_version": "1.12.2",
            "dbt_duckdb_version": "1.11.0",
            "ducklake_extension_version": "0.3",
            "config_hash": "a" * 16,
            "generator_seed": 42,
            "model_version": "v1",
            "tf_snapshot_id": "7",
        },
        "phases": [
            {
                "name": "ingest",
                "wall_ms": 1200.0,
                "wall_ms_cv": 0.04,
                "records_per_sec": 833.3,
                "candidate_pair_count": 0,
                "pairs_above_auto_merge": 0,
                "memory_peak_bytes": 512 * 1024**2,
                "snapshot_count": 1,
            }
        ],
        "memory": {
            "duckdb_buffer_peak_bytes": 1024**3,
            "rss_peak_bytes": 2 * 1024**3,
            "cgroup_peak_bytes": 3 * 1024**3,
        },
        "quality": {
            "edge_precision": 0.99,
            "edge_recall": 0.95,
            "edge_f1": 0.97,
            "cluster_precision": 0.98,
            "cluster_recall": 0.94,
            "cluster_f1": 0.96,
        },
    }


def run_accessor(*args: str) -> subprocess.CompletedProcess[str]:
    """`python benchmarks/scales.py ...`, exactly as the S9.2 preflight invokes it."""
    return subprocess.run(
        [sys.executable, "benchmarks/scales.py", *args],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
    )


def test_scales_yaml_matches_s10_2() -> None:
    expected = spec_scale_table()
    loaded = scales_module.load_scales()

    assert sorted(loaded) == sorted(expected), (
        "scales.yaml must define exactly S10.2's four scales; `smoke` is required so "
        "the weekly cron fits a 40-minute job, and `1m` is defined but not dispatchable"
    )

    for name, row in expected.items():
        scale = loaded[name]
        for field in scales_module.FIELD_NAMES:
            assert getattr(scale, field) == row[field], (
                f"scale '{name}' field '{field}': scales.yaml has "
                f"{getattr(scale, field)!r}, S10.2 states {row[field]!r}"
            )

    # The two rules S10.2 states about the scale SET rather than about a row.
    assert loaded["smoke"].dispatchable, "the weekly cron dispatches `smoke` (S9.2)"
    assert not loaded["1m"].dispatchable and not loaded["1m"].baseline_committed, (
        "`1m` is defined but not dispatchable; adding it to the dispatch list is a "
        "separate change that commits its baseline first (S9.2)"
    )


def test_field_accessor_cli() -> None:
    # The two literals S9.2's preflight actually pipes into `$GITHUB_ENV`.
    smoke = run_accessor("--scale", "smoke", "--field", "min_free_gb")
    assert smoke.returncode == 0, smoke.stderr
    assert smoke.stdout == "4\n", (
        f"the preflight compares free disk against this value verbatim: {smoke.stdout!r}"
    )

    cpu = run_accessor("--scale", "100k", "--field", "cpu_limit")
    assert cpu.returncode == 0, cpu.stderr
    assert cpu.stdout == "6\n"

    # Every column is reachable, and none of them prints a unit, a label or a banner:
    # whatever lands on stdout becomes the value of an environment variable.
    for field in scales_module.FIELD_NAMES:
        result = run_accessor("--scale", "10k", "--field", field)
        assert result.returncode == 0, result.stderr
        assert result.stdout.count("\n") == 1 and result.stdout.strip(), result.stdout

    for args, offender in (
        (("--scale", "500k", "--field", "cpu_limit"), "500k"),
        (("--scale", "smoke", "--field", "cpu_quota"), "cpu_quota"),
    ):
        result = run_accessor(*args)
        assert result.returncode != 0, (
            "an unknown scale or field must fail loudly; the preflight cannot tell an "
            "empty export from a missing one, and the run is then measured in the "
            "wrong envelope (S9.2)"
        )
        assert result.stdout == "", f"nothing may reach stdout on failure: {result.stdout!r}"
        assert offender in result.stderr, result.stderr


def test_envelope_consistency_rules(tmp_path: Path) -> None:
    committed = scales_module.load_scales()

    for name, scale in committed.items():
        vcpus = scales_module.RUNNER_VCPUS[scale.runner]
        assert scale.cpu_limit <= vcpus, (
            f"scale '{name}': a cpu quota above the runner's {vcpus} vCPU is "
            "unenforceable and leaves S10.4 comparing against an unreachable limit"
        )
        assert not scale.dispatchable or scale.baseline_committed, name

    document = yaml.safe_load(SCALES_YAML.read_text(encoding="utf-8"))

    # A DuckDB limit at or above the container limit leaves nothing for the Python
    # heap and the dbt subprocess, so the container is OOM-killed rather than spilling.
    over = {name: dict(row) for name, row in document.items()}
    over["10k"]["duckdb_memory_limit"] = over["10k"]["mem_limit"].upper()
    over_path = tmp_path / "over.yaml"
    over_path.write_text(yaml.safe_dump(over), encoding="utf-8")
    with pytest.raises(BenchResultError, match="10k"):
        scales_module.load_scales(over_path)

    # Dispatching a scale with no committed baseline produces a NO_BASELINE run that
    # can gate nothing, on a runner nobody chose to pay for.
    undispatchable = {name: dict(row) for name, row in document.items()}
    undispatchable["1m"]["dispatchable"] = True
    unbaselined_path = tmp_path / "unbaselined.yaml"
    unbaselined_path.write_text(yaml.safe_dump(undispatchable), encoding="utf-8")
    with pytest.raises(BenchResultError, match="1m"):
        scales_module.load_scales(unbaselined_path)

    # A quota the machine cannot supply.
    oversized = {name: dict(row) for name, row in document.items()}
    oversized["smoke"]["cpu_limit"] = scales_module.RUNNER_VCPUS["ubuntu-latest"] + 1
    oversized_path = tmp_path / "oversized.yaml"
    oversized_path.write_text(yaml.safe_dump(oversized), encoding="utf-8")
    with pytest.raises(BenchResultError, match="smoke"):
        scales_module.load_scales(oversized_path)


def test_write_result_refuses_invalid_document(tmp_path: Path) -> None:
    schema = schema_module.load_schema()
    top_level = list(schema["required"])
    fingerprint_keys = list(schema["properties"]["fingerprint"]["required"])

    assert {"blocking_recall", "verdict", "incremental_ratio"} <= set(top_level)
    assert "config_hash" in fingerprint_keys

    for key in top_level:
        document = valid_document()
        del document[key]
        destination = tmp_path / f"missing-{key}.json"
        with pytest.raises(BenchResultError, match=re.escape(key)):
            schema_module.write_result(document, destination)
        assert not destination.exists(), (
            f"removing '{key}' left a file behind; a half-written latest.json is what "
            "makes --compare read a document that cannot exist"
        )

    for key in fingerprint_keys:
        document = valid_document()
        del document["fingerprint"][key]
        destination = tmp_path / f"missing-fingerprint-{key}.json"
        with pytest.raises(BenchResultError, match=re.escape(key)):
            schema_module.write_result(document, destination)
        assert not destination.exists()

    # A wrong type is a violation too: `true` is an `int` in Python, and a wall time of
    # `true` would otherwise validate and then be divided by.
    document = valid_document()
    document["phases"][0]["wall_ms"] = True
    destination = tmp_path / "boolean-wall-ms.json"
    with pytest.raises(BenchResultError, match="wall_ms"):
        schema_module.write_result(document, destination)
    assert not destination.exists()

    document = valid_document()
    document["verdict"] = "SLOWER"
    with pytest.raises(BenchResultError, match="SLOWER"):
        schema_module.validate_bench_result(document)


def test_write_result_writes_canonical_json(tmp_path: Path) -> None:
    document = valid_document()
    # A subdirectory that does not exist yet: the CI job creates `artifacts/bench/`
    # on the host, but a local `--out` need not have been created by anyone.
    destination = tmp_path / "bench" / "latest.json"

    written = schema_module.write_result(document, destination)
    assert written == destination

    expected = json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    assert destination.read_text(encoding="utf-8") == expected, (
        "baselines are committed and reviewed as diffs, so the rendering has to be "
        "stable: sorted keys, one indent, trailing newline"
    )

    reread = json.loads(destination.read_text(encoding="utf-8"))
    schema_module.validate_bench_result(reread)


def test_compose_bench_service_contract() -> None:
    """S7.1's `benchmark` service, read as authored -- no daemon, no `docker` CLI."""
    compose = yaml.safe_load(COMPOSE_FILE.read_text(encoding="utf-8"))
    service = compose["services"]["benchmark"]

    assert service["profiles"] == ["bench"], (
        "the benchmark service must sit behind the `bench` profile, or the PR path starts it (S7.1)"
    )
    assert service["pull_policy"] == "never", (
        "`er-pipeline:ci` is a local-only tag; a registry pull for it can only fail"
    )
    assert service["environment"]["ER_CONFIG"] == "/app/configs/test.yaml", (
        "ER_CONFIG is mandatory on benchmark as well as pipeline; without it every "
        "command exits 2 at config load (S6.1, S7.1)"
    )

    volumes = service["volumes"]
    assert "../artifacts:/app/artifacts" in volumes, (
        "without the bind mount `latest.json` never reaches the host and the CI "
        "upload step fails with if-no-files-found: error (S9.2)"
    )
    assert "benchdata:/app/.bench" in volumes, (
        "the generated corpus is kept out of the artifacts mount, or every run "
        "uploads its whole input corpus (S10.1)"
    )
    assert "benchdata" in compose["volumes"]

    limits = service["deploy"]["resources"]["limits"]
    defaults = {}
    for key, expression in (("cpus", limits["cpus"]), ("memory", limits["memory"])):
        match = INTERPOLATION_DEFAULT.match(str(expression))
        assert match, (
            f"deploy.resources.limits.{key} must stay an interpolation with a default: "
            f"the S9.2 preflight exports the scale's row, and a bare literal here would "
            f"squeeze a 100k dispatch into the 2-CPU envelope (got {expression!r})"
        )
        defaults[key] = match.group("default")

    assert defaults == {"cpus": "2", "memory": "6g"}

    # The defaults are not a third statement of the envelope: S7.1 says they ARE the
    # smoke/10k row, so they are checked against scales.yaml rather than against a
    # literal that could drift away from it.
    smoke = scales_module.get_scale("smoke")
    assert defaults["cpus"] == str(smoke.cpu_limit)
    assert defaults["memory"] == smoke.mem_limit

    # S10.2: `ER_DUCKDB_THREADS` equals `cpu_limit`, always -- written as one shared
    # expression precisely so the two cannot be set independently.
    assert compose["x-er-env"]["ER_DUCKDB_THREADS"] == limits["cpus"]
