"""Golden oracle for name_variants, driven through the dbt macro via a seeded
MacroHarness (nickname_variants registered as source('lake', ...)).

    uv run python rust/crates/er-std/tests/gen_variants_golden.py
"""
import csv
import json
import sys
from pathlib import Path

sys.path.insert(0, "tests/unit/dbt")
from harness import MacroHarness, RESULT_COLUMN  # noqa: E402

from er.config.loader import load_config  # noqa: E402
from er.dbt_runner import render_dbt_vars  # noqa: E402

HERE = Path("rust/crates/er-std/tests")
SEED = Path("src/er/std/nickname_variants.csv")
UNIT = "\x1f"


def seed_pairs():
    with SEED.open(encoding="utf-8", newline="") as h:
        return [(r["variant_a"], r["variant_b"]) for r in csv.DictReader(h)]


INPUTS = [None, "", "Robert", "bob", "bobby", "james", "elizabeth", "catherine",
          "smith", "O'Brien", "José", "  Mike  ", "susan", "dave", "chris", "anthony"]


def main() -> None:
    dbt_vars = render_dbt_vars(load_config("configs/test.yaml"), "R1")
    pairs = seed_pairs()
    h = MacroHarness(vars=dbt_vars)
    try:
        table = h.register_seed("nickname_variants",
                                [{"variant_a": a, "variant_b": b, "active": True} for a, b in pairs])
        h.register_relation("lake.nickname_variants", table)
        cases = []
        for value in INPUTS:
            lst = h.eval_macro("name_variants", value)[0][RESULT_COLUMN]
            cases.append({"input": value, "name_variants": UNIT.join(lst)})
    finally:
        h.close()
    (HERE / "variants_golden.json").write_text(
        json.dumps({"_comment": "dbt name_variants via seeded MacroHarness. Regen: uv run python rust/crates/er-std/tests/gen_variants_golden.py",
                    "lexicon": [list(p) for p in pairs], "cases": cases}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(cases)} variant cases over {len(pairs)} lexicon pairs")


if __name__ == "__main__":
    main()
