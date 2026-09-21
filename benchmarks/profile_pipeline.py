"""Run the gated tiny -> smoke -> 10k profiling campaign against an isolated Compose lake."""

from __future__ import annotations

import argparse
import csv
import gzip
import hashlib
import json
import os
import shutil
import sys
import time
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import yaml
from profile_report import write_report
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.lake.ducklake import connect
from er.lake.model_registry import register_model
from er.lake.objectstore import ObjectStore
from er.matching.tf import tf_tables_path
from er.obs.profiling import ResourceSampler, cgroup_readings, span, stream_command

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


def validate_coverage(directory: Path, *, trained: bool) -> dict[str, Any]:
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
        "match.incremental",
        "reconcile.label_propagation",
    }
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
) -> dict[str, Any]:
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
    sampler = ResourceSampler(directory / "resources.jsonl")
    sampler.start()
    started = time.monotonic()

    def command(args: list[str], run_id: str | None = None, phase: str = "setup") -> None:
        invocation = uuid.uuid4().hex
        os.environ["ER_PROFILE_INVOCATION_ID"] = invocation
        argv = list(args)
        if argv[0] == "er":
            argv += ["--config", str(config_path), "--json"]
            if run_id is not None:
                argv += ["--run-id", run_id]
        command_dir = directory / "commands" / invocation
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

    try:
        command(["er", "init"])
        if case == "tiny":
            corpus = ROOT / "fixtures/static/incremental_batch"
            base_inputs, batch_inputs = corpus / "base", corpus / "batch"
            base_records, batch_records = 23, 6
            install_fixture(config)
        else:
            from scales import get_scale

            scale = get_scale(case)
            corpus = (corpus_root or out) / f"corpus-{case}"
            base_records, batch_records = scale.records, scale.incremental_batch
            if not corpus.exists():
                command(
                    [
                        sys.executable,
                        "-m",
                        "fixtures.generator.cli",
                        "--personas",
                        str(scale.personas),
                        "--records",
                        str(scale.records),
                        "--batch",
                        str(scale.incremental_batch),
                        "--seed",
                        str(config.generator.seed),
                        "--out",
                        str(corpus),
                        "--config",
                        str(config_path),
                    ]
                )
            base_inputs, batch_inputs = corpus, corpus / "batch"
        drops = {}
        for phase, inputs in (("base", base_inputs), ("batch", batch_inputs)):
            drop = directory / f"drop-{phase}"
            for source in SOURCES:
                (drop / source).mkdir(parents=True)
                shutil.copy2(inputs / f"{source}.csv", drop / source / f"{source}.csv")
            drops[phase] = drop
        result.update(base_records=base_records, incremental_records=batch_records)
        result["input_sha256"] = {
            f"{phase}/{source}.csv": hashlib.sha256(
                (drop / source / f"{source}.csv").read_bytes()
            ).hexdigest()
            for phase, drop in drops.items()
            for source in SOURCES
        }
        for phase, mode in (("base", "full"), ("batch", "incremental")):
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
                result[f"{phase}_candidate_pairs"] = connection.execute(
                    "SELECT count(*) FROM (SELECT DISTINCT a.record_key, b.record_key "
                    "FROM lake.main.int_blocking_keys a JOIN lake.main.int_blocking_keys b "
                    "ON a.key_type=b.key_type AND a.key_value=b.key_value "
                    "AND a.record_key < b.record_key)"
                ).fetchone()[0]
                assert tables["int_std_records"] == tables["entity_membership"], tables
                assert tables["golden_records"] > 0, tables
                if phase == "base":
                    assert tables["raw_records"] == base_records, tables
                else:
                    assert tables["raw_records"] >= base_records, tables
                if case == "tiny":
                    check_fixture(connection, phase)
                groups: dict[str, list[str]] = {}
                for key, entity in connection.execute(
                    "SELECT record_key, entity_id FROM lake.main.entity_membership"
                ).fetchall():
                    groups.setdefault(entity, []).append(key)
                result[f"{phase}_partition_hash"] = hashlib.sha256(
                    json.dumps(sorted(sorted(group) for group in groups.values())).encode()
                ).hexdigest()
                if compare_outputs:
                    result[f"{phase}_semantic_hashes"] = save_semantic_outputs(
                        connection, config, directory, phase
                    )
        with span("validation.quality", unit="pairs"), connect() as connection:
            if case != "tiny":
                from report import _quality_contribution

                result["quality"] = _quality_contribution(
                    connection, corpus, config.thresholds.auto_merge
                )
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
            result["coverage"] = validate_coverage(directory, trained=case != "tiny")
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


def save_semantic_outputs(
    connection: Any, config: Any, directory: Path, phase: str
) -> dict[str, str]:
    """Compare business values independently of generated IDs and execution times."""
    labels = dict(
        connection.execute(
            "SELECT entity_id, min(record_key) FROM lake.main.entity_membership GROUP BY entity_id"
        ).fetchall()
    )
    ignored = {
        "int_std_records": {"ingest_batch_id", "ingested_at"},
        "int_blocking_keys": set(),
        "golden_records": {"assembled_at"},
        "golden_lineage": {"assembled_at"},
    }
    hashes = {}
    for table, excluded in ignored.items():
        cursor = connection.execute(f"SELECT * FROM lake.main.{table}")
        names = [column[0] for column in cursor.description]
        rows = []
        for values in cursor.fetchall():
            row = {
                name: labels[value] if name == "entity_id" else value
                for name, value in zip(names, values, strict=True)
                if name not in excluded
            }
            rows.append(json.dumps(row, sort_keys=True, default=str, ensure_ascii=False))
        hashes[table] = hashlib.sha256("\n".join(sorted(rows)).encode()).hexdigest()
    pairs = connection.execute(
        "SELECT DISTINCT a.record_key, b.record_key FROM lake.main.int_blocking_keys a "
        "JOIN lake.main.int_blocking_keys b ON a.key_type=b.key_type AND a.key_value=b.key_value "
        "AND a.record_key < b.record_key ORDER BY 1,2"
    ).fetchall()
    hashes["candidate_pairs"] = hashlib.sha256(json.dumps(pairs).encode()).hexdigest()
    scores = connection.execute(
        "SELECT rec_a_key, rec_b_key, match_probability, rec_a_content_hash, "
        "rec_b_content_hash, is_active FROM lake.main.match_scores ORDER BY 1,2"
    ).fetchall()
    with gzip.open(directory / f"{phase}-scores.json.gz", "wt") as handle:
        json.dump(scores, handle)
    classes = [
        (
            row[0],
            row[1],
            row[2] >= config.thresholds.auto_merge,
            row[2] >= config.thresholds.review_low,
            *row[3:],
        )
        for row in scores
    ]
    hashes["score_classifications"] = hashlib.sha256(json.dumps(classes).encode()).hexdigest()
    return hashes


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
