"""Run the gated tiny -> smoke -> 10k profiling campaign against an isolated Compose lake."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import sys
import time
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import yaml
from large_validation import (
    block_statistics,
    candidate_pair_count,
    copy_input,
    file_sha256,
    partition_sha256,
    quality_from_csv,
)
from profile_report import write_report
from scales import Scale
from semantic_outputs import save_semantic_outputs
from storage_sampler import StorageSampler
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.lake.ducklake import connect
from er.lake.model_registry import register_model
from er.lake.objectstore import ObjectStore
from er.matching.tf import tf_tables_path
from er.obs.profiling import cgroup_readings, span, stream_command

SOURCES = ("crm", "billing", "webforms")
ROOT = Path(__file__).resolve().parents[1]


def read_events(directory: Path) -> list[dict[str, Any]]:
    records = []
    for path in directory.glob("events-*.jsonl"):
        records.extend(json.loads(line) for line in path.read_text().splitlines() if line)
    return records


def install_fixture(config: Any) -> None:
    fixture = ROOT / "fixtures/static/model_test_v1"
    meta = json.loads(fixture.with_suffix(".meta.json").read_text())
    with span("setup.fixture_model", unit="models"), connect() as connection:
        # Bulk import avoids one catalog transaction for every reference TF term.
        connection.execute(
            "INSERT INTO lake.main.tf_lookup SELECT * FROM read_csv(?, header=true)",
            [str(fixture.with_suffix(".tf.csv"))],
        )
        register_model(
            connection,
            ObjectStore.from_env(),
            model_version=meta["model_version"],
            model_uri_prefix=config.storage.model_uri_prefix,
            settings_json=fixture.with_suffix(".json").read_text(),
            metrics=meta,
            corpus_snapshot=0,
            tf_snapshot_id=meta["tf_snapshot_id"],
            tf_tables_path=tf_tables_path(meta["model_version"], meta["tf_snapshot_id"]),
            config_hash=config_hash(config),
            run_id=str(ULID()),
        )


def check_fixture(connection: Any, phase: str = "base") -> None:
    sys.path.insert(0, str(ROOT / "tests"))
    from helpers.compare import assert_golden_equal, assert_partition_equal
    from helpers.expected import label_map_from_membership

    expected = ROOT / (
        "fixtures/static/base_10/expected/base"
        if phase == "base"
        else "fixtures/static/incremental_batch/expected/batch"
    )
    membership = connection.execute(
        "SELECT record_key, entity_id FROM lake.main.entity_membership"
    ).fetchall()
    assert_partition_equal(membership, expected / "membership.csv")
    label_map = label_map_from_membership(membership)
    assert_golden_equal(
        connection.execute(
            "SELECT * REPLACE (CAST(birth_date AS VARCHAR) AS birth_date) "
            "FROM lake.main.golden_records"
        ).df(),
        expected / "golden.csv",
        label_map,
    )
    with (expected / "std_hashes.csv").open() as handle:
        rows = list(csv.DictReader(handle))
    from er.lake.hashing import STD_HASH_COLUMNS, table_content_hash

    projection = ("source_system", "source_record_id", *STD_HASH_COLUMNS)
    actual = {}
    for values in connection.execute(
        f"SELECT {', '.join(projection)} FROM lake.main.int_std_records"
    ).fetchall():
        row = dict(zip(projection, values, strict=True))
        actual[f"{row['source_system']}:{row['source_record_id']}"] = table_content_hash(row)
    assert actual == {
        f"{row['source_system']}:{row['source_record_id']}": row["std_hash"] for row in rows
    }


def validate_coverage(
    directory: Path,
    *,
    trained: bool,
    initial_only: bool = False,
    batch_mode: str = "incremental",
) -> dict[str, Any]:
    events = read_events(directory)
    starts = {event["span_id"] for event in events if event["event"] == "span_start"}
    ends = {event["span_id"] for event in events if event["event"] == "span_end"}
    assert not ends - starts, "completed spans have no corresponding start"
    unfinished = starts - ends
    names = {event.get("name") for event in events if event["event"] == "span_end"}
    required = {
        "ingest",
        "standardize",
        "match",
        "reconcile",
        "assemble",
        "match.full",
        "reconcile.label_propagation",
    }
    if not initial_only and batch_mode == "incremental":
        required.add("match.incremental")
    if trained:
        required |= {"train", "train.fit", "train.estimation_call", "train.publish_model"}
    assert required <= names, f"missing transformations: {required - names}"
    assert not unfinished, f"unfinished spans: {unfinished}"
    errors = [event for event in events if event["event"].endswith("_error")]
    assert not errors, f"telemetry errors: {errors[:3]}"
    dbt_nodes = {event["name"] for event in events if event["event"] == "dbt_model"}
    models = {name for name in dbt_nodes if name.startswith("model.")}
    expected_models = {
        f"model.er.{name}"
        for name in (
            "stg_crm",
            "stg_billing",
            "stg_webforms",
            "int_std_records",
            "int_blocking_keys",
            "golden_records",
            "golden_lineage",
            "golden_display",
        )
    }
    assert expected_models <= models, f"missing dbt models: {expected_models - models}"
    counted = [event for event in events if event["event"] == "dbt_model_end"]
    assert expected_models <= {event["name"] for event in counted}, "missing dbt model counts"
    for event in counted:
        assert all(
            event["metrics"].get(key) is not None
            for key in ("rows_in", "rows_out", "rows_written", "input_unit", "output_unit")
        ), event
    for event in events:
        if event["event"] == "span_end" and event["name"] in required & {
            "ingest",
            "standardize",
            "train",
            "match",
            "reconcile",
            "assemble",
        }:
            assert event["status"] == "succeeded", event
            assert all(
                event["metrics"].get(key) is not None
                for key in ("rows_in", "rows_out", "input_unit", "output_unit")
            ), event
    sql = [event for event in events if event["event"] == "sql_profile"]
    assert sql, "no SQL operator profiles were collected"
    assert len({event["profile_path"] for event in sql}) == len(sql), (
        "SQL profiles were overwritten"
    )
    for stage in (
        "ingest",
        "standardize",
        "match",
        "reconcile",
        "assemble",
        *(["train"] if trained else []),
    ):
        assert any(event.get("stage") == stage for event in sql), f"no SQL profiles for {stage}"
    return {
        "spans": len(ends),
        "sql_profiles": len(sql),
        "dbt_models": len(models),
        "dbt_nodes": len(dbt_nodes),
    }


@contextmanager
def _case_environment() -> Iterator[None]:
    """Restore the caller's lake and profiling settings, including on failure."""
    previous = {key: value for key, value in os.environ.items() if key.startswith("ER_")}
    try:
        yield
    finally:
        for key in tuple(os.environ):
            if key.startswith("ER_"):
                del os.environ[key]
        os.environ.update(previous)


@_case_environment()
def run_case(
    out: Path,
    case: str,
    iteration: int,
    *,
    detailed: bool = True,
    config_template: Path | None = None,
    label: str | None = None,
    corpus_root: Path | None = None,
    compare_outputs: bool = False,
    initial_only: bool = False,
    workload: Scale | None = None,
    prediction_matrix: bool = False,
    batch_mode: str = "incremental",
    with_correction: bool = False,
    generator_profile: str = "baseline",
    blocking_experiments: bool = False,
    lsh_seed: int | None = None,
    onnx_directory: Path | None = None,
) -> dict[str, Any]:
    if initial_only and case == "tiny":
        raise ValueError("initial-only measurements require a generated benchmark scale")
    if workload is not None and workload.name != case:
        raise ValueError("workload name must match the measured case")
    if batch_mode not in ("incremental", "full"):
        raise ValueError("batch mode must be incremental or full")
    label = label or (f"{case}-{iteration}" if detailed else f"{case}-control")
    directory = out / label
    directory.mkdir(parents=True)
    os.environ["ER_PROFILE_SESSION_ID"] = out.name
    os.environ["ER_PROFILE_DIR"] = str(directory) if detailed else ""
    os.environ["ER_PROFILE_SQL"] = "1" if detailed else "0"
    namespace = f"profile_{uuid.uuid4().hex}"
    os.environ["ER_LAKE_METADATA_SCHEMA"] = namespace
    os.environ["ER_LAKE_DATA_PATH"] = f"s3://lake/profile/{namespace}/data/"
    document = yaml.safe_load((config_template or ROOT / "configs/test.yaml").read_text())
    document["storage"]["data_path"] = os.environ["ER_LAKE_DATA_PATH"]
    document["storage"]["model_uri_prefix"] = f"s3://lake/profile/{namespace}/models/"
    comparable_config = {
        **document,
        "storage": {**document["storage"], "data_path": "<trial>", "model_uri_prefix": "<trial>"},
    }
    config_path = directory / "config.yaml"
    config_path.write_text(yaml.safe_dump(document, sort_keys=False))
    os.environ["ER_CONFIG"] = str(config_path)
    config = load_config(config_path)
    result: dict[str, Any] = {
        "case": case,
        "label": label,
        "iteration": iteration,
        "detailed": detailed,
        "initial_only": initial_only,
        "batch_mode": batch_mode,
        "with_correction": with_correction,
        "generator_profile": generator_profile,
        "namespace": namespace,
        "status": "running",
        "commands": [],
        "config_hash": config_hash(config),
        "generator_seed": config.generator.seed,
        "source_sha256": os.environ.get("ER_SOURCE_SHA", "unknown"),
        "semantic_config_sha256": hashlib.sha256(
            json.dumps(comparable_config, sort_keys=True).encode()
        ).hexdigest(),
    }
    sampler = StorageSampler(directory / "resources.jsonl")
    sampler.start()
    started = time.monotonic()

    def command(args: list[str], run_id: str | None = None, phase: str = "setup") -> dict[str, Any]:
        invocation = uuid.uuid4().hex
        os.environ["ER_PROFILE_INVOCATION_ID"] = invocation
        argv = list(args)
        if argv[0] == "er":
            argv += ["--config", str(config_path), "--json"]
            if run_id is not None:
                argv += ["--run-id", run_id]
        command_dir = directory / "commands" / invocation
        if initial_only:
            print(f"[benchmark] {phase}: {' '.join(args[:4])}", flush=True)
        cpu_before = cgroup_readings()
        tick = time.monotonic_ns()
        with span(f"command.{argv[1]}", run_id=run_id, phase=phase) as metrics:
            completed = stream_command(argv, directory=command_dir)
            metrics["exit_code"] = completed.returncode
        ended = time.monotonic_ns()
        cpu_after = cgroup_readings()
        entry: dict[str, Any] = {
            "command": args[:4],
            "run_id": run_id,
            "duration_ms": (ended - tick) / 1_000_000,
            "started_ns": tick,
            "ended_ns": ended,
            "cpu": {
                key: max(0, cpu_after[key] - cpu_before[key])
                if cpu_before.get(key) is not None and cpu_after.get(key) is not None
                else None
                for key in ("usage_usec", "user_usec", "system_usec", "throttled_usec")
            },
            "phase": phase,
            "invocation_id": invocation,
            "exit_code": completed.returncode,
            "stages": [],
        }
        for line in (command_dir / "stderr.log").read_text().splitlines():
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict) and "seq" in value and "stage" in value:
                entry["stages"].append(value)
        result["commands"].append(entry)
        (directory / "result.json").write_text(json.dumps(result, indent=2))
        if completed.returncode not in (0, 10):
            raise RuntimeError(f"{args[:3]} failed ({completed.returncode}); see {command_dir}")
        return entry

    try:
        command(["er", "init"])
        if case == "tiny":
            corpus = ROOT / "fixtures/static/incremental_batch"
            base_inputs, batch_inputs = corpus / "base", corpus / "batch"
            base_records, batch_records = 23, 6
            install_fixture(config)
        else:
            from scales import get_scale

            scale = workload or get_scale(case)
            suffix = "-initial" if initial_only else ""
            if generator_profile != "baseline":
                suffix += "-" + generator_profile
            base_records = scale.records
            batch_records = 0 if initial_only else scale.incremental_batch
            identity = {
                "profile": generator_profile,
                "seed": config.generator.seed,
                "records": base_records,
                "personas": scale.personas,
                "batch": batch_records,
                "sources": {
                    name: source.model_dump(mode="json") for name, source in config.sources.items()
                },
            }
            identity_hash = hashlib.sha256(
                json.dumps(identity, sort_keys=True).encode()
            ).hexdigest()
            corpus = (corpus_root or out) / f"corpus-{case}{suffix}-{identity_hash[:16]}"
            if not corpus.exists():
                command(
                    [
                        sys.executable,
                        "-m",
                        "fixtures.generator.cli",
                        *(
                            ["--profile", generator_profile]
                            if generator_profile != "baseline"
                            else []
                        ),
                        "--personas",
                        str(scale.personas),
                        "--records",
                        str(scale.records),
                        "--batch",
                        str(batch_records),
                        "--seed",
                        str(config.generator.seed),
                        "--out",
                        str(corpus),
                        "--config",
                        str(config_path),
                    ]
                )
                (corpus / "benchmark-input.json").write_text(json.dumps(identity, sort_keys=True))
            elif (
                not (corpus / "benchmark-input.json").exists()
                or json.loads((corpus / "benchmark-input.json").read_text()) != identity
            ):
                raise ValueError(f"incomplete or incompatible cached corpus: {corpus}")
            base_inputs, batch_inputs = corpus, corpus / "batch"
        drops = {}
        deliveries = [("base", base_inputs)]
        if not initial_only:
            deliveries.append(("batch", batch_inputs))
        for phase, inputs in deliveries:
            drop = directory / f"drop-{phase}"
            for source in SOURCES:
                (drop / source).mkdir(parents=True)
                copy_input(inputs / f"{source}.csv", drop / source / f"{source}.csv")
            drops[phase] = drop
        result.update(base_records=base_records, incremental_records=batch_records)
        result["input_sha256"] = {
            f"{phase}/{source}.csv": file_sha256(drop / source / f"{source}.csv")
            for phase, drop in drops.items()
            for source in SOURCES
        }
        for phase in drops:
            mode = "full" if phase == "base" else batch_mode
            run_id = str(ULID())
            for source in SOURCES:
                command(
                    ["er", "ingest", "--source", source, "--path", str(drops[phase])], run_id, phase
                )
            if phase == "base" and case != "tiny":
                command(["er", "standardize"], run_id, phase)
                command(["er", "train"], str(ULID()), phase)
                command(["er", "match", "--mode", "full"], run_id, phase)
                command(["er", "reconcile"], run_id, phase)
                command(["er", "assemble"], run_id, phase)
            else:
                command(["er", "run-all", "--mode", mode, "--skip-ingest"], run_id, phase)
            with span(f"validation.{phase}", unit="records"), connect() as connection:
                tables = {
                    name: connection.execute(f"SELECT count(*) FROM lake.main.{name}").fetchone()[0]
                    for name in (
                        "raw_records",
                        "int_std_records",
                        "int_blocking_keys",
                        "match_scores",
                        "entity_membership",
                        "golden_records",
                        "golden_lineage",
                    )
                }
                result[f"{phase}_counts"] = tables
                result[f"{phase}_blocking"] = block_statistics(connection)
                result[f"{phase}_candidate_pairs"] = candidate_pair_count(connection)
                assert tables["int_std_records"] == tables["entity_membership"], tables
                assert tables["golden_records"] > 0, tables
                if phase == "base":
                    assert tables["raw_records"] == base_records, tables
                    if case != "tiny":
                        result["output_validation"] = validate_initial_outputs(
                            connection, base_records
                        )
                        if not initial_only:
                            result["base_quality"] = quality_from_csv(
                                connection,
                                corpus,
                                config.thresholds.auto_merge,
                                blocked_count=result["base_candidate_pairs"],
                                include_batch=False,
                            )
                else:
                    assert tables["raw_records"] >= base_records, tables
                if case == "tiny":
                    check_fixture(connection, phase)
                result[f"{phase}_partition_hash"] = partition_sha256(connection)
                if compare_outputs:
                    result[f"{phase}_semantic_hashes"] = save_semantic_outputs(
                        connection, config, directory, phase
                    )
        if prediction_matrix:
            from prediction_matrix import run_matrix

            result["prediction_matrix"] = run_matrix(directory, command)
        if blocking_experiments:
            from blocking_experiments import run_experiments

            if lsh_seed is None:
                raise ValueError("LSH experiments require an explicit seed")
            with connect() as connection:
                result["blocking_experiments"] = run_experiments(
                    connection,
                    config,
                    corpus,
                    seed=lsh_seed,
                    include_batch=not initial_only,
                    onnx_directory=onnx_directory,
                )
            (directory / "blocking-experiments.json").write_text(
                json.dumps(result["blocking_experiments"], indent=2)
            )
        if with_correction:
            with connect() as connection:
                result["pre_correction_quality"] = quality_from_csv(
                    connection,
                    corpus,
                    config.thresholds.auto_merge,
                    blocked_count=result[f"{phase}_candidate_pairs"],
                    include_batch=not initial_only,
                )
            command(["er", "correct"], str(ULID()), "correction")
            with connect() as connection:
                result["correction_quality"] = quality_from_csv(
                    connection,
                    corpus,
                    config.thresholds.auto_merge,
                    blocked_count=result[f"{phase}_candidate_pairs"],
                    include_batch=not initial_only,
                )
                result["correction_partition_hash"] = partition_sha256(connection)
        with span("validation.quality", unit="pairs"), connect() as connection:
            if case != "tiny":
                result["quality"] = result.get("pre_correction_quality") or quality_from_csv(
                    connection,
                    corpus,
                    config.thresholds.auto_merge,
                    blocked_count=result[f"{phase}_candidate_pairs"],
                )
                result[f"{phase}_quality"] = result["quality"]
            from fingerprint import environment_fingerprint

            active = connection.execute(
                "SELECT model_version, tf_snapshot_id FROM lake.main.model_registry "
                "WHERE status='active'"
            ).fetchone()
            result["fingerprint"] = environment_fingerprint(
                scale=case,
                connection=connection,
                config_hash=config_hash(config),
                generator_seed=config.generator.seed,
                model_version=active[0],
                tf_snapshot_id=active[1],
            )
            result["ledger"] = {
                table: json.loads(
                    connection.execute(f"SELECT * FROM lake.main.{table}")
                    .df()
                    .to_json(orient="records", date_format="iso")
                )
                for table in ("runs", "run_stages")
            }
        ingests = [
            stage
            for entry in result["commands"]
            for stage in entry["stages"]
            if stage["stage"] == "ingest"
        ]
        assert sum(stage["rows_in"] for stage in ingests) == base_records + batch_records
        if detailed:
            result["coverage"] = validate_coverage(
                directory,
                trained=case != "tiny",
                initial_only=initial_only,
                batch_mode=batch_mode,
            )
        result["status"] = "succeeded"
    except BaseException as error:
        result.update(status="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        sampler.stop()
        result["sampler_error"] = sampler.error
        resources = directory / "resources.jsonl"
        samples = (
            [json.loads(line) for line in resources.read_text().splitlines()]
            if resources.exists()
            else []
        )
        availability_error = (
            sampler.error
            or (
                "cgroup memory unavailable"
                if not any(sample.get("memory.current") is not None for sample in samples)
                else None
            )
            or (
                "cgroup CPU unavailable"
                if not any(sample.get("usage_usec") is not None for sample in samples)
                else None
            )
        )
        if availability_error and result["status"] == "succeeded":
            result.update(status="failed", error=availability_error)
        result["total_ms"] = (time.monotonic() - started) * 1000
        result["processing_ms"] = sum(
            entry["duration_ms"]
            for entry in result["commands"]
            if entry["command"][0] == "er" and entry["command"][1] != "init"
        )
        (directory / "result.json").write_text(json.dumps(result, indent=2))
    if result["status"] != "succeeded":
        raise RuntimeError(result["error"])
    return result


def validate_initial_outputs(connection: Any, records: int) -> dict[str, int]:
    """Check complete membership and golden lineage without fetching the full lake."""
    from er.lake.columns import GOLDEN_LINEAGE_ATTRIBUTES

    def count(query: str, parameters: list[Any] | None = None) -> int:
        return int(connection.execute(query, parameters or []).fetchone()[0])

    counts = {
        table: count(f"SELECT count(*) FROM lake.main.{table}")
        for table in ("raw_records", "int_std_records", "entity_membership", "golden_records")
    }
    for table in ("raw_records", "int_std_records", "entity_membership"):
        assert counts[table] == records, f"{table}: {counts[table]} != {records}"
    for table in ("int_std_records", "entity_membership"):
        assert count(f"SELECT count(DISTINCT record_key) FROM lake.main.{table}") == records
    assert (
        count(
            "SELECT count(*) FROM lake.main.int_std_records s "
            "ANTI JOIN lake.main.entity_membership m USING (record_key)"
        )
        == 0
    ), "standardized records missing membership"
    entities = count("SELECT count(DISTINCT entity_id) FROM lake.main.entity_membership")
    assert 0 < entities == counts["golden_records"], "golden count differs from entity count"
    assert count("SELECT count(DISTINCT entity_id) FROM lake.main.golden_records") == entities
    assert (
        count(
            "SELECT count(*) FROM lake.main.entity_membership m "
            "ANTI JOIN lake.main.golden_records g USING (entity_id)"
        )
        == 0
    ), "entities missing golden records"
    assert (
        count(
            "SELECT count(*) FROM (SELECT g.entity_id, a.attribute "
            "FROM lake.main.golden_records g CROSS JOIN unnest(?::VARCHAR[]) a(attribute) "
            "LEFT JOIN lake.main.golden_lineage l USING (entity_id, attribute) "
            "GROUP BY g.entity_id, a.attribute HAVING count(l.record_key) != 1)",
            [list(GOLDEN_LINEAGE_ATTRIBUTES)],
        )
        == 0
    ), "golden lineage has missing or duplicate attribute decisions"
    lineage = count("SELECT count(*) FROM lake.main.golden_lineage")
    assert lineage == entities * len(GOLDEN_LINEAGE_ATTRIBUTES), "unexpected lineage rows"
    assert (
        count(
            "SELECT count(*) FROM lake.main.golden_lineage l "
            "ANTI JOIN lake.main.entity_membership m USING (entity_id, record_key)"
        )
        == 0
    ), "lineage winners are not members of their entities"
    return {**counts, "entities": entities, "golden_lineage": lineage}


def validate_repeat_outputs(out: Path) -> None:
    runs = [json.loads(path.read_text()) for path in out.glob("10k-*/result.json")]
    successful = [run for run in runs if run["status"] == "succeeded"]
    fields = ("base_counts", "batch_counts", "base_partition_hash", "batch_partition_hash")
    differences = [
        field
        for field in fields
        if successful and any(run[field] != successful[0][field] for run in successful[1:])
    ]
    (out / "repeat-validation.json").write_text(
        json.dumps(
            {
                "passes_checked": len(successful),
                "matching_outputs": not differences,
                "differences": differences,
            },
            indent=2,
        )
    )
    if differences:
        raise AssertionError(f"repeated/control outputs differ: {differences}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--case", choices=("tiny", "smoke", "10k", "campaign"), default="campaign")
    parser.add_argument("--repeat", type=int, default=3)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    try:
        # The image intentionally does not carry developer-installed dbt packages.
        completed = stream_command(
            ["dbt", "deps", "--project-dir", "dbt"], directory=args.out / "dbt-deps"
        )
        if completed.returncode:
            raise RuntimeError("dbt deps failed")
        for case in ("tiny", "smoke", "10k") if args.case == "campaign" else (args.case,):
            count = args.repeat if case == "10k" else 1
            for iteration in range(1, count + 1):
                print(f"[profile] starting {case}, pass {iteration}", flush=True)
                run_case(args.out, case, iteration)
                write_report(args.out)
            if case == "10k":
                run_case(args.out, case, 1, detailed=False)
                validate_repeat_outputs(args.out)
    finally:
        write_report(args.out)


if __name__ == "__main__":
    main()
