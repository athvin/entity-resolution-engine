"""`golden_display` is presentation-only, and this is what makes that falsifiable (S4.6).

S4.6 states the property as a prohibition: `golden_display` "is presentation casing only
and is never read by the matching layer, so matching-layer data is never re-cased". A
prohibition is the one kind of claim a model cannot demonstrate about itself — the
relation looks identical whether or not something reads it — so it needs a guard, and
the guard needs to be shown to bite.

**A scan that passes today proves nothing about tomorrow.** The failure this exists to
catch is a reference somebody adds later, so `test_scan_fails_on_an_introduced_reference`
runs the same scanner over a synthetic tree carrying the literal and asserts it reports
it. Without that arm, a scanner with a broken glob would pass forever and nobody would
know.

**Two arms, because they see different things.** The source scan catches a raw SQL string
or a Python constant; the dbt-manifest arm catches a `ref('golden_display')`, which the
scan would see only as a name inside a file it may not be looking at. A node with zero
children is the manifest's way of saying nothing downstream reads it.
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path
from typing import Final

import pytest

from er.lake.columns import GOLDEN_MART_RELATIONS

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: The relation whose isolation is the subject.
DISPLAY: Final = "golden_display"

#: The trees the matching layer lives in, plus the two dbt layers that run BEFORE
#: assembly. A reference from any of them is the defect: `src/er/matching` and
#: `src/er/entities` are the scoring and clustering paths, `src/er/ingest` is upstream of
#: both, and `staging`/`intermediate` are the dbt models every stage builds.
GUARDED_TREES: Final[tuple[str, ...]] = (
    "src/er/matching",
    "src/er/entities",
    "src/er/ingest",
    "dbt/models/staging",
    "dbt/models/intermediate",
)

#: What a reference looks like in either language. Not a word-boundary regex: a
#: `ref('golden_display')`, a qualified `lake.main.golden_display` and a bare constant
#: all contain the literal, and the guard should be blunt rather than clever.
SUFFIXES: Final[tuple[str, ...]] = (".py", ".sql", ".yml", ".yaml")

MANIFEST: Final = REPO_ROOT / "dbt" / "target" / "manifest.json"


def scan(roots: Iterable[Path]) -> list[str]:
    """Every file under ``roots`` whose text contains the relation name.

    Returned as paths rather than a bool so a failure names the offender, and taken as an
    argument rather than hard-coded so the same scanner can be pointed at a synthetic
    tree — which is what makes the guard checkable.
    """
    found: list[str] = []
    for root in roots:
        if not root.is_dir():
            continue
        for path in sorted(root.rglob("*")):
            if path.suffix not in SUFFIXES or "__pycache__" in path.parts:
                continue
            if DISPLAY in path.read_text(encoding="utf-8"):
                found.append(
                    str(path.relative_to(REPO_ROOT) if root.is_relative_to(REPO_ROOT) else path)
                )
    return found


def test_no_matching_module_references_golden_display() -> None:
    """AC6: nothing in the matching, entity, ingest or pre-assembly dbt trees reads it."""
    roots = [REPO_ROOT / tree for tree in GUARDED_TREES]
    present = [root for root in roots if root.is_dir()]
    assert len(present) == len(roots), (
        f"guarded trees are missing: {[str(r) for r in roots if not r.is_dir()]}. A scan "
        "over directories that do not exist passes vacuously."
    )

    offenders = scan(roots)
    assert not offenders, (
        f"{offenders} reference {DISPLAY!r}. S4.6 makes it presentation casing only: a "
        "matching-layer read means matching-layer data has been re-cased, and the "
        "symptom is a score that changes because somebody improved a display string."
    )


def test_scan_fails_on_an_introduced_reference(tmp_path: Path) -> None:
    """AC6: the guard bites. Proven against a synthetic tree, never the real one.

    Writing the offending file into `src/` and deleting it afterwards would leave a
    matching module referencing `golden_display` if this test failed in between — which
    is the exact state the guard exists to prevent.
    """
    fake = tmp_path / "src" / "er" / "matching"
    fake.mkdir(parents=True)
    (fake / "clean.py").write_text("GOLDEN = 'golden_records'\n", encoding="utf-8")
    assert scan([fake]) == [], "the scanner reports a file that does not reference it"

    (fake / "leaky.py").write_text(
        f"QUERY = 'SELECT * FROM lake.main.{DISPLAY}'\n", encoding="utf-8"
    )
    offenders = scan([fake])
    assert len(offenders) == 1 and "leaky.py" in offenders[0], (
        f"the scanner did not report an introduced reference: {offenders}. Every other "
        "assertion in this file is worthless if this one does not hold."
    )


def test_golden_display_has_no_downstream_dbt_children() -> None:
    """AC7: the compiled manifest gives the node zero children.

    Catches what the source scan cannot: a `ref('golden_display')` from a model the scan
    is not pointed at. The manifest is generated here rather than read from whatever a
    previous command left behind, so the answer describes the tree as it stands.
    """
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            "dbt.cli.main",
            "parse",
            "--project-dir",
            str(REPO_ROOT / "dbt"),
            "--profiles-dir",
            str(REPO_ROOT / "dbt" / "profiles"),
            "--target",
            "mem",
        ],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        check=False,
    )
    if completed.returncode != 0 or not MANIFEST.is_file():
        pytest.fail(
            f"`dbt parse` did not produce a manifest (exit {completed.returncode}); the "
            f"child check cannot run.\n{completed.stdout[-2000:]}\n{completed.stderr[-2000:]}"
        )

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    node = next(
        (key for key in manifest["nodes"] if key.endswith(f".{DISPLAY}")),
        None,
    )
    assert node is not None, (
        f"{DISPLAY} is not a node in the compiled manifest; it must exist for its "
        "isolation to mean anything"
    )

    children = manifest.get("child_map", {}).get(node, [])
    # Its own data tests are children and are not readers: a `unique` on entity_id is
    # part of the relation's contract, not something consuming it downstream.
    models = [child for child in children if child.startswith("model.")]
    assert models == [], (
        f"{DISPLAY} has downstream model(s) {models}. Nothing may build on it: it is the "
        "end of the graph by design (S4.6)."
    )


def test_golden_mart_relations_names_all_three() -> None:
    """AC8: the reap list ER-092 consumes names every relation that must be reaped."""
    assert GOLDEN_MART_RELATIONS == ("golden_records", "golden_lineage", "golden_display")
    assert DISPLAY in GOLDEN_MART_RELATIONS, (
        "golden_display is missing from the reap list. dbt's delete+insert only removes "
        "keys present in the built batch, so a retired entity's display row would "
        "survive forever -- and it is the row a consumer is most likely to read (S4.6)."
    )
    assert len(set(GOLDEN_MART_RELATIONS)) == len(GOLDEN_MART_RELATIONS)
