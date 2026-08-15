#!/usr/bin/env python3
"""Lint the S8.2.1 fixture format over one or more scenario directories.

A mis-sorted expected file, a header that drifted from S8.2.1's literal, a
pasted ULID where a symbolic `entity_label` belongs or a `VOLATILE_COLUMNS`
member committed as a column are all authoring mistakes. Left to the scenario
tests they surface hours later as an integration failure in a test that is
expensive to reproduce and that points at the pipeline rather than at the
fixture. This script turns each of them into a lint failure naming the file, the
line and the rule.

Usage:
  python3 scripts/validate_fixtures.py                    # every scenario in fixtures/static/
  python3 scripts/validate_fixtures.py PATH [PATH ...]    # a scenario dir, or a dir of them
  python3 scripts/validate_fixtures.py --list-rules       # print the rule vocabulary

Exit codes: 0 clean; 1 at least one violation; 2 usage.

Every rule has a committed negative self-test fixture under
`tests/fixtures/scenarios/`, and `tests/unit/fixtures/test_fixture_format.py`
parametrises over `--list-rules`, so a rule added here without a fixture that
fails for exactly that reason fails the suite. A linter with no failing arm
proves nothing.

Like `scripts/board.py` and `scripts/lint_spec.py`, this is a repo script run by
a bare interpreter and imports the standard library only -- plus
`tests/helpers/scenario.py`, which is the single definition site for the phase
vocabulary and the header literals, and `src/er/lake/columns.py`, which is the
single definition site for the volatile-column set (S5.0).
"""

from __future__ import annotations

import argparse
import csv
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

REPO_ROOT = Path(__file__).resolve().parents[1]


def _import(module: str) -> ModuleType:
    """Import a repo module after putting `src/` and `tests/` on `sys.path`.

    Bare `python3 scripts/validate_fixtures.py` starts with only `scripts/` on
    the path. The bootstrap has to run before the import, and a bare `sys.path`
    statement followed by an import is an E402 this ticket may not suppress, so
    the two happen together inside a function called once below.
    """
    for directory in (REPO_ROOT / "src", REPO_ROOT / "tests"):
        entry = str(directory)
        if entry not in sys.path:
            sys.path.insert(0, entry)
    return __import__(module, fromlist=["_"])


scenario = _import("helpers.scenario")
expected = _import("helpers.expected")
columns = _import("er.lake.columns")

#: The rule vocabulary. Each name is one class of authoring mistake and each has
#: exactly one committed negative self-test fixture.
RULES: tuple[str, ...] = (
    "manifest",
    "phase-dir",
    "expected-phase",
    "unknown-file",
    "header",
    "volatile-column",
    "entity-label",
    "null-token",
    "sort-order",
    "assertion-phase",
)

#: Spellings of NULL that are not S8.2.1's `\N`. Each is what some exporter
#: writes -- a database `NULL`, pandas' `NaN`, Python's `None`, a lowercased
#: token -- and none is a plausible value in any column of the eight headers, so
#: finding one means a null was written the wrong way rather than that a record
#: really holds that string. An empty field is NOT in this set: S8.2.1 makes the
#: empty string a distinct value from NULL, so flagging it would be wrong.
ALTERNATE_NULLS: frozenset[str] = frozenset({"NULL", "Null", "null", "None", "NaN", "nan", "\\n"})


@dataclass(frozen=True)
class Violation:
    """One rule broken, at one place."""

    path: Path
    line: int
    rule: str
    message: str

    def render(self, root: Path) -> str:
        try:
            shown: Path | str = self.path.relative_to(root)
        except ValueError:
            shown = self.path
        return f"{shown}:{self.line}: {self.rule}: {self.message}"


def validate_scenario(directory: Path) -> list[Violation]:
    """Every way ``directory`` departs from the S8.2.1 format."""
    try:
        loaded = scenario.load_scenario_dir(directory)
    except scenario.ScenarioError as exc:
        # The scenario cannot be read at all, so no later rule has a subject.
        return [Violation(exc.path, exc.line, exc.rule, exc.detail)]

    violations: list[Violation] = []
    violations += _check_root_entries(directory, loaded)
    violations += _check_declared_phases(directory, loaded)
    violations += _check_expected_tree(directory, loaded)
    violations += _check_aux_files(loaded)
    for phase in loaded.phases:
        for relation, path in sorted(loaded.expected[phase].items()):
            if path is not None:
                violations += _check_csv(path, scenario.expected_header_key(relation))
    return violations


def _check_root_entries(directory: Path, loaded: scenario.Scenario) -> list[Violation]:
    """The scenario root holds the manifest, the phase dirs, `expected/` and the aux files."""
    declared = set(loaded.phases)
    aux_declared = set(loaded.manifest.aux_files)
    manifest_path = directory / scenario.MANIFEST_NAME
    violations: list[Violation] = []

    for entry in sorted(directory.iterdir()):
        if entry.name == scenario.MANIFEST_NAME:
            continue
        if entry.is_dir():
            if entry.name == scenario.EXPECTED_DIR:
                continue
            if entry.name not in scenario.PHASES:
                violations.append(
                    Violation(
                        entry,
                        0,
                        "phase-dir",
                        f"{entry.name!r} is not a phase; the vocabulary is "
                        f"{{{', '.join(scenario.PHASES)}}}",
                    )
                )
            elif entry.name not in declared:
                violations.append(
                    Violation(
                        entry,
                        0,
                        "phase-dir",
                        f"phase directory {entry.name!r} is on disk but "
                        f"{scenario.MANIFEST_NAME} declares {sorted(declared)}",
                    )
                )
        elif entry.name not in scenario.AUX_FILES:
            violations.append(
                Violation(
                    entry,
                    0,
                    "unknown-file",
                    f"{entry.name!r} is not part of the scenario format; the root holds "
                    f"{scenario.MANIFEST_NAME}, the phase directories, {scenario.EXPECTED_DIR}/ "
                    f"and {list(scenario.AUX_FILES)}",
                )
            )
        elif entry.name not in aux_declared:
            violations.append(
                Violation(
                    entry,
                    0,
                    "unknown-file",
                    f"{entry.name!r} is present but {scenario.MANIFEST_NAME} does not declare it "
                    "in aux_files",
                )
            )

    for aux in aux_declared:
        if not (directory / aux).is_file():
            violations.append(
                Violation(manifest_path, 0, "manifest", f"aux_files names {aux!r}, which is absent")
            )
    return violations


def _check_declared_phases(directory: Path, loaded: scenario.Scenario) -> list[Violation]:
    """Every declared phase is a delivery, here or in the `base_scenario` chain."""
    manifest_path = directory / scenario.MANIFEST_NAME
    return [
        Violation(
            manifest_path,
            0,
            "phase-dir",
            f"phase {phase!r} is declared but has no directory here or in the base_scenario chain",
        )
        for phase in loaded.phases
        if not loaded.inputs[phase]
    ]


def _check_expected_tree(directory: Path, loaded: scenario.Scenario) -> list[Violation]:
    """`expected/` holds one directory per phase the scenario has, and nothing else."""
    root = directory / scenario.EXPECTED_DIR
    if not root.is_dir():
        return []
    declared = set(loaded.phases)
    relations = {f"{relation}.csv" for relation in scenario.EXPECTED_RELATIONS}
    violations: list[Violation] = []

    for entry in sorted(root.iterdir()):
        if not entry.is_dir():
            violations.append(
                Violation(
                    entry,
                    0,
                    "unknown-file",
                    f"{entry.name!r} is a file directly under {scenario.EXPECTED_DIR}/; "
                    "expectations live in a per-phase directory",
                )
            )
            continue
        if entry.name not in scenario.PHASES:
            violations.append(
                Violation(
                    entry,
                    0,
                    "expected-phase",
                    f"{entry.name!r} is not a phase; the vocabulary is "
                    f"{{{', '.join(scenario.PHASES)}}}",
                )
            )
        elif entry.name not in declared:
            violations.append(
                Violation(
                    entry,
                    0,
                    "expected-phase",
                    f"claims about phase {entry.name!r}, which the scenario does not have "
                    f"(phases: {sorted(declared)})",
                )
            )
        violations += [
            Violation(
                child,
                0,
                "unknown-file",
                f"{child.name!r} is not one of the five relations a phase claims: "
                f"{sorted(relations)}",
            )
            for child in sorted(entry.iterdir())
            if child.name not in relations
        ]
    return violations


def _check_aux_files(loaded: scenario.Scenario) -> list[Violation]:
    """The three scenario-root files: header literals, plus the assertion phase vocabulary."""
    violations: list[Violation] = []
    for name, path in sorted(loaded.aux.items()):
        violations += _check_csv(path, name)
        if name == "assertions.csv":
            violations += _check_assertion_phases(path, set(loaded.phases))
    return violations


def _check_assertion_phases(path: Path, declared: set[str]) -> list[Violation]:
    header, rows = _read_csv(path)
    if "phase" not in header:
        # The header literal is already reported by `_check_csv`.
        return []
    index = header.index("phase")
    violations: list[Violation] = []
    for line_number, row in rows:
        if index >= len(row):
            continue
        phase = row[index]
        if phase not in scenario.PHASES:
            violations.append(
                Violation(
                    path,
                    line_number,
                    "assertion-phase",
                    f"phase {phase!r} is outside the vocabulary {{{', '.join(scenario.PHASES)}}}",
                )
            )
        elif phase not in declared:
            violations.append(
                Violation(
                    path,
                    line_number,
                    "assertion-phase",
                    f"phase {phase!r} is not a phase this scenario has "
                    f"(phases: {sorted(declared)})",
                )
            )
    return violations


def _check_csv(path: Path, header_key: str) -> list[Violation]:
    """Header literal, volatile columns, symbolic labels, null token and sort order."""
    header, rows = _read_csv(path)
    if not header:
        return [Violation(path, 1, "header", "file has no header row")]

    violations: list[Violation] = []
    volatile = [column for column in header if column in columns.VOLATILE_COLUMNS]
    if volatile:
        # A volatile column also makes the header differ from the literal. Report
        # the cause rather than both, so this file fails for its own reason.
        violations.append(
            Violation(
                path,
                1,
                "volatile-column",
                f"column(s) {volatile} are in VOLATILE_COLUMNS (S5.0) and never appear in a "
                "committed expected file",
            )
        )
    else:
        literal = scenario.EXPECTED_HEADERS[header_key]
        if tuple(header) != literal:
            violations.append(
                Violation(
                    path,
                    1,
                    "header",
                    f"header is {','.join(header)!r}; S8.2.1 pins {header_key} to "
                    f"{','.join(literal)!r}",
                )
            )

    violations += _check_rows(path, header, rows)
    return violations


def _check_rows(
    path: Path, header: list[str], rows: list[tuple[int, list[str]]]
) -> list[Violation]:
    violations: list[Violation] = []
    label_index = header.index(expected.LABEL_COLUMN) if expected.LABEL_COLUMN in header else -1
    previous: tuple[bytes, ...] | None = None
    previous_line = 0

    for line_number, row in rows:
        if len(row) != len(header):
            violations.append(
                Violation(
                    path,
                    line_number,
                    "header",
                    f"row has {len(row)} fields, the header has {len(header)}",
                )
            )
            continue

        if label_index >= 0:
            problem = scenario.symbolic_label_error(
                None if row[label_index] == expected.NULL_TOKEN else row[label_index]
            )
            if problem is not None:
                violations.append(Violation(path, line_number, "entity-label", problem))

        parsed = {
            column: (None if field == expected.NULL_TOKEN else field)
            for column, field in zip(header, row, strict=True)
        }
        for column, field in zip(header, row, strict=True):
            note = _null_token_problem(field)
            if note is not None:
                violations.append(
                    Violation(path, line_number, "null-token", f"column {column!r} {note}")
                )

        # `sort_key` re-encodes NULL as the token the file literally holds, so the
        # key it derives from the parsed row reproduces the file's own byte order.
        key = expected.sort_key(parsed, header)
        if previous is not None and key < previous:
            violations.append(
                Violation(
                    path,
                    line_number,
                    "sort-order",
                    f"row sorts before line {previous_line}; every expected file is stored "
                    "ascending, byte-wise on the UTF-8 column tuple in header order",
                )
            )
        previous = key
        previous_line = line_number
    return violations


def _null_token_problem(field: str) -> str | None:
    if field in ALTERNATE_NULLS:
        return f"is {field!r}; S8.2.1 spells NULL as the two-character token {expected.NULL_TOKEN}"
    if field != expected.NULL_TOKEN and field.strip() == expected.NULL_TOKEN:
        return f"is {field!r}; the null token carries no surrounding whitespace"
    return None


def _read_csv(path: Path) -> tuple[list[str], list[tuple[int, list[str]]]]:
    """Read a fixture CSV as raw text: the header, and (line number, fields) per row.

    Raw rather than through `load_expected`, because three of the rules are about
    what the file literally holds -- the null token's spelling, the byte order of
    the stored rows, a label the loader would refuse to return at all -- and a
    loader that normalises or raises would hide them.
    """
    with path.open(encoding="utf-8", newline="") as handle:
        records = [record for record in csv.reader(handle) if record]
    if not records:
        return [], []
    return records[0], list(enumerate(records[1:], start=2))


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "paths",
        nargs="*",
        type=Path,
        help="a scenario directory or a directory of them (default: fixtures/static/)",
    )
    parser.add_argument(
        "--list-rules", action="store_true", help="print the rule vocabulary and exit"
    )
    args = parser.parse_args(argv)

    if args.list_rules:
        for rule in RULES:
            print(rule)
        return 0

    directories: list[Path] = []
    for path in args.paths or [REPO_ROOT / "fixtures" / "static"]:
        if not path.is_dir():
            print(f"validate_fixtures: {path} is not a directory", file=sys.stderr)
            return 2
        if (path / scenario.MANIFEST_NAME).is_file():
            directories.append(path)
        else:
            directories.extend(scenario.discover_scenarios(path))

    violations = [
        violation for directory in directories for violation in validate_scenario(directory)
    ]
    for violation in violations:
        print(violation.render(REPO_ROOT), file=sys.stderr)
    if violations:
        print(
            f"FAIL {len(violations)} violation(s) over {len(directories)} scenario(s)",
            file=sys.stderr,
        )
        return 1
    print(f"OK {len(directories)} scenario(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
