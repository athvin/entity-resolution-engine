"""Golden oracle for the S4.4.2 never-cut fixpoint, from the pure Python
er.review.never_cut.never_cut_fixpoint over crafted edge + never-assertion cases.

    uv run python rust/crates/er-reconcile/tests/gen_never_cut_golden.py
"""
import json
from datetime import datetime
from pathlib import Path

from er.review.assertions import NEVER, Assertion
from er.review.never_cut import never_cut_fixpoint

HERE = Path("rust/crates/er-reconcile/tests")
_T = datetime(2026, 1, 1)


def never(a, b, aid):
    lo, hi = (a, b) if a < b else (b, a)
    return Assertion(assertion_id=aid, rec_a_key=lo, rec_b_key=hi, kind=NEVER, active=True,
                     created_by="x", created_at=_T)


CASES = [
    {"name": "no_never", "edges": [["a", "b", 0.96]], "assertions": [], "cut_protect": 1.0},
    {"name": "satisfied", "edges": [["a", "b", 0.96]], "assertions": [("a", "x", "A1")], "cut_protect": 1.0},
    {"name": "simple_cut", "edges": [["a", "b", 0.96], ["b", "c", 0.90]], "assertions": [("a", "c", "A1")], "cut_protect": 1.0},
    {"name": "protected_escalate", "edges": [["a", "b", 0.96], ["b", "c", 0.97]], "assertions": [("a", "c", "A1")], "cut_protect": 0.95},
    {"name": "multi_iteration", "edges": [["a", "b", 0.9], ["b", "d", 0.8], ["a", "d", 0.7]], "assertions": [("a", "d", "A1")], "cut_protect": 1.0},
    {"name": "two_pairs", "edges": [["a", "b", 0.9], ["b", "c", 0.9], ["d", "e", 0.9], ["e", "f", 0.9]], "assertions": [("a", "c", "A1"), ("d", "f", "A2")], "cut_protect": 1.0},
]


def main() -> None:
    out = []
    for case in CASES:
        edges = [(a, b, p) for a, b, p in case["edges"]]
        assertions = [never(a, b, aid) for a, b, aid in case["assertions"]]
        result = never_cut_fixpoint(edges, assertions, nodes=(), cut_protect_probability=case["cut_protect"], max_iterations=50)
        out.append({
            "name": case["name"],
            "edges": case["edges"],
            "assertions": case["assertions"],
            "cut_protect": case["cut_protect"],
            "cuts": [[c.rec_a_key, c.rec_b_key, c.match_probability, c.assertion_id] for c in result.cuts],
            "escalations": [list(e) for e in result.escalations],
            "iterations": result.iterations,
        })
    (HERE / "never_cut_golden.json").write_text(
        json.dumps({"_comment": "never_cut_fixpoint golden from er.review.never_cut. Regen: uv run python rust/crates/er-reconcile/tests/gen_never_cut_golden.py",
                    "cases": out}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(out)} never-cut cases")


if __name__ == "__main__":
    main()
