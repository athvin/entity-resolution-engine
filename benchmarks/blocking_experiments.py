"""Exploratory blocking arms over one frozen model; all row operations stay in SQL.

This never changes lake tables. End-to-end workload gates must be measured by the
pipeline harness before a candidate-generation experiment can be promoted.
"""

from __future__ import annotations

import hashlib
import time
from contextlib import ExitStack
from pathlib import Path
from typing import Any

import duckdb
from large_validation import candidate_pair_count, quality_from_csv
from splink import Linker

from er.config.schema import Config
from er.entities.cluster import label_propagate_relations
from er.lake.bulk import staged_query
from er.lake.model_registry import active_model, load_model_settings
from er.lake.objectstore import ObjectStore
from er.matching.api import cleanup_splink, splink_api
from er.matching.model import blocking_rules_from_config, scoring_settings
from er.matching.tf import register_tf

PRIME = 2_147_483_647
BANDS = 8
ROWS_PER_BAND = 4
MAX_BUCKET = 500
VIEWS = {
    "name": (
        "trim(concat_ws(' ', nullif(given_name,''), nullif(family_name,'')))",
        "year(birth_date)::VARCHAR",
    ),
    "address": (
        "trim(concat_ws(' ',addr_number,addr_street,addr_unit,addr_city,addr_region,addr_postal))",
        "nullif(substr(family_name,1,1),'')",
    ),
}


def coefficients(seed: int, count: int) -> list[tuple[int, int]]:
    """Explicit SHA-256 recipe; no Python/DuckDB version-dependent hash()."""
    values = []
    for slot in range(count):
        digest = hashlib.sha256(f"er-minhash-v1:{seed}:{slot}".encode()).digest()
        values.append(
            (
                int.from_bytes(digest[:8], "big") % (PRIME - 1) + 1,
                int.from_bytes(digest[8:16], "big") % PRIME,
            )
        )
    return values


def minhash_keys(
    c: duckdb.DuckDBPyConnection,
    inputs: str,
    *,
    seed: int,
    bands: int = BANDS,
    rows: int = ROWS_PER_BAND,
) -> str:
    """Stored MinHash band keys for distinct view inputs, with integer arithmetic."""
    if bands < 1 or rows < 1:
        raise ValueError("band counts and rows must be positive")
    c.execute(
        "CREATE OR REPLACE TEMP TABLE bench_minhash_keys "
        "(input_id VARCHAR, band INTEGER, value VARCHAR)"
    )
    with ExitStack() as stack:
        grams = stack.enter_context(
            staged_query(
                c,
                f"SELECT DISTINCT input_id, ('0x' || substr(md5(substr(text, pos, 3)),1,8))"
                f"::UBIGINT % {PRIME} AS gram FROM {inputs}, "
                "LATERAL range(1, length(text)-1) p(pos) WHERE length(text)>=3",
            )
        )
        for band in range(bands):
            expressions = [
                f"min((gram * {a}::UBIGINT + {b}::UBIGINT) % {PRIME})::VARCHAR"
                for a, b in coefficients(seed, bands * rows)[band * rows : (band + 1) * rows]
            ]
            c.execute(
                "INSERT INTO bench_minhash_keys SELECT input_id, ?, "
                f"sha256(concat_ws(':', {', '.join(expressions)})) FROM {grams} GROUP BY input_id",
                [band],
            )
    return "bench_minhash_keys"


def run_experiments(
    c: duckdb.DuckDBPyConnection,
    cfg: Config,
    corpus: Path,
    *,
    seed: int,
    include_batch: bool,
    onnx_directory: Path | None = None,
) -> dict[str, Any]:
    """Compare SQL rules and additive/replacement LSH on identical scoring inputs."""
    active = active_model(c)
    settings = load_model_settings(c, ObjectStore.from_env(), active.model_version)
    results: dict[str, Any] = {
        "seed": seed,
        "bands": BANDS,
        "rows_per_band": ROWS_PER_BAND,
        "max_bucket": MAX_BUCKET,
        "views": VIEWS,
        "model_version": active.model_version,
        "tf_snapshot_id": active.tf_snapshot_id,
        "promotion_eligible": False,
        "note": "Exploratory match/cluster timings; excludes ingestion, training and assembly.",
        "arms": {},
    }
    base_payload, base_rules = blocking_rules_from_config(cfg)
    try:
        with ExitStack() as stack:

            def stage(sql: str, parameters: list[Any] | None = None) -> str:
                return stack.enter_context(staged_query(c, sql, parameters or []))

            def evaluate(name: str, records: str, keys: str, rules: list[Any], prep: float) -> None:
                tick = time.monotonic()
                api = splink_api(c)
                # This pinned Splink backend discovers columns by bare table name.
                # Register a local view, not a qualified temp-table identifier.
                c.execute(f"CREATE OR REPLACE VIEW er_bench_records AS SELECT * FROM {records}")
                linker = Linker(
                    api.register("er_bench_records"),
                    settings={
                        **scoring_settings(cfg, settings),
                        "blocking_rules_to_generate_predictions": rules,
                    },
                )
                register_tf(linker, c, cfg, active.model_version, active.tf_snapshot_id)
                predicted = linker.inference.predict(
                    threshold_match_probability=cfg.thresholds.review_low
                )
                scores = stage(
                    "SELECT record_key_l rec_a_key, record_key_r rec_b_key, "
                    f"match_probability, true is_active FROM {predicted.physical_name}"
                )
                edges = stage(
                    f"SELECT * FROM {scores} WHERE match_probability >= ?",
                    [cfg.thresholds.auto_merge],
                )
                with label_propagate_relations(
                    c, records, edges, max_iterations=cfg.clustering.max_iterations
                ) as (labels, _):
                    membership = stage(f"SELECT record_key, label entity_id FROM {labels}")
                elapsed = time.monotonic() - tick
                candidates = candidate_pair_count(c, keys_relation=keys)
                quality = quality_from_csv(
                    c,
                    corpus,
                    cfg.thresholds.auto_merge,
                    blocked_count=candidates,
                    include_batch=include_batch,
                    keys_relation=keys,
                    membership_relation=membership,
                    scores_relation=scores,
                )
                results["arms"][name] = {
                    "preprocessing_seconds": prep,
                    "match_cluster_seconds": elapsed,
                    "candidate_pairs": candidates,
                    "quality": quality,
                    "key_rows": c.execute(f"SELECT count(*) FROM {keys}").fetchone()[0],
                }
                cleanup_splink(api)

            # Splink resolves local input relations; its registered table has one row per record.
            records = stage("SELECT * FROM lake.main.int_std_records")
            evaluate("current", records, "lake.main.int_blocking_keys", base_rules, 0)
            retained = [
                (p, r)
                for p, r in zip(base_payload, base_rules, strict=True)
                if p["key_type"] != "name_postal"
            ]
            no_broad = stage(
                "SELECT * FROM lake.main.int_blocking_keys WHERE key_type <> 'name_postal'"
            )
            evaluate("without_name_postal", records, no_broad, [r for _, r in retained], 0)

            tick = time.monotonic()
            views = stage(
                " UNION ALL ".join(
                    f"SELECT record_key, '{view}' AS view_name, {expr} AS text, {guard} AS guard "
                    "FROM lake.main.int_std_records"
                    for view, (expr, guard) in VIEWS.items()
                )
            )
            inputs = stage(
                f"SELECT DISTINCT sha256(text) input_id, text FROM {views} "
                "WHERE length(text)>=3 AND guard IS NOT NULL"
            )
            results["distinct_inputs"] = c.execute(f"SELECT count(*) FROM {inputs}").fetchone()[0]
            preparation = time.monotonic() - tick
            providers = ["minhash"] + (["onnx"] if onnx_directory is not None else [])
            for provider in providers:
                tick = time.monotonic()
                if provider == "minhash":
                    signatures = minhash_keys(c, inputs, seed=seed)
                else:
                    from vector_runtime import doctor, onnx_keys

                    assert onnx_directory is not None
                    results["onnx_provider"] = doctor(onnx_directory)
                    results["onnx_bits_per_band"] = ROWS_PER_BAND * 4
                    signatures = onnx_keys(
                        c, inputs, onnx_directory, seed=seed, bands=BANDS, bits=ROWS_PER_BAND * 4
                    )
                raw = stage(
                    f"SELECT v.record_key, v.view_name || '_' || k.band AS key_type, "
                    "k.value || ':' || v.guard AS key_value "
                    f"FROM {views} v JOIN {signatures} k ON sha256(v.text)=k.input_id "
                    "WHERE v.guard IS NOT NULL"
                )
                stopped = stage(
                    f"SELECT key_type,key_value FROM {raw} GROUP BY ALL "
                    "HAVING count(DISTINCT record_key)>?",
                    [MAX_BUCKET],
                )
                keys = stage(
                    f"SELECT r.* FROM {raw} r ANTI JOIN {stopped} s USING(key_type,key_value)"
                )
                columns = [f"{view}_{band}" for view in VIEWS for band in range(BANDS)]
                pivot = stage(
                    "SELECT record_key, "
                    + ", ".join(
                        f"max(key_value) FILTER (WHERE key_type='{column}') AS {column}"
                        for column in columns
                    )
                    + f" FROM {keys} GROUP BY record_key"
                )
                wide = stage(
                    f"SELECT s.*, p.* EXCLUDE(record_key) FROM {records} s "
                    f"LEFT JOIN {pivot} p USING(record_key)"
                )
                elapsed = preparation + time.monotonic() - tick
                results[f"{provider}_stopped_buckets"] = c.execute(
                    f"SELECT count(*) FROM {stopped}"
                ).fetchone()[0]
                results[f"{provider}_raw_key_rows"] = c.execute(
                    f"SELECT count(*) FROM {raw}"
                ).fetchone()[0]
                vector_rules = [f"l.{col} = r.{col}" for col in columns]
                for name, ordinary, rules in [
                    ("plus", "lake.main.int_blocking_keys", base_rules),
                    ("replace", no_broad, [r for _, r in retained]),
                ]:
                    union = stage(
                        f"SELECT record_key,key_type,key_value FROM {ordinary} "
                        f"UNION ALL SELECT record_key,key_type,key_value FROM {keys}"
                    )
                    evaluate(f"{provider}_{name}", wide, union, [*rules, *vector_rules], elapsed)
    finally:
        # Every experimental intermediate lives outside DuckLake.
        c.execute("DROP TABLE IF EXISTS bench_minhash_keys")
        c.execute("DROP TABLE IF EXISTS bench_onnx_keys")
        c.execute("DROP VIEW IF EXISTS er_bench_records")
    return results
