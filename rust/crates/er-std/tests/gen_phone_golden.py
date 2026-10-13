"""Golden oracle for the phone_e164 SQL generator: drive the REAL dbt macro
through MacroHarness (renders Jinja -> runs in DuckDB) over US and GB regions.

    uv run python rust/crates/er-std/tests/gen_phone_golden.py
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, "tests/unit/dbt")
from harness import MacroHarness  # noqa: E402

from er.config.loader import load_config  # noqa: E402
from er.dbt_runner import render_dbt_vars  # noqa: E402

HERE = Path("rust/crates/er-std/tests")
dbt_vars = render_dbt_vars(load_config("configs/test.yaml"), "R1")  # standardization.phone_default_region = US


def gb(vars):
    return {"standardization": {**vars["standardization"], "phone_default_region": "GB"}}


US_INPUTS = [None, "", "N/A", "unknown", "-", "  (415) 555-0132 ", "415-555-0132",
             "+14155550132", "4155550132", "1-415-555-0132", "415.555.0132",
             "+442071838750", "555-0132", "+1234567890123456", "04155550132", "14155550132"]
GB_INPUTS = ["+442071838750", "4155550132", "(415) 555-0132", "1-415-555-0132", None, "020 7183 8750"]


def main() -> None:
    h = MacroHarness(vars=dbt_vars)
    try:
        cases = []
        for region, inputs, vars in [("US", US_INPUTS, None), ("GB", GB_INPUTS, gb(dbt_vars))]:
            for value in inputs:
                row = h.eval_macro("phone_e164", value, vars=vars)[0]
                cases.append({"input": value, "region": region,
                              "phone_e164": row["phone_e164"], "phone_valid": row["phone_valid"]})
    finally:
        h.close()
    doc = {"_comment": "Golden phone_e164 oracle from the dbt macro via MacroHarness. Regenerate: uv run python rust/crates/er-std/tests/gen_phone_golden.py",
           "cases": cases}
    (HERE / "phone_golden.json").write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(cases)} phone cases")


if __name__ == "__main__":
    main()
