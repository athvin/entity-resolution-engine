"""Golden oracle for the INV-PERM reconcile_plan fixpoint, from the pure Python
er.entities.reconcile.reconcile_plan over crafted partition pairs.

    uv run python rust/crates/er-reconcile/tests/gen_reconcile_golden.py
"""
import json
from pathlib import Path

from er.entities.ids import CountingIdFactory
from er.entities.reconcile import reconcile_plan

HERE = Path("rust/crates/er-reconcile/tests")

CASES = [
    {"name": "all_create", "p_old": {}, "p_new": [["a", "b"], ["c"]]},
    {"name": "noop", "p_old": {"E1": ["a", "b"]}, "p_new": [["a", "b"]]},
    {"name": "merge", "p_old": {"E1": ["a", "b"], "E2": ["c", "d"]}, "p_new": [["a", "b", "c", "d"]]},
    {"name": "split", "p_old": {"E1": ["a", "b", "c", "d"]}, "p_new": [["a", "b"], ["c", "d"]]},
    {"name": "tie_min_key", "p_old": {"E1": ["a"], "E2": ["b"]}, "p_new": [["a", "b"]]},
    {"name": "retire_and_orphan", "p_old": {"E1": ["a"], "E2": ["b"]}, "p_new": [["b"]]},
    {"name": "absorb_smaller", "p_old": {"E1": ["a", "b"], "E2": ["c"]}, "p_new": [["a", "b", "c"]]},
]


def main() -> None:
    out = []
    for case in CASES:
        plan = reconcile_plan(case["p_old"], [list(g) for g in case["p_new"]], CountingIdFactory())
        out.append({
            "name": case["name"],
            "p_old": case["p_old"],
            "p_new": case["p_new"],
            "assignments": [[a.record_key, a.entity_id] for a in plan.assignments],
            "transitions": [
                {"entity_id": t.entity_id, "status": t.status,
                 "merged_into": t.merged_into, "is_new": t.is_new}
                for t in plan.transitions
            ],
            "events": [
                {"entity_id": e.entity_id, "event_type": e.event_type,
                 "details": dict(e.details), "details_hash": e.details_hash}
                for e in plan.events
            ],
            "minted": list(plan.minted),
        })
    (HERE / "reconcile_golden.json").write_text(
        json.dumps({"_comment": "reconcile_plan golden from er.entities.reconcile (CountingIdFactory start=0). Regen: uv run python rust/crates/er-reconcile/tests/gen_reconcile_golden.py",
                    "cases": out}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(out)} reconcile cases")


if __name__ == "__main__":
    main()
