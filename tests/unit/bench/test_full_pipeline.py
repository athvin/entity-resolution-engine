"""Capacity, accounting and output-integrity checks for an initial-load measurement."""

import importlib
import json
import subprocess
import sys
from argparse import Namespace
from pathlib import Path

import duckdb
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))
benchmark = importlib.import_module("full_pipeline")
profile = importlib.import_module("profile_pipeline")


def test_experimental_ten_million_scale_keeps_local_envelope() -> None:
    preset = benchmark.get_scale("10m")
    local = benchmark.local_scale(preset, cpus=8, memory=int(11.65 * benchmark.GIB))
    assert (local.records, local.personas, local.incremental_batch) == (10_000_000, 4_000_000, 0)
    assert (local.cpu_limit, local.mem_limit, local.duckdb_memory_limit) == (2, "10g", "4GB")
    assert "10m" not in benchmark.load_scales()


def test_million_record_capacity_checks_docker_not_host_cpu_count() -> None:
    scale = benchmark.get_scale("1m")
    errors = benchmark.capacity_errors(
        scale, cpus=8, memory=8 * benchmark.GIB, disk=10 * benchmark.GIB
    )
    assert len(errors) == 3
    assert (
        benchmark.capacity_errors(
            scale, cpus=12, memory=57 * benchmark.GIB, disk=120 * benchmark.GIB
        )
        == []
    )
    assert (
        len(
            benchmark.capacity_errors(
                scale, cpus=12, memory=56 * benchmark.GIB, disk=120 * benchmark.GIB
            )
        )
        == 1
    )  # The substrate needs memory beyond the pipeline container's limit.


def test_local_mode_keeps_million_records_and_uses_available_resources() -> None:
    preset = benchmark.get_scale("1m")
    local = benchmark.local_scale(preset, cpus=8, memory=int(7.65 * benchmark.GIB))
    assert local.records == 1_000_000
    assert local.personas == 400_000
    assert local.cpu_limit == 2
    assert local.mem_limit == "6g"
    assert local.duckdb_memory_limit == "4GB"
    assert (
        benchmark.capacity_errors(
            local, cpus=8, memory=int(7.65 * benchmark.GIB), disk=9 * benchmark.GIB
        )
        == []
    )
    assert preset.cpu_limit == 12
    assert preset.mem_limit == "56g"
    larger = benchmark.local_scale(preset, cpus=8, memory=int(11.65 * benchmark.GIB))
    assert larger.mem_limit == "10g"
    assert larger.duckdb_memory_limit == "4GB"
    assert larger.cpu_limit == 2


def test_runtime_verifies_the_measured_envelope_instead_of_the_larger_preset() -> None:
    from report import comparability_violations

    manifest = {
        "cpu_limit": 6,
        "mem_limit": "6g",
        "duckdb_memory_limit": "3GB",
        "min_free_gb": 2,
    }
    scale = benchmark.measured_scale(benchmark.get_scale("1m"), manifest)
    result = {
        "fingerprint": {
            "scale": "1m",
            "cgroup_cpu_max": "6.0",
            "cgroup_memory_max": 6 * benchmark.GIB,
            "er_duckdb_threads": 6,
            "er_duckdb_memory_limit": "3GB",
        },
        "phases": [],
    }
    assert comparability_violations(result, scale) == []
    result["fingerprint"]["cgroup_cpu_max"] = "2.0"
    assert len(comparability_violations(result, scale)) == 1


def measured_run(directory: Path) -> Path:
    directory.mkdir()
    commands = [
        {
            "command": ["er", stage],
            "phase": "setup" if stage == "init" else "base",
            "duration_ms": 10_000 if stage == "init" else 1_000,
            "started_ns": i * 10,
            "ended_ns": i * 10 + 5,
            "exit_code": 0,
            "cpu": {"usage_usec": 500_000, "throttled_usec": 0},
        }
        for i, stage in enumerate(["init"] + ["ingest"] * 3 + list(benchmark.STAGES[1:]))
    ]
    (directory / "resources.jsonl").write_text(
        "\n".join(
            json.dumps({"monotonic_ns": i * 10 + 1, "memory.current": 9999 if i == 0 else 100})
            for i in range(len(commands))
        )
    )
    path = directory / "result.json"
    path.write_text(
        json.dumps(
            {
                "status": "succeeded",
                "commands": commands,
                "base_records": 1_000,
                "total_ms": 30_000,
                "base_counts": {"golden_records": 400, "golden_lineage": 2400},
                "output_validation": {"entity_membership": 1000},
                "base_partition_hash": "stable",
                "input_sha256": {"crm": "stable"},
                "quality": {},
                "fingerprint": {},
            }
        )
    )
    return path


def test_initial_load_accounting_includes_training_and_all_ingests_but_excludes_setup(
    tmp_path: Path,
) -> None:
    result = benchmark.summarize_run(measured_run(tmp_path / "run-001"))
    assert result["processing_seconds"] == 8
    assert result["stage_seconds"]["ingest"] == 3
    assert result["stage_seconds"]["train"] == 1
    assert result["records_per_second"] == 125
    assert result["setup_command_seconds"] == 10
    assert result["resources"]["cpu_s"] == 4
    assert result["resources"]["sampled_memory_peak_bytes"] == 100


@pytest.mark.parametrize("defect", ["missing_assembly", "skipped_assembly"])
def test_incomplete_pipeline_cannot_be_reported_as_success(tmp_path: Path, defect: str) -> None:
    path = measured_run(tmp_path / "run-001")
    run = json.loads(path.read_text())
    if defect == "missing_assembly":
        run["commands"].pop()
    else:
        run["commands"][-1]["exit_code"] = 10
    path.write_text(json.dumps(run))
    with pytest.raises(ValueError, match="initial load|initial-load"):
        benchmark.summarize_run(path)


def test_single_pass_has_no_invented_variation_and_repeats_detect_partition_changes(
    tmp_path: Path,
) -> None:
    measured_run(tmp_path / "run-001")
    manifest = {"scale": "smoke", "status": "succeeded", "repeat": 1, "records": 1_000}
    result = benchmark.write_report(tmp_path, manifest)
    assert result["summary"]["cv"] is None
    assert result["summary"]["outputs_repeatable"] is None
    path = measured_run(tmp_path / "run-002")
    run = json.loads(path.read_text())
    run["base_partition_hash"] = "different"
    path.write_text(json.dumps(run))
    result = benchmark.write_report(tmp_path, {**manifest, "repeat": 2})
    assert result["summary"]["cv"] == 0
    assert result["summary"]["outputs_repeatable"] is False


@pytest.fixture
def lake():
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute("CREATE TABLE lake.main.raw_records AS SELECT 1 AS id FROM range(2)")
    for table in ("int_std_records", "entity_membership"):
        connection.execute(
            f"CREATE TABLE lake.main.{table} AS "
            "SELECT * FROM (VALUES ('a', 'e1'), ('b', 'e1')) t(record_key, entity_id)"
        )
    connection.execute("CREATE TABLE lake.main.golden_records AS SELECT 'e1' AS entity_id")
    from er.lake.columns import GOLDEN_LINEAGE_ATTRIBUTES

    connection.execute(
        "CREATE TABLE lake.main.golden_lineage AS "
        "SELECT 'e1' AS entity_id, 'a' AS record_key, unnest(?::VARCHAR[]) AS attribute",
        [list(GOLDEN_LINEAGE_ATTRIBUTES)],
    )
    try:
        yield connection
    finally:
        connection.close()


def test_output_validation_accepts_complete_membership_and_lineage(lake) -> None:
    assert profile.validate_initial_outputs(lake, 2)["golden_lineage"] == 6


@pytest.mark.parametrize(
    "mutation",
    [
        "UPDATE lake.main.entity_membership SET record_key='c' WHERE record_key='b'",
        "UPDATE lake.main.entity_membership SET record_key='a' WHERE record_key='b'",
        "UPDATE lake.main.golden_records SET entity_id='other'",
        "UPDATE lake.main.golden_lineage SET record_key='outside' WHERE attribute='email'",
        "UPDATE lake.main.golden_lineage SET attribute='email' WHERE attribute='birth_date'",
    ],
)
def test_equal_row_counts_cannot_hide_incomplete_outputs(lake, mutation: str) -> None:
    lake.execute(mutation)
    with pytest.raises(AssertionError):
        profile.validate_initial_outputs(lake, 2)


@pytest.mark.parametrize("keep_failed", [False, True])
def test_failed_worker_respects_cleanup_and_retains_failed_report(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, keep_failed: bool
) -> None:
    commands = []

    def invoke(command, **kwargs):
        commands.append(command)
        if "compose" in command and "run" in command:
            raise subprocess.CalledProcessError(137, command)
        return subprocess.CompletedProcess(command, 0)

    def output(command):
        if command[:2] == ["docker", "run"]:
            return str(20 * benchmark.GIB)
        return "test-identity"

    monkeypatch.setattr(benchmark.subprocess, "run", invoke)
    monkeypatch.setattr(benchmark, "output", output)
    monkeypatch.setattr(benchmark, "source_manifest", lambda: {"example.py": "hash"})
    args = Namespace(out=tmp_path / "measurement", scale="smoke", repeat=1, keep_failed=keep_failed)
    with pytest.raises(subprocess.CalledProcessError):
        benchmark.campaign(args, {"scale": "smoke", "records": 1000})
    run = next(command for command in commands if "compose" in command and "run" in command)
    project = run[run.index("-p") + 1]
    assert project.startswith("er-full-bench-")
    down = [command for command in commands if "compose" in command and "down" in command]
    image_removal = [command for command in commands if command[:3] == ["docker", "image", "rm"]]
    if keep_failed:
        assert not down and not image_removal
    else:
        assert len(down) == 1
        assert down[0][down[0].index("-p") + 1] == project
        assert image_removal
    document = json.loads((args.out / "results.json").read_text())
    assert document["manifest"]["status"] == "failed"
    assert document["manifest"].get("retained_project") == (project if keep_failed else None)
    assert "summary" not in document
    assert (args.out / "pass-001.log").exists()


def test_killed_worker_with_partial_json_still_gets_a_failure_report(tmp_path: Path) -> None:
    directory = tmp_path / "run-001"
    directory.mkdir()
    (directory / "result.json").write_text('{"status":')
    result = benchmark.write_report(
        tmp_path, {"scale": "smoke", "status": "failed", "repeat": 1, "records": 1000}
    )
    assert result["runs"][0]["status"] == "failed"
    assert "summary" not in result


@pytest.mark.parametrize("batch_mode", ["incremental", "full"])
def test_incremental_delivery_is_accounted_separately_from_matrix_probes(
    tmp_path: Path, batch_mode: str
) -> None:
    path = measured_run(tmp_path / "run-001")
    run = json.loads(path.read_text())
    for stage in ["ingest"] * 3 + ["run-all"]:
        run["commands"].append(
            {
                "command": ["er", stage],
                "phase": "batch",
                "duration_ms": 250,
                "started_ns": 100,
                "ended_ns": 101,
                "exit_code": 0,
                "cpu": {"usage_usec": 100, "throttled_usec": 0},
            }
        )
    run["commands"].append(
        {
            "command": ["er", "match"],
            "phase": "matrix",
            "duration_ms": 9000,
            "started_ns": 110,
            "ended_ns": 111,
            "exit_code": 0,
            "cpu": {"usage_usec": 100, "throttled_usec": 0},
        }
    )
    run.update(batch_mode=batch_mode, incremental_records=10, batch_counts={"golden_records": 405})
    with (path.parent / "resources.jsonl").open("a") as handle:
        handle.write('\n{"monotonic_ns": 100, "memory.current": 200, "spill_bytes": 5}\n')
        handle.write('{"monotonic_ns": 110, "memory.current": 9999, "spill_bytes": 9000}\n')
    path.write_text(json.dumps(run))
    measured = benchmark.summarize_run(path)
    assert measured["processing_seconds"] == 8
    prefix = "incremental" if batch_mode == "incremental" else "full_batch_reference"
    assert measured[f"{prefix}_seconds"] == 1
    assert measured[f"{prefix}_records"] == 10
    assert measured["resources"]["sampled_memory_peak_bytes"] == 100
    assert measured[f"{prefix}_resources"]["sampled_memory_peak_bytes"] == 200
    assert measured[f"{prefix}_resources"]["sampled_spill_bytes_peak"] == 5
    if batch_mode == "full":
        assert "incremental_seconds" not in measured
