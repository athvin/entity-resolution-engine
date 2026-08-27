"""Unit tests for the S5 relation registry.

Every expectation is *parsed out of DesignDoc.md*, never hand-copied: the S5
`CREATE TABLE` blocks and dbt typed column lists, the S5.0 ownership table with its
logical keys, the `-- {…}` domains annotating the DDL, the S5.2 promoted-counter
list and the S5.0 canonical-pair paragraph. A spec edit without a registry edit —
or a registry edit without a spec edit — therefore fails here, which is the whole
reason `ddl.py`, the dbt contract generator and every later stage are allowed to
read their column lists from the registry instead of restating S5.

The parsers below are shared between the two sides: the generated
`create_table_sql` text goes through the same reader as the spec block, so the
comparison is column-for-column and type-for-type rather than a whitespace diff.
"""

from __future__ import annotations

import ast
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

from er.lake.columns import GOLDEN_SURVIVABLE_COLUMNS, STD_RECORD_COLUMNS
from er.lake.ducklake import LAKE_ALIAS
from er.lake.model import (
    ASSERTION_KINDS,
    DBT_OWNED,
    DDL_OWNED,
    DISPOSITIONS,
    ENTITY_STATUSES,
    EVENT_TYPES,
    GOLDEN_LINEAGE_ATTRIBUTES,
    MODEL_STATUSES,
    PAIR_RELATIONS,
    PROMOTED_COUNTERS,
    REBUILD_REASONS,
    REGISTRY,
    REVIEW_REASONS,
    REVIEW_STATUSES,
    REVIEW_SUBJECT_TYPES,
    RUN_MODES,
    RUN_STAGES,
    RUN_STATUSES,
    SCHEMA_QUALIFIER,
    Column,
    LogicalKey,
    Owner,
    create_table_sql,
    logical_key,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
MODEL_SOURCE = REPO_ROOT / "src" / "er" / "lake" / "model.py"
DESIGN_DOC = (REPO_ROOT / "DesignDoc.md").read_text(encoding="utf-8")

# S5's own count of each owner's relations; the ticket and S5.0's table agree on
# fourteen + eight, and a parser that silently found fewer would make every
# comparison below vacuous.
DDL_RELATION_COUNT = 14
DBT_RELATION_COUNT = 8

# The three `golden_records` columns survivorship does not produce (S5).
GOLDEN_NON_SURVIVABLE = ("entity_id", "survivorship_version", "assembled_at")

# What DuckLake cannot enforce and `create_table_sql` may therefore never emit
# (S5.0). `ARRAY` is the fixed-size type; `LIST(VARCHAR)` is supported and stays.
FORBIDDEN_SQL = (
    "PRIMARY KEY",
    "UNIQUE",
    "FOREIGN KEY",
    "CHECK",
    "DEFAULT",
    "ENUM",
    "SEQUENCE",
    "CREATE INDEX",
    "ARRAY",
)


def section(anchor: str) -> str:
    """The text of one anchored DesignDoc.md section, up to the next anchor."""
    start = DESIGN_DOC.index(f'<a id="{anchor}"></a>')
    following = DESIGN_DOC.find('<a id="', start + 1)
    return DESIGN_DOC[start : following if following != -1 else len(DESIGN_DOC)]


S5 = section("s5")
S5_0 = section("s5-0")
S5_2 = section("s5-2")


def split_top_level(text: str) -> list[str]:
    """Split on commas outside parentheses, so `LIST(VARCHAR)` stays one term."""
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


def strip_sql_comments(text: str) -> str:
    return re.sub(r"--[^\n]*", "", text)


def parse_declarations(body: str) -> tuple[Column, ...]:
    """Read a comma-separated column declaration list into :class:`Column` objects."""
    columns = []
    for declaration in split_top_level(" ".join(strip_sql_comments(body).split())):
        tokens = declaration.split()
        nullable = " ".join(tokens[2:]).upper() != "NOT NULL"
        columns.append(Column(tokens[0], tokens[1], nullable=nullable))
    return tuple(columns)


def parse_create_table(statement: str) -> tuple[str, tuple[Column, ...]]:
    """Read one `CREATE TABLE IF NOT EXISTS` statement — spec-authored or generated."""
    match = re.search(
        rf"CREATE TABLE IF NOT EXISTS {re.escape(SCHEMA_QUALIFIER)}\.(\w+)\s*\((.*?)\n\);",
        statement,
        re.DOTALL,
    )
    assert match is not None, f"not a parseable CREATE TABLE statement: {statement[:80]!r}"
    return match.group(1), parse_declarations(match.group(2))


def spec_ddl_tables() -> dict[str, tuple[Column, ...]]:
    """Every `ddl.py`-owned relation of S5, in the order S5 declares them."""
    tables: dict[str, tuple[Column, ...]] = {}
    for block in re.findall(r"^```sql\n(.*?)^```", S5, re.MULTILINE | re.DOTALL):
        for statement in re.findall(r"CREATE TABLE IF NOT EXISTS .*?\n\);", block, re.DOTALL):
            name, columns = parse_create_table(statement)
            tables[name] = columns
    return tables


def spec_dbt_tables() -> dict[str, tuple[Column, ...]]:
    """Every dbt-owned relation of S5, with `stg_<source>` expanded per source.

    S5 declares the three staging models once, as one shape under a comment naming
    them, so the sources are read from that comment rather than assumed.
    """
    tables: dict[str, tuple[Column, ...]] = {}
    for block in re.findall(r"^```sql\n(.*?)^```", S5, re.MULTILINE | re.DOTALL):
        if "CREATE TABLE" in block:
            continue
        stg_names = re.findall(r"^-- (stg_\w+(?:, stg_\w+)*)", block, re.MULTILINE)
        assert stg_names, "S5 no longer names the staging models it declares as one shape"
        for match in re.finditer(r"^([\w<>]+)\(", block, re.MULTILINE):
            depth = 0
            for index in range(match.end() - 1, len(block)):
                depth += {"(": 1, ")": -1}.get(block[index], 0)
                if depth == 0:
                    break
            columns = parse_declarations(block[match.end() : index])
            relation = match.group(1)
            expanded = stg_names[0].split(", ") if relation == "stg_<source>" else [relation]
            for name in expanded:
                tables[name] = columns
    return tables


def parse_key_cell(cell: str) -> tuple[LogicalKey, ...]:
    """The logical keys one S5.0 ownership row declares, in the order it lists them.

    Each key is a backticked column or column tuple; ` — ` introduces prose, `;`
    separates keys, and a filter appears either as ``where `active` `` or, for
    `model_registry`'s at-most-one-active-row rule, as a bare predicate with no
    columns of its own.
    """
    keys = []
    for term in cell.split("—")[0].split(";"):
        columns: tuple[str, ...] = ()
        where: str | None = None
        spans = re.findall(r"`([^`]+)`", term)
        if not spans:
            continue
        for span in spans:
            if "=" in span or f"where `{span}`" in term:
                where = span
            else:
                columns = tuple(part.strip() for part in span.strip("()").split(","))
        keys.append(LogicalKey(columns, where))
    return tuple(keys)


def spec_ownership_rows() -> list[tuple[list[str], str, tuple[LogicalKey, ...]]]:
    """The S5.0 ownership table: (relations, owner, logical keys) per row."""
    rows = []
    for line in S5_0.splitlines():
        if not line.startswith("| `"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        rows.append((re.findall(r"`([\w<>]+)`", cells[0]), cells[1], parse_key_cell(cells[2])))
    return rows


def spec_enum_domains() -> dict[tuple[str, str], frozenset[str]]:
    """Every `in {…}` domain annotating a column in the S5 DDL, by (relation, column)."""
    domains: dict[tuple[str, str], frozenset[str]] = {}
    relation = ""
    qualifier = re.escape(SCHEMA_QUALIFIER)
    for line in S5.splitlines():
        create = re.search(rf"CREATE TABLE IF NOT EXISTS {qualifier}\.(\w+)", line)
        declaration = re.match(r"^([\w<>]+)\(", line)
        if create:
            relation = create.group(1)
        elif declaration:
            relation = declaration.group(1)
        # A domain annotation is the WHOLE trailing comment and its members are
        # bare identifiers. Both constraints matter: `model_registry.params_path`
        # is commented `{storage.model_uri_prefix}model_v{N}.json`, which is a
        # path template, not an `in {…}` domain.
        braces = re.search(r"--\s*\{([\w, ]+)\}$", line.rstrip())
        if braces and relation:
            column = line.split()[0]
            domains[relation, column] = frozenset(v.strip() for v in braces.group(1).split(","))
    return domains


def spec_promoted_counters() -> tuple[str, ...]:
    """The closed promoted-counter list of S5.2, in the order it states them."""
    listed = re.search(
        r"typed column\* of `run_stages`:(.+?)\. Each is a nullable", S5_2, re.DOTALL
    )
    assert listed is not None, "S5.2 no longer states the promoted counter list"
    return tuple(re.findall(r"`(\w+)`", listed.group(1)))


def spec_pair_relations(ddl_relations: dict[str, tuple[Column, ...]]) -> frozenset[str]:
    """The relations S5.0's canonical-pair paragraph binds to `rec_a_key < rec_b_key`."""
    paragraph = re.search(r"\*\*Canonical pair ordering\.\*\*(.+?)MUST satisfy", S5_0, re.DOTALL)
    assert paragraph is not None, "S5.0 no longer states the canonical pair ordering rule"
    return frozenset(n for n in re.findall(r"`([\w']+)`", paragraph.group(1)) if n in ddl_relations)


def test_owner_partition_is_exact_and_disjoint() -> None:
    ddl_relations, dbt_relations = spec_ddl_tables(), spec_dbt_tables()
    assert len(ddl_relations) == DDL_RELATION_COUNT
    assert len(dbt_relations) == DBT_RELATION_COUNT

    # The DDL blocks and the S5.0 ownership table are two statements of the same
    # partition; both must agree with the registry, or one of them has drifted.
    owners: dict[str, str] = {}
    for relations, owner, _ in spec_ownership_rows():
        for relation in relations:
            owners[relation] = owner
    assert {r for r, o in owners.items() if o == "ddl.py"} == set(ddl_relations)
    assert {r for r, o in owners.items() if o == "dbt"} == set(dbt_relations)

    assert set(DDL_OWNED) == set(ddl_relations)
    assert set(DBT_OWNED) == set(dbt_relations)
    assert not set(DDL_OWNED) & set(DBT_OWNED)
    assert set(REGISTRY) == set(DDL_OWNED) | set(DBT_OWNED)

    # D14: the owner is a property of the one spec, so `owner` and the partitions
    # cannot disagree even if a relation is added later.
    for name in DDL_OWNED:
        assert REGISTRY[name].owner is Owner.DDL
    for name in DBT_OWNED:
        assert REGISTRY[name].owner is Owner.DBT


def test_registry_matches_spec_s5_ddl() -> None:
    for relation, columns in spec_ddl_tables().items():
        generated_name, generated = parse_create_table(create_table_sql(REGISTRY[relation]))
        assert generated_name == relation
        # Column-for-column, type-for-type, in declared order, NOT NULL included.
        assert generated == columns, relation
        assert REGISTRY[relation].columns == columns, relation

    for relation, columns in spec_dbt_tables().items():
        assert REGISTRY[relation].columns == columns, relation

    # The registry is the sole column-list authority, so its `int_std_records`
    # must be the S5.0 tuple `columns.py` publishes to config validation and the
    # T-STD-1 projection.
    assert REGISTRY["int_std_records"].column_names == STD_RECORD_COLUMNS

    # Every lake reference is `lake.main.<relation>` (S4.0b), and `ducklake.py`
    # owns the alias it attaches under.
    assert SCHEMA_QUALIFIER == f"{LAKE_ALIAS}.main"


def test_only_not_null_constraints_are_emitted() -> None:
    for name, spec in REGISTRY.items():
        if spec.owner is Owner.DBT:
            # dbt materializes these under an enforced contract; `ddl.py` issuing
            # DDL against one is the ownership violation D14 forbids.
            with pytest.raises(ValueError, match="dbt-owned"):
                create_table_sql(spec)
            continue

        statement = create_table_sql(spec)
        upper = statement.upper()
        for forbidden in FORBIDDEN_SQL:
            assert forbidden not in upper, f"{name} emits {forbidden}"

        # Positive form: every body line is `<name> <type>[ NOT NULL]` and nothing
        # else, so a constraint spelled in a way FORBIDDEN_SQL misses still fails.
        assert statement.startswith(f"CREATE TABLE IF NOT EXISTS {SCHEMA_QUALIFIER}.{name} (\n")
        assert statement.endswith("\n);")
        for line in statement.splitlines()[1:-1]:
            assert re.fullmatch(
                r"  \w+ (VARCHAR|BIGINT|DOUBLE|BOOLEAN|DATE|TIMESTAMP|JSON|LIST\(VARCHAR\))"
                r"( NOT NULL)?,?",
                line,
            ), line


def test_logical_keys_match_s5_0_table() -> None:
    rows = spec_ownership_rows()
    # One row per relation, except that the three identically-keyed stg models
    # share a row: 14 + 8 relations over 20 rows.
    assert len(rows) == DDL_RELATION_COUNT + DBT_RELATION_COUNT - 2

    seen = set()
    for relations, _, keys in rows:
        for relation in relations:
            seen.add(relation)
            assert logical_key(relation) == keys, relation
            assert REGISTRY[relation].keys == keys, relation
    assert seen == set(REGISTRY)

    # The filtered and compound keys the ticket calls out, asserted against what
    # the parse produced so a parser that dropped a filter cannot pass silently.
    assert logical_key("assertions")[1] == LogicalKey(("rec_a_key", "rec_b_key"), "active")
    assert logical_key("cut_edges")[1] == LogicalKey(("rec_a_key", "rec_b_key"), "active")
    assert logical_key("review_queue")[1] == LogicalKey(
        ("subject_type", "rec_a_key", "rec_b_key", "entity_id", "reason"), "status='open'"
    )
    # At most one active model: uniqueness over no columns under the filter.
    assert logical_key("model_registry")[1] == LogicalKey((), "status='active'")
    assert logical_key("entity_events") == (
        LogicalKey(("event_id",)),
        LogicalKey(("run_id", "entity_id", "event_type", "details_hash")),
    )
    # Unfiltered: invalidation updates `is_active` in place, never adding a row.
    assert logical_key("match_scores") == (
        LogicalKey(("rec_a_key", "rec_b_key", "model_version", "tf_snapshot_id")),
    )


def test_enum_domains_are_exact() -> None:
    parsed = spec_enum_domains()
    declared = {
        (name, column): domain
        for name, spec in REGISTRY.items()
        for column, domain in spec.enums.items()
    }
    assert declared == parsed

    # AC5 names these six explicitly; the parse above is the authority, and these
    # restate it so a spec edit that drops a value fails as a readable diff.
    assert EVENT_TYPES == {
        "created",
        "member_added",
        "member_removed",
        "merged",
        "split",
        "retired",
        "edge_cut",
    }
    assert ENTITY_STATUSES == {"active", "merged", "retired"}
    assert RUN_MODES == {
        "incremental",
        "full",
        "train",
        "init",
        "maintain",
        "reset",
        "correction_pass",
        "stage",
    }
    assert RUN_STAGES == {
        "init",
        "ingest",
        "standardize",
        "train",
        "match",
        "reconcile",
        "assemble",
        "maintain",
        "reset",
    }
    assert REVIEW_REASONS == {"gray_band", "never_unsatisfiable", "coherence"}
    assert DISPOSITIONS == {"rebuild", "retire"}

    # The remaining vocabularies, each bound to the column it constrains.
    assert parsed["assertions", "kind"] == ASSERTION_KINDS
    assert parsed["review_queue", "subject_type"] == REVIEW_SUBJECT_TYPES
    assert parsed["review_queue", "status"] == REVIEW_STATUSES
    assert parsed["model_registry", "status"] == MODEL_STATUSES
    assert parsed["runs", "status"] == RUN_STATUSES == parsed["run_stages", "status"]
    assert parsed["runs", "rebuild_reason"] == REBUILD_REASONS
    # The constant carries S5's order for `golden_lineage`'s grid; the parsed DDL
    # comment is a set, so the comparison is made on sets and the binding between
    # the spec text and the one definition is unchanged.
    assert parsed["golden_lineage", "attribute"] == frozenset(GOLDEN_LINEAGE_ATTRIBUTES)

    # Every enum column is a plain VARCHAR: DuckLake has no ENUM type (S5.0).
    for relation, column in declared:
        declared_column = next(c for c in REGISTRY[relation].columns if c.name == column)
        assert declared_column.type == "VARCHAR", f"{relation}.{column}"


def test_promoted_counters_and_pair_relations() -> None:
    assert spec_promoted_counters() == PROMOTED_COUNTERS
    assert len(PROMOTED_COUNTERS) == 11

    run_stages = {column.name: column for column in REGISTRY["run_stages"].columns}
    for counter in PROMOTED_COUNTERS:
        assert run_stages[counter] == Column(counter, "BIGINT", nullable=True), counter

    # Closed list: a stage may add a name to the `counters` JSON payload, never a
    # column, because promoting one is a schema change under S5.1.
    assert "counters" in run_stages
    assert {name for name, column in run_stages.items() if column.type == "BIGINT"} == {
        *PROMOTED_COUNTERS,
        "seq",
        "snapshot_start",
        "snapshot_end",
    }

    assert PAIR_RELATIONS == {"match_scores", "assertions", "review_queue", "cut_edges"}
    assert spec_pair_relations(spec_ddl_tables()) == PAIR_RELATIONS
    for relation in PAIR_RELATIONS:
        names = REGISTRY[relation].column_names
        assert "rec_a_key" in names and "rec_b_key" in names, relation


def test_golden_survivable_columns_are_imported_not_restated() -> None:
    golden = REGISTRY["golden_records"].column_names
    assert tuple(c for c in golden if c not in GOLDEN_NON_SURVIVABLE) == GOLDEN_SURVIVABLE_COLUMNS

    # Survivorship copies standardized values, so the golden columns carry the
    # `int_std_records` types; a divergence would silently cast at assembly time.
    std = {column.name: column.type for column in REGISTRY["int_std_records"].columns}
    for column in REGISTRY["golden_records"].columns:
        if column.name in GOLDEN_SURVIVABLE_COLUMNS:
            assert column.type == std[column.name], column.name

    # M4: `columns.py` owns these names. A second assignment here is the copy that
    # would drift, and importing is the only way the registry may reference them.
    #
    # `GOLDEN_LINEAGE_ATTRIBUTES` joined the list in ER-088: it had been a hard-coded
    # frozenset here while `columns.py` derives it from the survivable set, which is two
    # listings of one consequence of S5. The import is checked by parsing rather than by
    # substring, so grouping the names on one `from` line -- which is what the formatter
    # does -- cannot make this guard silently stop looking.
    source = MODEL_SOURCE.read_text(encoding="utf-8")
    owned = ("GOLDEN_SURVIVABLE_COLUMNS", "VOLATILE_COLUMNS", "GOLDEN_LINEAGE_ATTRIBUTES")
    for name in owned:
        assert re.search(rf"^\s*{name}\s*(:[^=]+)?=[^=]", source, re.MULTILINE) is None, name

    imported_from_columns = {
        alias.name
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.ImportFrom) and node.module == "er.lake.columns"
        for alias in node.names
    }
    for name in ("GOLDEN_SURVIVABLE_COLUMNS", "GOLDEN_LINEAGE_ATTRIBUTES"):
        assert name in imported_from_columns, (
            f"{name} is neither assigned nor imported from er.lake.columns in model.py"
        )


def test_registry_module_is_pure() -> None:
    tree = ast.parse(MODEL_SOURCE.read_text(encoding="utf-8"))
    imported = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imported.extend(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            imported.append(node.module or "")
    for name in imported:
        assert name.split(".")[0] not in {"duckdb", "psycopg"}, f"model.py imports {name}"

    # The transitive graph, not just the direct imports: the registry must land
    # before anything persists, so importing it may not pull in a driver.
    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO_ROOT / "src")
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "import json, sys;"
            "import er.lake.model;"
            "print(json.dumps(sorted(m for m in sys.modules "
            "if m.split('.')[0] in {'er', 'duckdb', 'psycopg'})))",
        ],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == ["er", "er.lake", "er.lake.columns", "er.lake.model"]
