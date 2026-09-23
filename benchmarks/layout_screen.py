"""Compare raw-record layouts on a disposable PostgreSQL/MinIO DuckLake stack.

This isolates storage writes and source/batch scans; it does not measure the full
pipeline. Run after Compose bootstrap, with the generated corpus mounted read-only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
import uuid
from pathlib import Path

from er.config.loader import load_config
from er.lake.ducklake import connect, current_snapshot, sql_literal


def run(corpus: Path, out: Path, repeat: int) -> dict:
    out.mkdir(parents=True, exist_ok=False)
    cfg = load_config(Path("configs/default.yaml"))
    result: dict = {"scope": "raw-record writes and source/batch scans only", "trials": []}
    result["input_sha256"] = {
        str(p.relative_to(corpus)): hashlib.sha256(p.read_bytes()).hexdigest()
        for phase in (corpus, corpus / "batch")
        for p in phase.glob("*.csv")
        if p.name != "truth.csv"
    }
    with connect() as c:
        result["settings"] = c.execute(
            "SELECT current_setting('threads'), current_setting('memory_limit'), version()"
        ).fetchone()
        result["ducklake_version"] = c.execute(
            "SELECT extension_version FROM duckdb_extensions() WHERE extension_name='ducklake'"
        ).fetchone()[0]
        c.execute(
            "CREATE TEMP TABLE inputs (source_system VARCHAR, source_record_id VARCHAR, "
            "ingest_batch_id VARCHAR, payload JSON, content_hash VARCHAR)"
        )
        for phase, folder in (("base", corpus), ("batch", corpus / "batch")):
            for source, spec in cfg.sources.items():
                column = '"' + spec.record_id_column.replace('"', '""') + '"'
                c.execute(
                    f"INSERT INTO inputs SELECT ?, {column}, ?, to_json(t), sha256(to_json(t)) "
                    "FROM read_csv(?, all_varchar=true, header=true) t",
                    [source, phase, str(folder / f"{source}.csv")],
                )
        expected = dict(
            c.execute("SELECT ingest_batch_id,count(*) FROM inputs GROUP BY 1").fetchall()
        )
        result["records"] = expected
        arms = ["current", "ordered", "partitioned", "both"]
        for iteration in range(repeat):
            for arm in arms if iteration % 2 == 0 else list(reversed(arms)):
                name = "layout_" + uuid.uuid4().hex
                table = f"lake.main.{name}"
                trial: dict = {"arm": arm, "iteration": iteration + 1, "phases": {}}
                started = time.perf_counter()
                c.execute(f"CREATE TABLE {table} AS SELECT * FROM inputs LIMIT 0")
                try:
                    if arm in ("partitioned", "both"):
                        c.execute(f"ALTER TABLE {table} SET PARTITIONED BY (source_system)")
                    if arm in ("ordered", "both"):
                        c.execute(
                            f"ALTER TABLE {table} SET SORTED BY "
                            "(source_system, ingest_batch_id, source_record_id)"
                        )
                    trial["ddl_seconds"] = time.perf_counter() - started
                    snapshots = {}
                    for phase in ("base", "batch"):
                        tick = time.perf_counter()
                        # Production ingests each source separately. Preserve that
                        # natural clustering rather than inventing a random baseline.
                        for source in cfg.sources:
                            c.execute(
                                f"INSERT INTO {table} SELECT * FROM inputs "
                                "WHERE source_system=? AND ingest_batch_id=?",
                                [source, phase],
                            )
                        write_seconds = time.perf_counter() - tick
                        tick = time.perf_counter()
                        counts, payload_bytes = 0, 0
                        for source in cfg.sources:
                            count, size = c.execute(
                                f"SELECT count(*),sum(length(payload::VARCHAR)) FROM {table} "
                                "WHERE source_system=? AND ingest_batch_id NOT IN "
                                "(SELECT unnest(?::VARCHAR[]))",
                                [source, [] if phase == "base" else ["base"]],
                            ).fetchone()
                            counts += count
                            payload_bytes += size
                        read_seconds = time.perf_counter() - tick
                        assert counts == expected[phase], (counts, expected)
                        snapshots[phase] = current_snapshot(c)
                        trial["phases"][phase] = {
                            "write_seconds": write_seconds,
                            "read_seconds": read_seconds,
                            "total_seconds": write_seconds + read_seconds,
                            "records": counts,
                            "payload_characters": payload_bytes,
                        }
                    # Diagnostics follow both timed deliveries, and replay only reads.
                    for phase, snapshot in snapshots.items():
                        files = c.execute(
                            f"SELECT data_file_size_bytes FROM ducklake_list_files('lake', "
                            f"{sql_literal(name)}, snapshot_version => {snapshot})"
                        ).fetchall()
                        trial["phases"][phase]["files"] = len(files)
                        trial["phases"][phase]["file_bytes"] = sum(row[0] for row in files)
                        profile = out / f"{iteration + 1}-{arm}-{phase}.json"
                        c.execute("SET enable_profiling='json'")
                        c.execute(f"SET profiling_output={sql_literal(str(profile))}")
                        c.execute(
                            f"SELECT count(*),sum(length(payload::VARCHAR)) FROM {table} "
                            f"AT (VERSION => {snapshot}) WHERE source_system='crm' "
                            "AND ingest_batch_id NOT IN (SELECT unnest(?::VARCHAR[]))",
                            [[] if phase == "base" else ["base"]],
                        ).fetchall()
                        c.execute("PRAGMA disable_profiling")
                        trial["phases"][phase]["profile"] = profile.name
                    trial["status"] = "passed"
                finally:
                    c.execute("PRAGMA disable_profiling")
                    c.execute(f"DROP TABLE {table}")
                result["trials"].append(trial)
                (out / "results.json").write_text(json.dumps(result, indent=2) + "\n")
                print(arm, iteration + 1, trial["phases"], flush=True)
    result["status"] = "passed"
    (out / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=3)
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("repeat must be positive")
    run(args.corpus, args.out, args.repeat)
