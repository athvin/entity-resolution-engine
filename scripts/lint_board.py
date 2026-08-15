#!/usr/bin/env python3
"""Lint the implementation board in ``docs/implementation``.

This is the board gate of the S9.1 static job. Its S9.1 duty is the S12 parity
check: fail if any relation in S5 or any test id in S8.3 is unassigned to a
milestone in S12, or if a milestone cites an id that does not exist. That is
what keeps an S8.3 arm from being split off -- T-KEY-1a / T-KEY-1b,
T-IDEM-1a / T-IDEM-1, T-DEL-1a / T-DEL-1 -- and then never gated by anything.

It also keeps the board itself machine-consistent: frontmatter schema,
``id`` <-> filename agreement, duplicate ids, a non-empty ``verify``,
``spec_refs`` that resolve to real ``<a id=...>`` anchors in DesignDoc.md, and a
``depends_on`` graph that is closed and acyclic.

Every enum comes from ``scripts/board.py`` by reference, never by copy.
board.py is the runtime writer that enforces these values when a ticket
transitions; a second declaration here would drift from it, and the writer
would win. The module imports nothing outside the standard library and
board.py.

Usage:
    python3 scripts/lint_board.py                       # lints docs/implementation
    python3 scripts/lint_board.py docs/implementation
    python3 scripts/lint_board.py <dir> --spec <designdoc>

Exit codes: 0 clean; 1 defects, one DEFECT line each on stderr; 2 usage or an
unreadable board directory.
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import sys
from pathlib import Path
from types import ModuleType
from typing import NamedTuple

EXIT_OK = 0
EXIT_DEFECTS = 1
EXIT_USAGE = 2

REPO_ROOT = Path(__file__).resolve().parent.parent
BOARD_DIR = REPO_ROOT / "docs" / "implementation"
SPEC_PATH = REPO_ROOT / "DesignDoc.md"


def _load_board_module() -> ModuleType:
    """Import ``scripts/board.py`` by path.

    By path rather than by name: ``scripts/`` is not a package and this linter
    is invoked as a script from the repository root, so a plain ``import board``
    would depend on the caller's ``sys.path``. Loading it from a path next to
    this file works from any working directory without mutating ``sys.path``.
    """
    path = Path(__file__).resolve().parent / "board.py"
    spec = importlib.util.spec_from_file_location("er_board_writer", path)
    if spec is None or spec.loader is None:  # pathological; keeps mypy honest
        raise RuntimeError(f"cannot load the board writer at {path}")
    module = importlib.util.module_from_spec(spec)
    # Registered before execution: board.py's ``@dataclass`` decorators resolve
    # their own module out of ``sys.modules`` while the class body is being
    # processed, and raise if it is not there yet.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


board = _load_board_module()


class FieldRule(NamedTuple):
    """One frontmatter field's contract. ``enum=()`` means "any value"."""

    required: bool
    enum: tuple[str, ...]


# The enum-valued fields, aliased from board.py rather than restated.
ENUM_FIELDS: dict[str, tuple[str, ...]] = {
    "status": board.STATUSES,
    "size": board.SIZES,
    "kind": board.KINDS,
    "gates": board.GATES,
}

# Must be present and non-empty. board.py's own required set plus ``spec_refs``,
# which board.py validates separately rather than listing there: a ticket that
# cites no section of the spec is exactly what this linter exists to catch.
REQUIRED_FIELDS: tuple[str, ...] = tuple(sorted(board.REQUIRED_FIELDS | {"spec_refs"}))

# ``kind`` and ``gates`` are checked against their enum but are not required:
# board.py defaults them ("code", "fast") when a ticket omits them, so demanding
# them here would fail tickets the runtime writer accepts.
FRONTMATTER_SCHEMA: dict[str, FieldRule] = {
    name: FieldRule(required=name in REQUIRED_FIELDS, enum=ENUM_FIELDS.get(name, ()))
    for name in sorted({*REQUIRED_FIELDS, *ENUM_FIELDS})
}

ANCHOR_RE = re.compile(r'<a id="([^"]+)"></a>')
# Test ids carry an optional lowercase arm suffix -- T-KEY-1a is a different
# test from T-KEY-1b, and an extractor that stopped at the digit would report
# both arms as assigned the moment either one was.
TEST_ID_RE = re.compile(r"\bT-[A-Z]+(?:-[A-Z0-9]+)+[a-z]?\b")
# Relation names are unqualified snake_case identifiers in backticks. The dot in
# `ddl.py` is what keeps M1's prose out of the relation set.
RELATION_RE = re.compile(r"`([a-z][a-z0-9_]*)`")
MILESTONE_ROW_RE = re.compile(r"^\|\s*\*\*(M\d+)")
S83_ROW_RE = re.compile(r"^\|\s*(T-[A-Z]+(?:-[A-Z0-9]+)+[a-z]?)\s*\|")

MILESTONE_COLUMNS = ("Milestone", "Contents", "Relations first written", "Exit criteria")
RELATIONS_COLUMN = MILESTONE_COLUMNS.index("Relations first written")


def _section(text: str, anchor: str) -> str:
    """The slice of ``text`` from ``anchor`` up to the next anchor."""
    start = text.find(f'<a id="{anchor}"></a>')
    if start == -1:
        return ""
    following = text.find('<a id="', start + 1)
    return text[start:] if following == -1 else text[start:following]


def _cells(row: str) -> list[str]:
    return [cell.strip() for cell in row.strip().strip("|").split("|")]


def _is_empty(value: object) -> bool:
    if value is None:
        return True
    if isinstance(value, str):
        return not value.strip()
    if isinstance(value, list):
        return not value
    return False


def spec_anchors(spec_text: str) -> set[str]:
    return set(ANCHOR_RE.findall(spec_text))


def s8_3_test_ids(spec_text: str) -> list[str]:
    """Every test id in the S8.3 scenario table, in document order."""
    ids: list[str] = []
    for line in _section(spec_text, "s8-3").splitlines():
        match = S83_ROW_RE.match(line)
        if match:
            ids.append(match.group(1))
    return ids


def s5_relations(spec_text: str) -> list[str]:
    """Every relation named in the S5.0 ownership table, in document order.

    S5.0 is the normative ownership table, so it is the one listing that names
    all three ``stg_*`` models individually -- the S5 DDL block spells them as
    the placeholder ``stg_<source>``.
    """
    relations: list[str] = []
    for line in _section(spec_text, "s5-0").splitlines():
        if not line.startswith("|"):
            continue
        cells = _cells(line)
        # The Owner column is the discriminator: it is exactly the two owners of
        # S5.0's ownership rule, so header and separator rows drop out.
        if len(cells) < 2 or cells[1] not in ("ddl.py", "dbt"):
            continue
        relations.extend(RELATION_RE.findall(cells[0]))
    return relations


def _lint_tickets(board_dir: Path, spec_text: str) -> list[str]:
    defects: list[str] = []
    paths = sorted(board_dir.glob("ER-*.md"))
    if not paths:
        return [f"{board_dir}: no ER-*.md ticket files found"]

    tickets: dict[Path, object] = {}
    by_id: dict[str, list[str]] = {}
    depends: dict[str, list[str]] = {}

    for path in paths:
        name = path.name
        if not board.TICKET_RE.match(name):
            defects.append(f"{name}: filename is not of the form ER-<NNN>-<slug>.md")
        try:
            ticket = board.load_ticket(path, strict=False)
        except board.BoardError as exc:
            defects.append(f"{name}: unparseable frontmatter: {exc}")
            continue
        tickets[path] = ticket
        defects.extend(ticket.defects)

        fields = ticket.fields
        for field_name, rule in FRONTMATTER_SCHEMA.items():
            value = fields.get(field_name)
            if _is_empty(value):
                if rule.required:
                    defects.append(f"{name}: field '{field_name}' is missing or empty")
                continue
            if rule.enum and value not in rule.enum:
                defects.append(
                    f"{name}: field '{field_name}' is '{value}', which is not one of {rule.enum}"
                )

        ticket_id = ticket.id
        if ticket_id and not board.ID_RE.match(ticket_id):
            defects.append(f"{name}: field 'id' is malformed: '{ticket_id}'")
        elif ticket_id and not name.startswith(ticket_id + "-"):
            defects.append(f"{name}: filename does not start with its id '{ticket_id}'")
        if ticket_id:
            by_id.setdefault(ticket_id, []).append(name)
            depends[ticket_id] = list(ticket.depends_on)

    for ticket_id, names in sorted(by_id.items()):
        if len(names) > 1:
            defects.append(f"duplicate id '{ticket_id}' in " + " and ".join(sorted(names)))

    anchors = spec_anchors(spec_text)
    for path, ticket in sorted(tickets.items()):
        for ref in ticket.fields.get("spec_refs", []):
            if ref.startswith("gap:"):
                continue
            anchor = ref.rsplit("#", 1)[-1]
            if anchor not in anchors:
                defects.append(
                    f"{path.name}: spec_refs '{ref}' resolves to no "
                    f'<a id="{anchor}"></a> anchor in the spec'
                )

    defects.extend(_lint_dependencies(by_id, depends))
    return defects


def _lint_dependencies(by_id: dict[str, list[str]], depends: dict[str, list[str]]) -> list[str]:
    defects: list[str] = []
    for ticket_id in sorted(depends):
        for dep in depends[ticket_id]:
            if dep not in by_id:
                defects.append(
                    f"{ticket_id}: depends_on '{dep}', which is not a ticket on the board"
                )

    # Iterative DFS colouring, so a deep dependency chain cannot exhaust the
    # recursion limit. Each cycle is reported once, rotated to start at its
    # smallest id, so the same loop found from two entry points reads as one
    # defect rather than two.
    colour: dict[str, int] = {}
    reported: set[tuple[str, ...]] = set()

    def visit(start: str) -> None:
        stack: list[tuple[str, list[str]]] = [(start, list(depends.get(start, [])))]
        colour[start] = 1
        path = [start]
        while stack:
            node, pending = stack[-1]
            if not pending:
                colour[node] = 2
                stack.pop()
                path.pop()
                continue
            dep = pending.pop(0)
            if dep not in by_id:
                continue
            if colour.get(dep, 0) == 1:
                cycle = path[path.index(dep) :]
                pivot = cycle.index(min(cycle))
                rotated = tuple(cycle[pivot:] + cycle[:pivot])
                if rotated not in reported:
                    reported.add(rotated)
                    defects.append("dependency cycle: " + " -> ".join((*rotated, rotated[0])))
                continue
            if colour.get(dep, 0) == 0:
                colour[dep] = 1
                path.append(dep)
                stack.append((dep, list(depends.get(dep, []))))

    for ticket_id in sorted(depends):
        if colour.get(ticket_id, 0) == 0:
            visit(ticket_id)
    return defects


def _lint_s12_parity(spec_text: str) -> list[str]:
    """S9.1: every S5 relation and every S8.3 id is assigned to a milestone.

    Assignment is "at least one", never "exactly one". A milestone cell may name
    an arm another milestone owns -- M1's exit criteria say "T-KEY-1a and not
    T-KEY-1b" -- so four ids legitimately appear twice. The invariant that
    matters is that no arm is forgotten.
    """
    defects: list[str] = []
    section = _section(spec_text, "s12")
    rows = [line for line in section.splitlines() if MILESTONE_ROW_RE.match(line)]

    # Non-vacuity. Each of these three extractions can only report "nothing is
    # unassigned" when it finds nothing at all, so an empty result is a defect in
    # its own right rather than a pass.
    if not rows:
        defects.append("S12: no milestone rows found; the parity check would be vacuous")
    declared_ids = s8_3_test_ids(spec_text)
    if not declared_ids:
        defects.append("S8.3: no test ids found; the parity check would be vacuous")
    declared_relations = s5_relations(spec_text)
    if not declared_relations:
        defects.append("S5.0: no relations found; the parity check would be vacuous")
    if defects:
        return defects

    cited_ids: dict[str, str] = {}
    cited_relations: dict[str, str] = {}
    for row in rows:
        milestone_match = MILESTONE_ROW_RE.match(row)
        assert milestone_match is not None  # the row list is built from this match
        milestone = milestone_match.group(1)
        cells = _cells(row)
        if len(cells) != len(MILESTONE_COLUMNS):
            defects.append(
                f"S12: milestone {milestone} has {len(cells)} cells, expected "
                f"{len(MILESTONE_COLUMNS)} ({' | '.join(MILESTONE_COLUMNS)}); "
                "an unescaped '|' inside a cell would silently mis-column the parity check"
            )
            continue
        for test_id in TEST_ID_RE.findall(row):
            cited_ids.setdefault(test_id, milestone)
        for relation in RELATION_RE.findall(cells[RELATIONS_COLUMN]):
            cited_relations.setdefault(relation, milestone)
    if defects:
        return defects

    for test_id in declared_ids:
        if test_id not in cited_ids:
            defects.append(
                f"S12: test id '{test_id}' from S8.3 is assigned to no milestone; "
                "an arm that no milestone gates is an arm nothing runs"
            )
    for test_id, milestone in sorted(cited_ids.items()):
        if test_id not in declared_ids:
            defects.append(
                f"S12: milestone {milestone} cites test id '{test_id}', which S8.3 does not declare"
            )
    for relation in declared_relations:
        if relation not in cited_relations:
            defects.append(f"S12: relation '{relation}' from S5.0 is written first by no milestone")
    for relation, milestone in sorted(cited_relations.items()):
        if relation not in declared_relations:
            defects.append(
                f"S12: milestone {milestone} names relation '{relation}', which S5 does not declare"
            )
    return defects


def lint_board(board_dir: Path, spec_path: Path) -> list[str]:
    """Every defect in the board and in its S12 parity with the spec."""
    spec_text = spec_path.read_text(encoding="utf-8")
    return _lint_tickets(board_dir, spec_text) + _lint_s12_parity(spec_text)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="lint_board.py", description=__doc__)
    parser.add_argument(
        "board_dir",
        nargs="?",
        default=str(BOARD_DIR),
        help="directory of ER-<NNN>-<slug>.md tickets (default: docs/implementation)",
    )
    parser.add_argument(
        "--spec",
        default=str(SPEC_PATH),
        help="the design document the S12 parity and spec_refs resolve against",
    )
    args = parser.parse_args(argv)

    board_dir = Path(args.board_dir)
    spec_path = Path(args.spec)
    if not board_dir.is_dir():
        print(f"lint_board: not a directory: {board_dir}", file=sys.stderr)
        return EXIT_USAGE
    if not spec_path.is_file():
        print(f"lint_board: no such spec: {spec_path}", file=sys.stderr)
        return EXIT_USAGE

    defects = lint_board(board_dir, spec_path)
    if defects:
        for defect in defects:
            print(f"DEFECT {defect}", file=sys.stderr)
        print(f"\n{len(defects)} defect(s) in {board_dir}", file=sys.stderr)
        return EXIT_DEFECTS

    ticket_count = len(sorted(board_dir.glob("ER-*.md")))
    print(f"OK {ticket_count} tickets; S12 covers every S8.3 id and every S5 relation")
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
