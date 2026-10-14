"""Golden for CONTRADICTION-1, from the pure Python
er.review.assertions.check_contradiction_1 over crafted always/never sets.

    uv run python rust/crates/er-reconcile/tests/gen_contradiction_golden.py
"""
import json
from datetime import datetime
from pathlib import Path

from er.review.assertions import ALWAYS, NEVER, Assertion, check_contradiction_1

HERE = Path("rust/crates/er-reconcile/tests")
_T = datetime(2026, 1, 1)


def a(kind, x, y, aid, active=True):
    lo, hi = (x, y) if x < y else (y, x)
    return Assertion(assertion_id=aid, rec_a_key=lo, rec_b_key=hi, kind=kind, active=active,
                     created_by="x", created_at=_T)


# each case: list of (kind, x, y, aid[, active])
CASES = {
    "clean": [(ALWAYS, "a", "b", "L1"), (NEVER, "c", "d", "N1")],
    "direct": [(ALWAYS, "a", "b", "L1"), (NEVER, "a", "b", "N1")],
    "transitive": [(ALWAYS, "a", "b", "L1"), (ALWAYS, "b", "c", "L2"), (NEVER, "a", "c", "N1")],
    "never_outside_closure": [(ALWAYS, "a", "b", "L1"), (NEVER, "a", "x", "N1")],
    "retracted_never": [(ALWAYS, "a", "b", "L1"), (NEVER, "a", "b", "N1", False)],
    "two_contradictions": [(ALWAYS, "a", "b", "L1"), (ALWAYS, "c", "d", "L2"),
                           (NEVER, "a", "b", "N1"), (NEVER, "c", "d", "N2")],
}


def main() -> None:
    out = []
    for name, rows in CASES.items():
        assertions = [a(*r) for r in rows]
        found = check_contradiction_1(assertions)
        out.append({
            "name": name,
            "assertions": [[("always" if r[0] == ALWAYS else "never"), r[1], r[2], r[3],
                            (r[4] if len(r) > 4 else True)] for r in rows],
            "contradictions": [
                {"rec_a_key": c.rec_a_key, "rec_b_key": c.rec_b_key,
                 "never_assertion_id": c.never_assertion_id,
                 "always_assertion_ids": list(c.always_assertion_ids),
                 "component": sorted(c.component)}
                for c in found
            ],
        })
    (HERE / "contradiction_golden.json").write_text(
        json.dumps({"_comment": "check_contradiction_1 golden. Regen: uv run python rust/crates/er-reconcile/tests/gen_contradiction_golden.py",
                    "cases": out}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(out)} contradiction cases")


if __name__ == "__main__":
    main()
