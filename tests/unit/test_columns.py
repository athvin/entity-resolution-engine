"""Unit tests for the three shared column sets of S5.0.

Every expectation here is *parsed out of DesignDoc.md*, never hand-copied: the S5.0
``VOLATILE_COLUMNS`` and ``GOLDEN_SURVIVABLE_COLUMNS`` python blocks, the S5
``int_std_records`` / ``golden_records`` DDL, the T-STD-1 ``std_hash`` column list
of S8.3 and the S6.1 V2 survivorship key set. A spec edit without a code edit — or
a code edit without a spec edit — therefore fails here rather than surfacing as a
silently NULL golden attribute or a determinism diff nobody can explain.

The last two tests assert the module's two structural properties: it imports the
stdlib only (S5.0 — every package consumes it, so any ``er`` import closes a
cycle), and each constant has exactly one definition site in the repo (M4).
"""

from __future__ import annotations

import ast
import json
import os
import re
import subprocess
import sys
from pathlib import Path

from er.lake.columns import (
    GOLDEN_SURVIVABLE_COLUMNS,
    STD_RECORD_COLUMNS,
    VOLATILE_COLUMNS,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
COLUMNS_SOURCE = REPO_ROOT / "src" / "er" / "lake" / "columns.py"
DESIGN_DOC = (REPO_ROOT / "DesignDoc.md").read_text(encoding="utf-8")

# The three provenance/key columns of `golden_records` that survivorship does not
# produce (S5, paragraph under the dbt-owned DDL).
GOLDEN_NON_SURVIVABLE = ("entity_id", "survivorship_version", "assembled_at")

# The stdlib modules S5.0's dependency-free rule allows this module to import.
ALLOWED_IMPORTS = frozenset({"typing", "collections.abc"})


def split_top_level(text: str) -> list[str]:
    """Split on commas outside parentheses.

    Both parsed lists carry parenthesised terms with commas of their own —
    ``name_variants LIST(VARCHAR)`` in the DDL and ``array_to_string(name_variants,
    '\\x1f')`` in T-STD-1 — so a plain ``str.split(',')`` would invent columns.
    """
    parts: list[str] = []
    current: list[str] = []
    depth = 0
    for char in text:
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        if char == "," and depth == 0:
            parts.append("".join(current))
            current = []
        else:
            current.append(char)
    parts.append("".join(current))
    return [part.strip() for part in parts if part.strip()]


def spec_python_literal(name: str) -> object:
    """Return ``name``'s value as authored in a fenced python block of DesignDoc.md."""
    for block in re.findall(r"^```python\n(.*?)^```", DESIGN_DOC, re.MULTILINE | re.DOTALL):
        try:
            tree = ast.parse(block)
        except SyntaxError:
            continue  # several spec snippets are elided, not runnable modules
        for node in tree.body:
            if not isinstance(node, ast.Assign):
                continue
            if not any(isinstance(t, ast.Name) and t.id == name for t in node.targets):
                continue
            value = node.value
            if isinstance(value, ast.Call) and getattr(value.func, "id", "") == "frozenset":
                return frozenset(ast.literal_eval(value.args[0]))
            return ast.literal_eval(value)
    raise AssertionError(f"DesignDoc.md declares no `{name} = ...` python block")


def spec_ddl_columns(relation: str) -> tuple[str, ...]:
    """Column names of a dbt-owned relation, in the DDL order S5 declares them in."""
    opening = re.compile(rf"^{re.escape(relation)}\(", re.MULTILINE)
    for block in re.findall(r"^```sql\n(.*?)^```", DESIGN_DOC, re.MULTILINE | re.DOTALL):
        # `-- …` trailers annotate columns and would otherwise be read as one.
        body = re.sub(r"--[^\n]*", "", block)
        found = opening.search(body)
        if found is None:
            continue
        depth = 0
        for index in range(found.end() - 1, len(body)):
            depth += {"(": 1, ")": -1}.get(body[index], 0)
            if depth == 0:
                declarations = body[found.end() : index]
                return tuple(part.split()[0] for part in split_top_level(declarations))
    raise AssertionError(f"DesignDoc.md declares no DDL for `{relation}`")


def spec_table_row(prefix: str) -> str:
    """The one Markdown table row of DesignDoc.md whose first cell is ``prefix``."""
    rows = [line for line in DESIGN_DOC.splitlines() if line.startswith(f"| {prefix} |")]
    assert len(rows) == 1, f"expected exactly one `{prefix}` row, found {len(rows)}"
    return rows[0]


def spec_std_hash_columns() -> tuple[str, ...]:
    """The columns T-STD-1's `std_hash` concatenates, in its stated order."""
    listed = re.search(r"concatenation of `([^`]+)`", spec_table_row("T-STD-1"))
    assert listed is not None, "the T-STD-1 row states no hash column list"
    columns = []
    for term in split_top_level(listed.group(1)):
        # A term may be a function over the column, e.g. array_to_string(c, sep).
        call = re.fullmatch(r"[a-z_]+\((.*)\)", term)
        columns.append(split_top_level(call.group(1))[0] if call else term)
    return tuple(columns)


def spec_survivorship_keyset() -> frozenset[str]:
    """The `survivorship:` key set S6.1 V2 pins, before `address` is expanded."""
    keys = re.search(r"set\(survivorship\.keys\(\)\) == \{([^}]+)\}", spec_table_row("V2"))
    assert keys is not None, "the V2 row states no survivorship key set"
    return frozenset(key.strip() for key in keys.group(1).split(","))


def test_volatile_columns_match_spec() -> None:
    assert spec_python_literal("VOLATILE_COLUMNS") == VOLATILE_COLUMNS
    assert isinstance(VOLATILE_COLUMNS, frozenset)
    assert len(VOLATILE_COLUMNS) == 9


def test_golden_survivable_columns_are_ordered_and_match_spec() -> None:
    # The constant sits on the right of every `==` so that the S5.0 single-definition
    # grep (`GOLDEN_SURVIVABLE_COLUMNS *=`) counts definition sites, not assertions.
    assert spec_python_literal("GOLDEN_SURVIVABLE_COLUMNS") == GOLDEN_SURVIVABLE_COLUMNS
    assert isinstance(GOLDEN_SURVIVABLE_COLUMNS, tuple)
    assert len(GOLDEN_SURVIVABLE_COLUMNS) == 11

    # DDL order, so it cannot be sorted for convenience without failing.
    assert tuple(sorted(GOLDEN_SURVIVABLE_COLUMNS)) != GOLDEN_SURVIVABLE_COLUMNS
    declared = spec_ddl_columns("golden_records")
    assert tuple(c for c in declared if c not in GOLDEN_NON_SURVIVABLE) == GOLDEN_SURVIVABLE_COLUMNS

    # The key, the two provenance stamps, and the two `int_std_records`-only inputs
    # to the `validated` rule are all excluded (S5.0).
    for excluded in (*GOLDEN_NON_SURVIVABLE, "email_valid", "phone_valid"):
        assert excluded not in GOLDEN_SURVIVABLE_COLUMNS


def test_std_record_columns_match_spec_ddl() -> None:
    assert spec_ddl_columns("int_std_records") == STD_RECORD_COLUMNS
    assert isinstance(STD_RECORD_COLUMNS, tuple)
    assert STD_RECORD_COLUMNS[0] == "record_key"  # the logical key and Splink's unique id

    # The only volatile columns on this relation are the two ingest stamps: a third
    # one would mean a run-scoped value had leaked into the standardized corpus.
    assert set(STD_RECORD_COLUMNS) & VOLATILE_COLUMNS == {"ingest_batch_id", "ingested_at"}


def test_t_std_1_hash_columns_are_subset_of_std_record_columns() -> None:
    hashed = spec_std_hash_columns()
    assert hashed, "parsed no columns out of the T-STD-1 row"
    assert set(hashed) <= set(STD_RECORD_COLUMNS), (
        f"T-STD-1 hashes columns absent from int_std_records: "
        f"{sorted(set(hashed) - set(STD_RECORD_COLUMNS))}"
    )
    # The hash is a determinism claim, so it must exclude the volatile stamps.
    assert not set(hashed) & VOLATILE_COLUMNS


def test_survivorship_keyset_expansion_equals_golden_survivable() -> None:
    keyset = spec_survivorship_keyset()
    assert "address" in keyset, "V2's key set no longer needs the addr_* expansion"

    addr_columns = frozenset(c for c in GOLDEN_SURVIVABLE_COLUMNS if c.startswith("addr_"))
    assert len(addr_columns) == 6, "S4.6 requires exactly six addr_* columns from one record"

    expanded = (keyset - {"address"}) | addr_columns
    assert expanded == set(GOLDEN_SURVIVABLE_COLUMNS)
    # `address` is a survivorship rule key, never a golden column.
    assert "address" not in spec_ddl_columns("golden_records")


def test_module_is_dependency_free() -> None:
    tree = ast.parse(COLUMNS_SOURCE.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                assert alias.name in ALLOWED_IMPORTS, f"columns.py imports {alias.name}"
        elif isinstance(node, ast.ImportFrom):
            assert node.level == 0, "columns.py uses a relative import"
            assert node.module in ALLOWED_IMPORTS, f"columns.py imports from {node.module}"

    # The transitive graph, not just the direct imports: importing the module in a
    # fresh interpreter may pull in its parent packages and nothing else.
    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO_ROOT / "src")
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "import json, sys;"
            "import er.lake.columns;"
            "print(json.dumps(sorted(m for m in sys.modules "
            "if m == 'er' or m.startswith('er.'))))",
        ],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == ["er", "er.lake", "er.lake.columns"]


def test_each_column_set_has_exactly_one_definition_site() -> None:
    # M4: one owner per fact. A second assignment anywhere under src/ or tests/ is a
    # copy that will drift, which is the whole reason this module exists.
    sources = [
        path
        for root in ("src", "tests")
        for path in (REPO_ROOT / root).rglob("*.py")
        if "__pycache__" not in path.parts
    ]
    for name in ("VOLATILE_COLUMNS", "GOLDEN_SURVIVABLE_COLUMNS", "STD_RECORD_COLUMNS"):
        assignment = re.compile(rf"^\s*{name}\s*(:[^=]+)?=[^=]", re.MULTILINE)
        sites = [
            path
            for path in sources
            if assignment.search(path.read_text(encoding="utf-8")) is not None
        ]
        assert sites == [COLUMNS_SOURCE], f"{name} is defined in {[str(p) for p in sites]}"
