"""Capture the migration oracle inside the immutable Splink 4 baseline image."""

import json
import os
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, "/app/tests")
import duckdb
import splink
from helpers.model import fixture_settings, fixture_tf_rows
from splink import Linker
from unit.matching.test_train_sequence import STD_RECORDS_DDL, corpus_rows

from er.config.loader import load_config
from er.matching.api import splink_api
from er.matching.tf import register_tf

assert splink.__version__ == "4.0.16"
os.environ["ER_DUCKDB_THREADS"] = "2"
config = load_config(Path("/app/configs/test.yaml"))
frozen = fixture_tf_rows()
by_column = defaultdict(list)
for row in frozen:
    by_column[row[2]].append((row[3], row[4]))
for values in by_column.values():
    values.sort(key=lambda entry: (-entry[1], entry[0]))
rows = []
for index, row in enumerate(corpus_rows()):
    row = list(row)
    persona = index // 2
    row[1] = by_column["given_name"][persona][0] + ("x" if index % 2 and persona % 3 == 0 else "")
    row[2] = by_column["family_name"][persona][0]
    row[3] = [by_column["given_name"][persona][0]]
    if persona % 4:
        row[4] = by_column["email"][persona][0]
    rows.append(row)
settings = fixture_settings()
settings["retain_intermediate_calculation_columns"] = True
# Exhaustive pairs exercise weak, null, negative and positive evidence as well as
# real term-frequency adjustments. Candidate-generation parity has its own gate.
settings["blocking_rules_to_generate_predictions"] = ["1=1"]
conn = duckdb.connect()
conn.execute("ATTACH ':memory:' AS lake")
conn.execute(STD_RECORDS_DDL)
conn.executemany("INSERT INTO lake.main.int_std_records VALUES (?, ?, ?, ?, ?, ?, ?, ?)", rows)
conn.execute(
    "CREATE TABLE lake.main.tf_lookup (model_version VARCHAR, tf_snapshot_id VARCHAR, "
    "column_name VARCHAR, value VARCHAR, tf_value DOUBLE)"
)
needed = {
    c: {row[i] for row in rows} for c, i in [("given_name", 1), ("family_name", 2), ("email", 4)]
}
tf = [row for row in frozen if row[3] in needed[row[2]]]
conn.executemany("INSERT INTO lake.main.tf_lookup VALUES (?, ?, ?, ?, ?)", tf)
api = splink_api(conn)
conn.execute("CREATE TABLE migration_corpus AS SELECT * FROM lake.main.int_std_records")
linker = Linker("migration_corpus", settings=settings, db_api=api)
register_tf(linker, conn, config, tf[0][0], tf[0][1])
pred = linker.inference.predict(threshold_match_probability=0.0)
cursor = conn.execute(f"SELECT * FROM {pred.physical_name} ORDER BY record_key_l, record_key_r")
cols = [d[0] for d in cursor.description]
keep = [
    i
    for i, c in enumerate(cols)
    if c in ("record_key_l", "record_key_r", "match_probability") or c.startswith(("gamma_", "bf_"))
]
scores = [{cols[i]: row[i] for i in keep} for row in cursor.fetchall()]
out = Path("/capture/oracle")
out.mkdir(exist_ok=True)
(out / "model.json").write_text(json.dumps(settings, sort_keys=True, indent=2) + "\n")
(out / "inputs.json").write_text(
    json.dumps({"records": rows, "tf": tf}, indent=2, default=str) + "\n"
)
(out / "scores.json").write_text(
    json.dumps(
        {"splink_version": splink.__version__, "source_commit": "fc5070e", "scores": scores},
        indent=2,
    )
    + "\n"
)
adjusted = sum(any(v != 1 for k, v in row.items() if k.startswith("bf_tf_adj_")) for row in scores)
print(f"Saved {len(scores)} pairs with {adjusted} TF-adjusted pairs")
