"""Golden oracle for label propagation (connected components, min-record_key
labels), from the real Python er.entities.cluster.label_propagate.

    uv run python rust/crates/er-reconcile/tests/gen_cluster_golden.py
"""
import json
from pathlib import Path

import duckdb
from er.entities.cluster import label_propagate

HERE = Path("rust/crates/er-reconcile/tests")
CASES = [
    {"name": "chain", "nodes": ["a", "b", "c"], "edges": [["a", "b"], ["b", "c"]]},
    {"name": "reversed", "nodes": ["a", "b", "c"], "edges": [["c", "b"], ["b", "a"]]},
    {"name": "two_components", "nodes": ["a", "b", "x", "y", "z"], "edges": [["a", "b"], ["x", "y"], ["y", "z"]]},
    {"name": "singletons", "nodes": ["m", "n", "o"], "edges": []},
    {"name": "star", "nodes": ["k", "a", "b", "c"], "edges": [["k", "a"], ["k", "b"], ["k", "c"]]},
    {"name": "cycle", "nodes": ["p", "q", "r"], "edges": [["p", "q"], ["q", "r"], ["r", "p"]]},
    {"name": "isolated_plus_edge", "nodes": ["z1", "a", "b"], "edges": [["a", "b"]]},
    {"name": "min_label", "nodes": ["banana", "apple", "cherry"], "edges": [["banana", "cherry"], ["cherry", "apple"]]},
    {"name": "unsorted_order", "nodes": ["c3", "a1", "b2"], "edges": [["b2", "a1"], ["c3", "b2"]]},
]


def main() -> None:
    out = []
    for case in CASES:
        conn = duckdb.connect()
        try:
            res = label_propagate(conn, case["nodes"], [tuple(e) for e in case["edges"]], max_iterations=50)
        finally:
            conn.close()
        out.append({**case, "labels": dict(res.labels)})
    (HERE / "cluster_golden.json").write_text(
        json.dumps({"_comment": "label_propagate golden from er.entities.cluster. Regen: uv run python rust/crates/er-reconcile/tests/gen_cluster_golden.py",
                    "cases": out}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(out)} label-prop cases")


if __name__ == "__main__":
    main()
