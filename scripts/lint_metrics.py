"""Fail on a second precision/recall implementation (DesignDoc.md S8.5, S9.1).

S8.5 makes `er.eval.pairwise_metrics` the ONLY quality-metric implementation in the
repository, and makes this lint the enforcement: it runs in the static job (S9.1)
and exits non-zero when any Python file outside `src/er/eval/metrics.py` looks like
it computes precision or recall itself.

Two detections, both over the AST rather than the text, so a comment or a docstring
naming "precision" cannot fail the build:

* **A definition by name.** A function or method named `pairwise_metrics`,
  `precision`, `recall` or `f1` (with or without a `pairwise_` prefix) outside the
  blessed module is a second implementation by declaration, whatever its body does.
* **The formula itself.** A division whose numerator names `tp` (or
  `true_positives`) and whose denominator mentions `fp`/`fn` (or their long forms)
  is the precision/recall arithmetic in any spelling — including inlined into a
  test helper that never earns a suspicious name.

``--root`` exists for the lint's own falsifiability test
(`tests/integration/test_match_quality.py`): pointing it at a temp tree carrying a
planted second implementation must fail, and pointing it at this repository must
pass — both arms, or the "exactly one implementation" rule is a wish rather than a
gate (S8.5).
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path
from typing import Final

#: The one module allowed to implement the metrics, repo-relative (S8.5).
BLESSED: Final = Path("src") / "er" / "eval" / "metrics.py"

#: Function names that ARE a metric implementation wherever they appear.
FORBIDDEN_NAMES: Final = frozenset(
    {"pairwise_metrics", "precision", "recall", "f1", "pairwise_precision", "pairwise_recall"}
)

#: Count spellings on either side of the formula's division.
_TRUE_POSITIVE_NAMES: Final = frozenset({"tp", "true_positives"})
_ERROR_COUNT_NAMES: Final = frozenset({"fp", "fn", "false_positives", "false_negatives"})

#: Directories that are never source: environments, caches, checkouts of vendored code.
SKIPPED_DIRS: Final = frozenset({".git", ".venv", "__pycache__", ".loop", "dbt_packages", "target"})


def _names_in(node: ast.AST) -> set[str]:
    """Every `Name` identifier under ``node``."""
    return {child.id for child in ast.walk(node) if isinstance(child, ast.Name)}


def _is_metric_division(node: ast.BinOp) -> bool:
    """Whether ``node`` divides a true-positive count by a sum involving fp/fn."""
    if not isinstance(node.op, ast.Div):
        return False
    numerator = _names_in(node.left)
    denominator = _names_in(node.right)
    return bool(numerator & _TRUE_POSITIVE_NAMES) and bool(denominator & _ERROR_COUNT_NAMES)


def offences_in(path: Path, tree: ast.AST) -> list[str]:
    """Every second-implementation sighting in one parsed file."""
    found: list[str] = []
    for node in ast.walk(tree):
        if (
            isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            and node.name in FORBIDDEN_NAMES
        ):
            found.append(f"{path}:{node.lineno}: def {node.name} is a second implementation")
        if isinstance(node, ast.BinOp) and _is_metric_division(node):
            found.append(
                f"{path}:{node.lineno}: a tp/(tp+fp|fn) division is the S8.5 formula; "
                "call er.eval.pairwise_metrics instead"
            )
    return found


def lint(root: Path) -> list[str]:
    """Every offence under ``root``, `BLESSED` excluded."""
    offences: list[str] = []
    for path in sorted(root.rglob("*.py")):
        relative = path.relative_to(root)
        if relative == BLESSED or SKIPPED_DIRS & set(relative.parts):
            continue
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        except SyntaxError as exc:
            offences.append(f"{relative}: unparseable ({exc.msg} at line {exc.lineno})")
            continue
        offences.extend(offences_in(relative, tree))
    return offences


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python scripts/lint_metrics.py",
        description="Fail on a precision/recall implementation outside src/er/eval/metrics.py.",
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="tree to scan; the repository root when omitted",
    )
    arguments = parser.parse_args(argv)
    offences = lint(arguments.root)
    for offence in offences:
        print(offence, file=sys.stderr)
    if offences:
        print(
            f"lint_metrics: {len(offences)} second-implementation sighting(s); "
            f"S8.5 permits exactly one, in {BLESSED}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
