"""Generate the base_10 match-score golden: the Python/Splink oracle this crate's
matching gate (`match_scores.rs`) reproduces at 1e-10.

This replicates the scoring CORE of `er.matching.full.score_full` with direct
Splink 5.0.0 calls — the committed settings model, frozen term frequency injected
via `register_term_frequency_lookup` (never recomputed from the corpus, D4/
INV-SCORE), prediction at the `review_low = 0.60` probability threshold, and the
same `least`/`greatest` canonicalization + highest-probability dedup as
`_selected_predictions_sql`. The lake/MERGE/review machinery `score_full` wraps
around that core is not exercised here; only the per-pair `match_probability` is.

Input is the committed Parquet corpus (`int_std_records_base_10.parquet`,
produced by `dump_corpus.rs` from Rust standardization, which `er-standardize`'s
std_hashes gate proves byte-exact vs Python). Run:
  uv run python rust/crates/er-match/tests/gen_match_golden.py
"""

from __future__ import annotations

import json
from pathlib import Path

import duckdb
from splink import DuckDBAPI, Linker

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"
CORPUS = FIXTURES / "int_std_records_base_10.parquet"
TF_CSV = FIXTURES / "model_test_v1.tf.csv"
MODEL = FIXTURES / "model_test_v1.json"
GOLDEN = FIXTURES / "match_scores_base_10.csv"

MODEL_VERSION = "v0001"
TF_SNAPSHOT_ID = "01JWMDTFSNAP00000000000001"
REVIEW_LOW = 0.60  # configs/test.yaml thresholds.review_low
# tf_columns(cfg) in config order (comparisons with `tf: true`).
TF_COLUMNS = ["given_name", "family_name", "email"]
TF_PREFIX = "tf_"


def main() -> None:
    con = duckdb.connect()
    # Pin the two determinism settings splink_api pins (plan/order stability).
    con.execute("SET threads = 1")
    con.execute("SET preserve_insertion_order = true")

    con.execute(
        f"CREATE TABLE er_match_corpus AS SELECT * FROM read_parquet('{CORPUS.as_posix()}')"
    )
    con.execute(
        "CREATE TABLE tf_lookup AS SELECT * FROM "
        f"read_csv('{TF_CSV.as_posix()}', header = true, "
        "columns = {'model_version': 'VARCHAR', 'tf_snapshot_id': 'VARCHAR', "
        "'column_name': 'VARCHAR', 'value': 'VARCHAR', 'tf_value': 'DOUBLE'})"
    )

    settings = json.loads(MODEL.read_text())
    # scoring_settings' one deviation from the frozen artifact. The committed
    # blocking rules are the config-derived ones, so they are kept as-is.
    settings["retain_intermediate_calculation_columns"] = True

    api = DuckDBAPI(connection=con)
    linker = Linker(api.register("er_match_corpus"), settings=settings)

    # Frozen TF: one register per column, aliased to Splink's <col>/tf_<col>.
    for column in TF_COLUMNS:
        relation = f"er_frozen_tf_{column}"
        con.execute(
            f'CREATE TEMP TABLE {relation} AS SELECT value AS "{column}", '
            f'tf_value AS "{TF_PREFIX}{column}" FROM tf_lookup '
            "WHERE model_version = ? AND tf_snapshot_id = ? AND column_name = ?",
            [MODEL_VERSION, TF_SNAPSHOT_ID, column],
        )
        linker.table_management.register_term_frequency_lookup(
            input_data=api.register(relation), col_name=column
        )

    predictions = linker.inference.predict(threshold_match_probability=REVIEW_LOW)
    relation = str(predictions.physical_name)

    rows = con.execute(
        f"""
        SELECT DISTINCT ON (rec_a_key, rec_b_key) rec_a_key, rec_b_key, match_probability
          FROM (
                SELECT least(record_key_l, record_key_r) AS rec_a_key,
                       greatest(record_key_l, record_key_r) AS rec_b_key,
                       match_probability, rowid AS prediction_id
                  FROM {relation}
                 WHERE record_key_l <> record_key_r
               )
         ORDER BY rec_a_key, rec_b_key, match_probability DESC, prediction_id
        """
    ).fetchall()

    with GOLDEN.open("w", newline="") as fh:
        fh.write("rec_a_key,rec_b_key,match_probability\n")
        for rec_a, rec_b, prob in rows:
            fh.write(f"{rec_a},{rec_b},{prob!r}\n")

    print(f"wrote {len(rows)} scored pairs to {GOLDEN}")


if __name__ == "__main__":
    main()
