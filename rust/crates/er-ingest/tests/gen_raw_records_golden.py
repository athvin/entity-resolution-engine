"""Golden oracle for er-ingest base-delivery content_hash, from the real Python
ingest hasher (er.ingest.hashing.content_hash) over base_10's base delivery.

    uv run python rust/crates/er-ingest/tests/gen_raw_records_golden.py
"""
import csv
import json
from pathlib import Path

from er.config.loader import load_config
from er.ingest.hashing import content_hash

BASE = Path("fixtures/static/base_10/base")
HERE = Path("rust/crates/er-ingest/tests")
cfg = load_config("configs/default.yaml")


def main() -> None:
    cases = []
    for source in ("crm", "billing", "webforms"):
        spec = cfg.sources[source]
        columns = tuple(spec.columns.values())  # declared (YAML) order
        with (BASE / f"{source}.csv").open(newline="", encoding="utf-8") as fh:
            for raw in csv.DictReader(fh):
                row = {k: v for k, v in raw.items() if k != "persona_id"}  # stripped at delivery
                md = spec.metadata_columns(row)
                cases.append({
                    "source_system": source,
                    "source_record_id": row[spec.record_id_column],
                    "payload": row,
                    "content_hash": content_hash(row, columns, metadata_columns=tuple(md)),
                    "metadata": list(md),
                })
    (HERE / "raw_records_golden.json").write_text(
        json.dumps({"_comment": "base_10 raw_records content_hash from er.ingest.hashing. Regen: uv run python rust/crates/er-ingest/tests/gen_raw_records_golden.py",
                    "cases": cases}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(cases)} raw_records cases; metadata-empty={all(not c['metadata'] for c in cases)}")


if __name__ == "__main__":
    main()
