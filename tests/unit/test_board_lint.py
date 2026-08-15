"""The board linter is wired into the unit layer, every defect class.

``scripts/lint_board.py`` is the S9.1 static-job gate that keeps the ticket board
machine-consistent and keeps S12 honest against S5 and S8.3. Every failing arm
below is built as a synthetic fixture -- a throwaway board in ``tmp_path``, or a
temporary copy of the design document -- never by editing the committed board or
DesignDoc.md, which the clean arms assert are and stay green.
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections.abc import Callable, Sequence
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
LINTER = "scripts/lint_board.py"
BOARD_DIR = "docs/implementation"
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"

MILESTONE_ROW = re.compile(r"^\|\s*\*\*(M\d+)")


def run_linter(*args: str) -> subprocess.CompletedProcess[str]:
    # sys.executable is the interpreter `python3` resolves to under `uv run`, and
    # the linter is pure stdlib, so this is the S9.1 command with a pinned python.
    return subprocess.run(
        [sys.executable, LINTER, *args],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )


def _flow(values: Sequence[str]) -> str:
    return "[" + ", ".join(f'"{v}"' for v in values) + "]"


def write_ticket(
    board_dir: Path,
    filename: str,
    ticket_id: str,
    *,
    depends_on: Sequence[str] = (),
    spec_refs: Sequence[str] = ("s5-0",),
    **overrides: str,
) -> Path:
    """One valid synthetic ticket; ``overrides`` inject raw frontmatter values.

    Overrides carry YAML text rather than Python values, so a test can write a
    field the writer would never emit -- an out-of-enum ``size``, an empty
    ``verify`` -- which is exactly what the linter has to catch.
    """
    fields: dict[str, str] = {
        "id": ticket_id,
        "title": '"synthetic fixture ticket"',
        "milestone": "M1",
        "status": "todo",
        "kind": "code",
        "size": "S",
        "gates": "fast",
        "depends_on": _flow(depends_on),
        "spec_refs": _flow(spec_refs),
        "attempts": "0",
        "verify": '"true"',
    }
    fields.update(overrides)
    frontmatter = "".join(f"{key}: {value}\n" for key, value in fields.items())
    body = (
        "## Acceptance criteria\n\n"
        "- [ ] AC1: the fixture exists.\n\n"
        "## Definition of Done\n\n"
        "- Acceptance criteria met\n"
    )
    path = board_dir / filename
    path.write_text(f"---\n{frontmatter}---\n{body}", encoding="utf-8")
    return path


def spec_copy(tmp_path: Path, name: str, rewrite: Callable[[str], str]) -> Path:
    """A copy of DesignDoc.md with ``rewrite`` applied to each S12 milestone row."""
    lines = DESIGN_DOC.read_text(encoding="utf-8").splitlines(keepends=True)
    out = [rewrite(line) if MILESTONE_ROW.match(line) else line for line in lines]
    path = tmp_path / name
    path.write_text("".join(out), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# clean arms
# ---------------------------------------------------------------------------


def test_committed_board_is_clean() -> None:
    result = run_linter(BOARD_DIR)
    assert result.returncode == 0, result.stderr


def test_default_path_is_docs_implementation() -> None:
    # S9.1 runs the linter with no argument, so the default has to be the board
    # directory and has to be resolved from the script rather than the cwd.
    default = run_linter()
    explicit = run_linter(BOARD_DIR)
    assert default.returncode == 0, default.stderr
    assert default.stdout == explicit.stdout


def test_clean_synthetic_board_passes(tmp_path: Path) -> None:
    # Non-vacuity for every failing arm below: the fixture builder produces a
    # board the linter accepts, so each defect a test injects is the only reason
    # that test's run is red.
    write_ticket(tmp_path, "ER-001-alpha.md", "ER-001")
    write_ticket(tmp_path, "ER-002-beta.md", "ER-002", depends_on=["ER-001"])
    result = run_linter(str(tmp_path))
    assert result.returncode == 0, result.stderr


# ---------------------------------------------------------------------------
# frontmatter, filename and id defects
# ---------------------------------------------------------------------------


def test_duplicate_id_fails(tmp_path: Path) -> None:
    write_ticket(tmp_path, "ER-001-first.md", "ER-001")
    write_ticket(tmp_path, "ER-001-second.md", "ER-001")
    result = run_linter(str(tmp_path))
    assert result.returncode == 1
    assert "duplicate id 'ER-001'" in result.stderr
    # Both filenames, because "there is a duplicate" is not actionable on a
    # hundred-ticket board; "these two files collide" is.
    assert "ER-001-first.md" in result.stderr
    assert "ER-001-second.md" in result.stderr


def test_filename_must_start_with_id(tmp_path: Path) -> None:
    write_ticket(tmp_path, "ER-002-mislabelled.md", "ER-003")
    result = run_linter(str(tmp_path))
    assert result.returncode == 1
    assert "ER-002-mislabelled.md" in result.stderr
    assert "ER-003" in result.stderr


def test_enum_and_required_field_violations_fail(tmp_path: Path) -> None:
    write_ticket(tmp_path, "ER-001-bad-status.md", "ER-001", status="nonsense")
    write_ticket(tmp_path, "ER-002-bad-size.md", "ER-002", size="XL")
    write_ticket(tmp_path, "ER-003-bad-kind.md", "ER-003", kind="prose")
    write_ticket(tmp_path, "ER-004-bad-gates.md", "ER-004", gates="medium")
    write_ticket(tmp_path, "ER-005-no-verify.md", "ER-005", verify='""')
    write_ticket(tmp_path, "ER-006-no-refs.md", "ER-006", spec_refs=())

    result = run_linter(str(tmp_path))
    assert result.returncode == 1
    for filename, field in [
        ("ER-001-bad-status.md", "status"),
        ("ER-002-bad-size.md", "size"),
        ("ER-003-bad-kind.md", "kind"),
        ("ER-004-bad-gates.md", "gates"),
        ("ER-005-no-verify.md", "verify"),
        ("ER-006-no-refs.md", "spec_refs"),
    ]:
        expected = f"{filename}: field '{field}'"
        assert expected in result.stderr, f"no defect naming {filename} and {field}"


def test_unresolved_spec_anchor_fails(tmp_path: Path) -> None:
    write_ticket(tmp_path, "ER-001-bad-ref.md", "ER-001", spec_refs=["s5-0", "s99-9"])
    result = run_linter(str(tmp_path))
    assert result.returncode == 1
    assert "ER-001-bad-ref.md" in result.stderr
    assert "s99-9" in result.stderr


def test_dependency_cycle_is_reported(tmp_path: Path) -> None:
    write_ticket(tmp_path, "ER-001-alpha.md", "ER-001", depends_on=["ER-002"])
    write_ticket(tmp_path, "ER-002-beta.md", "ER-002", depends_on=["ER-001"])
    write_ticket(tmp_path, "ER-003-dangling.md", "ER-003", depends_on=["ER-404"])

    result = run_linter(str(tmp_path))
    assert result.returncode == 1
    # In order, and rotated to the smallest id, so the same loop reads the same
    # way whichever entry point the traversal reached it from.
    assert "dependency cycle: ER-001 -> ER-002 -> ER-001" in result.stderr
    assert "ER-003: depends_on 'ER-404'" in result.stderr


# ---------------------------------------------------------------------------
# S12 parity (S9.1)
# ---------------------------------------------------------------------------


def test_s8_3_id_unassigned_in_s12_fails(tmp_path: Path) -> None:
    unassigned = spec_copy(tmp_path, "no-blk.md", lambda line: line.replace("T-BLK-1", ""))
    result = run_linter(BOARD_DIR, "--spec", str(unassigned))
    assert result.returncode == 1
    assert "T-BLK-1" in result.stderr

    # The other direction: a milestone gating a test that S8.3 never declares.
    unknown = spec_copy(
        tmp_path, "nope.md", lambda line: line.replace("T-BLK-1,", "T-BLK-1, T-NOPE-9,")
    )
    result = run_linter(BOARD_DIR, "--spec", str(unknown))
    assert result.returncode == 1
    assert "T-NOPE-9" in result.stderr

    # The relation half of the same parity rule, which is otherwise only ever
    # exercised by the green arm and could rot unnoticed.
    orphan = spec_copy(tmp_path, "no-display.md", lambda line: line.replace("`golden_display`", ""))
    result = run_linter(BOARD_DIR, "--spec", str(orphan))
    assert result.returncode == 1
    assert "golden_display" in result.stderr

    # An id named by two milestones is not a defect: M1 says "T-KEY-1a and not
    # T-KEY-1b" about an arm M2 owns, so four ids legitimately appear twice.
    assert run_linter(BOARD_DIR, "--spec", str(DESIGN_DOC)).returncode == 0


# ---------------------------------------------------------------------------
# exit codes
# ---------------------------------------------------------------------------


def test_unreadable_directory_is_a_usage_error(tmp_path: Path) -> None:
    result = run_linter(str(tmp_path / "does-not-exist"))
    assert result.returncode == 2
    assert "does-not-exist" in result.stderr
