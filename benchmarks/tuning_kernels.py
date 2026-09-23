"""Screen SQL/staging changes separately; complete workload trials decide promotion."""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import duckdb
from jinja2 import Environment, StrictUndefined

from er.lake.bulk import insert_batches
from er.matching.edges import materialize_current_edges


def decision(macro: Path, chain: list[str]) -> str:
    rules = Path("dbt/macros/survivorship/rules.sql").read_text()
    module = Environment(undefined=StrictUndefined).from_string(rules + macro.read_text()).module
    return str(module.survivorship_decision("email", chain, "member_rows"))


def run(args: argparse.Namespace) -> dict:
    result: dict = {"rows": args.rows, "repeat": args.repeat, "trials": []}
    with duckdb.connect() as c:
        c.execute("SET threads=2")
        c.execute("SET memory_limit='4GB'")
        c.execute(
            "CREATE TABLE members AS SELECT (i % ?)::VARCHAR AS entity_id, "
            "'crm:' || lpad(i::VARCHAR,12,'0') AS record_key, "
            "CASE WHEN i%17=0 THEN NULL ELSE 'person'||(i%?)::VARCHAR||'@example.test' "
            "END AS value, "
            "CASE WHEN i%5=0 THEN NULL ELSE i%7<>0 END AS email_valid, "
            "TIMESTAMP '2020-01-01' + (i%90)::INTEGER * INTERVAL '1 day' AS ingested_at, "
            "CASE WHEN i%11=0 THEN NULL ELSE TIMESTAMP '2024-01-01' + "
            "(i%70)::INTEGER * INTERVAL '1 day' END AS updated_at_source, "
            "CASE WHEN i%3=0 THEN 'crm' ELSE 'web' END AS source_system, "
            "MAP {'crm': struct_pack(priority_rank:=1), 'web':struct_pack(priority_rank:=2)} "
            "AS sources FROM range(?) t(i)",
            [args.rows * 2 // 5, args.rows * 2 // 5, args.rows],
        )
        macros = {
            "window": args.baseline_macro,
            "top_two": Path("benchmarks/experiments/survivorship_top_two.sql"),
            "lead": Path("benchmarks/experiments/survivorship_lead.sql"),
        }
        for phase, predicate in (("full", "true"), ("touched", "entity_id::BIGINT%6=0")):
            c.execute(
                "CREATE OR REPLACE TEMP VIEW member_rows AS "
                f"SELECT * FROM members WHERE {predicate}"
            )
            for chain in (
                ["validated", "source_priority", "recency"],
                ["frequency", "completeness", "recency"],
            ):
                for iteration in range(args.repeat):
                    for arm in list(macros) if iteration % 2 == 0 else list(reversed(macros)):
                        sql = decision(macros[arm], chain)
                        started = time.perf_counter()
                        c.execute(f"CREATE OR REPLACE TEMP TABLE answer_{arm} AS {sql}")
                        elapsed = time.perf_counter() - started
                        result["trials"].append(
                            {
                                "kernel": "survivorship",
                                "phase": phase,
                                "chain": chain,
                                "iteration": iteration + 1,
                                "arm": arm,
                                "seconds": elapsed,
                            }
                        )
                    for arm in ("top_two", "lead"):
                        differences = c.execute(
                            "SELECT count(*) FROM ((TABLE answer_window "
                            f"EXCEPT ALL TABLE answer_{arm}) "
                            f"UNION ALL (TABLE answer_{arm} EXCEPT ALL TABLE answer_window))"
                        ).fetchone()[0]
                        assert differences == 0, differences
        for iteration in range(args.repeat):
            for size in [1024, 8192, 16384] if iteration % 2 == 0 else [16384, 8192, 1024]:
                c.execute("CREATE OR REPLACE TEMP TABLE staging (position BIGINT, key VARCHAR)")
                started = time.perf_counter()
                insert_batches(
                    c,
                    "INSERT INTO staging SELECT unnest(?::BIGINT[]), unnest(?::VARCHAR[])",
                    ((i, f"crm:{i:012}") for i in range(args.rows)),
                    columns=2,
                    batch_rows=size,
                )
                elapsed = time.perf_counter() - started
                assert c.execute(
                    "SELECT count(*), count(distinct position) FROM staging"
                ).fetchone() == (args.rows, args.rows)
                result["trials"].append(
                    {
                        "kernel": "staging",
                        "iteration": iteration + 1,
                        "arm": str(size),
                        "seconds": elapsed,
                    }
                )
        c.execute("ATTACH ':memory:' AS lake")
        c.execute(
            "CREATE TABLE lake.main.match_scores AS SELECT record_key AS rec_a_key, "
            "record_key||'z' AS rec_b_key, 0.95 AS match_probability, 'v1' AS model_version, "
            "'tf1' AS tf_snapshot_id, true AS is_active, ingested_at AS scored_at, "
            "'run1' AS run_id FROM members"
        )
        for iteration in range(args.repeat):
            for materialized in [False, True] if iteration % 2 == 0 else [True, False]:
                started = time.perf_counter()
                name = materialize_current_edges(c, "v1", "tf1", materialized=materialized)
                for predicate in (
                    "true",
                    "match_probability>=0.9",
                    "rec_a_key<rec_b_key",
                    "match_probability<0.99",
                ):
                    assert c.execute(
                        f"SELECT count(*) FROM {name} WHERE {predicate}"
                    ).fetchone() == (args.rows,)
                c.execute(f"DROP {'TABLE' if materialized else 'VIEW'} {name}")
                result["trials"].append(
                    {
                        "kernel": "current_edges",
                        "iteration": iteration + 1,
                        "arm": "table" if materialized else "view",
                        "seconds": time.perf_counter() - started,
                    }
                )
    result["status"] = "passed"
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rows", type=int, default=1_000_000)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--baseline-macro", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    arguments = parser.parse_args()
    if arguments.rows < 10 or arguments.repeat < 1:
        parser.error("rows must be at least 10 and repeat must be positive")
    run(arguments)
