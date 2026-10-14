"""Golden oracle for source_metadata, driven through the dbt macro via MacroHarness.

    uv run python rust/crates/er-std/tests/gen_metadata_golden.py
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, "tests/unit/dbt")
from harness import MacroHarness, RESULT_COLUMN  # noqa: E402

from er.config.loader import load_config  # noqa: E402
from er.config.schema import CANONICAL_ATTRIBUTES  # noqa: E402
from er.dbt_runner import render_dbt_vars  # noqa: E402

HERE = Path("rust/crates/er-std/tests")
cfg = load_config("configs/test.yaml")
s = cfg.sources["crm"]
spec = {"record_id_column": s.record_id_column, "updated_at_column": s.updated_at_column, "columns": dict(s.columns)}
excluded = [s.record_id_column, s.updated_at_column, "persona_id"] + [s.columns[a] for a in CANONICAL_ATTRIBUTES]
dbt_vars = render_dbt_vars(cfg, "R1")

base = {"crm_id": "1", "first_name": "Ann", "last_name": "Lee", "email_address": "a@b.com",
        "phone": "5551234", "street_address": "1 Main", "city": "X", "state": "CA",
        "zip": "90001", "dob": "1990-01-01", "last_modified": "t", "persona_id": "P1"}
PAYLOADS = [
    json.dumps({**base, "notes": "vip", "loyalty": "gold"}),       # two extras
    json.dumps(base),                                               # no extras -> {}
    json.dumps({**base, "z_last": "1", "a_first": "2", "m_mid": "3"}),  # sorting
    json.dumps({**base, "a/b": "slash", "c~d": "tilde"}),           # JSON-pointer escaping
]


def main() -> None:
    h = MacroHarness(vars=dbt_vars)
    try:
        cases = []
        for payload in PAYLOADS:
            val = h.eval_macro("source_metadata", payload, spec)[0][RESULT_COLUMN]
            cases.append({"payload": payload, "excluded": excluded, "metadata": val})
    finally:
        h.close()
    (HERE / "metadata_golden.json").write_text(
        json.dumps({"_comment": "dbt source_metadata via MacroHarness. Regen: uv run python rust/crates/er-std/tests/gen_metadata_golden.py",
                    "cases": cases}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(cases)} metadata cases; excluded={excluded}")


if __name__ == "__main__":
    main()
