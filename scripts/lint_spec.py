#!/usr/bin/env python3
"""Lint DesignDoc.md.

This is the guard that keeps the specification honest once the implementation
loop is running. The loop may not edit the spec, and this linter makes any
regression toward the v1.0 draft a hard CI failure.

It must pass against the amended v1.1 document AND fail against the committed
v1.0 copy at ``tests/fixtures/designdoc_v1.0.md`` -- a linter that passes on both
is checking nothing. CI asserts both arms.

Usage:
    python3 scripts/lint_spec.py DesignDoc.md
    python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md

Exit codes: 0 clean; 1 defects found; 2 usage.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Patterns that must never appear. The literal strings live HERE, not in the
# spec, so the document cannot trip its own linter by describing the rule.
FORBIDDEN = [
    (r"entity-resolution-design-doc", "cites the non-existent companion design document"),
    (r"design doc §", "cites the non-existent companion design document"),
    (r"\bTBD\b", "unresolved placeholder marker"),
    (r"\bFIXME\b", "unresolved placeholder marker"),
    (r"<canonical attributes", "unresolved schema placeholder"),
    (r"<pin from", "unresolved version pin in S2.1"),
    (r"\bstd_records\b(?!/)", "uses the v1.0 name; the relation is int_std_records"),
    (r"(?<!int_)\bblocking_keys\b", "uses the v1.0 name; the relation is int_blocking_keys"),
    # Match the DDL form `PRIMARY KEY (cols)` only. The spec legitimately says
    # DuckLake supports no PRIMARY KEY, and T-KEY-1 asserts that declaring one
    # raises -- neither of those is a defect.
    (r"PRIMARY KEY\s*\(", "declares a PRIMARY KEY; DuckLake enforces none (gap report B2)"),
    (r"<\d+-hex digest>", "unresolved image digest in S2.1"),
    (r"\brec_a\b(?!_key)", "pair columns are rec_a_key / rec_b_key"),
    (r"\brec_b\b(?!_key)", "pair columns are rec_a_key / rec_b_key"),
    (r"exactly one DuckLake snapshot", "false: dbt-backed stages commit many snapshots"),
    (r"misses transitive links", "false for the S4.5 algorithm; name the real loss vectors"),
]

# Sections that must exist. These are the twelve the gap report found missing
# plus the ones tickets cite by anchor.
REQUIRED_SECTIONS = [
    ("2.1", "Pinned Versions"),
    ("4.0", "CLI contract"),
    ("4.0b", "Connection & writer model"),
    ("4.7", "Failure semantics"),
    ("5.0", "Ownership & key model"),
    ("5.1", "Schema evolution"),
    ("5.2", "Run & batch metadata"),
    ("8.2.1", "Fixture & expected-output format"),
    ("8.5", "Quality metric definitions"),
    ("12.1", "Decision lock"),
]

# Vocabulary the implementation depends on. Each entry is a term that at least
# one ticket's acceptance criteria reference; if it vanishes from the spec, that
# ticket becomes unimplementable, so its absence is a spec defect.
REQUIRED_TOKENS = [
    "INV-PERM",
    "INV-EQ",
    "INV-SCORE",
    "CONTRADICTION-1",
    "record_key",
    "rec_a_key",
    "tf_snapshot_id",
    "tf_lookup",
    "cut_edges",
    "edge_cut",
    "never_unsatisfiable",
    "cut_protect_probability",
    "er_touched_entities",
    "VOLATILE_COLUMNS",
    "GOLDEN_SURVIVABLE_COLUMNS",
    "golden_display",
    "int_std_records",
    "int_blocking_keys",
    "find_matches_to_new_records",
    "dedupe_only",
    "register_term_frequency_lookup",
    "member_removed",
    "--full-refresh-keys",
    "is_deleted",
    "run_stages",
    "ingest_batches",
    "config_hash",
    "advisory lock",
    "tiebreak_deterministic",
    "splink_scratch",
]

# The locked decisions. Each must appear in S12.1 marked LOCKED.
LOCKED_DECISIONS = ["D2", "D3", "D4", "D5", "D6", "D7", "D8"]

HEADING_RE = re.compile(r"^(#{2,4})\s+(\d+(?:\.\d+)*[a-z]?)[.)]?\s+(.*)$")
ANCHOR_RE = re.compile(r'^<a id="([^"]+)"></a>\s*$')


def anchor_for(number: str) -> str:
    return "s" + number.replace(".", "-")


def lint(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    defects: list[str] = []

    for pattern, why in FORBIDDEN:
        for match in re.finditer(pattern, text):
            lineno = text[: match.start()].count("\n") + 1
            defects.append(f"{path.name}:{lineno}: forbidden '{match.group(0)}' -- {why}")

    seen_anchors: dict[str, int] = {}
    found_sections: set[str] = set()
    for i, line in enumerate(lines):
        anchor_match = ANCHOR_RE.match(line)
        if anchor_match:
            anchor = anchor_match.group(1)
            if anchor in seen_anchors:
                defects.append(
                    f"{path.name}:{i + 1}: duplicate anchor '{anchor}' "
                    f"(first seen at line {seen_anchors[anchor]})"
                )
            else:
                seen_anchors[anchor] = i + 1
            continue

        heading = HEADING_RE.match(line)
        if not heading:
            continue
        number = heading.group(2)
        found_sections.add(number)
        expected = anchor_for(number)
        previous = lines[i - 1] if i else ""
        previous_anchor = ANCHOR_RE.match(previous)
        if not previous_anchor:
            defects.append(
                f"{path.name}:{i + 1}: heading '{number}' has no <a id=\"{expected}\"></a> "
                "on the line before it; ticket spec_refs resolve against these anchors"
            )
        elif previous_anchor.group(1) != expected:
            defects.append(
                f"{path.name}:{i + 1}: heading '{number}' is anchored "
                f"'{previous_anchor.group(1)}', expected '{expected}'"
            )

    for number, title in REQUIRED_SECTIONS:
        if number not in found_sections:
            defects.append(f"{path.name}: missing required section {number} ({title})")

    for token in REQUIRED_TOKENS:
        if token not in text:
            defects.append(f"{path.name}: required term '{token}' appears nowhere in the spec")

    lock_section = ""
    lock_start = text.find('<a id="s12-1">')
    if lock_start != -1:
        lock_section = text[lock_start : lock_start + 6000]
    for decision in LOCKED_DECISIONS:
        if not re.search(rf"\b{decision}\b", lock_section):
            defects.append(f"{path.name}: decision {decision} is not recorded in S12.1")
    if lock_section and "LOCKED" not in lock_section:
        defects.append(f"{path.name}: S12.1 does not mark its decisions LOCKED")

    if not re.search(r"^\*\*Version:\*\*\s*1\.1", text, re.M):
        defects.append(f"{path.name}: header does not declare Version 1.1")

    return defects


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="lint_spec.py", description=__doc__)
    parser.add_argument("path", nargs="?", default="DesignDoc.md")
    parser.add_argument(
        "--expect-fail",
        action="store_true",
        help="invert the result: succeed only if the document HAS defects. "
        "Used on the committed v1.0 copy to prove the linter is not vacuous.",
    )
    args = parser.parse_args(argv)

    path = Path(args.path)
    if not path.exists():
        print(f"lint_spec: no such file: {path}", file=sys.stderr)
        return 2

    defects = lint(path)

    if args.expect_fail:
        if defects:
            print(f"OK {path} has {len(defects)} defect(s), as expected for the v1.0 baseline")
            return 0
        print(
            f"FAIL {path} is clean, but --expect-fail requires defects. "
            "The linter has become vacuous: it would pass on the unamended draft.",
            file=sys.stderr,
        )
        return 1

    if defects:
        for defect in defects:
            print(f"DEFECT {defect}", file=sys.stderr)
        print(f"\n{len(defects)} defect(s) in {path}", file=sys.stderr)
        return 1

    print(f"OK {path}: sections, anchors and vocabulary all present")
    return 0


if __name__ == "__main__":
    sys.exit(main())
