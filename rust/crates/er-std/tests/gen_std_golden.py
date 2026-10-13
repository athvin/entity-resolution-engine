"""Golden oracles for the email_norm + name_norm SQL generators, driven through
the REAL dbt macros via MacroHarness (render Jinja -> run in DuckDB).

    uv run python rust/crates/er-std/tests/gen_std_golden.py
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, "tests/unit/dbt")
from harness import MacroHarness, RESULT_COLUMN  # noqa: E402

from er.config.loader import load_config  # noqa: E402
from er.dbt_runner import render_dbt_vars  # noqa: E402

HERE = Path("rust/crates/er-std/tests")
dbt_vars = render_dbt_vars(load_config("configs/test.yaml"), "R1")
STD = dbt_vars["standardization"]  # email_placeholders=[test@test.com, noreply@example.com], strip_plus=false


def with_std(**overrides):
    return {"standardization": {**STD, **overrides}}


EMAIL_INPUTS = [
    None, "", "Alice@Example.COM", "  Carol@Example.com  ", "bob+promo@example.com",
    "x+y+z@a.com", "test@test.com", "noreply@example.com", "not-an-email", "a@b",
    "a@b.c", "N/A", "UPPER+Tag@Domain.COM",
]
NAME_INPUTS = [
    None, "", "  Smith  ", "O'Brien", "Mary-Jane", "José", "José",
    "Anne_Marie", "D'Angelo-Smith", "van der Berg", "N/A", "null", "McDonald’s",
    "J.R.R.", "José María",
]


def main() -> None:
    h = MacroHarness(vars=dbt_vars)
    try:
        email_cases = []
        for strip_plus in (False, True):
            vars = with_std(email_strip_plus_addressing=strip_plus)
            for value in EMAIL_INPUTS:
                row = h.eval_macro("email_norm", value, vars=vars)[0]
                email_cases.append({
                    "input": value, "strip_plus": strip_plus,
                    "placeholders": list(STD["email_placeholders"]),
                    "email": row["email"], "email_valid": row["email_valid"],
                })
        name_cases = []
        for value in NAME_INPUTS:
            row = h.eval_macro("name_norm", value)[0]
            name_cases.append({"input": value, "name": row[RESULT_COLUMN]})
    finally:
        h.close()
    (HERE / "email_golden.json").write_text(
        json.dumps({"_comment": "dbt email_norm via MacroHarness. Regen: uv run python rust/crates/er-std/tests/gen_std_golden.py",
                    "cases": email_cases}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    (HERE / "name_golden.json").write_text(
        json.dumps({"_comment": "dbt name_norm via MacroHarness. Regen: uv run python rust/crates/er-std/tests/gen_std_golden.py",
                    "cases": name_cases}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(email_cases)} email + {len(name_cases)} name cases")


if __name__ == "__main__":
    main()
